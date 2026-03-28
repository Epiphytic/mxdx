pub mod claim;
pub mod config;
pub mod failure;
pub mod index;
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

    // Initialize all components
    let _router = router::Router::new();
    let _watchlist = watchlist::Watchlist::new();
    let _claims = claim::ClaimTracker::new();
    let _index = index::CapabilityIndex::new();

    tracing::info!("coordinator initialized, ready for routing");

    // Main event loop will be connected to Matrix sync in integration.
    // For now, the coordinator starts up, initializes all components, and exits.

    Ok(())
}
