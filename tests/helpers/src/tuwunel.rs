use anyhow::{bail, Context, Result};
use std::io::Write;
use std::net::TcpListener;
use std::process::{Child, Command};
use std::time::Duration;

use crate::matrix_client::TestMatrixClient;

const REGISTRATION_TOKEN: &str = "mxdx-test-token";
const HEALTH_CHECK_TIMEOUT: Duration = Duration::from_secs(30);
const HEALTH_CHECK_INTERVAL: Duration = Duration::from_millis(100);

pub struct TuwunelInstance {
    pub port: u16,
    pub server_name: String,
    process: Child,
    _data_dir: tempfile::TempDir,
}

impl TuwunelInstance {
    /// Start a tuwunel instance on an OS-assigned port (mxdx-ji1).
    /// Takes NO port argument — always binds to port 0 equivalent.
    pub async fn start() -> Result<Self> {
        let port = pick_free_port()?;
        let data_dir = tempfile::TempDir::new().context("Failed to create temp dir")?;
        let db_path = data_dir.path().join("db");
        std::fs::create_dir_all(&db_path)?;

        let server_name = format!("test-{}.localhost", port);

        let config_path = data_dir.path().join("tuwunel.toml");
        let config = format!(
            r#"[global]
server_name = "{server_name}"
database_path = "{db_path}"
address = ["127.0.0.1"]
port = {port}
allow_registration = true
registration_token = "{REGISTRATION_TOKEN}"
log = "error"
new_user_displayname_suffix = ""
"#,
            server_name = server_name,
            db_path = db_path.display(),
            port = port,
        );
        let mut f = std::fs::File::create(&config_path)?;
        f.write_all(config.as_bytes())?;

        let tuwunel_bin = find_tuwunel_binary()?;
        let process = Command::new(&tuwunel_bin)
            .arg("-c")
            .arg(&config_path)
            .stdout(std::process::Stdio::null())
            .stderr(std::process::Stdio::null())
            .spawn()
            .with_context(|| format!("Failed to spawn tuwunel at {}", tuwunel_bin))?;

        let instance = TuwunelInstance {
            port,
            server_name,
            process,
            _data_dir: data_dir,
        };

        instance.wait_for_health().await?;

        Ok(instance)
    }

    /// Wait for tuwunel to respond to health checks.
    async fn wait_for_health(&self) -> Result<()> {
        let url = format!("http://127.0.0.1:{}/_matrix/client/versions", self.port);
        let client = reqwest::Client::new();
        let deadline = tokio::time::Instant::now() + HEALTH_CHECK_TIMEOUT;

        loop {
            if tokio::time::Instant::now() > deadline {
                bail!(
                    "Tuwunel on port {} did not become healthy within {:?}",
                    self.port,
                    HEALTH_CHECK_TIMEOUT
                );
            }

            match client.get(&url).send().await {
                Ok(resp) if resp.status().is_success() => return Ok(()),
                _ => tokio::time::sleep(HEALTH_CHECK_INTERVAL).await,
            }
        }
    }

    /// Register a user on this tuwunel instance.
    pub async fn register_user(&self, username: &str, password: &str) -> Result<TestMatrixClient> {
        let url = format!(
            "http://127.0.0.1:{}/_matrix/client/v3/register",
            self.port
        );
        let client = reqwest::Client::new();
        let body = serde_json::json!({
            "username": username,
            "password": password,
            "auth": {
                "type": "m.login.registration_token",
                "token": REGISTRATION_TOKEN
            }
        });

        let resp = client
            .post(&url)
            .json(&body)
            .send()
            .await
            .context("Failed to send registration request")?;

        let status = resp.status();
        let resp_body: serde_json::Value = resp
            .json()
            .await
            .context("Failed to parse registration response")?;

        if !status.is_success() {
            bail!(
                "Registration failed with status {}: {}",
                status,
                resp_body
            );
        }

        Ok(TestMatrixClient {
            user_id: resp_body["user_id"]
                .as_str()
                .context("Missing user_id")?
                .to_string(),
            access_token: resp_body["access_token"]
                .as_str()
                .context("Missing access_token")?
                .to_string(),
            device_id: resp_body["device_id"]
                .as_str()
                .context("Missing device_id")?
                .to_string(),
            homeserver_url: format!("http://127.0.0.1:{}", self.port),
        })
    }

    /// Stop the tuwunel process.
    pub async fn stop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

impl Drop for TuwunelInstance {
    fn drop(&mut self) {
        let _ = self.process.kill();
        let _ = self.process.wait();
    }
}

/// Pick a free port by binding to port 0 and reading the assigned port.
fn pick_free_port() -> Result<u16> {
    let listener = TcpListener::bind("127.0.0.1:0").context("Failed to bind to port 0")?;
    let port = listener.local_addr()?.port();
    drop(listener);
    Ok(port)
}

/// Find the tuwunel binary.
fn find_tuwunel_binary() -> Result<String> {
    for path in &["/usr/sbin/tuwunel", "/usr/local/bin/tuwunel"] {
        if std::path::Path::new(path).exists() {
            return Ok(path.to_string());
        }
    }
    // Check PATH
    if let Ok(output) = Command::new("which").arg("tuwunel").output() {
        if output.status.success() {
            let path = String::from_utf8_lossy(&output.stdout).trim().to_string();
            if !path.is_empty() {
                return Ok(path);
            }
        }
    }
    bail!("tuwunel binary not found. Install tuwunel to run integration tests.")
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn tuwunel_starts_and_responds_to_health_check() {
        let mut instance = TuwunelInstance::start().await.unwrap();
        let resp = reqwest::get(format!(
            "http://127.0.0.1:{}/_matrix/client/versions",
            instance.port
        ))
        .await
        .unwrap();
        assert!(resp.status().is_success());
        instance.stop().await;
    }

    #[tokio::test]
    async fn tuwunel_can_register_user() {
        let mut instance = TuwunelInstance::start().await.unwrap();
        let client = instance
            .register_user("testuser", "testpass")
            .await
            .unwrap();
        assert!(!client.access_token.is_empty());
        assert!(client.user_id.starts_with("@testuser:"));
        instance.stop().await;
    }

    #[tokio::test]
    async fn tuwunel_uses_os_assigned_port() {
        // Verify no hardcoded ports (mxdx-ji1)
        let mut instance = TuwunelInstance::start().await.unwrap();
        assert_ne!(instance.port, 8008, "Port must not be default 8008");
        assert_ne!(instance.port, 0, "Port must be resolved from OS");
        instance.stop().await;
    }
}
