use axum::response::{IntoResponse, Response};
use axum::routing::get;
use axum::Router;
use http::header::CONTENT_TYPE;
use http::StatusCode;

use crate::state::AppState;

const MANIFEST: &str = include_str!("../../static/manifest.webmanifest");
const SERVICE_WORKER: &str = include_str!("../../static/sw.js");

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/manifest.webmanifest", get(manifest_handler))
        .route("/sw.js", get(service_worker_handler))
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
