//! Public Matrix server smoke tests.
//!
//! Minimal tests that verify MatrixClient works against a public homeserver.
//! These avoid creating rooms to stay under rate limits.
//!
//! For the full end-to-end test (command → execute → output round-trip),
//! see: crates/mxdx-launcher/tests/e2e_public_server.rs
//!
//! ## Setup
//!
//! Create `test-credentials.toml` in the repo root (gitignored):
//!
//! ```toml
//! [server]
//! url = "matrix.org"
//!
//! [account1]
//! username = "your-user"
//! password = "your-password"
//!
//! [account2]
//! username = "your-other-user"
//! password = "your-password"
//! ```
//!
//! Run with: cargo test -p mxdx-matrix --test public_server_compat -- --ignored

use mxdx_matrix::MatrixClient;

/// Credentials for a single account.
struct Credentials {
    hs_url: String,
    username: String,
    password: String,
}

/// Load credentials from test-credentials.toml or environment variables.
fn load_credentials() -> (Credentials, Option<Credentials>) {
    let toml_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .unwrap()
        .parent()
        .unwrap()
        .join("test-credentials.toml");

    if toml_path.exists() {
        let content = std::fs::read_to_string(&toml_path)
            .expect("Failed to read test-credentials.toml");
        let parsed: toml::Value = content.parse()
            .expect("Failed to parse test-credentials.toml");

        let hs_url = parsed["server"]["url"].as_str()
            .expect("server.url missing in test-credentials.toml")
            .to_string();

        let account1 = Credentials {
            hs_url: hs_url.clone(),
            username: parsed["account1"]["username"].as_str()
                .expect("account1.username missing").to_string(),
            password: parsed["account1"]["password"].as_str()
                .expect("account1.password missing").to_string(),
        };

        let account2 = parsed.get("account2").map(|a| Credentials {
            hs_url: hs_url.clone(),
            username: a["username"].as_str()
                .expect("account2.username missing").to_string(),
            password: a["password"].as_str()
                .expect("account2.password missing").to_string(),
        });

        return (account1, account2);
    }

    let hs_url = std::env::var("MXDX_PUBLIC_HS_URL")
        .expect("Set MXDX_PUBLIC_HS_URL or create test-credentials.toml");
    let account1 = Credentials {
        hs_url: hs_url.clone(),
        username: std::env::var("MXDX_PUBLIC_USERNAME")
            .expect("Set MXDX_PUBLIC_USERNAME"),
        password: std::env::var("MXDX_PUBLIC_PASSWORD")
            .expect("Set MXDX_PUBLIC_PASSWORD"),
    };
    let account2 = std::env::var("MXDX_PUBLIC_USERNAME2").ok().map(|u| Credentials {
        hs_url,
        username: u,
        password: std::env::var("MXDX_PUBLIC_PASSWORD2")
            .expect("Set MXDX_PUBLIC_PASSWORD2 if MXDX_PUBLIC_USERNAME2 is set"),
    });

    (account1, account2)
}

#[tokio::test]
#[ignore = "requires test-credentials.toml or env vars"]
async fn login_account1() {
    let (creds, _) = load_credentials();
    let client = MatrixClient::login_and_connect(&creds.hs_url, &creds.username, &creds.password)
        .await
        .expect("Account 1 login failed");
    assert!(client.is_logged_in());
    assert!(client.crypto_enabled().await);
}

#[tokio::test]
#[ignore = "requires test-credentials.toml or env vars"]
async fn login_account2() {
    let (_, creds2) = load_credentials();
    let creds = creds2.expect("Account 2 credentials required");
    let client = MatrixClient::login_and_connect(&creds.hs_url, &creds.username, &creds.password)
        .await
        .expect("Account 2 login failed");
    assert!(client.is_logged_in());
    assert!(client.crypto_enabled().await);
}
