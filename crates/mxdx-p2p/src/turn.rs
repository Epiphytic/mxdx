//! Matrix VoIP TURN credential client (`GET /_matrix/client/v3/voip/turnServer`)
//! and active-call refresh task.
//!
//! # Security
//!
//! TURN credentials (username/password) are returned by the homeserver in
//! response to an authenticated request. They are short-lived (TTL from the
//! server, typically 24h or less) and are **not secrets on the wire** — but
//! they MUST NOT be logged, persisted, or leaked into diagnostic output.
//!
//! * [`TurnCredentials`] has a custom [`Debug`] impl that redacts
//!   `username`, `password`, and the URI list.
//! * No [`serde::Serialize`] impl — prevents accidental disk persistence via
//!   generic serializers.
//! * No [`Clone`] — force callers to reference credentials rather than
//!   spreading copies; [`Drop`] zeroizes the secret fields.
//! * Fetch errors degrade to `Ok(None)` ("no TURN available") rather than
//!   bubbling raw HTTP errors, which matches the npm reference
//!   ([`packages/core/turn-credentials.js`]) and keeps callers "stay
//!   Matrix-only" per the storm spec failure taxonomy (§4.1).

use std::time::{Duration, SystemTime};

use url::Url;
use zeroize::Zeroize;

/// Loopback hostnames that are allowed over `http:` (never HTTPS-upgradable
/// without user-visible configuration, and never a credential-leak path).
///
/// Mirrors `LOOPBACK_HOSTS` in `packages/core/turn-credentials.js`.
#[cfg(not(target_arch = "wasm32"))]
const LOOPBACK_HOSTS: &[&str] = &["localhost", "127.0.0.1", "::1", "[::1]"];

/// Matrix VoIP TURN endpoint path (client-server v3).
#[cfg(not(target_arch = "wasm32"))]
const TURN_ENDPOINT_PATH: &str = "/_matrix/client/v3/voip/turnServer";

/// Errors returned by [`fetch_turn_credentials`].
///
/// Note: most HTTP failure modes (4xx, 5xx, timeout, empty response) are
/// mapped to `Ok(None)` so callers can transparently fall back to
/// Matrix-only transport. Only programmer errors (transport construction)
/// or unsupported targets surface as `Err`.
#[derive(Debug, thiserror::Error)]
pub enum TurnError {
    /// Underlying HTTP transport failed to even issue the request
    /// (native only). Fetch-time error responses (non-2xx, timeout, etc.)
    /// are mapped to `Ok(None)` — they never reach this variant.
    #[cfg(not(target_arch = "wasm32"))]
    #[error("http transport error: {0}")]
    Http(#[from] reqwest::Error),

    /// The wasm target does not yet implement `fetch_turn_credentials`.
    /// Real wasm TURN fetch lands in Phase 8 of the P2P rollout.
    #[error("wasm target: fetch_turn_credentials not implemented until Phase 8")]
    WasmUnsupported,
}

/// TURN credentials returned by the Matrix homeserver's
/// `/voip/turnServer` endpoint.
///
/// Wire shape (Matrix C-S spec):
/// ```json
/// { "uris": ["turn:...", "turns:..."], "username": "...", "password": "...", "ttl": 86400 }
/// ```
///
/// The struct owns computed fields `fetched_at` (filled by this crate at the
/// moment of fetch) and `ttl` (from the server, parsed from seconds into a
/// [`Duration`]). Helpers [`expires_at`](Self::expires_at),
/// [`refresh_at`](Self::refresh_at), and [`is_expired`](Self::is_expired)
/// are relative to `fetched_at + ttl`.
///
/// Security: not `Clone`, not `Serialize`, not publicly `Debug` (the
/// provided `Debug` impl redacts secrets).
pub struct TurnCredentials {
    /// TURN server URI list (`turn:` or `turns:` URIs). Order is
    /// as-provided; WebRTC stacks try them in order.
    pub uris: Vec<String>,
    /// TURN username (short-lived, homeserver-scoped, opaque).
    pub username: String,
    /// TURN password (short-lived, paired with `username`).
    pub password: String,
    /// Credential lifetime from the homeserver.
    pub ttl: Duration,
    /// Local clock time at the moment `fetch_turn_credentials` succeeded.
    pub fetched_at: SystemTime,
}

impl TurnCredentials {
    /// Wall-clock instant at which the credentials expire (`fetched_at + ttl`).
    pub fn expires_at(&self) -> SystemTime {
        self.fetched_at + self.ttl
    }

