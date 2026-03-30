use anyhow::Result;
use serde::{Deserialize, Serialize};
use std::path::PathBuf;

/// On-disk representation of an active session.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct PersistedSession {
    pub uuid: String,
    pub tmux_session: String,
    pub bin: String,
    pub args: Vec<String>,
    pub started_at: String, // ISO 8601
    pub thread_root: Option<String>,
}

/// Default file path for sessions: ~/.mxdx/sessions.json
fn default_sessions_path() -> Option<PathBuf> {
    dirs::home_dir().map(|h| h.join(".mxdx").join("sessions.json"))
}

/// Resolve sessions path: use `base_path` if provided, otherwise default.
fn sessions_path(base_path: Option<&std::path::Path>) -> Option<PathBuf> {
    match base_path {
        Some(base) => Some(base.join("sessions.json")),
        None => default_sessions_path(),
    }
}

/// Save active sessions to disk.
///
/// `base_path`: optional override for the directory containing sessions.json
/// (used in tests with tempdir). When `None`, uses `~/.mxdx/`.
pub fn save_sessions(
    sessions: &[PersistedSession],
    base_path: Option<&std::path::Path>,
) -> Result<()> {
    let path = sessions_path(base_path)
        .ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let json = serde_json::to_string_pretty(sessions)?;
    std::fs::write(&path, json)?;
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
    }
    Ok(())
}

/// Load persisted sessions from disk.
pub fn load_sessions(base_path: Option<&std::path::Path>) -> Result<Vec<PersistedSession>> {
    let path = sessions_path(base_path)
        .ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    if !path.exists() {
        return Ok(Vec::new());
    }
    let json = std::fs::read_to_string(&path)?;
    let sessions: Vec<PersistedSession> = serde_json::from_str(&json)?;
    Ok(sessions)
}

/// Remove the sessions file (e.g., when worker shuts down cleanly with no active sessions).
pub fn clear_sessions(base_path: Option<&std::path::Path>) -> Result<()> {
    let path = sessions_path(base_path)
        .ok_or_else(|| anyhow::anyhow!("no home directory"))?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    Ok(())
}

/// Check if a tmux session is still alive.
pub fn is_tmux_session_alive(session_name: &str) -> bool {
    std::process::Command::new("tmux")
        .args(["has-session", "-t", session_name])
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map(|s| s.success())
        .unwrap_or(false)
}

/// Recover orphaned sessions by checking which tmux sessions are still running.
/// Returns only sessions whose tmux session is still alive; updates the file accordingly.
pub fn recover_sessions(base_path: Option<&std::path::Path>) -> Result<Vec<PersistedSession>> {
    let sessions = load_sessions(base_path)?;
    let alive: Vec<_> = sessions
        .into_iter()
        .filter(|s| is_tmux_session_alive(&s.tmux_session))
        .collect();
    // Update the file with only alive sessions
    save_sessions(&alive, base_path)?;
    Ok(alive)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session(uuid: &str, tmux: &str) -> PersistedSession {
        PersistedSession {
            uuid: uuid.to_string(),
            tmux_session: tmux.to_string(),
            bin: "echo".to_string(),
            args: vec!["hello".to_string()],
            started_at: "2026-03-29T12:00:00Z".to_string(),
            thread_root: Some("$event123".to_string()),
        }
    }

    #[test]
    fn test_save_and_load_sessions() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();

        let sessions = vec![
            make_session("s-1", "tmux-s-1"),
            make_session("s-2", "tmux-s-2"),
        ];
        save_sessions(&sessions, Some(base)).unwrap();

        let loaded = load_sessions(Some(base)).unwrap();
        assert_eq!(loaded.len(), 2);
        assert_eq!(loaded[0].uuid, "s-1");
        assert_eq!(loaded[1].uuid, "s-2");
        assert_eq!(loaded[0].bin, "echo");
        assert_eq!(loaded[1].args, vec!["hello"]);
        assert_eq!(loaded, sessions);
    }

    #[test]
    fn test_load_returns_empty_when_no_file() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();

        let loaded = load_sessions(Some(base)).unwrap();
        assert!(loaded.is_empty());
    }

    #[test]
    fn test_clear_sessions_removes_file() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();

        let sessions = vec![make_session("s-1", "tmux-s-1")];
        save_sessions(&sessions, Some(base)).unwrap();

        let path = base.join("sessions.json");
        assert!(path.exists());

        clear_sessions(Some(base)).unwrap();
        assert!(!path.exists());
    }

    #[cfg(unix)]
    #[test]
    fn test_save_sessions_file_permissions() {
        use std::os::unix::fs::PermissionsExt;

        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();

        let sessions = vec![make_session("s-1", "tmux-s-1")];
        save_sessions(&sessions, Some(base)).unwrap();

        let path = base.join("sessions.json");
        let metadata = std::fs::metadata(&path).unwrap();
        let mode = metadata.permissions().mode() & 0o777;
        assert_eq!(mode, 0o600, "expected 0o600 permissions, got 0o{:o}", mode);
    }

    #[test]
    fn test_recover_sessions_filters_dead() {
        let tmp = tempfile::tempdir().unwrap();
        let base = tmp.path();

        // Save sessions with fake tmux names that definitely don't exist
        let sessions = vec![
            make_session("s-1", "mxdx-fake-nonexistent-1234"),
            make_session("s-2", "mxdx-fake-nonexistent-5678"),
        ];
        save_sessions(&sessions, Some(base)).unwrap();

        // Recover should filter all dead sessions
        let recovered = recover_sessions(Some(base)).unwrap();
        assert!(
            recovered.is_empty(),
            "expected no recovered sessions (fake tmux names), got {}",
            recovered.len()
        );

        // The file should now be updated with empty list
        let reloaded = load_sessions(Some(base)).unwrap();
        assert!(reloaded.is_empty());
    }
}
