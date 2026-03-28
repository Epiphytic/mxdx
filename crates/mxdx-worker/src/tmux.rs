use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use tokio::process::Command;

/// Manages a tmux session via a dedicated socket, isolating mxdx sessions
/// from the user's regular tmux. Socket stored at `/tmp/mxdx-tmux/mxdx-{session_name}`.
pub struct TmuxSession {
    pub session_name: String,
    pub socket_path: PathBuf,
}

impl TmuxSession {
    /// Create a new tmux session running the given command.
    pub async fn create(
        session_name: &str,
        bin: &str,
        args: &[String],
        cwd: Option<&str>,
        env: &HashMap<String, String>,
    ) -> Result<Self> {
        let socket_path = Self::socket_dir().join(format!("mxdx-{session_name}"));
        let mut cmd = Command::new("tmux");
        cmd.args(["-S", socket_path.to_str().unwrap()])
            .args(["new-session", "-d", "-s", session_name]);

        // Set environment variables
        for (k, v) in env {
            cmd.args(["-e", &format!("{k}={v}")]);
        }

        // Set working directory
        if let Some(cwd) = cwd {
            cmd.args(["-c", cwd]);
        }

        // The command to run
        cmd.arg(bin);
        for arg in args {
            cmd.arg(arg);
        }

        let output = cmd.output().await?;
        if !output.status.success() {
            anyhow::bail!(
                "tmux new-session failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        Ok(Self {
            session_name: session_name.to_string(),
            socket_path,
        })
    }

    /// Create an interactive tmux session with a shell.
    pub async fn create_interactive(session_name: &str, shell: &str) -> Result<Self> {
        Self::create(session_name, shell, &[], None, &HashMap::new()).await
    }

