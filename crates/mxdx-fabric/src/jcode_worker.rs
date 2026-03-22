use std::path::PathBuf;
use std::process::Stdio;
use std::time::{Duration, Instant};

use anyhow::Result;
use mxdx_matrix::RoomId;
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

const ALLOWED_PROVIDERS: &[&str] = &[
    "jcode",
    "claude",
    "openai",
    "openrouter",
    "azure",
    "opencode",
    "opencode-go",
    "zai",
    "chutes",
    "cerebras",
    "openai-compatible",
    "cursor",
    "copilot",
    "gemini",
    "google",
    "auto",
];

const SHELL_METACHARACTERS: &[char] = &[
    ';', '|', '&', '$', '`', '(', ')', '{', '}', '<', '>', '\\', '\n', '\0',
];

#[derive(Debug, Default)]
pub enum OutputFormat {
    #[default]
    Ndjson,
    Json,
    Text,
}

#[derive(Debug)]
pub struct JcodeOptions {
    pub provider: Option<String>,
    pub cwd: Option<PathBuf>,
    pub model: Option<String>,
    pub resume_session: Option<String>,
    pub quiet: bool,
    pub no_update: bool,
    pub no_selfdev: bool,
    pub trace: bool,
    pub output_format: OutputFormat,
}

impl Default for JcodeOptions {
    fn default() -> Self {
        Self {
            provider: None,
            cwd: None,
            model: None,
            resume_session: None,
            quiet: false,
            no_update: true,
            no_selfdev: true,
            trace: false,
            output_format: OutputFormat::default(),
        }
    }
}

fn contains_shell_metachar(s: &str) -> bool {
    s.contains(SHELL_METACHARACTERS)
}

fn validate_model(s: &str) -> bool {
    if s.is_empty() || s.len() > 64 {
        return false;
    }
    s.chars()
        .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '.' || c == '_' || c == ':')
}

fn validate_session_id(s: &str) -> bool {
    if s.is_empty() || s.len() > 64 {
        return false;
    }
    s.chars().all(|c| c.is_ascii_alphanumeric() || c == '-')
}

fn validate_cwd(s: &str) -> Option<PathBuf> {
    if contains_shell_metachar(s) {
        return None;
    }
    let path = PathBuf::from(s);
    if !path.is_absolute() {
        return None;
    }
    if s.contains("..") {
        return None;
    }
    Some(path)
}

impl JcodeOptions {
    pub fn from_payload(payload: &serde_json::Map<String, serde_json::Value>) -> Self {
        let mut opts = Self::default();

        if let Some(v) = payload.get("provider").and_then(|v| v.as_str()) {
            if contains_shell_metachar(v) {
                warn!(provider = %v, "provider contains shell metacharacters, using default");
            } else if ALLOWED_PROVIDERS.contains(&v) {
                opts.provider = Some(v.to_string());
            } else {
                warn!(provider = %v, "unknown provider, using default");
            }
        }

        if let Some(v) = payload.get("cwd").and_then(|v| v.as_str()) {
            match validate_cwd(v) {
                Some(path) => opts.cwd = Some(path),
                None => warn!(cwd = %v, "invalid cwd path, ignoring"),
            }
        }

        if let Some(v) = payload.get("model").and_then(|v| v.as_str()) {
            if validate_model(v) {
                opts.model = Some(v.to_string());
            } else {
                warn!(model = %v, "invalid model name, ignoring");
            }
        }

        if let Some(v) = payload.get("resume_session").and_then(|v| v.as_str()) {
            if validate_session_id(v) {
                opts.resume_session = Some(v.to_string());
            } else {
                warn!("invalid resume_session id, ignoring");
            }
        }

        if let Some(v) = payload.get("quiet").and_then(|v| v.as_bool()) {
            opts.quiet = v;
        }

        if let Some(v) = payload.get("no_update").and_then(|v| v.as_bool()) {
            opts.no_update = v;
        }

        if let Some(v) = payload.get("no_selfdev").and_then(|v| v.as_bool()) {
            opts.no_selfdev = v;
        }

        if let Some(v) = payload.get("trace").and_then(|v| v.as_bool()) {
            opts.trace = v;
        }

        if let Some(v) = payload.get("output_format").and_then(|v| v.as_str()) {
            opts.output_format = match v {
                "ndjson" => OutputFormat::Ndjson,
                "json" => OutputFormat::Json,
                "text" => OutputFormat::Text,
                _ => {
                    warn!(output_format = %v, "unknown output format, using default ndjson");
                    OutputFormat::Ndjson
                }
            };
        }

        opts
    }

