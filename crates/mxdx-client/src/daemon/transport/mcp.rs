use std::sync::Arc;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tracing::info;

use crate::daemon::handler::Handler;
use crate::protocol::IncomingMessage;

/// Run MCP server on stdin/stdout. This is a foreground operation
/// (used by `mxdx-client daemon mcp`).
///
/// MCP uses JSON-RPC 2.0 over stdio — one JSON object per line on stdin,
/// responses on stdout. This is the same protocol as our Unix socket
/// transport, so we can reuse the handler directly.
pub async fn serve_stdio(handler: Arc<Handler>) -> anyhow::Result<()> {
    info!("MCP stdio transport started");

    let stdin = tokio::io::stdin();
    let mut stdout = tokio::io::stdout();
    let mut reader = BufReader::new(stdin);

    let (notif_tx, mut notif_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    // Forward notifications to stdout
    let mut stdout_notif = tokio::io::stdout();
    tokio::spawn(async move {
        while let Some(msg) = notif_rx.recv().await {
            let _ = stdout_notif.write_all(msg.as_bytes()).await;
            let _ = stdout_notif.write_all(b"\n").await;
            let _ = stdout_notif.flush().await;
        }
    });

    let mut line = String::new();
    loop {
        line.clear();
        let n = reader.read_line(&mut line).await?;
        if n == 0 {
            break; // EOF
        }

        let trimmed = line.trim();
        if trimmed.is_empty() {
            continue;
        }

        match IncomingMessage::parse(trimmed) {
            Ok(IncomingMessage::Request(req)) => {
                let response = handler.handle_request(&req, &notif_tx).await;
                stdout.write_all(response.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
            Ok(IncomingMessage::Notification(_)) => {}
            Err(e) => {
                let err = crate::protocol::ErrorResponse::new(
                    crate::protocol::RequestId::Number(0),
                    crate::protocol::error::PARSE_ERROR,
                    format!("invalid JSON: {}", e),
                );
                let err_json = serde_json::to_string(&err).unwrap_or_default();
                stdout.write_all(err_json.as_bytes()).await?;
                stdout.write_all(b"\n").await?;
                stdout.flush().await?;
            }
        }
    }

    Ok(())
}
