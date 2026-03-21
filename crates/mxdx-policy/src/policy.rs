use std::collections::HashSet;
use std::num::NonZeroUsize;
use std::time::{Duration, Instant};

use lru::LruCache;
use tracing::{debug, warn};

/// Default replay cache capacity.
const DEFAULT_CACHE_CAPACITY: usize = 10_000;

/// Default TTL for replay cache entries.
const DEFAULT_TTL: Duration = Duration::from_secs(3600); // 1 hour

/// Policy enforcement engine for mxdx.
///
/// Provides:
/// - Replay protection via an LRU cache with TTL (mxdx-rpl)
/// - Authorization checks for user actions
pub struct PolicyEngine {
    /// LRU cache mapping event IDs to the time they were first seen.
    replay_cache: LruCache<String, Instant>,
    /// TTL for replay cache entries.
    ttl: Duration,
    /// Set of authorized user IDs.
    authorized_users: HashSet<String>,
}

impl PolicyEngine {
    /// Create a new PolicyEngine with default capacity and TTL.
    pub fn new() -> Self {
        Self {
            replay_cache: LruCache::new(NonZeroUsize::new(DEFAULT_CACHE_CAPACITY).unwrap()),
            ttl: DEFAULT_TTL,
            authorized_users: HashSet::new(),
        }
    }

    /// Create a new PolicyEngine with custom capacity and TTL.
    pub fn with_capacity_and_ttl(capacity: usize, ttl: Duration) -> Self {
        Self {
            replay_cache: LruCache::new(NonZeroUsize::new(capacity).expect("capacity must be > 0")),
            ttl,
            authorized_users: HashSet::new(),
        }
    }

    /// Add an authorized user ID.
    pub fn authorize_user(&mut self, user_id: &str) {
        self.authorized_users.insert(user_id.to_string());
    }

    /// Remove an authorized user ID.
    pub fn revoke_user(&mut self, user_id: &str) {
        self.authorized_users.remove(user_id);
    }

    /// Check if an event ID has already been seen (replay detection).
    /// Returns `true` if the event is a replay (already seen and not expired).
    pub fn check_replay(&mut self, event_id: &str) -> bool {
        if let Some(seen_at) = self.replay_cache.get(event_id) {
            if seen_at.elapsed() < self.ttl {
                debug!(event_id, "Replay detected: event already seen");
                return true;
            }
            // Entry expired — remove it and treat as new
            self.replay_cache.pop(event_id);
        }
        false
    }

    /// Mark an event ID as seen in the replay cache.
    pub fn mark_seen(&mut self, event_id: &str) {
        self.replay_cache.put(event_id.to_string(), Instant::now());
    }

    /// Check if a user is authorized to perform an action.
    /// Returns `true` if the user is in the authorized set.
    pub fn is_authorized(&self, user_id: &str, action: &str) -> bool {
        let authorized = self.authorized_users.contains(user_id);
        if !authorized {
            warn!(user_id, action, "Unauthorized action attempt");
        } else {
            debug!(user_id, action, "Action authorized");
        }
        authorized
    }

    /// Process a command event. Returns `Ok(())` if the command should be executed,
    /// or `Err` with a reason if it should be rejected.
    ///
    /// This combines replay detection and authorization in a single call.
    pub fn evaluate(
        &mut self,
        event_id: &str,
        user_id: &str,
        action: &str,
    ) -> Result<(), PolicyRejection> {
        // Check replay first
        if self.check_replay(event_id) {
            return Err(PolicyRejection::Replay);
        }

        // Check authorization
        if !self.is_authorized(user_id, action) {
            return Err(PolicyRejection::Unauthorized);
        }

        // Mark as seen only after passing all checks
        self.mark_seen(event_id);
        Ok(())
    }
}

impl Default for PolicyEngine {
    fn default() -> Self {
        Self::new()
    }
}

/// Reason a policy check was rejected.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PolicyRejection {
    /// The event was already processed (replay).
    Replay,
    /// The user is not authorized for this action.
    Unauthorized,
}

impl std::fmt::Display for PolicyRejection {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            PolicyRejection::Replay => write!(f, "replayed event rejected"),
            PolicyRejection::Unauthorized => write!(f, "unauthorized user rejected"),
        }
    }
}

impl std::error::Error for PolicyRejection {}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn new_event_is_not_replay() {
        let mut engine = PolicyEngine::new();
        assert!(!engine.check_replay("$event1"));
    }

    #[test]
    fn seen_event_is_replay() {
        let mut engine = PolicyEngine::new();
        engine.mark_seen("$event1");
        assert!(engine.check_replay("$event1"));
    }

    #[test]
    fn expired_event_is_not_replay() {
        let mut engine = PolicyEngine::with_capacity_and_ttl(100, Duration::from_millis(1));
        engine.mark_seen("$event1");
        std::thread::sleep(Duration::from_millis(5));
        assert!(!engine.check_replay("$event1"));
    }

    #[test]
    fn authorized_user_is_authorized() {
        let mut engine = PolicyEngine::new();
        engine.authorize_user("@alice:example.com");
        assert!(engine.is_authorized("@alice:example.com", "execute"));
    }

    #[test]
    fn unauthorized_user_is_rejected() {
        let engine = PolicyEngine::new();
        assert!(!engine.is_authorized("@bob:example.com", "execute"));
    }

    #[test]
    fn revoked_user_is_rejected() {
        let mut engine = PolicyEngine::new();
        engine.authorize_user("@alice:example.com");
        engine.revoke_user("@alice:example.com");
        assert!(!engine.is_authorized("@alice:example.com", "execute"));
    }

    #[test]
    fn evaluate_authorized_new_event_passes() {
        let mut engine = PolicyEngine::new();
        engine.authorize_user("@alice:example.com");
        assert!(engine
            .evaluate("$evt1", "@alice:example.com", "execute")
            .is_ok());
    }

    #[test]
    fn evaluate_unauthorized_user_rejected() {
        let mut engine = PolicyEngine::new();
        assert_eq!(
            engine.evaluate("$evt1", "@bob:example.com", "execute"),
            Err(PolicyRejection::Unauthorized)
        );
    }

    #[test]
    fn evaluate_replayed_event_rejected() {
        let mut engine = PolicyEngine::new();
        engine.authorize_user("@alice:example.com");
        engine
            .evaluate("$evt1", "@alice:example.com", "execute")
            .unwrap();
        assert_eq!(
            engine.evaluate("$evt1", "@alice:example.com", "execute"),
            Err(PolicyRejection::Replay)
        );
    }
}
