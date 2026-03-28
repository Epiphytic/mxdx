use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// Track which worker claimed which session
pub struct ClaimTracker {
    claims: HashMap<String, ClaimRecord>,
}

#[derive(Debug, Clone)]
pub struct ClaimRecord {
    pub session_uuid: String,
    pub worker_id: String,
    pub claimed_at: u64,
}

impl ClaimTracker {
    pub fn new() -> Self {
        Self {
            claims: HashMap::new(),
        }
    }

    /// Record a claim. Returns false if session was already claimed.
    pub fn record_claim(&mut self, session_uuid: &str, worker_id: &str) -> bool {
        if self.claims.contains_key(session_uuid) {
            return false; // Already claimed
        }
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_secs();
        self.claims.insert(
            session_uuid.to_string(),
            ClaimRecord {
                session_uuid: session_uuid.to_string(),
                worker_id: worker_id.to_string(),
                claimed_at: now,
            },
        );
        true
    }

    /// Get claim for a session
    pub fn get_claim(&self, session_uuid: &str) -> Option<&ClaimRecord> {
        self.claims.get(session_uuid)
    }

    /// Release a claim (session completed or abandoned)
    pub fn release_claim(&mut self, session_uuid: &str) {
        self.claims.remove(session_uuid);
    }

    /// Check if a session is claimed
    pub fn is_claimed(&self, session_uuid: &str) -> bool {
        self.claims.contains_key(session_uuid)
    }
}

impl Default for ClaimTracker {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn record_and_get_claim() {
        let mut tracker = ClaimTracker::new();
        assert!(tracker.record_claim("s-1", "worker-1"));

        let claim = tracker.get_claim("s-1").unwrap();
        assert_eq!(claim.session_uuid, "s-1");
        assert_eq!(claim.worker_id, "worker-1");
        assert!(claim.claimed_at > 0);
    }

    #[test]
    fn duplicate_claim_returns_false() {
        let mut tracker = ClaimTracker::new();
        assert!(tracker.record_claim("s-1", "worker-1"));
        assert!(!tracker.record_claim("s-1", "worker-2"));

        // Original claim is preserved
        let claim = tracker.get_claim("s-1").unwrap();
        assert_eq!(claim.worker_id, "worker-1");
    }

    #[test]
    fn release_allows_reclaim() {
        let mut tracker = ClaimTracker::new();
        assert!(tracker.record_claim("s-1", "worker-1"));
        tracker.release_claim("s-1");
        assert!(!tracker.is_claimed("s-1"));

        // Now another worker can claim it
        assert!(tracker.record_claim("s-1", "worker-2"));
        let claim = tracker.get_claim("s-1").unwrap();
        assert_eq!(claim.worker_id, "worker-2");
    }

    #[test]
    fn is_claimed_check() {
        let mut tracker = ClaimTracker::new();
        assert!(!tracker.is_claimed("s-1"));

        tracker.record_claim("s-1", "worker-1");
        assert!(tracker.is_claimed("s-1"));
        assert!(!tracker.is_claimed("s-2"));
    }

    #[test]
    fn get_claim_returns_none_for_unknown() {
        let tracker = ClaimTracker::new();
        assert!(tracker.get_claim("s-nonexistent").is_none());
    }

    #[test]
    fn release_nonexistent_is_no_op() {
        let mut tracker = ClaimTracker::new();
        tracker.release_claim("s-nonexistent"); // should not panic
    }
}
