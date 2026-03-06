use axum::extract::State;
use axum::response::sse::{Event, KeepAlive, Sse};
use axum::routing::get;
use axum::Router;
use tokio_stream::wrappers::BroadcastStream;
use tokio_stream::StreamExt;

use crate::state::{AppState, LauncherInfo};

pub fn routes() -> Router<AppState> {
    Router::new().route("/sse/launchers", get(launcher_sse_handler))
}

async fn launcher_sse_handler(
    State(state): State<AppState>,
) -> Sse<impl tokio_stream::Stream<Item = Result<Event, std::convert::Infallible>>> {
    let rx = state.launcher_tx.subscribe();
    let stream = BroadcastStream::new(rx).filter_map(|result| match result {
        Ok(info) => Some(Ok(Event::default()
            .event("launcher-update")
            .data(render_launcher_oob_fragment(&info)))),
        Err(_) => None,
    });

    Sse::new(stream).keep_alive(KeepAlive::default())
}

fn render_launcher_oob_fragment(l: &LauncherInfo) -> String {
    let mem_pct = if l.memory_total_bytes > 0 {
        (l.memory_used_bytes as f64 / l.memory_total_bytes as f64) * 100.0
    } else {
        0.0
    };

    format!(
        r#"<div id="launcher-{id}" class="launcher-card" data-id="{id}" hx-swap-oob="true">
  <h2>{id}</h2>
  <span class="status status-{status}">{status}</span>
  <p>Host: {hostname}</p>
  <p>CPU: {cpu}%</p>
  <p>Memory: {mem:.1}%</p>
</div>"#,
        id = l.id,
        status = l.status,
        hostname = l.hostname,
        cpu = l.cpu_usage_percent,
        mem = mem_pct,
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::routes::build_router;
    use crate::state::LauncherStatus;
    use axum::body::Body;
    use http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn sse_endpoint_returns_200() {
        let state = AppState::new();
        let app = build_router(state);

        let response = app
            .oneshot(
                Request::builder()
                    .uri("/sse/launchers")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let content_type = response
            .headers()
            .get("content-type")
            .unwrap()
            .to_str()
            .unwrap();
        assert!(
            content_type.contains("text/event-stream"),
            "expected text/event-stream, got {content_type}"
        );
    }

    #[tokio::test]
    async fn sse_pushes_launcher_update() {
        let state = AppState::new();
        // Subscribe BEFORE building the router so we can verify the broadcast works
        let mut rx = state.launcher_tx.subscribe();

        let info = LauncherInfo {
            id: "launcher-xyz".into(),
            status: LauncherStatus::Online,
            cpu_usage_percent: 55.0,
            memory_used_bytes: 2_000_000_000,
            memory_total_bytes: 4_000_000_000,
            hostname: "node-42".into(),
        };

        // Trigger update through AppState
        state.update_launcher(info.clone()).await;

        // Verify the broadcast channel received the update
        let received = rx.recv().await.expect("should receive launcher update");
        assert_eq!(received.id, "launcher-xyz");
        assert_eq!(received.hostname, "node-42");
        assert_eq!(received.cpu_usage_percent, 55.0);

        // Verify the OOB fragment contains expected HTMX attributes
        let html = render_launcher_oob_fragment(&received);
        assert!(html.contains("hx-swap-oob=\"true\""));
        assert!(html.contains("launcher-xyz"));
        assert!(html.contains("node-42"));
        assert!(html.contains("55%"));
        assert!(html.contains("50.0%")); // 2GB/4GB memory

        // Verify state was also updated
        let launchers = state.launchers.read().await;
        assert_eq!(launchers.len(), 1);
        assert_eq!(launchers[0].id, "launcher-xyz");
    }

    #[test]
    fn render_oob_fragment_contains_expected_fields() {
        let info = LauncherInfo {
            id: "launcher-test".into(),
            status: LauncherStatus::Online,
            cpu_usage_percent: 33.3,
            memory_used_bytes: 1_000_000_000,
            memory_total_bytes: 2_000_000_000,
            hostname: "test-host".into(),
        };

        let html = render_launcher_oob_fragment(&info);
        assert!(html.contains("launcher-test"));
        assert!(html.contains("hx-swap-oob=\"true\""));
        assert!(html.contains("online"));
        assert!(html.contains("33.3"));
        assert!(html.contains("50.0")); // 1GB/2GB = 50%
        assert!(html.contains("test-host"));
    }
}
