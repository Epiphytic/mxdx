use std::collections::HashMap;
use std::time::{Duration, Instant};

use serde_json::Value;
use tracing::{debug, info, warn};

use crate::error::{MatrixClientError, Result};
use crate::rooms::LauncherTopology;
use crate::MatrixClient;
use matrix_sdk::ruma::RoomId;

const FAIL_WINDOW: Duration = Duration::from_secs(5 * 60);
const FAIL_THRESHOLD: usize = 5;
const MAX_SEEN_EVENTS: usize = 10_000;
const EVICT_BATCH: usize = 2_000;

/// Per-server account credentials for connecting to a homeserver.
#[derive(Debug, Clone)]
pub struct ServerAccount {
    pub homeserver: String,
    pub username: String,
    pub password: String,
}

/// Entry for a connected server with its measured latency.
struct ServerEntry {
    client: MatrixClient,
    server: String,
    password: String,
    latency_ms: f64,
}

/// Circuit breaker state per server.
struct CircuitBreaker {
    failures: Vec<Instant>,
    status: ServerStatus,
}

impl CircuitBreaker {
    fn new() -> Self {
        Self {
            failures: Vec::new(),
            status: ServerStatus::Healthy,
        }
    }
}

/// Health status of a server in the multi-homeserver pool.
#[derive(Clone, Copy, PartialEq, Debug)]
pub enum ServerStatus {
    Healthy,
    Down,
}

/// Health info for a server, returned by `server_health()`.
#[derive(Debug, Clone)]
pub struct ServerHealth {
    pub server: String,
    pub status: ServerStatus,
    pub latency_ms: f64,
}

/// Multi-homeserver Matrix client with circuit breaker failover.
///
/// Wraps N `MatrixClient` instances. Sends through the preferred server
/// (lowest latency or explicitly pinned). Circuit breaker triggers failover
/// on repeated failures. Events received from multiple servers are
/// deduplicated by event ID.
pub struct MultiHsClient {
    entries: Vec<ServerEntry>,
    preferred_index: usize,
    preferred_override: Option<String>,
    breakers: HashMap<String, CircuitBreaker>,
    seen_events: HashMap<String, Instant>,
    /// Insertion-order tracking for FIFO eviction of seen events.
    seen_order: Vec<String>,
    on_preferred_change: Option<Box<dyn Fn(&str, &str) + Send + Sync>>,
}

impl MultiHsClient {
    /// Connect to multiple homeservers, measure latency, and select preferred.
    ///
    /// Logs into each homeserver sequentially via `MatrixClient::login_and_connect()`,
    /// measures latency by timing an initial `sync_once()`, and selects the
    /// preferred server (explicit override or lowest latency).
    pub async fn connect(
        accounts: &[ServerAccount],
        preferred_server: Option<&str>,
    ) -> Result<Self> {
        if accounts.is_empty() {
            return Err(MatrixClientError::Other(anyhow::anyhow!(
                "MultiHsClient::connect requires at least one server account"
            )));
        }

        let mut entries = Vec::with_capacity(accounts.len());

        for account in accounts {
            let client = MatrixClient::login_and_connect(
                &account.homeserver,
                &account.username,
                &account.password,
            )
            .await?;

            // Measure latency via a sync cycle
            let start = Instant::now();
            let latency_ms = match client.sync_once().await {
                Ok(()) => start.elapsed().as_secs_f64() * 1000.0,
                Err(_) => f64::MAX, // Failed sync gets worst latency
            };

            info!(
                server = %account.homeserver,
                latency_ms = latency_ms,
                "Connected to homeserver"
            );

            entries.push(ServerEntry {
                client,
                server: account.homeserver.clone(),
                password: account.password.clone(),
                latency_ms,
            });
        }

        let preferred_override = preferred_server.map(String::from);

        // Select preferred index
        let preferred_index = if let Some(ref override_server) = preferred_override {
            entries
                .iter()
                .position(|e| e.server == *override_server)
                .unwrap_or(0)
        } else {
            entries
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.latency_ms.partial_cmp(&b.latency_ms).unwrap())
                .map(|(i, _)| i)
                .unwrap_or(0)
        };

