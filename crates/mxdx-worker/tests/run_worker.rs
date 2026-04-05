use mxdx_types::config::{AccountConfig, DefaultsConfig, WorkerConfig};
use mxdx_worker::config::WorkerRuntimeConfig;

#[tokio::test]
async fn worker_starts_with_default_config_and_returns_ok() {
    let defaults = DefaultsConfig {
        accounts: vec![AccountConfig {
            user_id: "@test-worker:localhost".into(),
            homeserver: "https://localhost:8448".into(),
            password: None,
        }],
        ..Default::default()
    };
    let worker = WorkerConfig::default();
    let config = WorkerRuntimeConfig::from_parts(defaults, worker);

    let result = mxdx_worker::run_worker(config).await;
    assert!(result.is_ok(), "run_worker should succeed: {result:?}");
}

#[tokio::test]
async fn worker_starts_with_no_accounts() {
    // Even with no accounts, the worker should start (falling back to @worker:localhost)
    let defaults = DefaultsConfig::default();
    let worker = WorkerConfig::default();
    let config = WorkerRuntimeConfig::from_parts(defaults, worker);

    let result = mxdx_worker::run_worker(config).await;
    assert!(result.is_ok(), "run_worker should succeed with no accounts: {result:?}");
}