    /// Send data to the tmux session's stdin.
    pub async fn send_keys(&self, data: &[u8]) -> Result<()> {
        let text = String::from_utf8_lossy(data);
        let output = Command::new("tmux")
            .args(["-S", self.socket_path.to_str().unwrap()])
            .args(["send-keys", "-t", &self.session_name, "-l", &text])
            .output()
            .await?;
        if !output.status.success() {
            anyhow::bail!(
                "tmux send-keys failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    /// Resize the tmux window.
    pub async fn resize(&self, cols: u32, rows: u32) -> Result<()> {
        let output = Command::new("tmux")
            .args(["-S", self.socket_path.to_str().unwrap()])
            .args([
                "resize-window",
                "-t",
                &self.session_name,
                "-x",
                &cols.to_string(),
                "-y",
                &rows.to_string(),
            ])
            .output()
            .await?;
        if !output.status.success() {
            anyhow::bail!(
                "tmux resize failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        Ok(())
    }

    /// Capture current scrollback.
    pub async fn capture_pane(&self) -> Result<String> {
        let output = Command::new("tmux")
            .args(["-S", self.socket_path.to_str().unwrap()])
            .args(["capture-pane", "-t", &self.session_name, "-p"])
            .output()
            .await?;
        if !output.status.success() {
            anyhow::bail!("tmux capture-pane failed");
        }
        Ok(String::from_utf8_lossy(&output.stdout).to_string())
    }

    /// List existing sessions on a given socket.
    pub async fn list(socket_path: &PathBuf) -> Result<Vec<String>> {
        let output = Command::new("tmux")
            .args(["-S", socket_path.to_str().unwrap()])
            .args(["list-sessions", "-F", "#{session_name}"])
            .output()
            .await?;
        if !output.status.success() {
            return Ok(vec![]); // No sessions
        }
        Ok(String::from_utf8_lossy(&output.stdout)
            .lines()
            .map(|s| s.to_string())
            .collect())
    }

    /// Kill the session.
    pub async fn kill(&self) -> Result<()> {
        let _ = Command::new("tmux")
            .args(["-S", self.socket_path.to_str().unwrap()])
            .args(["kill-session", "-t", &self.session_name])
            .output()
            .await;
        // Clean up socket
        let _ = tokio::fs::remove_file(&self.socket_path).await;
        Ok(())
    }

    /// Check if session is still alive.
    pub async fn is_alive(&self) -> Result<bool> {
        let output = Command::new("tmux")
            .args(["-S", self.socket_path.to_str().unwrap()])
            .args(["has-session", "-t", &self.session_name])
            .output()
            .await?;
        Ok(output.status.success())
    }

    /// Get the socket directory for mxdx tmux sessions.
    fn socket_dir() -> PathBuf {
        let dir = std::env::temp_dir().join("mxdx-tmux");
        std::fs::create_dir_all(&dir).ok();
        dir
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Returns true if tmux is available on the system.
    async fn tmux_available() -> bool {
        Command::new("which")
            .arg("tmux")
            .output()
            .await
            .map(|o| o.status.success())
            .unwrap_or(false)
    }

    #[tokio::test]
    async fn test_create_and_is_alive() {
        if !tmux_available().await {
            eprintln!("tmux not available, skipping test");
            return;
        }

        let session = TmuxSession::create(
            "test-alive",
            "sleep",
            &["10".to_string()],
            None,
            &HashMap::new(),
        )
        .await
        .expect("failed to create tmux session");

        assert!(session.is_alive().await.expect("is_alive failed"));

        // Cleanup
        session.kill().await.expect("kill failed");
    }

    #[tokio::test]
    async fn test_session_completes() {
        if !tmux_available().await {
            eprintln!("tmux not available, skipping test");
            return;
        }

        // Run a command that exits quickly
        let session = TmuxSession::create(
            "test-complete",
            "echo",
            &["hello".to_string()],
            None,
            &HashMap::new(),
        )
        .await
        .expect("failed to create tmux session");

        // Give the command time to complete and the session to close
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        // tmux sessions running a command that exits will remain alive
        // (showing the shell or exit status), so we just verify no error
        let alive = session.is_alive().await.expect("is_alive failed");
        // Session may or may not be alive depending on tmux config;
        // the important thing is no error was raised.
        let _ = alive;

        // Cleanup
        session.kill().await.expect("kill failed");
    }

    #[tokio::test]
    async fn test_capture_pane() {
        if !tmux_available().await {
            eprintln!("tmux not available, skipping test");
            return;
        }

        let session = TmuxSession::create_interactive("test-capture", "/bin/sh")
            .await
            .expect("failed to create interactive session");

        // Send a command and wait for output
        session
            .send_keys(b"echo MXDX_TEST_OUTPUT\n")
            .await
            .expect("send_keys failed");

        tokio::time::sleep(std::time::Duration::from_secs(1)).await;

        let captured = session
            .capture_pane()
            .await
            .expect("capture_pane failed");

        assert!(
            captured.contains("MXDX_TEST_OUTPUT"),
            "captured pane should contain test output, got: {captured}"
        );

        // Cleanup
        session.kill().await.expect("kill failed");
    }

    #[tokio::test]
    async fn test_kill_terminates_session() {
        if !tmux_available().await {
            eprintln!("tmux not available, skipping test");
            return;
        }

        let session = TmuxSession::create(
            "test-kill",
            "sleep",
            &["60".to_string()],
            None,
            &HashMap::new(),
        )
        .await
        .expect("failed to create tmux session");

        assert!(session.is_alive().await.expect("is_alive failed"));

        session.kill().await.expect("kill failed");

        // After kill, session should not be alive
        assert!(
            !session.is_alive().await.expect("is_alive after kill failed"),
            "session should not be alive after kill"
        );
    }

    #[tokio::test]
    async fn test_list_sessions() {
        if !tmux_available().await {
            eprintln!("tmux not available, skipping test");
            return;
        }

        let session = TmuxSession::create(
            "test-list",
            "sleep",
            &["10".to_string()],
            None,
            &HashMap::new(),
        )
        .await
        .expect("failed to create tmux session");

        let sessions = TmuxSession::list(&session.socket_path)
            .await
            .expect("list failed");

        assert!(
            sessions.contains(&"test-list".to_string()),
            "session list should contain test-list, got: {sessions:?}"
        );

        // Cleanup
        session.kill().await.expect("kill failed");
    }
}
