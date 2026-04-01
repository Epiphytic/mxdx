pub mod handler;
pub mod sessions;
pub mod subscriptions;
pub mod transport;

/// Run the daemon main loop. Placeholder — will be fully wired in Task 8.
pub async fn run_daemon(
    _config: crate::config::ClientRuntimeConfig,
    _profile: &str,
) -> anyhow::Result<()> {
    anyhow::bail!("Daemon mode not yet implemented — will be wired in Task 8")
}
