pub mod dashboard;
pub mod sse;
pub mod static_files;

use axum::Router;
use http::header::HeaderName;
use http::HeaderValue;
use tower_http::set_header::SetResponseHeaderLayer;

use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    // No CORS layer — browser default same-origin policy applies.
    // Adding Access-Control-Allow-Origin would weaken security.
    let csp = SetResponseHeaderLayer::overriding(
        HeaderName::from_static("content-security-policy"),
        HeaderValue::from_static(
            "default-src 'self'; script-src 'self' 'wasm-unsafe-eval'; style-src 'self' 'unsafe-inline'; connect-src 'self' https: wss:; worker-src 'self'",
        ),
    );

    Router::new()
        .merge(dashboard::routes())
        .merge(sse::routes())
        .merge(static_files::routes())
        .layer(csp)
        .with_state(state)
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::body::Body;
    use http::{Request, StatusCode};
    use tower::ServiceExt;

    fn build_test_app() -> Router {
        let state = AppState::new();
        build_router(state)
    }

    fn build_test_app_with_launchers(launchers: Vec<crate::state::LauncherInfo>) -> Router {
        let state = AppState::new();
        // We can't async-set in a non-async fn, so use try_write
        *state.launchers.try_write().unwrap() = launchers;
        build_router(state)
    }

    #[tokio::test]
    async fn dashboard_returns_200() {
        let app = build_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/dashboard")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);
    }

    #[tokio::test]
    async fn dashboard_shows_launcher_cards() {
        use crate::state::{LauncherInfo, LauncherStatus};

        let launchers = vec![LauncherInfo {
            id: "launcher-abc".into(),
            status: LauncherStatus::Online,
            cpu_usage_percent: 42.5,
            memory_used_bytes: 4_000_000_000,
            memory_total_bytes: 8_000_000_000,
            hostname: "worker-01".into(),
        }];

        let app = build_test_app_with_launchers(launchers);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/dashboard")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let html = String::from_utf8(body.to_vec()).unwrap();

        assert!(html.contains("launcher-abc"), "should contain launcher ID");
        assert!(html.contains("42.5"), "should contain CPU value");
        assert!(html.contains("online"), "should contain status");
    }

    #[tokio::test]
    async fn manifest_returns_json() {
        let app = build_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/manifest.webmanifest")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let ct = response
            .headers()
            .get("content-type")
            .expect("should have content-type");
        assert_eq!(ct, "application/manifest+json");

        let body = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        let json: serde_json::Value =
            serde_json::from_slice(&body).expect("manifest should be valid JSON");
        assert_eq!(json["short_name"], "mxdx");
    }

    #[tokio::test]
    async fn csp_header_is_set() {
        let app = build_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/dashboard")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::OK);

        let csp = response
            .headers()
            .get("content-security-policy")
            .expect("should have CSP header");
        let csp_str = csp.to_str().unwrap();
        assert!(csp_str.contains("default-src 'self'"));
        assert!(csp_str.contains("script-src 'self' 'wasm-unsafe-eval'"));
        assert!(csp_str.contains("worker-src 'self'"));
    }

    #[tokio::test]
    async fn unknown_route_returns_spa_fallback() {
        let app = build_test_app();
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/nonexistent")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();
        // SPA fallback: returns index.html (200) or "not built" message
        // In test without dist/ dir, this returns the fallback message
        let status = response.status();
        assert!(
            status == StatusCode::OK || status == StatusCode::NOT_FOUND,
            "expected 200 or 404, got {status}"
        );
    }
}
