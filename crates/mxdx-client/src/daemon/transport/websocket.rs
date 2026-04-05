use std::net::SocketAddr;
use std::sync::Arc;
use futures_util::{SinkExt, StreamExt};
use tokio::net::TcpListener;
use tokio_tungstenite::accept_async;
use tracing::{info, warn, error};

use crate::daemon::handler::{Handler, NotificationSink};
use crate::protocol::IncomingMessage;

/// Start a WebSocket server. Each connection gets its own handler task.
pub async fn serve(
    bind: &str,
    port: u16,
    handler: Arc<Handler>,
) -> anyhow::Result<SocketAddr> {
    let addr = format!("{}:{}", bind, port);
    let listener = TcpListener::bind(&addr).await?;
    let local_addr = listener.local_addr()?;
    info!(address = %local_addr, "WebSocket transport listening");

    tokio::spawn(async move {
        loop {
            match listener.accept().await {
                Ok((stream, peer)) => {
                    let handler = Arc::clone(&handler);
                    tokio::spawn(async move {
                        match accept_async(stream).await {
                            Ok(ws_stream) => {
                                if let Err(e) = handle_ws_client(ws_stream, handler).await {
                                    warn!(peer = %peer, error = %e, "WebSocket client error");
                                }
                            }
                            Err(e) => {
                                warn!(peer = %peer, error = %e, "WebSocket handshake failed");
                            }
                        }
                    });
                }
                Err(e) => {
                    error!(error = %e, "failed to accept TCP connection");
                }
            }
        }
    });

    Ok(local_addr)
}

async fn handle_ws_client(
    ws_stream: tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>,
    handler: Arc<Handler>,
) -> anyhow::Result<()> {
    let (mut ws_sink, mut ws_stream) = ws_stream.split();
    let (notif_tx, mut notif_rx) = tokio::sync::mpsc::unbounded_channel::<String>();

    let sink_handle = tokio::spawn(async move {
        while let Some(msg) = notif_rx.recv().await {
            if ws_sink
                .send(tokio_tungstenite::tungstenite::Message::Text(msg))
                .await
                .is_err()
            {
                break;
            }
        }
    });

    while let Some(msg) = ws_stream.next().await {
        let msg = msg?;
        if let tokio_tungstenite::tungstenite::Message::Text(text) = msg {
            match IncomingMessage::parse(&text) {
                Ok(IncomingMessage::Request(req)) => {
                    let response = handler.handle_request(&req, &notif_tx).await;
                    notif_tx.send(response).ok();
                }
                Ok(IncomingMessage::Notification(_)) => {
                    // Client-sent notifications are informational; no response required.
                }
                Err(e) => {
                    let err = crate::protocol::ErrorResponse::new(
                        crate::protocol::RequestId::Number(0),
                        crate::protocol::error::PARSE_ERROR,
                        format!("invalid JSON: {}", e),
                    );
                    notif_tx
                        .send(serde_json::to_string(&err).unwrap_or_default())
                        .ok();
                }
            }
        }
    }

    sink_handle.abort();
    Ok(())
}
