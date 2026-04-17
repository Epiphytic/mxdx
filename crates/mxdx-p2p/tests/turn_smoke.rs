#![cfg(not(target_arch = "wasm32"))]
//! Manual smoke test for `fetch_turn_credentials` against a live Matrix
//! homeserver.
//!
//! This test is `#[ignore]`d by default so the standard `cargo test -p
//! mxdx-p2p` run stays network-free. Operators (or CI jobs with secrets)
//! run it on demand:
//!
//! ```sh
//! export TEST_HS_URL=https://ca1-beta.mxdx.dev
//! export TEST_TOKEN=<from test-credentials.toml>
//! cargo test -p mxdx-p2p --test turn_smoke -- --ignored --nocapture
//! ```
//!
//! The test asserts only non-sensitive shape properties (URI count,
//! non-zero TTL). Credentials themselves (`username`, `password`) are
//! never printed — the redacted [`Debug`] impl of `TurnCredentials`
//! enforces this at the type level.
//!
//! On a homeserver that does not provision TURN, `fetch_turn_credentials`
//! returns `Ok(None)` — this is treated as a pass with a printed note,
//! not a failure, because the code-path under test is correct.

#![cfg(not(target_arch = "wasm32"))]

use mxdx_p2p::turn::fetch_turn_credentials;
use url::Url;

#[tokio::test]
#[ignore = "requires TEST_HS_URL and TEST_TOKEN env vars; run with --ignored"]
async fn turn_smoke_against_live_homeserver() {
    let hs = std::env::var("TEST_HS_URL").expect(
        "set TEST_HS_URL (e.g. https://ca1-beta.mxdx.dev) to run the TURN smoke test",
    );
    let tok = std::env::var("TEST_TOKEN")
        .expect("set TEST_TOKEN (from test-credentials.toml) to run the TURN smoke test");

    let url = Url::parse(&hs).expect("TEST_HS_URL must parse as a URL");

    let result = fetch_turn_credentials(&url, &tok).await;

    match result {
        Ok(Some(creds)) => {
            assert!(!creds.uris.is_empty(), "homeserver returned empty uris");
            assert!(
                creds.ttl.as_secs() > 0,
                "homeserver returned non-positive ttl"
            );
            // Non-sensitive observability only — the redacted Debug impl
            // ensures no username/password/uris appear in the output.
            eprintln!(
                "TURN smoke: fetched ok ({:?})",
                creds
            );
            eprintln!(
                "TURN smoke: {} uri(s), ttl {:?}, refresh_at {:?}",
                creds.uris.len(),
                creds.ttl,
                creds.refresh_at()
            );
        }
        Ok(None) => {
            eprintln!(
                "TURN smoke: homeserver did not provision TURN \
                 (Ok(None)) — treating as pass (correct code-path)"
            );
        }
        Err(e) => {
            panic!("TURN smoke failed with transport error: {e}");
        }
    }
}
