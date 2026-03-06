use std::time::Duration;
use tokio::process::Command;

pub struct TmuxSession {
    name: String,
}

fn is_valid_session_name(name: &str) -> bool {
    !name.is_empty()
        && name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '_' || c == '-')
}

impl TmuxSession {
    pub async fn create(
        name: &str,
        command: &str,
        cols: u32,
        rows: u32,
    ) -> Result<Self, anyhow::Error> {
        if !is_valid_session_name(name) {
            anyhow::bail!("invalid tmux session name: {name}");
        }

        let output = Command::new("tmux")
            .args([
                "new-session",
                "-d",
                "-s",
                name,
                "-x",
                &cols.to_string(),
                "-y",
                &rows.to_string(),
                command,
            ])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("tmux new-session failed: {stderr}");
        }

        Ok(Self {
            name: name.to_string(),
        })
    }

    pub async fn send_input(&self, data: &str) -> Result<(), anyhow::Error> {
        let output = Command::new("tmux")
            .args(["send-keys", "-t", &self.name, "-l", "--", data])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("tmux send-keys failed: {stderr}");
        }

        Ok(())
    }

    pub async fn capture_pane(&self) -> Result<String, anyhow::Error> {
        let output = Command::new("tmux")
            .args(["capture-pane", "-t", &self.name, "-p"])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("tmux capture-pane failed: {stderr}");
        }

        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    pub async fn capture_pane_until(
        &self,
        expected: &str,
        timeout: Duration,
    ) -> Result<String, anyhow::Error> {
        let start = tokio::time::Instant::now();
        loop {
            let content = self.capture_pane().await?;
            if content.contains(expected) {
                return Ok(content);
            }
            if start.elapsed() >= timeout {
                anyhow::bail!(
                    "timed out waiting for {:?} in pane output",
                    expected
                );
            }
            tokio::time::sleep(Duration::from_millis(100)).await;
        }
    }

    pub async fn resize(&self, cols: u32, rows: u32) -> Result<(), anyhow::Error> {
        let output = Command::new("tmux")
            .args([
                "resize-window",
                "-t",
                &self.name,
                "-x",
                &cols.to_string(),
                "-y",
                &rows.to_string(),
            ])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("tmux resize-window failed: {stderr}");
        }

        Ok(())
    }

    pub async fn kill(&self) -> Result<(), anyhow::Error> {
        let output = Command::new("tmux")
            .args(["kill-session", "-t", &self.name])
            .output()
            .await?;

        if !output.status.success() {
            let stderr = String::from_utf8_lossy(&output.stderr);
            anyhow::bail!("tmux kill-session failed: {stderr}");
        }

        Ok(())
    }
}

impl Drop for TmuxSession {
    fn drop(&mut self) {
        let _ = std::process::Command::new("tmux")
            .args(["kill-session", "-t", &self.name])
            .output();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::time::Duration;

    #[tokio::test]
    async fn tmux_session_creates_and_captures_output() {
        let name = format!("test-pty-{}", std::process::id());
        let session = TmuxSession::create(&name, "/bin/bash", 80, 24)
            .await
            .unwrap();
        session.send_input("echo hello-tmux\n").await.unwrap();

        let output = session
            .capture_pane_until("hello-tmux", Duration::from_secs(2))
            .await
            .unwrap();
        assert!(output.contains("hello-tmux"));

        session.kill().await.unwrap();
    }

    #[tokio::test]
    async fn tmux_session_name_validated() {
        let result = TmuxSession::create("../../evil", "/bin/bash", 80, 24).await;
        assert!(result.is_err());
    }

    #[tokio::test]
    async fn tmux_session_resize_works() {
        let name = format!("test-resize-{}", std::process::id());
        let session = TmuxSession::create(&name, "/bin/bash", 80, 24)
            .await
            .unwrap();
        session.resize(120, 40).await.unwrap();
        session.kill().await.unwrap();
    }
}