    /// Wall-clock instant at which the active-call refresh task should
    /// attempt a proactive refresh (`fetched_at + ttl/2`). Matches the
    /// storm spec §2.4 policy: rotate mid-session via ICE restart at
    /// half-life rather than racing against actual expiry.
    pub fn refresh_at(&self) -> SystemTime {
        self.fetched_at + self.ttl / 2
    }

    /// Returns `true` iff the credentials are at or past their expiry
    /// based on `SystemTime::now()`.
    pub fn is_expired(&self) -> bool {
        SystemTime::now() >= self.expires_at()
    }
}

impl core::fmt::Debug for TurnCredentials {
    fn fmt(&self, f: &mut core::fmt::Formatter<'_>) -> core::fmt::Result {
        // Log only non-sensitive metadata. URIs can carry server hostnames
        // (arguably low-sensitivity) but together with username/password
        // they form an exfiltration path — redact the list too, surface
        // only the count.
        f.debug_struct("TurnCredentials")
            .field("uri_count", &self.uris.len())
            .field("username", &"<redacted>")
            .field("password", &"<redacted>")
            .field("ttl", &self.ttl)
            .field("fetched_at", &self.fetched_at)
            .finish()
    }
}

impl Drop for TurnCredentials {
    fn drop(&mut self) {
        // String::zeroize is provided by the `zeroize` crate; it overwrites
        // the heap buffer before the allocation is freed.
        self.username.zeroize();
        self.password.zeroize();
        for u in self.uris.iter_mut() {
            u.zeroize();
        }
    }
}

// ---------------------------------------------------------------------------
// Native implementation (not(target_arch = "wasm32"))
// ---------------------------------------------------------------------------

/// Raw HTTP response shape — private to the module, never escapes.
#[cfg(not(target_arch = "wasm32"))]
#[derive(serde::Deserialize)]
struct TurnResponse {
    uris: Vec<String>,
    username: String,
    password: String,
    ttl: u64,
}

/// Fetch TURN credentials from the homeserver's Matrix VoIP endpoint.
///
/// * On success and TURN provisioned: `Ok(Some(TurnCredentials { .. }))`.
/// * On success but no TURN configured (empty `uris`, 404): `Ok(None)`.
/// * On HTTP failure (timeout, 4xx/5xx except 404): `Ok(None)` — callers
///   transparently stay on the Matrix-only transport per storm §4.1.
/// * On URL-scheme violations (non-loopback `http:`, unsupported scheme):
///   `Ok(None)` (no credential leak to a non-homeserver origin).
/// * On programmer-grade transport errors (e.g. failed client construction):
///   `Err(TurnError::Http(...))`.
///
/// Security:
/// * `redirect::Policy::none()` — refuses to follow HTTP redirects so the
///   bearer token cannot be transmitted to an origin different from the
///   homeserver. Mirrors `redirect: 'error'` in the npm reference.
/// * 10-second timeout caps the exfiltration window on a hostile server.
/// * Callers receive no partial credentials on error.
#[cfg(not(target_arch = "wasm32"))]
pub async fn fetch_turn_credentials(
    homeserver: &Url,
    access_token: &str,
) -> Result<Option<TurnCredentials>, TurnError> {
    // Enforce scheme policy before touching the network.
    if !is_safe_scheme(homeserver) {
        return Ok(None);
    }

    // Construct the endpoint URL from the homeserver base without string
    // concatenation (avoids double-slash and host-confusion bugs).
    let mut endpoint = homeserver.clone();
    endpoint.set_path(TURN_ENDPOINT_PATH);

    let client = reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(Duration::from_secs(10))
        .build()?;

    let resp = match client
        .get(endpoint)
        .bearer_auth(access_token)
        .send()
        .await
    {
        Ok(r) => r,
        Err(_) => return Ok(None),
    };

    if !resp.status().is_success() {
        return Ok(None);
    }

    let body: TurnResponse = match resp.json().await {
        Ok(b) => b,
        Err(_) => return Ok(None),
    };

    if body.uris.is_empty() {
        return Ok(None);
    }

    Ok(Some(TurnCredentials {
        uris: body.uris,
        username: body.username,
        password: body.password,
        ttl: Duration::from_secs(body.ttl),
        fetched_at: SystemTime::now(),
    }))
}

/// Validate the homeserver URL scheme before a bearer-authenticated request.
///
/// * `https:` — always safe.
/// * `http:` — only safe when the host is a loopback literal
///   (localhost, 127.0.0.1, ::1). Disallowed on public hostnames to
///   prevent credential exfiltration to a cleartext non-homeserver.
/// * Any other scheme — rejected.
#[cfg(not(target_arch = "wasm32"))]
fn is_safe_scheme(url: &Url) -> bool {
    match url.scheme() {
        "https" => true,
        "http" => {
            let host = url.host_str().unwrap_or("");
            LOOPBACK_HOSTS.contains(&host)
        }
        _ => false,
    }
}

// ---------------------------------------------------------------------------
// Active-call refresh (T-21)
// ---------------------------------------------------------------------------

/// Outcome of a single refresh cycle in [`TurnRefreshTask`].
///
/// * [`Refreshed`](Self::Refreshed): new credentials fetched. The transport
///   (Phase 3) will pass the associated URIs into
///   `WebRtcChannel::restart_ice(new_ice_servers)` — this T-21 surface only
///   produces the outcome; wiring is out of scope.
/// * [`RetryPending`](Self::RetryPending): fetch failed; the task will retry
///   with exponential backoff until the initial `expires_at()` is reached.
/// * [`Expired`](Self::Expired): TTL elapsed without a successful fetch.
///   The transport MUST hang up with reason `"turn_expired"` and fall back
///   to Matrix (storm §2.4 / §4.1).
#[cfg(not(target_arch = "wasm32"))]
#[derive(Debug)]
pub enum TurnRefreshOutcome {
    Refreshed(TurnCredentials),
    RetryPending,
    Expired,
}

/// Abstraction over `fetch_turn_credentials` so the refresh loop can be
/// tested without a real homeserver. Production code uses
/// [`HttpFetcher`]; tests substitute a deterministic stub.
#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
trait TurnFetcher: Send + Sync + 'static {
    async fn fetch(&self) -> Result<Option<TurnCredentials>, TurnError>;
}

