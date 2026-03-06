use anyhow::{Context, Result};
use std::process::Command;

use crate::tuwunel::TuwunelInstance;

pub struct FederatedPair {
    pub hs_a: TuwunelInstance,
    pub hs_b: TuwunelInstance,
    _cert_dir: tempfile::TempDir,
}

impl FederatedPair {
    /// Start two federated tuwunel instances with TLS on OS-assigned ports.
    /// Uses self-signed certs with `allow_invalid_tls_certificates` for
    /// inter-server trust. Server names use `.localhost` which resolves
    /// to loopback, with port included so federation goes direct.
    pub async fn start() -> Result<Self> {
        let cert_dir = tempfile::TempDir::new().context("Failed to create cert dir")?;

        let (cert_a, key_a) = generate_self_signed_cert(cert_dir.path(), "hs-a")?;
        let (cert_b, key_b) = generate_self_signed_cert(cert_dir.path(), "hs-b")?;

        let hs_a = TuwunelInstance::start_federated("hs-a.localhost", &cert_a, &key_a)
            .await
            .context("Failed to start hs_a")?;

        let hs_b = TuwunelInstance::start_federated("hs-b.localhost", &cert_b, &key_b)
            .await
            .context("Failed to start hs_b")?;

        Ok(FederatedPair {
            hs_a,
            hs_b,
            _cert_dir: cert_dir,
        })
    }

    pub async fn stop(&mut self) {
        self.hs_a.stop().await;
        self.hs_b.stop().await;
    }
}

/// Generate a self-signed certificate using openssl.
/// Returns (cert_path, key_path).
fn generate_self_signed_cert(
    dir: &std::path::Path,
    name: &str,
) -> Result<(std::path::PathBuf, std::path::PathBuf)> {
    let key_path = dir.join(format!("{}.key", name));
    let cert_path = dir.join(format!("{}.crt", name));

    let status = Command::new("openssl")
        .args([
            "req",
            "-x509",
            "-newkey",
            "ec",
            "-pkeyopt",
            "ec_paramgen_curve:prime256v1",
            "-nodes",
            "-keyout",
            &key_path.to_string_lossy(),
            "-out",
            &cert_path.to_string_lossy(),
            "-days",
            "1",
            "-subj",
            &format!("/CN={}.localhost", name),
            "-addext",
            "subjectAltName=IP:127.0.0.1,DNS:localhost",
        ])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .context("Failed to run openssl")?;

    if !status.success() {
        anyhow::bail!("openssl cert generation failed for {}", name);
    }

    Ok((cert_path, key_path))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    #[ignore] // DNS: .localhost doesn't resolve to loopback on all platforms/CI. Deferred to Phase 11.
    async fn two_instances_can_federate() {
        let mut pair = FederatedPair::start().await.unwrap();
        let user_a = pair.hs_a.register_user("alice", "pass").await.unwrap();
        let user_b = pair.hs_b.register_user("bob", "pass").await.unwrap();

        let room_id = user_a.create_room().await.unwrap();
        user_a.invite(&room_id, user_b.mxid()).await.unwrap();

        let invite = user_b
            .wait_for_invite(&room_id, std::time::Duration::from_secs(10))
            .await;
        assert!(
            invite.is_ok(),
            "Federation invite not received: {:?}",
            invite
        );

        pair.stop().await;
    }
}
