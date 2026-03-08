use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use http::header::CONTENT_TYPE;
use http::StatusCode;
use tower_http::services::ServeDir;

use crate::state::AppState;

const MANIFEST: &str = include_str!("../../static/manifest.webmanifest");
const SERVICE_WORKER: &str = include_str!("../../static/sw.js");

/// Path to the Vite-built web console output.
/// Set via MXDX_WEB_DIST env var, defaults to "packages/web-console/dist".
fn dist_dir() -> String {
    std::env::var("MXDX_WEB_DIST")
        .unwrap_or_else(|_| "packages/web-console/dist".to_string())
}

pub fn routes() -> Router<AppState> {
    let dist = dist_dir();

    // Serve static assets from Vite build output.
    // SPA fallback: non-file requests fall through to index.html.
    let serve_dir = ServeDir::new(&dist)
        .fallback(tower::service_fn(|_req| async {
            let dist = dist_dir();
            let index_path = std::path::PathBuf::from(&dist).join("index.html");
            match tokio::fs::read_to_string(&index_path).await {
                Ok(html) => Ok((
                    StatusCode::OK,
                    [(CONTENT_TYPE, "text/html; charset=utf-8")],
                    html,
                ).into_response()),
                Err(_) => Ok((
                    StatusCode::NOT_FOUND,
                    "Web console not built. Run: cd packages/web-console && npx vite build",
                ).into_response()),
            }
        }));

    Router::new()
        .route("/manifest.webmanifest", get(manifest_handler))
        .route("/sw.js", get(service_worker_handler))
        .fallback_service(serve_dir)
}

async fn manifest_handler() -> Response {
    (
        StatusCode::OK,
        [(CONTENT_TYPE, "application/manifest+json")],
        MANIFEST,
    )
        .into_response()
}

async fn service_worker_handler() -> Response {
    (
        StatusCode::OK,
        [(CONTENT_TYPE, "application/javascript")],
        SERVICE_WORKER,
    )
        .into_response()
}