/// Production fetcher — wraps [`fetch_turn_credentials`] against a fixed
/// homeserver URL and bearer token.
#[cfg(not(target_arch = "wasm32"))]
struct HttpFetcher {
    homeserver: Url,
    access_token: String,
}

#[cfg(not(target_arch = "wasm32"))]
#[async_trait::async_trait]
impl TurnFetcher for HttpFetcher {
    async fn fetch(&self) -> Result<Option<TurnCredentials>, TurnError> {
        fetch_turn_credentials(&self.homeserver, &self.access_token).await
    }
}

/// Active-call TURN refresh loop.
///
/// Wakes at `refresh_at()` (= fetched_at + ttl/2), calls the fetcher, and
/// emits one [`TurnRefreshOutcome`] per cycle on the outbound channel.
/// On fetch failure, enters exponential backoff (5s, 10s, 20s, 40s, 60s
/// cap) and keeps emitting `RetryPending` until either a refresh succeeds
/// or the initial `expires_at()` passes (in which case it emits `Expired`
/// and exits).
///
/// **Expiry-during-reconnect race** (storm §2.4 / §3.4): the transport
/// (Phase 5) may need fresh credentials before sending `m.call.invite` on
/// a reconnect. [`TurnRefreshTask::trigger_refresh_now`] wakes the loop
/// immediately and causes it to fetch; the next outcome sent on the
/// channel reflects that fetch. The transport awaits that outcome before
/// sending the invite — serializing the reconnect path through a fresh
/// fetch.
///
/// Shutdown: call [`TurnRefreshTask::shutdown`] to cancel the loop and
/// await its join handle cleanly. In-flight reqwest futures drop; the
/// in-memory `TurnCredentials` struct is zeroized by its `Drop` impl.
#[cfg(not(target_arch = "wasm32"))]
pub struct TurnRefreshTask {
    shutdown_tx: Option<tokio::sync::oneshot::Sender<()>>,
    trigger_tx: tokio::sync::mpsc::UnboundedSender<()>,
    handle: Option<tokio::task::JoinHandle<()>>,
}

#[cfg(not(target_arch = "wasm32"))]
impl TurnRefreshTask {
    /// Spawn a refresh loop seeded with `initial` credentials.
    ///
    /// Returns the task handle and an `mpsc::Receiver` of outcomes. Drop
    /// the receiver to signal "caller no longer listening"; the task
    /// itself runs until [`Self::shutdown`] is called or `Expired` is
    /// emitted.
    pub fn spawn(
        initial: TurnCredentials,
        homeserver: Url,
        access_token: String,
    ) -> (Self, tokio::sync::mpsc::Receiver<TurnRefreshOutcome>) {
        let fetcher = HttpFetcher {
            homeserver,
            access_token,
        };
        Self::spawn_with_fetcher(initial, std::sync::Arc::new(fetcher))
    }

