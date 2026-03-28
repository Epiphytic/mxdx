pub mod config;
pub mod router;
pub mod watchlist;

use anyhow::Result;
use config::CoordinatorRuntimeConfig;

/// Run the coordinator with the given configuration.
/// This is the main entry point for the coordinator binary.
pub async fn run_coordinator(config: CoordinatorRuntimeConfig) -> Result<()> {
    tracing::info!(
        room = ?config.coordinator.room,
        "starting mxdx-coordinator"
    );

    // Initialize router
    let _router = router::Router::new();
    tracing::info!("router initialized");

    // Initialize watchlist
    let _watchlist = watchlist::Watchlist::new();
    tracing::info!("watchlist initialized");

    tracing::info!("coordinator initialized, ready for events");

    // Main event loop will be connected to Matrix sync in integration.
    // For now, the coordinator starts up, initializes all components, and exits.

    Ok(())
}
