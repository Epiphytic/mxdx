use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

/// A watched session being monitored for heartbeat misses and timeouts.
#[derive(Debug, Clone)]
pub struct WatchedSession {
    pub session_uuid: String,
    pub worker_id: String,
    pub room_id: String,
    pub started_at: u64,
    pub last_heartbeat: u64,
    pub heartbeat_interval_seconds: u64,
    pub timeout_seconds: Option<u64>,
}

/// What went wrong with a session.
#[derive(Debug, Clone, PartialEq)]
pub enum WatchAlert {
    HeartbeatMiss {
        session_uuid: String,
        worker_id: String,
        seconds_since_last: u64,
    },
    Timeout {
        session_uuid: String,
        worker_id: String,
        elapsed_seconds: u64,
    },
}

/// Watchlist monitors active sessions for heartbeat misses and timeouts.
pub struct Watchlist {
    sessions: HashMap<String, WatchedSession>,
}

impl Watchlist {
    pub fn new() -> Self {
        Self {
            sessions: HashMap::new(),
        }
    }

    /// Start watching a session.
    pub fn watch(&mut self, session: WatchedSession) {
        self.sessions
            .insert(session.session_uuid.clone(), session);
    }

    /// Stop watching a session.
    pub fn unwatch(&mut self, session_uuid: &str) {
        self.sessions.remove(session_uuid);
    }

    /// Record a heartbeat for a session, updating its last_heartbeat timestamp.
    pub fn record_heartbeat(&mut self, session_uuid: &str) {
        if let Some(session) = self.sessions.get_mut(session_uuid) {
            session.last_heartbeat = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs();
        }
    }

    /// Check all sessions for alerts. Returns alerts for sessions that have issues.
    ///
    /// Heartbeat miss: triggered when no heartbeat has been received for 2x the interval.
    /// Timeout: triggered when a session has been running longer than its timeout.
    pub fn check(&self) -> Vec<WatchAlert> {
        self.check_at(
            SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .unwrap()
                .as_secs(),
        )
    }

    /// Check all sessions for alerts at a specific timestamp (for testing).
    pub fn check_at(&self, now: u64) -> Vec<WatchAlert> {
        let mut alerts = vec![];
        for session in self.sessions.values() {
            // Heartbeat miss: no heartbeat for 2x interval
            let since_last = now.saturating_sub(session.last_heartbeat);
            if since_last > session.heartbeat_interval_seconds * 2 {
                alerts.push(WatchAlert::HeartbeatMiss {
                    session_uuid: session.session_uuid.clone(),
                    worker_id: session.worker_id.clone(),
                    seconds_since_last: since_last,
                });
            }
            // Timeout
            if let Some(timeout) = session.timeout_seconds {
                let elapsed = now.saturating_sub(session.started_at);
                if elapsed > timeout {
                    alerts.push(WatchAlert::Timeout {
                        session_uuid: session.session_uuid.clone(),
                        worker_id: session.worker_id.clone(),
                        elapsed_seconds: elapsed,
                    });
                }
            }
        }
        alerts
    }

    /// Number of currently watched sessions.
    pub fn watched_count(&self) -> usize {
        self.sessions.len()
    }
}

impl Default for Watchlist {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_session(uuid: &str, started_at: u64, last_heartbeat: u64) -> WatchedSession {
        WatchedSession {
            session_uuid: uuid.into(),
            worker_id: "worker-1".into(),
            room_id: "!room:example.com".into(),
            started_at,
            last_heartbeat,
            heartbeat_interval_seconds: 30,
            timeout_seconds: None,
        }
    }

    #[test]
    fn watch_and_unwatch() {
        let mut wl = Watchlist::new();
        assert_eq!(wl.watched_count(), 0);

        wl.watch(make_session("s-1", 1000, 1000));
        assert_eq!(wl.watched_count(), 1);

        wl.watch(make_session("s-2", 1000, 1000));
        assert_eq!(wl.watched_count(), 2);

        wl.unwatch("s-1");
        assert_eq!(wl.watched_count(), 1);

        wl.unwatch("s-2");
        assert_eq!(wl.watched_count(), 0);
    }

    #[test]
    fn unwatch_nonexistent_is_no_op() {
        let mut wl = Watchlist::new();
        wl.watch(make_session("s-1", 1000, 1000));
        wl.unwatch("s-999");
        assert_eq!(wl.watched_count(), 1);
    }

    #[test]
    fn no_alerts_for_healthy_session() {
        let mut wl = Watchlist::new();
        // Session started at t=1000, last heartbeat at t=1050, interval=30s
        wl.watch(make_session("s-1", 1000, 1050));

        // Check at t=1070 (20s since last heartbeat, under 2x30=60)
        let alerts = wl.check_at(1070);
        assert!(alerts.is_empty());
    }