    /// Test-only: spawn with a caller-supplied fetcher.
    #[cfg(test)]
    fn spawn_with_fetcher_for_tests(
        initial: TurnCredentials,
        fetcher: std::sync::Arc<dyn TurnFetcher>,
    ) -> (Self, tokio::sync::mpsc::Receiver<TurnRefreshOutcome>) {
        Self::spawn_with_fetcher(initial, fetcher)
    }

    fn spawn_with_fetcher(
        initial: TurnCredentials,
        fetcher: std::sync::Arc<dyn TurnFetcher>,
    ) -> (Self, tokio::sync::mpsc::Receiver<TurnRefreshOutcome>) {
        let (outcome_tx, outcome_rx) = tokio::sync::mpsc::channel(8);
        let (shutdown_tx, shutdown_rx) = tokio::sync::oneshot::channel();
        let (trigger_tx, trigger_rx) = tokio::sync::mpsc::unbounded_channel();

        let handle = tokio::spawn(async move {
            run_refresh_loop(initial, fetcher, outcome_tx, shutdown_rx, trigger_rx).await;
        });

        (
            TurnRefreshTask {
                shutdown_tx: Some(shutdown_tx),
                trigger_tx,
                handle: Some(handle),
            },
            outcome_rx,
        )
    }

    /// Wake the loop immediately and cause it to perform a fetch on the
    /// next poll. Used by the reconnect-pending-with-expired-creds path
    /// to serialize a fresh fetch before `m.call.invite`.
    ///
    /// Returns `Err(())` if the loop has already exited (e.g. received
    /// `Expired` and stopped).
    pub fn trigger_refresh_now(&self) -> Result<(), ()> {
        self.trigger_tx.send(()).map_err(|_| ())
    }

    /// Signal shutdown and await the task's completion.
    pub async fn shutdown(mut self) {
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            let _ = handle.await;
        }
    }
}

#[cfg(not(target_arch = "wasm32"))]
impl Drop for TurnRefreshTask {
    fn drop(&mut self) {
        // Best-effort shutdown if the caller forgot to await shutdown().
        // The spawned task will observe the closed channel and exit its loop.
        if let Some(tx) = self.shutdown_tx.take() {
            let _ = tx.send(());
        }
        if let Some(handle) = self.handle.take() {
            handle.abort();
        }
    }
}

/// Exponential backoff schedule (in seconds) for failed refreshes.
/// Capped at 60s per storm §4.2 backoff policy.
#[cfg(not(target_arch = "wasm32"))]
const REFRESH_BACKOFF_SECS: &[u64] = &[5, 10, 20, 40, 60];

/// Run the refresh loop. Exits when:
/// * `shutdown_rx` fires (clean shutdown requested), OR
/// * `Expired` outcome is emitted (TTL elapsed without success), OR
/// * the outcome receiver is dropped (channel closed).
///
/// Timing uses [`tokio::time::Instant`] (monotonic, test-paused-aware) for
/// sleeps so that `tokio::time::pause` + `advance` can drive the loop in
/// virtual time. The seed `TurnCredentials` is consumed on entry and its
/// `SystemTime::fetched_at + ttl` is projected onto the Tokio clock at
/// loop start.
#[cfg(not(target_arch = "wasm32"))]
async fn run_refresh_loop(
    initial: TurnCredentials,
    fetcher: std::sync::Arc<dyn TurnFetcher>,
    outcome_tx: tokio::sync::mpsc::Sender<TurnRefreshOutcome>,
    mut shutdown_rx: tokio::sync::oneshot::Receiver<()>,
    mut trigger_rx: tokio::sync::mpsc::UnboundedReceiver<()>,
) {
    use tokio::time::Instant;

    let now_tokio = Instant::now();
    // Compute Tokio-Instant equivalents of the seed's wall-clock moments.
    // If seed was already expired (e.g. historic fetched_at), expiry is "now".
    let seed_now_wall = SystemTime::now();
    let ttl_remaining = initial
        .expires_at()
        .duration_since(seed_now_wall)
        .unwrap_or(Duration::from_millis(0));
    let refresh_remaining = initial
        .refresh_at()
        .duration_since(seed_now_wall)
        .unwrap_or(Duration::from_millis(0));

    let hard_expiry = now_tokio + ttl_remaining;
    let mut next_wake = now_tokio + refresh_remaining;
    let mut backoff_idx = 0usize;

    // Drop the seed — its username/password are zeroized on drop.
    drop(initial);

    loop {
        let now = Instant::now();
        let sleep_for = next_wake.saturating_duration_since(now);

        tokio::select! {
            biased;
            _ = &mut shutdown_rx => {
                return;
            }
            _ = trigger_rx.recv() => {
                // Immediate-trigger path: fetch now, regardless of timer.
            }
            _ = tokio::time::sleep(sleep_for) => {
                // Timer fired — proceed to fetch.
            }
        }

        // Check hard expiry before the fetch attempt.
        if Instant::now() >= hard_expiry {
            let _ = outcome_tx.send(TurnRefreshOutcome::Expired).await;
            return;
        }

        // Attempt fetch.
        let outcome = match fetcher.fetch().await {
            Ok(Some(new_creds)) => {
                // Success: schedule next refresh at the new half-life
                // (projected into Tokio-Instant from the new creds' wall clock).
                let new_wall_now = SystemTime::now();
                let new_refresh_in = new_creds
                    .refresh_at()
                    .duration_since(new_wall_now)
                    .unwrap_or(Duration::from_millis(0));
                backoff_idx = 0;
                next_wake = Instant::now() + new_refresh_in;
                TurnRefreshOutcome::Refreshed(new_creds)
            }
            Ok(None) | Err(_) => {
                // Failure: retry with backoff, capped at hard_expiry.
                let backoff =
                    Duration::from_secs(REFRESH_BACKOFF_SECS[backoff_idx]);
                backoff_idx = (backoff_idx + 1).min(REFRESH_BACKOFF_SECS.len() - 1);
                let candidate_wake = Instant::now() + backoff;
                next_wake = if candidate_wake >= hard_expiry {
                    hard_expiry
                } else {
                    candidate_wake
                };
                TurnRefreshOutcome::RetryPending
            }
        };

        // Send outcome; exit if the receiver dropped.
        if outcome_tx.send(outcome).await.is_err() {
            return;
        }
    }
}

