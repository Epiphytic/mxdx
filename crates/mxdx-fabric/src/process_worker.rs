use std::collections::HashMap;
use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::Result;
use mxdx_matrix::RoomId;
use mxdx_types::events::capability::{
    CapabilityAdvertisement, InputSchema, SchemaProperty, WorkerTool,
};
use mxdx_types::events::fabric::{TaskEvent, TaskStatus};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::process::Command;
use tracing::{debug, info, warn};

use crate::worker::WorkerClient;

const BATCH_INTERVAL: Duration = Duration::from_secs(30);
const BATCH_MAX_BYTES: usize = 4096;
const TAIL_LINES: usize = 50;

const EVENT_STREAM_OFFER: &str = "org.mxdx.fabric.stream_offer";
const STREAM_ACCEPT_TIMEOUT: Duration = Duration::from_secs(30);

/// Payload for a generic process execution task.
///
/// The client (e.g. openclaw-fabric-plugin) specifies everything: the binary
/// to run, its arguments, environment variables, and working directory.
/// The worker just executes — it has no knowledge of jcode, claude, or any
/// other specific tool.
#[derive(Debug, Clone)]
pub struct ProcessPayload {
    pub bin: String,
    pub args: Vec<String>,
    pub env: HashMap<String, String>,
    pub cwd: Option<PathBuf>,
}

impl ProcessPayload {
    /// Parse a process payload from a task's JSON payload.
    ///
    /// Expected shape:
    /// ```json
    /// {
    ///   "bin": "jcode",
    ///   "args": ["run", "--ndjson", "do the thing"],
    ///   "env": {"SOME_VAR": "value"},
    ///   "cwd": "/home/user/project"
    /// }
    /// ```
    pub fn from_task_payload(payload: &serde_json::Value) -> Option<Self> {
        let obj = payload.as_object()?;

        let bin = obj.get("bin").and_then(|v| v.as_str())?.to_string();

        let args = obj
            .get("args")
            .and_then(|v| v.as_array())
            .map(|arr| {
                arr.iter()
                    .filter_map(|v| v.as_str())
                    .map(|s| s.to_string())
                    .collect()
            })
            .unwrap_or_default();

        let env = obj
            .get("env")
            .and_then(|v| v.as_object())
            .map(|map| {
                map.iter()
                    .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                    .collect()
            })
            .unwrap_or_default();

        let cwd = obj
            .get("cwd")
            .and_then(|v| v.as_str())
            .map(PathBuf::from);

        Some(Self {
            bin,
            args,
            env,
            cwd,
        })
    }
}

/// A generic process worker that executes arbitrary binaries.
///
/// The worker does not know about jcode, claude, or any specific tool.
/// All tool-specific knowledge (binary name, args, env vars) lives in the
/// client that constructs the task payload. The worker just:
///
/// 1. Claims the task via Matrix state events
/// 2. Spawns `Command::new(payload.bin).args(payload.args).envs(payload.env)`
/// 3. Streams stdout lines as Matrix heartbeat events
/// 4. Posts the raw output as the final result event
pub struct ProcessWorker {
    worker_client: WorkerClient,
}

impl ProcessWorker {
    pub fn new(worker_client: WorkerClient) -> Self {
        Self { worker_client }
    }

    pub fn worker_client(&self) -> &WorkerClient {
        &self.worker_client
    }

    pub async fn run_task(
        &self,
        task: TaskEvent,
        room_id: &RoomId,
        task_event_id: String,
    ) -> Result<()> {
        if !self.worker_client.try_claim(&task, room_id).await? {
            debug!(
                uuid = %task.uuid,
                "claim lost, another worker took the task"
            );
            return Ok(());
        }

        let payload = match ProcessPayload::from_task_payload(&task.payload) {
            Some(p) => p,
            None => {
                warn!(
                    uuid = %task.uuid,
                    "task payload missing required 'bin' field or malformed"
                );
                let thread_id = if task_event_id.is_empty() {
                    None
                } else {
                    Some(task_event_id.as_str())
                };
                self.worker_client
                    .post_result(
                        &task.uuid,
                        TaskStatus::Failed,
                        None,
                        Some("task payload missing required 'bin' field".to_string()),
                        0,
                        room_id,
                        thread_id,
                    )
                    .await?;
                return Ok(());
            }
        };

        info!(
            uuid = %task.uuid,
            bin = %payload.bin,
            "claim won, spawning process"
        );

        let use_p2p = task.p2p_stream && task.heartbeat_interval_seconds < 5;

        if use_p2p {
            self.run_task_p2p(&task, &payload, room_id, &task_event_id)
                .await
        } else {
            self.run_task_matrix(&task, &payload, room_id, &task_event_id)
                .await
        }
    }

