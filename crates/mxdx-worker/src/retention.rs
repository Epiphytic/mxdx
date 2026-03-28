use mxdx_types::events::session::CompletedSessionState;
use std::time::{SystemTime, UNIX_EPOCH};

pub struct RetentionSweeper {
    retention_days: u64,
}

impl RetentionSweeper {
    pub fn new(retention_days: u64) -> Self {
        Self { retention_days }
    }

    /// Check if a completed session is expired (older than retention window).
    pub fn is_expired(&self, completed: &CompletedSessionState) -> bool {
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        let retention_seconds = self.retention_days * 24 * 60 * 60;
        now.saturating_sub(completed.completion_time) > retention_seconds
    }

    /// Filter a list of (state_key, CompletedSessionState) pairs to find expired ones.
    /// Returns the state keys of expired sessions.
    pub fn find_expired(&self, sessions: &[(String, CompletedSessionState)]) -> Vec<String> {
        sessions
            .iter()
            .filter(|(_, s)| self.is_expired(s))
            .map(|(key, _)| key.clone())
            .collect()
    }

    pub fn retention_days(&self) -> u64 {
        self.retention_days
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn now_secs() -> u64 {
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs()
    }

    fn make_completed(completion_time: u64) -> CompletedSessionState {
        CompletedSessionState {
            exit_code: Some(0),
            duration_seconds: 60,
            completion_time,
        }
    }

    #[test]
    fn session_within_retention_is_not_expired() {
        let sweeper = RetentionSweeper::new(90);
        // Completed 1 day ago
        let recent = make_completed(now_secs() - 24 * 60 * 60);
        assert!(!sweeper.is_expired(&recent));
    }

    #[test]
    fn session_older_than_retention_is_expired() {
        let sweeper = RetentionSweeper::new(90);
        // Completed 91 days ago
        let old = make_completed(now_secs() - 91 * 24 * 60 * 60);
        assert!(sweeper.is_expired(&old));
    }

    #[test]
    fn find_expired_returns_only_expired_state_keys() {
        let sweeper = RetentionSweeper::new(30);
        let now = now_secs();
        let sessions = vec![
            ("session/abc/completed".into(), make_completed(now - 5 * 24 * 60 * 60)),   // 5 days ago — fresh
            ("session/def/completed".into(), make_completed(now - 31 * 24 * 60 * 60)),  // 31 days ago — expired
            ("session/ghi/completed".into(), make_completed(now - 60 * 24 * 60 * 60)),  // 60 days ago — expired
            ("session/jkl/completed".into(), make_completed(now - 1 * 24 * 60 * 60)),   // 1 day ago — fresh
        ];
        let expired = sweeper.find_expired(&sessions);
        assert_eq!(expired.len(), 2);
        assert!(expired.contains(&"session/def/completed".to_string()));
        assert!(expired.contains(&"session/ghi/completed".to_string()));
    }

    #[test]
    fn session_at_exact_retention_boundary_is_not_expired() {
        let sweeper = RetentionSweeper::new(90);
        // Completed exactly 90 days ago (the boundary uses >, not >=)
        let boundary = make_completed(now_secs() - 90 * 24 * 60 * 60);
        assert!(!sweeper.is_expired(&boundary));
    }

    #[test]
    fn retention_days_accessor() {
        let sweeper = RetentionSweeper::new(180);
        assert_eq!(sweeper.retention_days(), 180);
    }
}