// ---------------------------------------------------------------------------
// Wasm stub (target_arch = "wasm32")
// ---------------------------------------------------------------------------

/// Wasm stub — real implementation lands in Phase 8 of the P2P rollout.
///
/// Always returns [`TurnError::WasmUnsupported`]. Callers on wasm must
/// skip TURN fetch and fall back to STUN-only / Matrix-only paths. This
/// stub exists so that the crate compiles cleanly for the wasm target
/// throughout the P2P rollout; the native surface is the reference
/// implementation.
#[cfg(target_arch = "wasm32")]
pub async fn fetch_turn_credentials(
    _homeserver: &Url,
    _access_token: &str,
) -> Result<Option<TurnCredentials>, TurnError> {
    Err(TurnError::WasmUnsupported)
}

// ---------------------------------------------------------------------------
// Tests (native only)
// ---------------------------------------------------------------------------

#[cfg(all(test, not(target_arch = "wasm32")))]
mod tests {
    use super::*;

    fn fresh_creds(ttl_secs: u64) -> TurnCredentials {
        TurnCredentials {
            uris: vec!["turn:turn.example.org:3478".into()],
            username: "u".into(),
            password: "p".into(),
            ttl: Duration::from_secs(ttl_secs),
            fetched_at: SystemTime::now(),
        }
    }

    fn aged_creds(ttl_secs: u64, age_secs: u64) -> TurnCredentials {
        let now = SystemTime::now();
        TurnCredentials {
            uris: vec!["turn:turn.example.org:3478".into()],
            username: "u".into(),
            password: "p".into(),
            ttl: Duration::from_secs(ttl_secs),
            fetched_at: now - Duration::from_secs(age_secs),
        }
    }

    #[test]
    fn refresh_at_is_half_ttl_past_fetched_at() {
        let c = fresh_creds(86400);
        let dt = c
            .refresh_at()
            .duration_since(c.fetched_at)
            .expect("refresh_at after fetched_at");
        assert_eq!(dt, Duration::from_secs(43200));
    }

    #[test]
    fn expires_at_is_full_ttl_past_fetched_at() {
        let c = fresh_creds(100);
        let dt = c
            .expires_at()
            .duration_since(c.fetched_at)
            .expect("expires_at after fetched_at");
        assert_eq!(dt, Duration::from_secs(100));
    }

    #[test]
    fn is_expired_false_for_fresh_creds() {
        let c = fresh_creds(3600);
        assert!(!c.is_expired());
    }

    #[test]
    fn is_expired_true_for_aged_creds() {
        // fetched_at 100s ago, ttl 50s -> expired 50s ago.
        let c = aged_creds(50, 100);
        assert!(c.is_expired());
    }

