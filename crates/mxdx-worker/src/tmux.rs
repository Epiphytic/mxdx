use std::collections::HashMap;
use std::path::PathBuf;

use anyhow::Result;
use tokio::io::AsyncBufReadExt;
use tokio::process::Command;

/// Manages a tmux session via a dedicated socket, isolating mxdx sessions
/// from the user's regular tmux. Socket stored at `/tmp/mxdx-tmux/mxdx-{session_name}`.
///
/// Non-interactive commands are launched through `mxdx-exec`, a thin wrapper
/// that sends the exit code to a Unix domain socket immediately on completion.
/// The worker awaits this socket instead of polling `is_alive()`.
pub struct TmuxSession {
    pub session_name: String,
    pub socket_path: PathBuf,
    /// UDS path where mxdx-exec sends the exit code on completion.
    pub exit_notify_path: PathBuf,
}

impl TmuxSession {
    /// Resolve the full path to the `mxdx-exec` binary.
    /// Checks the directory of the currently running binary first, then its
    /// parent (handles test binaries in `target/debug/deps/`).
    fn mxdx_exec_path() -> PathBuf {
        if let Ok(exe) = std::env::current_exe() {
            // Try same directory as current binary
            let mut candidate = exe.clone();
            candidate.pop();
            candidate.push("mxdx-exec");
            if candidate.exists() {
                return candidate;
            }
            // Try parent directory (for test binaries in deps/)
            let mut candidate = exe;
            candidate.pop(); // remove binary name
            candidate.pop(); // remove deps/
            candidate.push("mxdx-exec");
            if candidate.exists() {
                return candidate;
            }
        }
        // Fallback
        PathBuf::from("mxdx-exec")
    }

    /// Create a new tmux session running the given command via `mxdx-exec`.
    ///
    /// The command is wrapped: `mxdx-exec --notify <uds_path> -- <bin> <args...>`
    /// This preserves all tmux features (capture, resize, attach) while giving
    /// immediate exit code notification through the UDS.
    pub async fn create(
        session_name: &str,
        bin: &str,
        args: &[String],
        cwd: Option<&str>,
        env: &HashMap<String, String>,
    ) -> Result<Self> {
        let socket_path = Self::socket_dir().join(format!("mxdx-{session_name}"));
        let exit_notify_path = Self::socket_dir().join(format!("mxdx-{session_name}.notify"));

        // Clean up any stale socket from a previous run
        let _ = std::fs::remove_file(&exit_notify_path);

        // Create the UDS listener BEFORE starting tmux so mxdx-exec can connect
        let listener = std::os::unix::net::UnixListener::bind(&exit_notify_path)?;
        // Set non-blocking so we can convert to tokio later
        listener.set_nonblocking(true)?;

        let exec_path = Self::mxdx_exec_path();
        if !exec_path.exists() {
            anyhow::bail!(
                "mxdx-exec not found at {}. Build with: cargo build -p mxdx-worker --bin mxdx-exec",
                exec_path.display()
            );
        }

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

        // Launch: mxdx-exec --notify <uds> -- <bin> <args...>
        cmd.arg(exec_path.to_str().unwrap());
        cmd.args(["--notify", exit_notify_path.to_str().unwrap()]);
        cmd.arg("--");
        cmd.arg(bin);
        for arg in args {
            cmd.arg(arg);
        }

        let output = cmd.output().await?;
        if !output.status.success() {
            // Clean up the listener socket on failure
            let _ = std::fs::remove_file(&exit_notify_path);
            anyhow::bail!(
                "tmux new-session failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }

        // Spawn a background task that accepts the UDS connection from mxdx-exec
        // and writes the exit code to a file for later retrieval.
        let exit_file = Self::socket_dir().join(format!("mxdx-{session_name}.exit"));
        let notify_path_cleanup = exit_notify_path.clone();

        tokio::spawn(async move {
            let tokio_listener = match tokio::net::UnixListener::from_std(listener) {
                Ok(l) => l,
                Err(e) => {
                    tracing::warn!("Failed to convert UDS listener to tokio: {e}");
                    return;
                }
            };

            match tokio_listener.accept().await {
                Ok((stream, _)) => {
                    let reader = tokio::io::BufReader::new(stream);
                    let mut lines = reader.lines();
                    if let Ok(Some(line)) = lines.next_line().await {
                        // Write exit code to file for retrieval
                        let _ = std::fs::write(&exit_file, line.trim());
                    }
                }
                Err(e) => {
                    tracing::warn!("UDS accept failed: {e}");
                }
            }

            let _ = tokio::fs::remove_file(&notify_path_cleanup).await;
        });

        Ok(Self {
            session_name: session_name.to_string(),
            socket_path,
            exit_notify_path,
        })
    }

    /// Create an interactive tmux session with a shell.
    /// Interactive sessions run the shell directly (no mxdx-exec wrapper).
    pub async fn create_interactive(session_name: &str, shell: &str) -> Result<Self> {
        let socket_path = Self::socket_dir().join(format!("mxdx-{session_name}"));
        let exit_notify_path = Self::socket_dir().join(format!("mxdx-{session_name}.notify"));

        let mut cmd = Command::new("tmux");
        cmd.args(["-S", socket_path.to_str().unwrap()])
            .args(["new-session", "-d", "-s", session_name])
            .arg(shell);

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
            exit_notify_path,
        })
    }

    /// Read the exit code written by mxdx-exec via UDS -> background task -> file.
    /// Returns None if the exit code hasn't been received yet.
    pub fn read_exit_code(&self) -> Option<i32> {
        let exit_file = Self::socket_dir().join(format!("mxdx-{}.exit", self.session_name));
        std::fs::read_to_string(&exit_file)
            .ok()
            .and_then(|s| s.trim().parse::<i32>().ok())
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
        // Clean up socket and notify files
        let _ = tokio::fs::remove_file(&self.socket_path).await;
        let _ = tokio::fs::remove_file(&self.exit_notify_path).await;
        let exit_file = Self::socket_dir().join(format!("mxdx-{}.exit", self.session_name));
        let _ = tokio::fs::remove_file(&exit_file).await;
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
        session.kill().await.expect("kill failed");
    }

    #[tokio::test]
    async fn test_exit_code_nonzero() {
        if !tmux_available().await {
            eprintln!("tmux not available, skipping test");
            return;
        }

        let session = TmuxSession::create(
            "test-exit-nz",
            "/bin/false",
            &[],
            None,
            &HashMap::new(),
        )
        .await
        .expect("failed to create tmux session");

        // Wait for the process to exit (mxdx-exec notifies via UDS)
        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let exit_code = session.read_exit_code();
        assert_eq!(exit_code, Some(1), "exit code should be 1 for /bin/false");
        session.kill().await.expect("kill failed");
    }

    #[tokio::test]
    async fn test_exit_code_success() {
        if !tmux_available().await {
            eprintln!("tmux not available, skipping test");
            return;
        }

        let session = TmuxSession::create(
            "test-exit-ok",
            "/bin/true",
            &[],
            None,
            &HashMap::new(),
        )
        .await
        .expect("failed to create tmux session");

        tokio::time::sleep(std::time::Duration::from_secs(2)).await;

        let exit_code = session.read_exit_code();
        assert_eq!(exit_code, Some(0), "exit code should be 0 for /bin/true");
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

        session.kill().await.expect("kill failed");
    }
}