    async fn run_task_matrix(
        &self,
        task: &TaskEvent,
        payload: &ProcessPayload,
        room_id: &RoomId,
        task_event_id: &str,
    ) -> Result<()> {
        let cwd = payload
            .cwd
            .clone()
            .unwrap_or_else(|| PathBuf::from("/tmp"));

        let start = Instant::now();
        let mut child = Command::new(&payload.bin)
            .args(&payload.args)
            .envs(&payload.env)
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdout = child.stdout.take().expect("stdout was piped");
        let mut reader = BufReader::new(stdout).lines();

        let mut batch_buf = String::new();
        let mut batch_timer = Instant::now();
        let mut all_lines: Vec<String> = Vec::new();

        loop {
            match reader.next_line().await {
                Ok(Some(line)) => {
                    all_lines.push(line.clone());

                    batch_buf.push_str(&line);
                    batch_buf.push('\n');

                    if batch_buf.len() >= BATCH_MAX_BYTES || batch_timer.elapsed() >= BATCH_INTERVAL
                    {
                        self.flush_heartbeat(&task.uuid, &batch_buf, room_id, task_event_id)
                            .await;
                        batch_buf.clear();
                        batch_timer = Instant::now();
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    warn!(uuid = %task.uuid, error = %e, "error reading process stdout");
                    break;
                }
            }
        }

        if !batch_buf.is_empty() {
            self.flush_heartbeat(&task.uuid, &batch_buf, room_id, task_event_id)
                .await;
        }

        let status = child.wait().await?;
        let duration = start.elapsed().as_secs();

        let tail = Self::tail_lines(&all_lines);

        let (task_status, error_msg) = if status.success() {
            (TaskStatus::Success, None)
        } else {
            let code = status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string());
            (
                TaskStatus::Failed,
                Some(format!("{} exited with code {code}", payload.bin)),
            )
        };

        info!(
            uuid = %task.uuid,
            bin = %payload.bin,
            exit_code = ?status.code(),
            duration_seconds = duration,
            "process completed"
        );

        let thread_id = if task_event_id.is_empty() {
            None
        } else {
            Some(task_event_id)
        };

        self.worker_client
            .post_result(
                &task.uuid,
                task_status,
                Some(serde_json::Value::String(tail)),
                error_msg,
                duration,
                room_id,
                thread_id,
            )
            .await?;

        Ok(())
    }