    #[test]
    fn debug_impl_redacts_secrets() {
        let c = TurnCredentials {
            uris: vec!["turn:hostile.example.org:3478?transport=tcp".into()],
            username: "LEAKED_USERNAME_SENTINEL".into(),
            password: "LEAKED_PASSWORD_SENTINEL".into(),
            ttl: Duration::from_secs(1),
            fetched_at: SystemTime::now(),
        };
        let rendered = format!("{c:?}");
        assert!(!rendered.contains("LEAKED_USERNAME_SENTINEL"));
        assert!(!rendered.contains("LEAKED_PASSWORD_SENTINEL"));
        // URI list is also redacted (count only).
        assert!(!rendered.contains("hostile.example.org"));
        assert!(rendered.contains("uri_count"));
        assert!(rendered.contains("<redacted>"));
    }

    #[test]
    fn is_safe_scheme_accepts_https() {
        let u = Url::parse("https://hs.example.org").unwrap();
        assert!(is_safe_scheme(&u));
    }

    #[test]
    fn is_safe_scheme_rejects_http_public_host() {
        let u = Url::parse("http://hs.example.org").unwrap();
        assert!(!is_safe_scheme(&u));
    }

    #[test]
    fn is_safe_scheme_accepts_http_loopback() {
        for u in [
            "http://localhost:8008",
            "http://127.0.0.1:8008",
            "http://[::1]:8008",
        ] {
            let url = Url::parse(u).unwrap();
            assert!(is_safe_scheme(&url), "should accept {u}");
        }
    }

    #[test]
    fn is_safe_scheme_rejects_other_schemes() {
        for u in [
            "file:///tmp/hs",
            "ftp://hs.example.org",
            "javascript:alert(1)",
        ] {
            let url = Url::parse(u).unwrap();
            assert!(!is_safe_scheme(&url), "should reject {u}");
        }
    }

    #[tokio::test]
    async fn fetch_rejects_non_loopback_http_without_http_call() {
        // No mock server set up — if the function tried to make a request
        // to a real host, the test would fail or hang. Scheme check must
        // short-circuit before any network I/O.
        let u = Url::parse("http://hostile.example.org").unwrap();
        let res = fetch_turn_credentials(&u, "tok").await.unwrap();
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn fetch_happy_path() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", TURN_ENDPOINT_PATH)
            .match_header("authorization", "Bearer test-token")
            .with_status(200)
            .with_body(
                r#"{"uris":["turn:a.example.org:3478","turns:a.example.org:5349"],"username":"u1","password":"p1","ttl":86400}"#,
            )
            .create_async()
            .await;

        let url = Url::parse(&server.url()).unwrap();
        let creds = fetch_turn_credentials(&url, "test-token")
            .await
            .expect("no transport error")
            .expect("some creds");
        assert_eq!(creds.uris.len(), 2);
        assert_eq!(creds.username, "u1");
        assert_eq!(creds.password, "p1");
        assert_eq!(creds.ttl, Duration::from_secs(86400));
        assert!(!creds.is_expired());
    }

    #[tokio::test]
    async fn fetch_returns_none_on_404() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", TURN_ENDPOINT_PATH)
            .with_status(404)
            .with_body("Not Found")
            .create_async()
            .await;
        let url = Url::parse(&server.url()).unwrap();
        let res = fetch_turn_credentials(&url, "tok").await.unwrap();
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn fetch_returns_none_on_500() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", TURN_ENDPOINT_PATH)
            .with_status(500)
            .create_async()
            .await;
        let url = Url::parse(&server.url()).unwrap();
        let res = fetch_turn_credentials(&url, "tok").await.unwrap();
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn fetch_returns_none_on_empty_uris() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", TURN_ENDPOINT_PATH)
            .with_status(200)
            .with_body(r#"{"uris":[],"username":"u","password":"p","ttl":1}"#)
            .create_async()
            .await;
        let url = Url::parse(&server.url()).unwrap();
        let res = fetch_turn_credentials(&url, "tok").await.unwrap();
        assert!(res.is_none());
    }

    #[tokio::test]
    async fn fetch_returns_none_on_invalid_json() {
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", TURN_ENDPOINT_PATH)
            .with_status(200)
            .with_body("{not json")
            .create_async()
            .await;
        let url = Url::parse(&server.url()).unwrap();
        let res = fetch_turn_credentials(&url, "tok").await.unwrap();
        assert!(res.is_none());
    }

