use std::path::{Path, PathBuf};
use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixListener;
use tokio::sync::Mutex;
use tracing::{info, warn, error};

use crate::daemon::handler::{Handler, NotificationSink};
use crate::protocol::IncomingMessage;

/// Start listening on a Unix socket. Spawns a task per client connection.
pub async fn serve(
    socket_path: &Path,
    handler: Arc<Handler>,
) -> anyhow::Result<()> {
    if socket_path.exists() {
        std::fs::remove_file(socket_path)?;
    }

    if let Some(parent) = socket_path.parent() {
        std::fs::create_dir_all(parent)?;
    }

    let listener = UnixListener::bind(socket_path)?;

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(socket_path, std::fs::Permissions::from_mode(0o600))?;
    }

    info!(path = %socket_path.display(), "Unix socket transport listening");

    loop {
        match listener.accept().await {
            Ok((stream, _addr)) => {
                let handler = Arc::clone(&handler);
                tokio::spawn(async move {
                    if let Err(e) = handle_client(stream, handler).await {
                        warn!(error = %e, "client connection error");
                    }
                });
            }
            Err(e) => {
                error!(error = %e, "failed to accept Unix socket connection");
            }
        }
    }
}

async fn handle_client(
    stream: tokio::net::UnixStream,
    handler: Arc<Handler>,
) -> anyhow::Result<()> {
    let (reader, writer) = stream.into_split();
    let mut reader = BufReader::new(reader);
    let writer = Arc::new(Mutex::new(writer));

    let (notif_tx, mut notif_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let writer_clone = Arc::clone(&writer);
    let notif_writer = tokio::spawn(async move {
        while let Some(msg) = notif_rx.recv().await {
            let mut w = writer_clone.lock().await;
            if w.write_all(msg.as_bytes()).await.is_err() {
                break;
            }
            if !msg.ends_with('\n') {
                if w.write_all(b"\n").await.is_err() {
                    break;
                }
            }
            let _ = w.flush().await;
        }
    });

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break;
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match IncomingMessage::parse(trimmed) {
            Ok(IncomingMessage::Request(req)) => {
                let response = handler.handle_request(&req, &notif_tx).await;
                let mut w = writer.lock().await;
                w.write_all(response.as_bytes()).await?;
                w.write_all(b"\n").await?;
                w.flush().await?;
            }
            Ok(IncomingMessage::Notification(_notif)) => {
                // Client-to-daemon notifications — handle later
            }
            Err(e) => {
                let err = crate::protocol::ErrorResponse::new(
                    crate::protocol::RequestId::Number(0),
                    crate::protocol::error::PARSE_ERROR,
                    format!("invalid JSON: {}", e),
                );
                let err_json = serde_json::to_string(&err).unwrap_or_default();
                let mut w = writer.lock().await;
                w.write_all(err_json.as_bytes()).await?;
                w.write_all(b"\n").await?;
                w.flush().await?;
            }
        }
    }

    notif_writer.abort();
    Ok(())
}

/// Compute the socket path for a profile.
pub fn socket_path(profile: &str) -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(".mxdx")
        .join("daemon")
        .join(format!("{}.sock", profile))
}

/// Compute the PID file path for a profile.
pub fn pid_path(profile: &str) -> PathBuf {
    dirs::home_dir()
        .expect("no home directory")
        .join(".mxdx")
        .join("daemon")
        .join(format!("{}.pid", profile))
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    #[tokio::test]
    async fn unix_socket_roundtrip() {
        let dir = tempfile::tempdir().unwrap();
        let sock = dir.path().join("test.sock");

        let handler = Arc::new(Handler::new("test"));
        let sock_clone = sock.clone();

        let server = tokio::spawn(async move {
            serve(&sock_clone, handler).await.unwrap();
        });

        tokio::time::sleep(std::time::Duration::from_millis(100)).await;

        let stream = UnixStream::connect(&sock).await.unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);

        writer.write_all(br#"{"jsonrpc":"2.0","id":1,"method":"daemon.status"}"#).await.unwrap();
        writer.write_all(b"\n").await.unwrap();
        writer.flush().await.unwrap();

        let mut line = String::new();
        reader.read_line(&mut line).await.unwrap();
        assert!(line.contains("\"profile\":\"test\""));
        assert!(line.contains("\"uptime_seconds\""));

        server.abort();
    }
}