    #[test]
    fn heartbeat_miss_detection() {
        let mut wl = Watchlist::new();
        // Session with 30s heartbeat interval, last heartbeat at t=1000
        wl.watch(make_session("s-1", 900, 1000));

        // Check at t=1061 (61s since last heartbeat, exceeds 2x30=60)
        let alerts = wl.check_at(1061);
        assert_eq!(alerts.len(), 1);
        match &alerts[0] {
            WatchAlert::HeartbeatMiss {
                session_uuid,
                seconds_since_last,
                ..
            } => {
                assert_eq!(session_uuid, "s-1");
                assert_eq!(*seconds_since_last, 61);
            }
            _ => panic!("expected HeartbeatMiss alert"),
        }
    }

    #[test]
    fn heartbeat_miss_at_exact_boundary_does_not_trigger() {
        let mut wl = Watchlist::new();
        wl.watch(make_session("s-1", 900, 1000));

        // Check at t=1060 (exactly 2x interval, should NOT trigger since we need > 2x)
        let alerts = wl.check_at(1060);
        assert!(alerts.is_empty());
    }

    #[test]
    fn timeout_detection() {
        let mut wl = Watchlist::new();
        let mut session = make_session("s-1", 1000, 1500);
        session.timeout_seconds = Some(600); // 10 minute timeout

        wl.watch(session);

        // Check at t=1601 (601s elapsed, exceeds 600s timeout)
        let alerts = wl.check_at(1601);
        assert!(alerts.iter().any(|a| matches!(a, WatchAlert::Timeout {
            session_uuid, elapsed_seconds, ..
        } if session_uuid == "s-1" && *elapsed_seconds == 601)));
    }

    #[test]
    fn timeout_at_exact_boundary_does_not_trigger() {
        let mut wl = Watchlist::new();
        let mut session = make_session("s-1", 1000, 1500);
        session.timeout_seconds = Some(600);

        wl.watch(session);

        // Check at t=1600 (exactly 600s, should NOT trigger since we need > timeout)
        let alerts = wl.check_at(1600);
        let timeout_alerts: Vec<_> = alerts
            .iter()
            .filter(|a| matches!(a, WatchAlert::Timeout { .. }))
            .collect();
        assert!(timeout_alerts.is_empty());
    }

    #[test]
    fn both_heartbeat_miss_and_timeout() {
        let mut wl = Watchlist::new();
        let mut session = make_session("s-1", 1000, 1000);
        session.timeout_seconds = Some(100);

        wl.watch(session);

        // Check at t=1200 (200s since heartbeat > 60, 200s elapsed > 100s timeout)
        let alerts = wl.check_at(1200);
        assert_eq!(alerts.len(), 2);

        let has_heartbeat = alerts
            .iter()
            .any(|a| matches!(a, WatchAlert::HeartbeatMiss { .. }));
        let has_timeout = alerts
            .iter()
            .any(|a| matches!(a, WatchAlert::Timeout { .. }));
        assert!(has_heartbeat);
        assert!(has_timeout);
    }

    #[test]
    fn no_timeout_when_none() {
        let mut wl = Watchlist::new();
        // Session with no timeout set, but stale heartbeat
        let mut session = make_session("s-1", 1000, 1000);
        session.timeout_seconds = None;

        wl.watch(session);

        // Check at t=999999 — should get heartbeat miss but no timeout
        let alerts = wl.check_at(999999);
        assert!(alerts
            .iter()
            .all(|a| matches!(a, WatchAlert::HeartbeatMiss { .. })));
    }

    #[test]
    fn record_heartbeat_updates_timestamp() {
        let mut wl = Watchlist::new();
        wl.watch(make_session("s-1", 1000, 1000));

        // Record a heartbeat — this uses SystemTime::now(), so we just verify
        // that it doesn't panic and the session is still tracked
        wl.record_heartbeat("s-1");
        assert_eq!(wl.watched_count(), 1);
    }

    #[test]
    fn record_heartbeat_for_nonexistent_session_is_no_op() {
        let mut wl = Watchlist::new();
        wl.record_heartbeat("s-nonexistent");
        assert_eq!(wl.watched_count(), 0);
    }

    #[test]
    fn multiple_sessions_checked_independently() {
        let mut wl = Watchlist::new();
        // s-1: healthy
        wl.watch(make_session("s-1", 1000, 1050));
        // s-2: stale heartbeat
        wl.watch(make_session("s-2", 1000, 900));

        let alerts = wl.check_at(1070);
        // Only s-2 should have an alert (170s since last heartbeat > 60)
        assert_eq!(alerts.len(), 1);
        match &alerts[0] {
            WatchAlert::HeartbeatMiss {
                session_uuid, ..
            } => {
                assert_eq!(session_uuid, "s-2");
            }
            _ => panic!("expected HeartbeatMiss for s-2"),
        }
    }
}