    async fn run_task_p2p(
        &self,
        task: &TaskEvent,
        payload: &ProcessPayload,
        room_id: &RoomId,
        task_event_id: &str,
    ) -> Result<()> {
        let socket_path = format!("/tmp/mxdx-fabric-{}.sock", task.uuid);

        let _ = tokio::fs::remove_file(&socket_path).await;

        let listener = UnixListener::bind(&socket_path)?;

        info!(
            uuid = %task.uuid,
            socket_path = %socket_path,
            "created Unix domain socket for P2P stream"
        );

        let stream_offer = serde_json::json!({
            "socket_path": socket_path,
            "worker_id": self.worker_client.worker_id(),
        });
        let state_key = format!("task/{}/stream", task.uuid);
        self.worker_client
            .post_state_event(EVENT_STREAM_OFFER, &state_key, stream_offer, room_id)
            .await?;

        info!(
            uuid = %task.uuid,
            "posted stream offer state event"
        );

        let accept_result = tokio::time::timeout(STREAM_ACCEPT_TIMEOUT, listener.accept()).await;

        let mut unix_stream = match accept_result {
            Ok(Ok((stream, _addr))) => {
                info!(uuid = %task.uuid, "P2P stream connection accepted");
                Some(stream)
            }
            Ok(Err(e)) => {
                warn!(uuid = %task.uuid, error = %e, "failed to accept P2P connection");
                None
            }
            Err(_) => {
                warn!(uuid = %task.uuid, "timed out waiting for P2P connection, falling back to Matrix heartbeats");
                None
            }
        };

        let cwd = payload
            .cwd
            .clone()
            .unwrap_or_else(|| PathBuf::from("/tmp"));

        let start = Instant::now();
        let mut child = Command::new(&payload.bin)
            .args(&payload.args)
            .envs(&payload.env)
            .current_dir(&cwd)
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()?;

        let stdout = child.stdout.take().expect("stdout was piped");
        let mut reader = BufReader::new(stdout).lines();

        let mut batch_buf = String::new();
        let mut batch_timer = Instant::now();
        let mut all_lines: Vec<String> = Vec::new();

        loop {
            match reader.next_line().await {
                Ok(Some(line)) => {
                    all_lines.push(line.clone());

                    if let Some(ref mut stream) = unix_stream {
                        let mut data = line.as_bytes().to_vec();
                        data.push(b'\n');
                        if let Err(e) = stream.write_all(&data).await {
                            warn!(uuid = %task.uuid, error = %e, "P2P stream write error, dropping stream");
                            unix_stream = None;
                        }
                    }

                    batch_buf.push_str(&line);
                    batch_buf.push('\n');

                    if batch_buf.len() >= BATCH_MAX_BYTES || batch_timer.elapsed() >= BATCH_INTERVAL
                    {
                        self.flush_heartbeat(&task.uuid, &batch_buf, room_id, task_event_id)
                            .await;
                        batch_buf.clear();
                        batch_timer = Instant::now();
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    warn!(uuid = %task.uuid, error = %e, "error reading process stdout");
                    break;
                }
            }
        }

        if !batch_buf.is_empty() {
            self.flush_heartbeat(&task.uuid, &batch_buf, room_id, task_event_id)
                .await;
        }

        if let Some(mut stream) = unix_stream {
            let _ = stream.shutdown().await;
        }

        let status = child.wait().await?;
        let duration = start.elapsed().as_secs();

        let tail = Self::tail_lines(&all_lines);

        let (task_status, error_msg) = if status.success() {
            (TaskStatus::Success, None)
        } else {
            let code = status
                .code()
                .map(|c| c.to_string())
                .unwrap_or_else(|| "signal".to_string());
            (
                TaskStatus::Failed,
                Some(format!("{} exited with code {code}", payload.bin)),
            )
        };

        info!(
            uuid = %task.uuid,
            bin = %payload.bin,
            exit_code = ?status.code(),
            duration_seconds = duration,
            "process completed (P2P mode)"
        );

        let thread_id = if task_event_id.is_empty() {
            None
        } else {
            Some(task_event_id)
        };

        self.worker_client
            .post_result(
                &task.uuid,
                task_status,
                Some(serde_json::Value::String(tail)),
                error_msg,
                duration,
                room_id,
                thread_id,
            )
            .await?;

        if let Err(e) = tokio::fs::remove_file(&socket_path).await {
            warn!(
                uuid = %task.uuid,
                socket_path = %socket_path,
                error = %e,
                "failed to clean up socket file"
            );
        } else {
            debug!(
                uuid = %task.uuid,
                socket_path = %socket_path,
                "cleaned up socket file"
            );
        }

        Ok(())
    }

    /// Probe whether a binary is available on this host.
    pub async fn probe_bin_version(bin: &str) -> Option<String> {
        match Command::new(bin)
            .arg("--version")
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .output()
            .await
        {
            Ok(output) if output.status.success() => {
                let raw = String::from_utf8_lossy(&output.stdout);
                let version = raw
                    .split_whitespace()
                    .find(|s| s.chars().next().is_some_and(|c| c.is_ascii_digit()))
                    .map(|s| s.trim().to_string());
                debug!(bin = %bin, version = ?version, "probed binary version");
                version
            }
            Ok(output) => {
                warn!(
                    bin = %bin,
                    exit_code = ?output.status.code(),
                    "{bin} --version returned non-zero"
                );
                None
            }
            Err(e) => {
                warn!(bin = %bin, error = %e, "failed to run {bin} --version");
                None
            }
        }
    }

    /// Build a generic capability advertisement for this worker.
    ///
    /// The advertisement describes what binaries are available on this host.
    /// The input schema is generic: `{bin, args, env, cwd}`.
    pub async fn build_capability_advertisement(
        &self,
        available_bins: &[&str],
    ) -> CapabilityAdvertisement {
        let worker_id = self.worker_client.worker_id().to_string();
        let host = hostname::get()
            .map(|h| h.to_string_lossy().to_string())
            .unwrap_or_else(|_| "unknown".into());

        let mut tools = Vec::new();

        for bin in available_bins {
            let version = Self::probe_bin_version(bin).await;
            let healthy = version.is_some();

            let tool = WorkerTool {
                name: (*bin).to_string(),
                version,
                description: format!("Process executor for {bin}"),
                healthy,
                input_schema: Self::build_process_input_schema(),
            };
            tools.push(tool);
        }

        CapabilityAdvertisement {
            worker_id,
            host,
            tools,
        }
    }

    /// Build the generic process input schema.
    ///
    /// This describes the `{bin, args, env, cwd}` payload format that all
    /// process tasks must conform to.
    pub fn build_process_input_schema() -> InputSchema {
        let mut properties = HashMap::new();
        properties.insert(
            "bin".into(),
            SchemaProperty {
                r#type: "string".into(),
                description: "Binary to execute".into(),
            },
        );
        properties.insert(
            "args".into(),
            SchemaProperty {
                r#type: "array".into(),
                description: "Command-line arguments".into(),
            },
        );
        properties.insert(
            "env".into(),
            SchemaProperty {
                r#type: "object".into(),
                description: "Environment variables (key-value pairs)".into(),
            },
        );
        properties.insert(
            "cwd".into(),
            SchemaProperty {
                r#type: "string".into(),
                description: "Working directory (absolute path)".into(),
            },
        );

        InputSchema {
            r#type: "object".into(),
            properties,
            required: vec!["bin".into()],
        }
    }

    pub async fn publish_capability_advertisement(
        &self,
        available_bins: &[&str],
        room_id: &RoomId,
    ) -> Result<()> {
        let ad = self.build_capability_advertisement(available_bins).await;
        self.worker_client
            .publish_capability_advertisement(&ad, room_id)
            .await
    }

    fn tail_lines(all_lines: &[String]) -> String {
        all_lines
            .iter()
            .rev()
            .take(TAIL_LINES)
            .collect::<Vec<_>>()
            .into_iter()
            .rev()
            .cloned()
            .collect::<Vec<_>>()
            .join("\n")
    }

    async fn flush_heartbeat(
        &self,
        task_uuid: &str,
        batch: &str,
        room_id: &RoomId,
        task_event_id: &str,
    ) {
        let thread_id = if task_event_id.is_empty() {
            None
        } else {
            Some(task_event_id)
        };
        if let Err(e) = self
            .worker_client
            .post_heartbeat(task_uuid, Some(batch.to_string()), room_id, thread_id)
            .await
        {
            warn!(
                uuid = %task_uuid,
                error = %e,
                "failed to post heartbeat"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_process_payload_from_task_payload() {
        let payload = serde_json::json!({
            "bin": "jcode",
            "args": ["run", "--ndjson", "do the thing"],
            "env": {"SOME_VAR": "value"},
            "cwd": "/home/user/project"
        });

        let pp = ProcessPayload::from_task_payload(&payload).unwrap();
        assert_eq!(pp.bin, "jcode");
        assert_eq!(pp.args, vec!["run", "--ndjson", "do the thing"]);
        assert_eq!(pp.env.get("SOME_VAR").unwrap(), "value");
        assert_eq!(pp.cwd, Some(PathBuf::from("/home/user/project")));
    }

    #[test]
    fn test_process_payload_minimal() {
        let payload = serde_json::json!({
            "bin": "echo"
        });

        let pp = ProcessPayload::from_task_payload(&payload).unwrap();
        assert_eq!(pp.bin, "echo");
        assert!(pp.args.is_empty());
        assert!(pp.env.is_empty());
        assert!(pp.cwd.is_none());
    }

    #[test]
    fn test_process_payload_missing_bin() {
        let payload = serde_json::json!({
            "args": ["run"],
        });

        let pp = ProcessPayload::from_task_payload(&payload);
        assert!(pp.is_none());
    }

    #[test]
    fn test_process_payload_claude_example() {
        let payload = serde_json::json!({
            "bin": "claude",
            "args": ["--print", "--permission-mode", "bypassPermissions", "--output-format", "stream-json", "do the thing"],
            "env": {"CLAUDE_CODE_AUTO_COMPACT_WINDOW": "700000"},
            "cwd": "/home/user/project"
        });

        let pp = ProcessPayload::from_task_payload(&payload).unwrap();
        assert_eq!(pp.bin, "claude");
        assert_eq!(pp.args.len(), 6);
        assert_eq!(
            pp.env.get("CLAUDE_CODE_AUTO_COMPACT_WINDOW").unwrap(),
            "700000"
        );
    }

    #[test]
    fn test_build_process_input_schema() {
        let schema = ProcessWorker::build_process_input_schema();
        assert_eq!(schema.r#type, "object");
        assert_eq!(schema.required, vec!["bin"]);

        let expected_props = ["bin", "args", "env", "cwd"];
        for prop in &expected_props {
            assert!(
                schema.properties.contains_key(*prop),
                "schema should contain property '{prop}'"
            );
        }
        assert_eq!(schema.properties.len(), expected_props.len());
    }

    #[test]
    fn test_build_process_input_schema_round_trips_json() {
        let schema = ProcessWorker::build_process_input_schema();
        let json = serde_json::to_string(&schema).unwrap();
        let parsed: InputSchema = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.r#type, "object");
        assert_eq!(parsed.required, vec!["bin"]);
        assert_eq!(parsed.properties.len(), 4);
    }

    #[test]
    fn test_process_payload_with_empty_env() {
        let payload = serde_json::json!({
            "bin": "ls",
            "args": ["-la"],
            "env": {}
        });

        let pp = ProcessPayload::from_task_payload(&payload).unwrap();
        assert_eq!(pp.bin, "ls");
        assert_eq!(pp.args, vec!["-la"]);
        assert!(pp.env.is_empty());
    }
}
