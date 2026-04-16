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
