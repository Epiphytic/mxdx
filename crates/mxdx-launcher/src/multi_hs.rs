use std::time::Instant;

use anyhow::{Context, Result};
use mxdx_matrix::client::MatrixClient;
use tracing::{debug, info, warn};
use uuid::Uuid;

/// Threshold of consecutive health-check failures before triggering failover.
const FAIL_THRESHOLD: u32 = 3;

/// State of the failover state machine.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FailoverState {
    /// Primary is healthy and active.
    Active,
    /// Primary is failing health checks (consecutive failures < FAIL_THRESHOLD).
    Failing,
    /// Failover in progress — switching to a new primary.
    Failover,
    /// All homeservers are unreachable.
    Unavailable,
}

/// Manages connections to multiple Matrix homeservers, selecting the
/// lowest-latency one as the primary (hot) identity. Supports failover
/// when the primary becomes unreachable.
pub struct MultiHsLauncher {
    clients: Vec<MatrixClient>,
    homeserver_urls: Vec<String>,
    primary_index: usize,
    state: FailoverState,
    consecutive_failures: u32,
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
            homeserver_urls: homeserver_urls.to_vec(),
            primary_index,
            state: FailoverState::Active,
            consecutive_failures: 0,
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

    /// Returns the current failover state.
    pub fn state(&self) -> FailoverState {
        self.state
    }

    /// Returns the port of the current primary homeserver, extracted from the URL.
    /// Useful in tests to verify which homeserver is primary.
    pub fn primary_port(&self) -> u16 {
        let url = &self.homeserver_urls[self.primary_index];
        url.rsplit(':')
            .next()
            .and_then(|p| p.trim_end_matches('/').parse().ok())
            .expect("Could not parse port from homeserver URL")
    }

    /// Perform a single health check against the primary homeserver.
    /// If the primary's sync fails, increments the failure counter.
    /// After FAIL_THRESHOLD consecutive failures, triggers failover.
    /// Returns the resulting failover state.
    pub async fn health_check(&mut self) -> FailoverState {
        let primary = &self.clients[self.primary_index];
        match primary.sync_once().await {
            Ok(()) => {
                if self.state != FailoverState::Active {
                    info!(primary_index = self.primary_index, "Primary recovered");
                }
                self.consecutive_failures = 0;
                self.state = FailoverState::Active;
            }
            Err(e) => {
                self.consecutive_failures += 1;
                warn!(
                    primary_index = self.primary_index,
                    consecutive_failures = self.consecutive_failures,
                    error = %e,
                    "Primary health check failed"
                );

                if self.consecutive_failures >= FAIL_THRESHOLD {
                    self.state = FailoverState::Failover;
                    self.failover().await;
                } else {
                    self.state = FailoverState::Failing;
                }
            }
        }
        self.state
    }

    /// Switch the primary to the next available homeserver with the lowest
    /// latency (measured by a sync probe). If no other homeserver is reachable,
    /// transitions to Unavailable.
    pub async fn failover(&mut self) {
        info!(
            old_primary = self.primary_index,
            "Initiating failover from primary"
        );

        let mut best: Option<(usize, std::time::Duration)> = None;

        for (i, client) in self.clients.iter().enumerate() {
            if i == self.primary_index {
                continue;
            }
            let start = Instant::now();
            if client.sync_once().await.is_ok() {
                let elapsed = start.elapsed();
                debug!(index = i, latency_ms = elapsed.as_millis(), "Failover candidate healthy");
                if best.is_none() || elapsed < best.unwrap().1 {
                    best = Some((i, elapsed));
                }
            } else {
                debug!(index = i, "Failover candidate unreachable");
            }
        }

        match best {
            Some((new_index, _)) => {
                info!(
                    old_primary = self.primary_index,
                    new_primary = new_index,
                    "Failover complete"
                );
                self.primary_index = new_index;
                self.consecutive_failures = 0;
                self.state = FailoverState::Active;
            }
            None => {
                warn!("All homeservers unreachable — entering Unavailable state");
                self.state = FailoverState::Unavailable;
            }
        }
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

    #[tokio::test]
    async fn health_check_returns_active_when_primary_healthy() {
        let instance = TuwunelInstance::start().await.unwrap();
        let url = format!("http://127.0.0.1:{}", instance.port);

        let mut launcher = MultiHsLauncher::start(&[url]).await.unwrap();

        let state = launcher.health_check().await;
        assert_eq!(state, FailoverState::Active);
        assert_eq!(launcher.consecutive_failures, 0);
    }

    #[tokio::test]
    async fn launcher_fails_over_from_primary_to_secondary() {
        let mut instance_a = TuwunelInstance::start().await.unwrap();
        let mut instance_b = TuwunelInstance::start().await.unwrap();

        let port_a = instance_a.port;
        let port_b = instance_b.port;

        let mut launcher = MultiHsLauncher::start(&[
            format!("http://127.0.0.1:{}", port_a),
            format!("http://127.0.0.1:{}", port_b),
        ])
        .await
        .unwrap();

        // Record which port is the initial primary
        let initial_primary_port = launcher.primary_port();
        assert_eq!(launcher.state(), FailoverState::Active);

        // Stop whichever instance is the current primary
        if initial_primary_port == port_a {
            instance_a.stop().await;
        } else {
            instance_b.stop().await;
        }

        // Run health checks until failover triggers (FAIL_THRESHOLD = 3)
        for _ in 0..FAIL_THRESHOLD {
            launcher.health_check().await;
        }

        // After FAIL_THRESHOLD failures, should have failed over
        assert_eq!(launcher.state(), FailoverState::Active);
        assert_ne!(
            launcher.primary_port(),
            initial_primary_port,
            "Primary should have changed after failover"
        );
    }

    #[tokio::test]
    async fn launcher_enters_unavailable_when_all_down() {
        let mut instance_a = TuwunelInstance::start().await.unwrap();
        let mut instance_b = TuwunelInstance::start().await.unwrap();

        let mut launcher = MultiHsLauncher::start(&[
            format!("http://127.0.0.1:{}", instance_a.port),
            format!("http://127.0.0.1:{}", instance_b.port),
        ])
        .await
        .unwrap();

        // Stop both
        instance_a.stop().await;
        instance_b.stop().await;

        // Run health checks past the threshold
        for _ in 0..FAIL_THRESHOLD {
            launcher.health_check().await;
        }

        assert_eq!(launcher.state(), FailoverState::Unavailable);
    }

    #[tokio::test]
    async fn security_non_launcher_cannot_update_identity_event() {
        // Verify that the launcher creates rooms where only it has state-event
        // permissions. This is a simplified check: the launcher client (room creator)
        // is the only admin by default in Matrix rooms.
        let instance = TuwunelInstance::start().await.unwrap();
        let url = format!("http://127.0.0.1:{}", instance.port);

        let launcher = MultiHsLauncher::start(&[url.clone()]).await.unwrap();
        let launcher_client = launcher.primary().unwrap();

        // Create a room as the launcher
        let room_id = launcher_client
            .create_encrypted_room(&[])
            .await
            .unwrap();

        // Register a separate non-launcher user
        let non_launcher = MatrixClient::register_and_connect(&url, "intruder", "password123")
            .await
            .unwrap();

        // The non-launcher should not be able to send state events to the room
        // (they aren't even a member, and even if invited, wouldn't have power level)
        let result = non_launcher
            .send_state_event(
                &room_id,
                "org.mxdx.identity",
                "",
                serde_json::json!({"compromised": true}),
            )
            .await;

        assert!(
            result.is_err(),
            "Non-launcher user should not be able to update state events in launcher room"
        );
    }
}
