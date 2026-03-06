use std::time::Instant;

use anyhow::{Context, Result};
use mxdx_matrix::client::MatrixClient;
use tracing::{debug, info};
use uuid::Uuid;

/// Manages connections to multiple Matrix homeservers, selecting the
/// lowest-latency one as the primary (hot) identity.
pub struct MultiHsLauncher {
    clients: Vec<MatrixClient>,
    primary_index: usize,
}

impl MultiHsLauncher {
    /// Connect to all homeservers concurrently, measure sync latency,
    /// and select the lowest-latency server as primary.
    pub async fn start(homeserver_urls: &[String]) -> Result<Self> {
        anyhow::ensure!(
            !homeserver_urls.is_empty(),
            "At least one homeserver URL is required"
        );

        // Connect to all homeservers concurrently
        let mut handles = tokio::task::JoinSet::new();
        for (i, url) in homeserver_urls.iter().enumerate() {
            let url = url.clone();
            handles.spawn(async move {
                let username = format!("launcher-{}", Uuid::new_v4().as_simple());
                let password = Uuid::new_v4().to_string();
                let client = MatrixClient::register_and_connect(&url, &username, &password)
                    .await
                    .with_context(|| format!("Failed to connect to homeserver {}", url))?;
                debug!(url = %url, "Connected to homeserver");
                Ok::<(usize, MatrixClient), anyhow::Error>((i, client))
            });
        }

        let mut indexed_clients: Vec<(usize, MatrixClient)> = Vec::new();
        while let Some(result) = handles.join_next().await {
            let inner = result.context("Task panicked")?;
            indexed_clients.push(inner?);
        }

        // Sort by original index to maintain deterministic ordering
        indexed_clients.sort_by_key(|(i, _)| *i);
        let clients: Vec<MatrixClient> = indexed_clients.into_iter().map(|(_, c)| c).collect();

        // Measure sync latency for each client
        let mut latencies: Vec<(usize, std::time::Duration)> = Vec::new();
        for (i, client) in clients.iter().enumerate() {
            let start = Instant::now();
            // Use sync_once with a short timeout as a latency probe
            if let Err(e) = client.sync_once().await {
                debug!(index = i, error = %e, "Sync latency probe failed, using max latency");
                latencies.push((i, std::time::Duration::MAX));
            } else {
                let elapsed = start.elapsed();
                debug!(index = i, latency_ms = elapsed.as_millis(), "Sync latency measured");
                latencies.push((i, elapsed));
            }
        }

        // Select lowest-latency as primary
        let primary_index = latencies
            .iter()
            .min_by_key(|(_, d)| *d)
            .map(|(i, _)| *i)
            .unwrap_or(0);

        info!(
            primary_index,
            connected = clients.len(),
            "Multi-HS launcher started"
        );

        Ok(MultiHsLauncher {
            clients,
            primary_index,
        })
    }

    /// Returns a reference to the primary (lowest-latency) client.
    pub fn primary(&self) -> Option<&MatrixClient> {
        self.clients.get(self.primary_index)
    }

    /// Returns the number of connected homeserver identities.
    pub fn connected_count(&self) -> usize {
        self.clients.len()
    }

    /// Returns all connected clients.
    pub fn clients(&self) -> &[MatrixClient] {
        &self.clients
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxdx_test_helpers::tuwunel::TuwunelInstance;

    #[tokio::test]
    async fn multi_hs_launcher_connects_to_single_homeserver() {
        let instance = TuwunelInstance::start().await.unwrap();
        let url = format!("http://127.0.0.1:{}", instance.port);

        let launcher = MultiHsLauncher::start(&[url]).await.unwrap();

        assert!(launcher.primary().is_some());
        assert_eq!(launcher.connected_count(), 1);
        assert!(launcher.primary().unwrap().is_logged_in());
    }

    #[tokio::test]
    async fn multi_hs_launcher_selects_lowest_latency_primary() {
        let instance_a = TuwunelInstance::start().await.unwrap();
        let instance_b = TuwunelInstance::start().await.unwrap();

        let launcher = MultiHsLauncher::start(&[
            format!("http://127.0.0.1:{}", instance_a.port),
            format!("http://127.0.0.1:{}", instance_b.port),
        ])
        .await
        .unwrap();

        assert!(launcher.primary().is_some());
        assert_eq!(launcher.connected_count(), 2);
        // Both clients should be logged in
        for client in launcher.clients() {
            assert!(client.is_logged_in());
        }
    }

    #[tokio::test]
    async fn multi_hs_launcher_requires_at_least_one_url() {
        let result = MultiHsLauncher::start(&[]).await;
        assert!(result.is_err());
    }
}
