use std::path::Path;
use std::process::Command;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;
use tracing::{info, warn};

use crate::protocol::{Request, Response, ErrorResponse};

/// Connect to the daemon's Unix socket. If not running, spawn it.
pub async fn connect_or_spawn(profile: &str) -> anyhow::Result<UnixStream> {
    let sock = crate::daemon::transport::unix::socket_path(profile);

    // Try connecting
    if let Ok(stream) = UnixStream::connect(&sock).await {
        return Ok(stream);
    }

    // Check PID file for stale daemon
    let pid_file = crate::daemon::transport::unix::pid_path(profile);
    if pid_file.exists() {
        let pid_str = std::fs::read_to_string(&pid_file).unwrap_or_default();
        if let Ok(pid) = pid_str.trim().parse::<u32>() {
            let alive = Path::new(&format!("/proc/{}", pid)).exists();
            if !alive {
                warn!(pid, "removing stale daemon PID file");
                let _ = std::fs::remove_file(&pid_file);
                let _ = std::fs::remove_file(&sock);
            } else {
                tokio::time::sleep(std::time::Duration::from_millis(500)).await;
                if let Ok(stream) = UnixStream::connect(&sock).await {
                    return Ok(stream);
                }
            }
        }
    }

    // Spawn daemon
    info!(profile, "spawning daemon");
    let exe = std::env::current_exe()?;
    Command::new(&exe)
        .args(["_daemon", "--profile", profile, "--detach"])
        .spawn()?;

    // Poll for socket readiness
    for _ in 0..100 {
        tokio::time::sleep(std::time::Duration::from_millis(100)).await;
        if let Ok(stream) = UnixStream::connect(&sock).await {
            return Ok(stream);
        }
    }

    anyhow::bail!("daemon failed to start within 10 seconds")
}

/// Send a JSON-RPC request and return the response.
pub async fn send_request(
    stream: &mut BufReader<tokio::net::unix::OwnedReadHalf>,
    writer: &mut tokio::net::unix::OwnedWriteHalf,
    method: &str,
    params: Option<serde_json::Value>,
    id: i64,
) -> anyhow::Result<serde_json::Value> {
    let req = Request::new(id, method, params);
    let json = serde_json::to_string(&req)?;
    writer.write_all(json.as_bytes()).await?;
    writer.write_all(b"\n").await?;
    writer.flush().await?;

    let mut line = String::new();
    loop {
        line.clear();
        let n = stream.read_line(&mut line).await?;
        if n == 0 {
            anyhow::bail!("daemon disconnected");
        }
        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        let value: serde_json::Value = serde_json::from_str(trimmed)?;
        if value.get("id").is_some() {
            if value.get("error").is_some() {
                let err: ErrorResponse = serde_json::from_value(value)?;
                anyhow::bail!("daemon error {}: {}", err.error.code, err.error.message);
            }
            let resp: Response = serde_json::from_value(value)?;
            return Ok(resp.result);
        }
        // Otherwise it's a notification — skip for non-streaming calls
    }
}