        // Initialize circuit breakers
        let mut breakers = HashMap::new();
        for entry in &entries {
            breakers.insert(entry.server.clone(), CircuitBreaker::new());
        }

        info!(
            preferred = %entries[preferred_index].server,
            latency_ms = entries[preferred_index].latency_ms,
            server_count = entries.len(),
            "MultiHsClient ready"
        );

        Ok(Self {
            entries,
            preferred_index,
            preferred_override,
            breakers,
            seen_events: HashMap::new(),
            seen_order: Vec::new(),
            on_preferred_change: None,
        })
    }

    /// Create a `MultiHsClient` from pre-connected `MatrixClient` instances.
    ///
    /// This is the test-friendly constructor that skips login. Each entry is
    /// a `(server_name, client, latency_ms)` tuple. Password is empty (no UIA).
    pub fn from_clients(
        clients: Vec<(String, MatrixClient, f64)>,
        preferred_server: Option<&str>,
    ) -> Self {
        let entries: Vec<ServerEntry> = clients
            .into_iter()
            .map(|(server, client, latency_ms)| ServerEntry {
                client,
                server,
                password: String::new(),
                latency_ms,
            })
            .collect();

        let preferred_override = preferred_server.map(String::from);

        let preferred_index = if let Some(ref override_server) = preferred_override {
            entries
                .iter()
                .position(|e| e.server == *override_server)
                .unwrap_or(0)
        } else {
            entries
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.latency_ms.partial_cmp(&b.latency_ms).unwrap())
                .map(|(i, _)| i)
                .unwrap_or(0)
        };

        let mut breakers = HashMap::new();
        for entry in &entries {
            breakers.insert(entry.server.clone(), CircuitBreaker::new());
        }

        Self {
            entries,
            preferred_index,
            preferred_override,
            breakers,
            seen_events: HashMap::new(),
            seen_order: Vec::new(),
            on_preferred_change: None,
        }
    }

    /// Register a callback invoked when the preferred server changes.
    /// The callback receives `(new_server, old_server)`.
    pub fn on_preferred_change<F>(&mut self, cb: F)
    where
        F: Fn(&str, &str) + Send + Sync + 'static,
    {
        self.on_preferred_change = Some(Box::new(cb));
    }

    /// Returns a reference to the preferred (active) `MatrixClient`.
    pub fn preferred(&self) -> &MatrixClient {
        &self.entries[self.preferred_index].client
    }

    /// Returns the explicitly pinned preferred server, if any.
    pub fn preferred_override(&self) -> Option<&str> {
        self.preferred_override.as_deref()
    }

    /// Returns the user ID from the preferred server.
    pub fn user_id(&self) -> &matrix_sdk::ruma::UserId {
        self.preferred().user_id()
    }

    /// Returns the preferred server name.
    pub fn preferred_server(&self) -> &str {
        &self.entries[self.preferred_index].server
    }

    /// Number of connected servers.
    pub fn server_count(&self) -> usize {
        self.entries.len()
    }

    /// Whether this is a single-server setup (skips breaker/dedup logic).
    pub fn is_single_server(&self) -> bool {
        self.entries.len() <= 1
    }

    /// Returns health info for all connected servers.
    pub fn server_health(&self) -> Vec<ServerHealth> {
        self.entries
            .iter()
            .map(|entry| {
                let status = self
                    .breakers
                    .get(&entry.server)
                    .map(|b| b.status)
                    .unwrap_or(ServerStatus::Healthy);
                ServerHealth {
                    server: entry.server.clone(),
                    status,
                    latency_ms: entry.latency_ms,
                }
            })
            .collect()
    }

    /// Returns all user IDs across all connected servers.
    pub fn all_user_ids(&self) -> Vec<String> {
        self.entries
            .iter()
            .map(|e| e.client.user_id().to_string())
            .collect()
    }

    // ── Sending API (routes through preferred, records success/failure) ──

    /// Send a custom event to a room via the preferred server.
    pub async fn send_event(&mut self, room_id: &RoomId, payload: Value) -> Result<String> {
        let idx = self.preferred_index;
        match self.entries[idx].client.send_event(room_id, payload).await {
            Ok(event_id) => {
                self.record_success(idx);
                Ok(event_id)
            }
            Err(err) => {
                self.record_failure(idx);
                Err(err)
            }
        }
    }

    /// Send a threaded event via the preferred server.
    pub async fn send_threaded_event(
        &mut self,
        room_id: &RoomId,
        event_type: &str,
        thread_root: &str,
        content: Value,
    ) -> Result<String> {
        let idx = self.preferred_index;
        match self.entries[idx]
            .client
            .send_threaded_event(room_id, event_type, thread_root, content)
            .await
        {
            Ok(event_id) => {
                self.record_success(idx);
                Ok(event_id)
            }
            Err(err) => {
                self.record_failure(idx);
                Err(err)
            }
        }
    }

    /// Send a state event via the preferred server.
    pub async fn send_state_event(
        &mut self,
        room_id: &RoomId,
        event_type: &str,
        state_key: &str,
        content: Value,
    ) -> Result<()> {
        let idx = self.preferred_index;
        match self.entries[idx]
            .client
            .send_state_event(room_id, event_type, state_key, content)
            .await
        {
            Ok(()) => {
                self.record_success(idx);
                Ok(())
            }
            Err(err) => {
                self.record_failure(idx);
                Err(err)
            }
        }
    }

    /// Get or create a launcher space via the preferred server.
    pub async fn get_or_create_launcher_space(
        &mut self,
        name: &str,
    ) -> Result<LauncherTopology> {
        let idx = self.preferred_index;
        match self.entries[idx]
            .client
            .get_or_create_launcher_space(name)
            .await
        {
            Ok(topology) => {
                self.record_success(idx);
                Ok(topology)
            }
            Err(err) => {
                self.record_failure(idx);
                Err(err)
            }
        }
    }

    /// Find a launcher space. On multi-server setups, queries all healthy servers.
    pub async fn find_launcher_space(
        &mut self,
        name: &str,
    ) -> Result<Option<LauncherTopology>> {
        if self.is_single_server() {
            return self.entries[0].client.find_launcher_space(name).await;
        }

        // Query all healthy servers, return first hit
        for i in 0..self.entries.len() {
            let status = self
                .breakers
                .get(&self.entries[i].server)
                .map(|b| b.status)
                .unwrap_or(ServerStatus::Healthy);
            if status == ServerStatus::Down {
                continue;
            }

            match self.entries[i].client.find_launcher_space(name).await {
                Ok(Some(topology)) => {
                    self.record_success(i);
                    return Ok(Some(topology));
                }
                Ok(None) => {
                    self.record_success(i);
                }
                Err(_) => {
                    self.record_failure(i);
                }
            }
        }
        Ok(None)
    }

    /// Perform a single sync via the preferred server.
    pub async fn sync_once(&mut self) -> Result<()> {
        let idx = self.preferred_index;
        match self.entries[idx].client.sync_once().await {
            Ok(()) => {
                self.record_success(idx);
                Ok(())
            }
            Err(err) => {
                self.record_failure(idx);
                Err(err)
            }
        }
    }

    /// Join a room via the preferred server.
    pub async fn join_room(&mut self, room_id: &RoomId) -> Result<()> {
        let idx = self.preferred_index;
        match self.entries[idx].client.join_room(room_id).await {
            Ok(()) => {
                self.record_success(idx);
                Ok(())
            }
            Err(err) => {
                self.record_failure(idx);
                Err(err)
            }
        }
    }

    /// Wait for E2EE key exchange on the preferred server.
    pub async fn wait_for_key_exchange(
        &mut self,
        room_id: &RoomId,
        timeout: Duration,
    ) -> Result<()> {
        let idx = self.preferred_index;
        match self.entries[idx]
            .client
            .wait_for_key_exchange(room_id, timeout)
            .await
        {
            Ok(()) => {
                self.record_success(idx);
                Ok(())
            }
            Err(err) => {
                self.record_failure(idx);
                Err(err)
            }
        }
    }

    // ── Receiving API ──

    /// Sync and collect events from a room.
    ///
    /// For single-server setups, delegates directly. For multi-server setups,
    /// collects from all healthy servers and deduplicates by event ID.
    pub async fn sync_and_collect_events(
        &mut self,
        room_id: &RoomId,
        timeout: Duration,
    ) -> Result<Vec<Value>> {
        if self.is_single_server() {
            return self.entries[0]
                .client
                .sync_and_collect_events(room_id, timeout)
                .await;
        }

        // Collect from all healthy servers, deduplicate
        let mut all_events: Vec<Value> = Vec::new();

        for i in 0..self.entries.len() {
            let status = self
                .breakers
                .get(&self.entries[i].server)
                .map(|b| b.status)
                .unwrap_or(ServerStatus::Healthy);
            if status == ServerStatus::Down {
                continue;
            }

            match self.entries[i]
                .client
                .sync_and_collect_events(room_id, timeout)
                .await
            {
                Ok(events) => {
                    self.record_success(i);
                    for event in events {
                        if let Some(event_id) = event.get("event_id").and_then(|v| v.as_str()) {
                            if !self.is_duplicate(event_id) {
                                all_events.push(event);
                            }
                        } else {
                            // No event_id — deliver anyway
                            all_events.push(event);
                        }
                    }
                }
                Err(err) => {
                    self.record_failure(i);
                    debug!(
                        server = %self.entries[i].server,
                        error = %err,
                        "Failed to collect events from server"
                    );
                }
            }
        }

        Ok(all_events)
    }

    // ── Deduplication ──

    /// Check if an event ID has already been seen. If not, records it.
    /// Returns `true` if the event is a duplicate.
    ///
    /// Uses FIFO eviction when the seen set exceeds `MAX_SEEN_EVENTS`.
    pub fn is_duplicate(&mut self, event_id: &str) -> bool {
        if self.is_single_server() {
            return false;
        }

        if self.seen_events.contains_key(event_id) {
            return true;
        }

        let now = Instant::now();
        self.seen_events.insert(event_id.to_string(), now);
        self.seen_order.push(event_id.to_string());

        // Evict oldest batch if over limit
        if self.seen_events.len() > MAX_SEEN_EVENTS {
            let to_evict = self.seen_order.drain(..EVICT_BATCH).collect::<Vec<_>>();
            for key in to_evict {
                self.seen_events.remove(&key);
            }
        }

        false
    }

    // ── Cross-Signing Trust Sync ──

    /// Bootstrap cross-signing on ALL connected servers and verify own identity.
    /// This must be called after connect() to ensure all identities are ready
    /// for cross-signing operations. Password from each ServerAccount is used
    /// for UIA if required.
    pub async fn bootstrap_all_cross_signing(&mut self) -> std::result::Result<(), Vec<(String, MatrixClientError)>> {
        if self.entries.is_empty() {
            return Ok(());
        }

        let mut errors = Vec::new();

        for entry in &self.entries {
            let password = if entry.password.is_empty() {
                None
            } else {
                Some(entry.password.as_str())
            };

            // Bootstrap cross-signing keys
            if let Err(e) = entry.client.bootstrap_cross_signing_if_needed(password).await {
                warn!(
                    server = %entry.server,
                    error = %e,
                    "Failed to bootstrap cross-signing"
                );
                errors.push((entry.server.clone(), e));
                continue;
            }

            // Verify own identity
            if let Err(e) = entry.client.verify_own_identity().await {
                warn!(
                    server = %entry.server,
                    error = %e,
                    "Failed to verify own identity"
                );
                errors.push((entry.server.clone(), e));
                continue;
            }

            info!(
                server = %entry.server,
                user_id = %entry.client.user_id(),
                "Cross-signing bootstrapped and own identity verified"
            );
        }

        if errors.len() == self.entries.len() {
            // All servers failed — return the errors
            return Err(errors);
        }

        Ok(())
    }

    /// Synchronize trust across all connected servers.
    ///
    /// For each server, collects the set of verified user localparts.
    /// Then, for any localpart verified on one server but not another,
    /// verifies the equivalent user on the other server.
    ///
    /// This ensures that when `@client:ca1` is verified by `@worker:ca1`,
    /// `@worker:ca2` will also verify `@client:ca2` — making failover seamless.
    ///
    /// Call this after bootstrap and periodically in the event loop.
    pub async fn sync_trust(&mut self, room_id: &RoomId) {
        if self.is_single_server() {
            return;
        }

        // Phase 1: Collect verified localparts from each server
        let mut all_verified_localparts: std::collections::HashSet<String> =
            std::collections::HashSet::new();

        for entry in &self.entries {
            match entry.client.get_verified_user_ids_in_room(room_id).await {
                Ok(verified_users) => {
                    for uid in &verified_users {
                        all_verified_localparts.insert(uid.localpart().to_string());
                    }
                }
                Err(e) => {
                    debug!(
                        server = %entry.server,
                        error = %e,
                        "Failed to get verified users, skipping"
                    );
                }
            }
        }

        if all_verified_localparts.is_empty() {
            return;
        }

        // Phase 2: Cross-verify — ensure each localpart is verified on all servers
        for localpart in &all_verified_localparts {
            for entry in &self.entries {
                let server_name = entry.client.user_id().server_name();
                let target_user_id_str = format!("@{localpart}:{server_name}");

                let target_uid = match matrix_sdk::ruma::UserId::parse(&target_user_id_str) {
                    Ok(uid) => uid,
                    Err(_) => continue,
                };

                // Skip if already verified on this server
                match entry.client.is_user_verified(&target_uid).await {
                    Ok(true) => continue,
                    Ok(false) => {}
                    Err(_) => continue,
                }

                // Verify the user on this server
                match entry.client.verify_user(&target_uid).await {
                    Ok(()) => {
                        info!(
                            server = %entry.server,
                            user = %target_user_id_str,
                            "Cross-server trust sync: verified user"
                        );
                    }
                    Err(e) => {
                        debug!(
                            server = %entry.server,
                            user = %target_user_id_str,
                            error = %e,
                            "Cross-server trust sync: failed to verify (user may not exist on this server)"
                        );
                    }
                }
            }
        }
    }

    /// Convenience: bootstrap cross-signing on all servers and sync trust for a room.
    /// Best-effort — logs warnings but doesn't fail if some servers can't bootstrap.
    pub async fn bootstrap_and_sync_trust(&mut self, room_id: &RoomId) {
        if let Err(errors) = self.bootstrap_all_cross_signing().await {
            for (server, e) in &errors {
                warn!(
                    server = %server,
                    error = %e,
                    "Cross-signing bootstrap failed on server"
                );
            }
        }
        self.sync_trust(room_id).await;
    }

    // ── Circuit Breaker ──

    /// Record a failure for a server. If failures exceed the threshold within
    /// the sliding window, marks the server as down and triggers failover.
    pub fn record_failure(&mut self, server_index: usize) {
        if self.is_single_server() {
            return;
        }

        let server = self.entries[server_index].server.clone();
        let now = Instant::now();

        if let Some(breaker) = self.breakers.get_mut(&server) {
            // Prune failures outside the window
            breaker.failures.retain(|t| now.duration_since(*t) < FAIL_WINDOW);
            breaker.failures.push(now);

            if breaker.failures.len() >= FAIL_THRESHOLD {
                // Check if ALL servers are failing — if so, reset everything
                let all_failing = self.entries.iter().all(|e| {
                    self.breakers
                        .get(&e.server)
                        .map(|b| {
                            b.status == ServerStatus::Down || b.failures.len() >= FAIL_THRESHOLD
                        })
                        .unwrap_or(false)
                });

                if all_failing {
                    warn!("All servers failing — resetting all circuit breakers");
                    for breaker in self.breakers.values_mut() {
                        breaker.failures.clear();
                        breaker.status = ServerStatus::Healthy;
                    }
                    return;
                }

                warn!(server = %server, "Server marked as DOWN — circuit breaker tripped");
                // Re-borrow after the all_failing check
                if let Some(breaker) = self.breakers.get_mut(&server) {
                    breaker.status = ServerStatus::Down;
                }

                if server_index == self.preferred_index {
                    self.trigger_failover();
                }
            }
        }
    }

    /// Record a success for a server. Clears failure history and marks healthy.
    pub fn record_success(&mut self, server_index: usize) {
        if self.is_single_server() {
            return;
        }

        let server = &self.entries[server_index].server;
        if let Some(breaker) = self.breakers.get_mut(server) {
            breaker.failures.clear();
            if breaker.status == ServerStatus::Down {
                info!(server = %server, "Server recovered — marking healthy");
                breaker.status = ServerStatus::Healthy;
            }
        }
    }

    /// Trigger failover to the lowest-latency healthy server.
    fn trigger_failover(&mut self) {
        let mut best_idx: Option<usize> = None;
        let mut best_latency = f64::MAX;

        for (i, entry) in self.entries.iter().enumerate() {
            if i == self.preferred_index {
                continue;
            }
            let status = self
                .breakers
                .get(&entry.server)
                .map(|b| b.status)
                .unwrap_or(ServerStatus::Healthy);
            if status == ServerStatus::Down {
                continue;
            }
            if entry.latency_ms < best_latency {
                best_latency = entry.latency_ms;
                best_idx = Some(i);
            }
        }

        if let Some(new_idx) = best_idx {
            let old_server = self.entries[self.preferred_index].server.clone();
            let new_server = self.entries[new_idx].server.clone();
            self.preferred_index = new_idx;

            info!(
                from = %old_server,
                to = %new_server,
                latency_ms = best_latency,
                "Failover: preferred server changed"
            );

            if let Some(ref cb) = self.on_preferred_change {
                cb(&new_server, &old_server);
            }
        } else {
            warn!("Failover: no healthy servers available");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Create a test-only multi-hs client with fake entries for failover tests.
    /// Uses transmute-free approach: we build a struct with entries that have
    /// the server/latency fields set, but the MatrixClient field is never accessed.
    struct FakeMultiHsClient {
        preferred_index: usize,
        breakers: HashMap<String, CircuitBreaker>,
        servers: Vec<(String, f64)>, // (server_name, latency_ms)
        seen_events: HashMap<String, Instant>,
        seen_order: Vec<String>,
    }

    impl FakeMultiHsClient {
        fn new(servers: Vec<(&str, f64)>) -> Self {
            let mut breakers = HashMap::new();
            let servers: Vec<(String, f64)> = servers
                .into_iter()
                .map(|(s, l)| {
                    breakers.insert(s.to_string(), CircuitBreaker::new());
                    (s.to_string(), l)
                })
                .collect();

            // Select lowest latency as preferred
            let preferred_index = servers
                .iter()
                .enumerate()
                .min_by(|(_, a), (_, b)| a.1.partial_cmp(&b.1).unwrap())
                .map(|(i, _)| i)
                .unwrap_or(0);

            Self {
                preferred_index,
                breakers,
                servers,
                seen_events: HashMap::new(),
                seen_order: Vec::new(),
            }
        }

        fn is_single_server(&self) -> bool {
            self.servers.len() <= 1
        }

        fn is_duplicate(&mut self, event_id: &str) -> bool {
            if self.is_single_server() {
                return false;
            }
            if self.seen_events.contains_key(event_id) {
                return true;
            }
            let now = Instant::now();
            self.seen_events.insert(event_id.to_string(), now);
            self.seen_order.push(event_id.to_string());
            if self.seen_events.len() > MAX_SEEN_EVENTS {
                let to_evict = self.seen_order.drain(..EVICT_BATCH).collect::<Vec<_>>();
                for key in to_evict {
                    self.seen_events.remove(&key);
                }
            }
            false
        }

        fn record_failure(&mut self, server_index: usize) {
            if self.is_single_server() {
                return;
            }
            let server = self.servers[server_index].0.clone();
            let now = Instant::now();

            if let Some(breaker) = self.breakers.get_mut(&server) {
                breaker
                    .failures
                    .retain(|t| now.duration_since(*t) < FAIL_WINDOW);
                breaker.failures.push(now);

                if breaker.failures.len() >= FAIL_THRESHOLD {
                    let all_failing = self.servers.iter().all(|(s, _)| {
                        self.breakers
                            .get(s)
                            .map(|b| {
                                b.status == ServerStatus::Down
                                    || b.failures.len() >= FAIL_THRESHOLD
                            })
                            .unwrap_or(false)
                    });

                    if all_failing {
                        for breaker in self.breakers.values_mut() {
                            breaker.failures.clear();
                            breaker.status = ServerStatus::Healthy;
                        }
                        return;
                    }

                    if let Some(breaker) = self.breakers.get_mut(&server) {
                        breaker.status = ServerStatus::Down;
                    }

                    if server_index == self.preferred_index {
                        self.trigger_failover();
                    }
                }
            }
        }

        fn record_success(&mut self, server_index: usize) {
            if self.is_single_server() {
                return;
            }
            let server = &self.servers[server_index].0;
            if let Some(breaker) = self.breakers.get_mut(server) {
                breaker.failures.clear();
                if breaker.status == ServerStatus::Down {
                    breaker.status = ServerStatus::Healthy;
                }
            }
        }

        fn trigger_failover(&mut self) {
            let mut best_idx: Option<usize> = None;
            let mut best_latency = f64::MAX;

            for (i, (server, latency)) in self.servers.iter().enumerate() {
                if i == self.preferred_index {
                    continue;
                }
                let status = self
                    .breakers
                    .get(server)
                    .map(|b| b.status)
                    .unwrap_or(ServerStatus::Healthy);
                if status == ServerStatus::Down {
                    continue;
                }
                if *latency < best_latency {
                    best_latency = *latency;
                    best_idx = Some(i);
                }
            }

            if let Some(new_idx) = best_idx {
                self.preferred_index = new_idx;
            }
        }
    }

    #[test]
    fn single_server_skips_breaker() {
        let mut client = FakeMultiHsClient::new(vec![("server-a", 50.0)]);
        assert!(client.is_single_server());

        // Failures should be no-ops on single server
        for _ in 0..10 {
            client.record_failure(0);
        }
        assert_eq!(
            client.breakers.get("server-a").unwrap().status,
            ServerStatus::Healthy
        );
        assert!(client.breakers.get("server-a").unwrap().failures.is_empty());
    }

    #[test]
    fn single_server_dedup_always_returns_false() {
        let mut client = FakeMultiHsClient::new(vec![("server-a", 50.0)]);
        assert!(!client.is_duplicate("$event1"));
        assert!(!client.is_duplicate("$event1")); // Same event, still false for single server
        assert!(client.seen_events.is_empty());
    }

    #[test]
    fn dedup_tracks_events_and_evicts() {
        let mut client = FakeMultiHsClient::new(vec![("server-a", 50.0), ("server-b", 100.0)]);

        assert!(!client.is_duplicate("$event1"));
        assert!(client.is_duplicate("$event1")); // Now it's a duplicate
        assert!(!client.is_duplicate("$event2"));
        assert_eq!(client.seen_events.len(), 2);

        // Fill up to trigger eviction
        for i in 0..MAX_SEEN_EVENTS {
            client.is_duplicate(&format!("$fill-{}", i));
        }

        // After eviction, size should be MAX_SEEN - EVICT_BATCH + 1 (approximately)
        assert!(client.seen_events.len() <= MAX_SEEN_EVENTS);
        // The oldest events should have been evicted
        assert!(!client.seen_events.contains_key("$event1"));
    }

    #[test]
    fn circuit_breaker_threshold() {
        let mut client = FakeMultiHsClient::new(vec![
            ("server-a", 50.0),
            ("server-b", 100.0),
            ("server-c", 150.0),
        ]);

        // Preferred starts at server-a (lowest latency)
        assert_eq!(client.preferred_index, 0);

        // 4 failures should not trip the breaker
        for _ in 0..FAIL_THRESHOLD - 1 {
            client.record_failure(0);
        }
        assert_eq!(
            client.breakers.get("server-a").unwrap().status,
            ServerStatus::Healthy
        );

        // 5th failure trips it
        client.record_failure(0);
        assert_eq!(
            client.breakers.get("server-a").unwrap().status,
            ServerStatus::Down
        );

        // Should have failed over to server-b (next lowest latency)
        assert_eq!(client.preferred_index, 1);
    }

    #[test]
    fn all_down_resets_breakers() {
        let mut client = FakeMultiHsClient::new(vec![("server-a", 50.0), ("server-b", 100.0)]);

        // Trip server-a
        for _ in 0..FAIL_THRESHOLD {
            client.record_failure(0);
        }
        assert_eq!(
            client.breakers.get("server-a").unwrap().status,
            ServerStatus::Down
        );

        // Now trip server-b — this should trigger all-down reset
        for _ in 0..FAIL_THRESHOLD {
            client.record_failure(1);
        }

        // All breakers should be reset
        assert_eq!(
            client.breakers.get("server-a").unwrap().status,
            ServerStatus::Healthy
        );
        assert_eq!(
            client.breakers.get("server-b").unwrap().status,
            ServerStatus::Healthy
        );
        assert!(client.breakers.get("server-a").unwrap().failures.is_empty());
        assert!(client.breakers.get("server-b").unwrap().failures.is_empty());
    }

    #[test]
    fn failover_picks_lowest_latency_healthy() {
        let mut client = FakeMultiHsClient::new(vec![
            ("server-a", 50.0),  // preferred (lowest latency)
            ("server-b", 200.0), // highest latency
            ("server-c", 100.0), // middle latency
        ]);

        assert_eq!(client.preferred_index, 0); // server-a

        // Trip server-a
        for _ in 0..FAIL_THRESHOLD {
            client.record_failure(0);
        }

        // Should failover to server-c (100ms), not server-b (200ms)
        assert_eq!(client.preferred_index, 2);
        assert_eq!(client.servers[client.preferred_index].0, "server-c");
    }

    #[test]
    fn record_success_clears_failures_and_recovers() {
        let mut client = FakeMultiHsClient::new(vec![
            ("server-a", 50.0),
            ("server-b", 100.0),
            ("server-c", 150.0),
        ]);

        // Trip server-a (not preferred won't trigger failover test, so trip server-b)
        // First accumulate some failures on server-b
        for _ in 0..FAIL_THRESHOLD {
            client.record_failure(1);
        }
        assert_eq!(
            client.breakers.get("server-b").unwrap().status,
            ServerStatus::Down
        );

        // Success should recover it
        client.record_success(1);
        assert_eq!(
            client.breakers.get("server-b").unwrap().status,
            ServerStatus::Healthy
        );
        assert!(client.breakers.get("server-b").unwrap().failures.is_empty());
    }
}