    pub fn build_args(&self, prompt: &str) -> Vec<String> {
        let mut args = Vec::new();

        let provider = self
            .provider
            .as_deref()
            .unwrap_or("claude");
        args.push("--provider".to_string());
        args.push(provider.to_string());

        if let Some(ref cwd) = self.cwd {
            args.push("-C".to_string());
            args.push(cwd.to_string_lossy().to_string());
        }

        if self.no_update {
            args.push("--no-update".to_string());
        }

        if self.no_selfdev {
            args.push("--no-selfdev".to_string());
        }

        if self.quiet {
            args.push("--quiet".to_string());
        }

        if self.trace {
            args.push("--trace".to_string());
        }

        if let Some(ref model) = self.model {
            args.push("-m".to_string());
            args.push(model.clone());
        }

        if let Some(ref session) = self.resume_session {
            args.push("--resume".to_string());
            args.push(session.clone());
        }

        args.push("run".to_string());

        match self.output_format {
            OutputFormat::Ndjson => args.push("--ndjson".to_string()),
            OutputFormat::Json => args.push("--json".to_string()),
            OutputFormat::Text => {}
        }

        args.push(prompt.to_string());

        args
    }
}

pub struct JcodeWorker {
    worker_client: WorkerClient,
    jcode_bin: PathBuf,
}

impl JcodeWorker {
    pub fn new(worker_client: WorkerClient, jcode_bin: Option<PathBuf>) -> Self {
        Self {
            worker_client,
            jcode_bin: jcode_bin.unwrap_or_else(|| PathBuf::from("jcode")),
        }
    }

    pub fn worker_client(&self) -> &WorkerClient {
        &self.worker_client
    }

    pub async fn run_task(&self, task: TaskEvent, room_id: &RoomId) -> Result<()> {
        if !self.worker_client.try_claim(&task, room_id).await? {
            debug!(
                uuid = %task.uuid,
                "claim lost, another worker took the task"
            );
            return Ok(());
        }

        info!(
            uuid = %task.uuid,
            "claim won, spawning jcode"
        );

        let use_p2p = task.p2p_stream && task.heartbeat_interval_seconds < 5;

        if use_p2p {
            self.run_task_p2p(&task, room_id).await
        } else {
            self.run_task_matrix(&task, room_id).await
        }
    }

