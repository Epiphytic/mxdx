pub mod dashboard;

use axum::Router;
use http::Method;
use tower_http::cors::{CorsLayer, AllowOrigin};

use crate::state::AppState;

pub fn build_router(state: AppState) -> Router {
    let cors = CorsLayer::new()
        .allow_methods([Method::GET])
        .allow_origin(AllowOrigin::exact(
            "null".parse().expect("valid header value"),
        ));

    Router::new()
        .merge(dashboard::routes())
        .layer(cors)
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

    fn build_test_app_with_launchers(
        launchers: Vec<crate::state::LauncherInfo>,
    ) -> Router {
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
    async fn unknown_route_returns_404() {
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
        assert_eq!(response.status(), StatusCode::NOT_FOUND);
    }
}