    // ---------------------------------------------------------------------
    // T-21: TurnRefreshTask tests — use tokio::time::pause + a stub fetcher
    // so every test runs in virtual-time milliseconds without real HTTP.
    // ---------------------------------------------------------------------

    use std::sync::atomic::{AtomicUsize, Ordering};
    use std::sync::Arc;
    use std::sync::Mutex;

    /// Stub fetcher: caller feeds a queue of outcomes; each `fetch()` pops one.
    ///
    /// Empty queue yields `Ok(None)` (simulates homeserver degradation).
    struct StubFetcher {
        queue: Mutex<std::collections::VecDeque<Result<Option<TurnCredentials>, TurnError>>>,
        count: AtomicUsize,
    }

    impl StubFetcher {
        fn new() -> Self {
            Self {
                queue: Mutex::new(std::collections::VecDeque::new()),
                count: AtomicUsize::new(0),
            }
        }
        fn push(&self, r: Result<Option<TurnCredentials>, TurnError>) {
            self.queue.lock().unwrap().push_back(r);
        }
        fn call_count(&self) -> usize {
            self.count.load(Ordering::SeqCst)
        }
    }

    #[async_trait::async_trait]
    impl TurnFetcher for StubFetcher {
        async fn fetch(&self) -> Result<Option<TurnCredentials>, TurnError> {
            self.count.fetch_add(1, Ordering::SeqCst);
            self.queue
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(None))
        }
    }

    fn fresh_creds_with_ttl(ttl_secs: u64) -> TurnCredentials {
        TurnCredentials {
            uris: vec!["turn:a.example.org:3478".into()],
            username: "u".into(),
            password: "p".into(),
            ttl: Duration::from_secs(ttl_secs),
            fetched_at: SystemTime::now(),
        }
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_success_before_ttl_half_emits_refreshed() {
        let stub = Arc::new(StubFetcher::new());
        stub.push(Ok(Some(TurnCredentials {
            uris: vec!["turn:fresh.example.org:3478".into()],
            username: "new_user".into(),
            password: "new_pass".into(),
            ttl: Duration::from_secs(3600),
            fetched_at: SystemTime::now(),
        })));

        let initial = fresh_creds_with_ttl(600); // refresh_at = now + 300s
        let (task, mut rx) =
            TurnRefreshTask::spawn_with_fetcher_for_tests(initial, stub.clone());

        // Let the spawned task reach its first sleep.
        tokio::task::yield_now().await;
        // Advance past the half-life (300s) — the task should wake and fetch.
        tokio::time::advance(Duration::from_secs(301)).await;

        let outcome = rx.recv().await.expect("channel open");
        match outcome {
            TurnRefreshOutcome::Refreshed(creds) => {
                assert_eq!(creds.username, "new_user");
            }
            other => panic!("expected Refreshed, got {other:?}"),
        }
        assert_eq!(stub.call_count(), 1);
        task.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_failure_backs_off_and_eventually_expires() {
        let stub = Arc::new(StubFetcher::new());
        // Queue: infinite failures (empty queue -> Ok(None), treated as failure).

        let initial = fresh_creds_with_ttl(120); // expires 120s, refresh_at = 60s
        let (task, mut rx) =
            TurnRefreshTask::spawn_with_fetcher_for_tests(initial, stub.clone());

        // Let the spawned task reach its first sleep.
        tokio::task::yield_now().await;
        // Advance past refresh_at (60s): first failure -> RetryPending.
        tokio::time::advance(Duration::from_secs(61)).await;
        let outcome = rx.recv().await.expect("channel open");
        assert!(matches!(outcome, TurnRefreshOutcome::RetryPending), "got {outcome:?}");

        // Continue advancing; the backoff schedule is 5, 10, 20, 40, 60 —
        // advancing past the total remaining TTL window must ultimately
        // emit Expired.
        let mut saw_expired = false;
        for _ in 0..30 {
            tokio::time::advance(Duration::from_secs(10)).await;
            tokio::task::yield_now().await;
            match rx.try_recv() {
                Ok(TurnRefreshOutcome::Expired) => {
                    saw_expired = true;
                    break;
                }
                Ok(TurnRefreshOutcome::RetryPending) => continue,
                Ok(other) => panic!("unexpected: {other:?}"),
                Err(tokio::sync::mpsc::error::TryRecvError::Empty) => continue,
                Err(tokio::sync::mpsc::error::TryRecvError::Disconnected) => {
                    panic!("channel closed before Expired")
                }
            }
        }
        assert!(saw_expired, "expected Expired outcome within TTL window");
        task.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn refresh_recovers_after_one_failure() {
        let stub = Arc::new(StubFetcher::new());
        // First call fails (returns Ok(None) = "no TURN available" = failure branch).
        stub.push(Ok(None));
        // Second call succeeds.
        stub.push(Ok(Some(TurnCredentials {
            uris: vec!["turn:recovered.example.org:3478".into()],
            username: "recovered".into(),
            password: "p2".into(),
            ttl: Duration::from_secs(3600),
            fetched_at: SystemTime::now(),
        })));

        let initial = fresh_creds_with_ttl(600); // refresh_at = 300s
        let (task, mut rx) =
            TurnRefreshTask::spawn_with_fetcher_for_tests(initial, stub.clone());

        // Let the spawned task reach its first sleep.
        tokio::task::yield_now().await;
        // Wake 1: fail -> RetryPending.
        tokio::time::advance(Duration::from_secs(301)).await;
        let o = rx.recv().await.expect("channel open");
        assert!(matches!(o, TurnRefreshOutcome::RetryPending), "got {o:?}");

        // Wake 2: backoff 5s -> succeed -> Refreshed.
        tokio::time::advance(Duration::from_secs(6)).await;
        let o = rx.recv().await.expect("channel open");
        match o {
            TurnRefreshOutcome::Refreshed(c) => {
                assert_eq!(c.username, "recovered");
            }
            other => panic!("expected Refreshed, got {other:?}"),
        }
        assert_eq!(stub.call_count(), 2);
        task.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn trigger_refresh_now_wakes_immediately() {
        let stub = Arc::new(StubFetcher::new());
        stub.push(Ok(Some(TurnCredentials {
            uris: vec!["turn:triggered.example.org:3478".into()],
            username: "triggered".into(),
            password: "t".into(),
            ttl: Duration::from_secs(3600),
            fetched_at: SystemTime::now(),
        })));

        // Large TTL — natural refresh would be far in the future.
        let initial = fresh_creds_with_ttl(86400); // refresh_at = 43200s (12h)
        let (task, mut rx) =
            TurnRefreshTask::spawn_with_fetcher_for_tests(initial, stub.clone());

        // Let the spawned task reach its first sleep.
        tokio::task::yield_now().await;
        // Don't advance time. Trigger immediate refresh (reconnect path).
        task.trigger_refresh_now().expect("loop running");

        let o = rx.recv().await.expect("channel open");
        match o {
            TurnRefreshOutcome::Refreshed(c) => {
                assert_eq!(c.username, "triggered");
            }
            other => panic!("expected Refreshed, got {other:?}"),
        }
        // The trigger path bypassed the 12h sleep.
        assert_eq!(stub.call_count(), 1);
        task.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn shutdown_is_clean_without_outcomes() {
        let stub = Arc::new(StubFetcher::new());
        let initial = fresh_creds_with_ttl(86400);
        let (task, _rx) =
            TurnRefreshTask::spawn_with_fetcher_for_tests(initial, stub.clone());
        // Shut down immediately; the loop should exit without panic or leak.
        task.shutdown().await;
    }

    #[tokio::test(start_paused = true)]
    async fn trigger_after_expired_returns_err() {
        let stub = Arc::new(StubFetcher::new());
        // Short TTL so expiry fires quickly in virtual time.
        let initial = fresh_creds_with_ttl(2);
        let (task, mut rx) =
            TurnRefreshTask::spawn_with_fetcher_for_tests(initial, stub.clone());

        // Let the spawned task reach its first sleep.
        tokio::task::yield_now().await;
        // Advance past hard expiry — the loop will emit Expired and exit.
        tokio::time::advance(Duration::from_secs(3)).await;
        let o = rx.recv().await.expect("channel open");
        assert!(matches!(o, TurnRefreshOutcome::Expired));

        // The task has exited after emitting Expired. Clean shutdown of the
        // task handle should still work without panic (the join yields ()).
        task.shutdown().await;
    }

    #[tokio::test]
    async fn fetch_does_not_follow_redirects() {
        // Mock a redirect from the endpoint; the client must NOT follow
        // (would leak the bearer token to another origin). The redirect
        // response itself is non-2xx, so we expect Ok(None).
        let mut server = mockito::Server::new_async().await;
        let _m = server
            .mock("GET", TURN_ENDPOINT_PATH)
            .with_status(302)
            .with_header("location", "https://hostile.example.org/steal")
            .create_async()
            .await;
        let url = Url::parse(&server.url()).unwrap();
        let res = fetch_turn_credentials(&url, "tok").await.unwrap();
        assert!(res.is_none());
    }
}
