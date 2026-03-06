use mxdx_web::routes::build_router;
use mxdx_web::state::AppState;
use tokio::net::TcpListener;

#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter("mxdx_web=debug,tower_http=debug")
        .init();

    let state = AppState::new();
    let app = build_router(state);

    let listener = TcpListener::bind("127.0.0.1:3000")
        .await
        .expect("failed to bind");

    tracing::info!("mxdx-web listening on {}", listener.local_addr().unwrap());

    axum::serve(listener, app).await.expect("server error");
}
