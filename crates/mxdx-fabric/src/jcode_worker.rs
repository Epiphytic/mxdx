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

        let start = Instant::now();
        let mut child = Command::new(&self.jcode_bin)
            .args(["--provider", "claude", "--ndjson", "run", prompt])
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

        let start = Instant::now();
        let mut child = Command::new(&self.jcode_bin)
            .args(["--provider", "claude", "--ndjson", "run", prompt])
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
