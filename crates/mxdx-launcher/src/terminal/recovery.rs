use anyhow::Result;
use std::collections::HashMap;
use tokio::process::Command;

/// List existing tmux sessions and their names.
pub async fn list_tmux_sessions() -> Result<Vec<String>> {
    let output = Command::new("tmux")
        .args(["list-sessions", "-F", "#{session_name}"])
        .output()
        .await?;

    if !output.status.success() {
        // No tmux server running = no sessions to recover
        return Ok(Vec::new());
    }

    let names = String::from_utf8_lossy(&output.stdout)
        .lines()
        .map(|l| l.to_string())
        .collect();
    Ok(names)
}

/// Recovery state: maps tmux session names to their DM room IDs.
/// Persisted to a JSON file in the launcher's data_dir.
#[derive(Debug, Clone, serde::Serialize, serde::Deserialize, Default)]
pub struct RecoveryState {
    pub sessions: HashMap<String, SessionState>,
}

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SessionState {
    pub session_id: String,
    pub dm_room_id: String,
    pub command: String,
    pub cols: u32,
    pub rows: u32,
    pub last_seq: u64,
}

impl RecoveryState {
    pub fn load(path: &std::path::Path) -> Result<Self> {
        if !path.exists() {
            return Ok(Self::default());
        }
        let data = std::fs::read_to_string(path)?;
        Ok(serde_json::from_str(&data)?)
    }

    pub fn save(&self, path: &std::path::Path) -> Result<()> {
        let data = serde_json::to_string_pretty(self)?;
        std::fs::write(path, data)?;
        Ok(())
    }

    pub fn add_session(&mut self, state: SessionState) {
        self.sessions.insert(state.session_id.clone(), state);
    }

    pub fn remove_session(&mut self, session_id: &str) {
        self.sessions.remove(session_id);
    }

    /// Match existing tmux sessions against saved state.
    /// Returns session states that can be recovered.
    pub fn recoverable_sessions(&self, tmux_sessions: &[String]) -> Vec<&SessionState> {
        self.sessions
            .values()
            .filter(|s| tmux_sessions.contains(&s.session_id))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    #[test]
    fn recovery_state_save_and_load() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("recovery.json");

        let mut state = RecoveryState::default();
        state.add_session(SessionState {
            session_id: "test-1".into(),
            dm_room_id: "!abc:localhost".into(),
            command: "/bin/bash".into(),
            cols: 80,
            rows: 24,
            last_seq: 42,
        });
        state.save(&path).unwrap();

        let loaded = RecoveryState::load(&path).unwrap();
        assert_eq!(loaded.sessions.len(), 1);
        assert_eq!(loaded.sessions["test-1"].last_seq, 42);
    }

    #[test]
    fn recovery_state_empty_file_returns_default() {
        let dir = TempDir::new().unwrap();
        let path = dir.path().join("nonexistent.json");
        let state = RecoveryState::load(&path).unwrap();
        assert!(state.sessions.is_empty());
    }

    #[test]
    fn recoverable_sessions_matches_tmux() {
        let mut state = RecoveryState::default();
        state.add_session(SessionState {
            session_id: "alive".into(),
            dm_room_id: "!room1:localhost".into(),
            command: "/bin/bash".into(),
            cols: 80,
            rows: 24,
            last_seq: 0,
        });
        state.add_session(SessionState {
            session_id: "dead".into(),
            dm_room_id: "!room2:localhost".into(),
            command: "/bin/bash".into(),
            cols: 80,
            rows: 24,
            last_seq: 0,
        });

        let tmux_sessions = vec!["alive".to_string(), "other".to_string()];
        let recoverable = state.recoverable_sessions(&tmux_sessions);
        assert_eq!(recoverable.len(), 1);
        assert_eq!(recoverable[0].session_id, "alive");
    }

    #[tokio::test]
    async fn list_tmux_sessions_returns_list() {
        // This will return empty or actual sessions depending on tmux state
        let sessions = list_tmux_sessions().await.unwrap();
        // Just verify it doesn't error
        drop(sessions);
    }
}