    async fn run_task_matrix(&self, task: &TaskEvent, room_id: &RoomId) -> Result<()> {
        let prompt = task
            .payload
            .get("prompt")
            .and_then(|v| v.as_str())
            .or(task.plan.as_deref())
            .unwrap_or("no prompt provided");

        let opts = if let Some(obj) = task.payload.as_object() {
            JcodeOptions::from_payload(obj)
        } else {
            JcodeOptions::default()
        };

        let args = opts.build_args(prompt);

        let start = Instant::now();
        let mut child = Command::new(&self.jcode_bin)
            .args(&args)
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
                        self.flush_heartbeat(&task.uuid, &batch_buf, room_id).await;
                        batch_buf.clear();
                        batch_timer = Instant::now();
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    warn!(uuid = %task.uuid, error = %e, "error reading jcode stdout");
                    break;
                }
            }
        }

        if !batch_buf.is_empty() {
            self.flush_heartbeat(&task.uuid, &batch_buf, room_id).await;
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
                Some(format!("jcode exited with code {code}")),
            )
        };

        info!(
            uuid = %task.uuid,
            exit_code = ?status.code(),
            duration_seconds = duration,
            "jcode process completed"
        );

        self.worker_client
            .post_result(
                &task.uuid,
                task_status,
                Some(serde_json::Value::String(tail)),
                error_msg,
                duration,
                room_id,
            )
            .await?;

        Ok(())
    }

    async fn run_task_p2p(&self, task: &TaskEvent, room_id: &RoomId) -> Result<()> {
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

        let prompt = task
            .payload
            .get("prompt")
            .and_then(|v| v.as_str())
            .or(task.plan.as_deref())
            .unwrap_or("no prompt provided");

        let opts = if let Some(obj) = task.payload.as_object() {
            JcodeOptions::from_payload(obj)
        } else {
            JcodeOptions::default()
        };

        let args = opts.build_args(prompt);

        let start = Instant::now();
        let mut child = Command::new(&self.jcode_bin)
            .args(&args)
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
                        self.flush_heartbeat(&task.uuid, &batch_buf, room_id).await;
                        batch_buf.clear();
                        batch_timer = Instant::now();
                    }
                }
                Ok(None) => break,
                Err(e) => {
                    warn!(uuid = %task.uuid, error = %e, "error reading jcode stdout");
                    break;
                }
            }
        }

        if !batch_buf.is_empty() {
            self.flush_heartbeat(&task.uuid, &batch_buf, room_id).await;
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
                Some(format!("jcode exited with code {code}")),
            )
        };

        info!(
            uuid = %task.uuid,
            exit_code = ?status.code(),
            duration_seconds = duration,
            "jcode process completed (P2P mode)"
        );

        self.worker_client
            .post_result(
                &task.uuid,
                task_status,
                Some(serde_json::Value::String(tail)),
                error_msg,
                duration,
                room_id,
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

    async fn flush_heartbeat(&self, task_uuid: &str, batch: &str, room_id: &RoomId) {
        if let Err(e) = self
            .worker_client
            .post_heartbeat(task_uuid, Some(batch.to_string()), room_id)
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
    fn test_jcode_options_defaults() {
        let opts = JcodeOptions::default();
        let args = opts.build_args("my prompt");
        assert_eq!(
            args,
            vec![
                "--provider",
                "claude",
                "--no-update",
                "--no-selfdev",
                "run",
                "--ndjson",
                "my prompt",
            ]
        );
    }

    #[test]
    fn test_jcode_options_full() {
        let payload: serde_json::Map<String, serde_json::Value> = serde_json::from_str(
            r#"{
                "prompt": "do the thing",
                "provider": "openai",
                "cwd": "/home/user/project",
                "model": "claude-opus-4-6",
                "resume_session": "abc-123",
                "quiet": true,
                "no_update": true,
                "no_selfdev": true,
                "trace": true,
                "output_format": "json"
            }"#,
        )
        .unwrap();

        let opts = JcodeOptions::from_payload(&payload);
        let args = opts.build_args("do the thing");

        assert_eq!(opts.provider.as_deref(), Some("openai"));
        assert_eq!(opts.cwd, Some(PathBuf::from("/home/user/project")));
        assert_eq!(opts.model.as_deref(), Some("claude-opus-4-6"));
        assert_eq!(opts.resume_session.as_deref(), Some("abc-123"));
        assert!(opts.quiet);
        assert!(opts.no_update);
        assert!(opts.no_selfdev);
        assert!(opts.trace);

        assert!(args.contains(&"--provider".to_string()));
        assert!(args.contains(&"openai".to_string()));
        assert!(args.contains(&"-C".to_string()));
        assert!(args.contains(&"/home/user/project".to_string()));
        assert!(args.contains(&"-m".to_string()));
        assert!(args.contains(&"claude-opus-4-6".to_string()));
        assert!(args.contains(&"--resume".to_string()));
        assert!(args.contains(&"abc-123".to_string()));
        assert!(args.contains(&"--quiet".to_string()));
        assert!(args.contains(&"--trace".to_string()));
        assert!(args.contains(&"--no-update".to_string()));
        assert!(args.contains(&"--no-selfdev".to_string()));
        assert!(args.contains(&"--json".to_string()));
        assert!(args.contains(&"do the thing".to_string()));

        let run_idx = args.iter().position(|a| a == "run").unwrap();
        let json_idx = args.iter().position(|a| a == "--json").unwrap();
        let prompt_idx = args.iter().position(|a| a == "do the thing").unwrap();
        assert!(run_idx < json_idx);
        assert!(json_idx < prompt_idx);

        let provider_idx = args.iter().position(|a| a == "--provider").unwrap();
        assert!(provider_idx < run_idx);
    }

    #[test]
    fn test_jcode_options_sanitization() {
        let payload: serde_json::Map<String, serde_json::Value> = serde_json::from_str(
            r#"{
                "provider": "claude; rm -rf /",
                "model": "model$(whoami)",
                "cwd": "/home/user/../etc",
                "resume_session": "sess;id"
            }"#,
        )
        .unwrap();

        let opts = JcodeOptions::from_payload(&payload);

        assert_eq!(opts.provider, None);
        assert_eq!(opts.model, None);
        assert_eq!(opts.cwd, None);
        assert_eq!(opts.resume_session, None);

        let args = opts.build_args("safe prompt");
        assert_eq!(
            args,
            vec![
                "--provider",
                "claude",
                "--no-update",
                "--no-selfdev",
                "run",
                "--ndjson",
                "safe prompt",
            ]
        );
    }

    #[test]
    fn test_jcode_options_invalid_provider() {
        let payload: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"provider": "totally-unknown-provider"}"#).unwrap();

        let opts = JcodeOptions::from_payload(&payload);
        assert_eq!(opts.provider, None);

        let args = opts.build_args("test");
        assert_eq!(args[0], "--provider");
        assert_eq!(args[1], "claude");
    }

    #[test]
    fn test_jcode_options_text_format() {
        let payload: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"output_format": "text"}"#).unwrap();

        let opts = JcodeOptions::from_payload(&payload);
        let args = opts.build_args("hello");

        assert!(!args.contains(&"--ndjson".to_string()));
        assert!(!args.contains(&"--json".to_string()));
        assert!(args.contains(&"run".to_string()));
        assert!(args.contains(&"hello".to_string()));
    }

    #[test]
    fn test_jcode_options_no_update_disabled() {
        let payload: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"no_update": false, "no_selfdev": false}"#).unwrap();

        let opts = JcodeOptions::from_payload(&payload);
        assert!(!opts.no_update);
        assert!(!opts.no_selfdev);

        let args = opts.build_args("test");
        assert!(!args.contains(&"--no-update".to_string()));
        assert!(!args.contains(&"--no-selfdev".to_string()));
    }

    #[test]
    fn test_jcode_options_relative_cwd_rejected() {
        let payload: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"cwd": "relative/path"}"#).unwrap();

        let opts = JcodeOptions::from_payload(&payload);
        assert_eq!(opts.cwd, None);
    }

    #[test]
    fn test_jcode_options_model_too_long() {
        let long_model = "a".repeat(65);
        let json = format!(r#"{{"model": "{}"}}"#, long_model);
        let payload: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(&json).unwrap();

        let opts = JcodeOptions::from_payload(&payload);
        assert_eq!(opts.model, None);
    }

    #[test]
    fn test_jcode_options_empty_payload() {
        let payload: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{}"#).unwrap();

        let opts = JcodeOptions::from_payload(&payload);
        let args = opts.build_args("prompt");
        assert_eq!(
            args,
            vec![
                "--provider",
                "claude",
                "--no-update",
                "--no-selfdev",
                "run",
                "--ndjson",
                "prompt",
            ]
        );
    }

    #[test]
    fn test_jcode_options_cwd_with_shell_metachar() {
        let payload: serde_json::Map<String, serde_json::Value> =
            serde_json::from_str(r#"{"cwd": "/home/user/$(whoami)"}"#).unwrap();

        let opts = JcodeOptions::from_payload(&payload);
        assert_eq!(opts.cwd, None);
    }

    #[test]
    fn test_jcode_options_all_providers_accepted() {
        for provider in ALLOWED_PROVIDERS {
            let json = format!(r#"{{"provider": "{}"}}"#, provider);
            let payload: serde_json::Map<String, serde_json::Value> =
                serde_json::from_str(&json).unwrap();
            let opts = JcodeOptions::from_payload(&payload);
            assert_eq!(
                opts.provider.as_deref(),
                Some(*provider),
                "provider '{}' should be accepted",
                provider
            );
        }
    }
}
