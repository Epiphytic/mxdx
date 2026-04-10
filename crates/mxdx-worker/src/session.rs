use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};

use anyhow::{bail, Result};
use mxdx_types::events::session::{
    ActiveSessionState, CompletedSessionState, SessionStatus, SessionTask,
};

use crate::tmux::TmuxSession;

#[derive(Debug, Clone, PartialEq)]
pub enum SessionState {
    Claimed,
    Running,
    Completed,
}

pub struct Session {
    pub uuid: String,
    pub task: SessionTask,
    pub state: SessionState,
    pub tmux: Option<TmuxSession>,
    pub started_at: u64,
    pub worker_id: String,
    pub pid: Option<u32>,
}

pub struct SessionManager {
    sessions: HashMap<String, Session>,
    worker_id: String,
}

impl SessionManager {
    pub fn new(worker_id: String) -> Self {
        Self {
            sessions: HashMap::new(),
            worker_id,
        }
    }

    /// Returns true if the given UUID is already tracked (claimed, running,
    /// or recently completed). Matrix redelivers events after reconnect and
    /// resync, so the main worker loop must dedupe on task UUID before
    /// calling `claim` — otherwise a redelivered task would crash the
    /// worker. See the `[duplicate task guard]` comment in `lib.rs`.
    pub fn contains_session(&self, uuid: &str) -> bool {
        self.sessions.contains_key(uuid)
    }

    /// Claim a session by creating the internal tracking entry.
    /// Returns the ActiveSessionState to be written as a state event.
    pub fn claim(&mut self, task: SessionTask) -> Result<ActiveSessionState> {
        let uuid = task.uuid.clone();
        if self.sessions.contains_key(&uuid) {
            bail!("session {} already exists", uuid);
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let active_state = ActiveSessionState {
            bin: task.bin.clone(),
            args: task.args.clone(),
            pid: None,
            start_time: now,
            client_id: task.sender_id.clone(),
            interactive: task.interactive,
            worker_id: self.worker_id.clone(),
        };
        let session = Session {
            uuid: uuid.clone(),
            task,
            state: SessionState::Claimed,
            tmux: None,
            started_at: now,
            worker_id: self.worker_id.clone(),
            pid: None,
        };
        self.sessions.insert(uuid, session);
        Ok(active_state)
    }

    /// Transition to running after process start.
    pub fn mark_running(&mut self, uuid: &str, pid: Option<u32>, tmux: TmuxSession) -> Result<()> {
        let session = self
            .sessions
            .get_mut(uuid)
            .ok_or_else(|| anyhow::anyhow!("session {} not found", uuid))?;
        if session.state != SessionState::Claimed {
            bail!(
                "session {} is not in Claimed state (current: {:?})",
                uuid,
                session.state
            );
        }
        session.state = SessionState::Running;
        session.pid = pid;
        session.tmux = Some(tmux);
        Ok(())
    }

    /// Complete a session. Returns CompletedSessionState to be written as state event.
    pub fn complete(
        &mut self,
        uuid: &str,
        _status: SessionStatus,
        exit_code: Option<i32>,
    ) -> Result<CompletedSessionState> {
        let session = self
            .sessions
            .get_mut(uuid)
            .ok_or_else(|| anyhow::anyhow!("session {} not found", uuid))?;
        if session.state == SessionState::Completed {
            bail!("session {} already completed", uuid);
        }
        let now = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();
        let duration = now.saturating_sub(session.started_at);
        session.state = SessionState::Completed;
        Ok(CompletedSessionState {
            exit_code,
            duration_seconds: duration,
            completion_time: now,
        })
    }

    /// List active (non-completed) sessions.
    pub fn active_sessions(&self) -> Vec<&Session> {
        self.sessions
            .values()
            .filter(|s| s.state != SessionState::Completed)
            .collect()
    }

    /// Get session by UUID.
    pub fn get(&self, uuid: &str) -> Option<&Session> {
        self.sessions.get(uuid)
    }

    /// Get mutable session by UUID.
    pub fn get_mut(&mut self, uuid: &str) -> Option<&mut Session> {
        self.sessions.get_mut(uuid)
    }

    /// Remove a completed session from tracking.
    pub fn remove(&mut self, uuid: &str) -> Option<Session> {
        self.sessions.remove(uuid)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task(uuid: &str) -> SessionTask {
        SessionTask {
            uuid: uuid.to_string(),
            sender_id: "@alice:example.com".to_string(),
            bin: "echo".to_string(),
            args: vec!["hello".to_string()],
            env: None,
            cwd: None,
            interactive: false,
            no_room_output: false,
            timeout_seconds: None,
            heartbeat_interval_seconds: 30,
            plan: None,
            required_capabilities: vec![],
            routing_mode: None,
            on_timeout: None,
            on_heartbeat_miss: None,
        }
    }

    fn make_tmux_session(name: &str) -> TmuxSession {
        TmuxSession {
            session_name: name.to_string(),
            socket_path: std::path::PathBuf::from(format!("/tmp/mxdx-tmux/mxdx-{name}")),
            exit_notify_path: std::path::PathBuf::from(format!(
                "/tmp/mxdx-tmux/mxdx-{name}.notify"
            )),
        }
    }

    #[test]
    fn state_machine_claimed_to_running_to_completed() {
        let mut mgr = SessionManager::new("@worker:example.com".to_string());
        let task = make_task("s-1");
        mgr.claim(task).unwrap();

        assert_eq!(mgr.get("s-1").unwrap().state, SessionState::Claimed);

        let tmux = make_tmux_session("s-1");
        mgr.mark_running("s-1", Some(1234), tmux).unwrap();
        assert_eq!(mgr.get("s-1").unwrap().state, SessionState::Running);

        let completed = mgr
            .complete("s-1", SessionStatus::Success, Some(0))
            .unwrap();
        assert_eq!(mgr.get("s-1").unwrap().state, SessionState::Completed);
        assert_eq!(completed.exit_code, Some(0));
    }

    #[test]
    fn claim_creates_correct_active_session_state() {
        let mut mgr = SessionManager::new("@worker:example.com".to_string());
        let mut task = make_task("s-2");
        task.interactive = true;
        task.bin = "bash".to_string();
        task.args = vec!["-c".to_string(), "ls".to_string()];

        let active = mgr.claim(task).unwrap();
        assert_eq!(active.bin, "bash");
        assert_eq!(active.args, vec!["-c", "ls"]);
        assert!(active.pid.is_none());
        assert!(active.interactive);
        assert_eq!(active.client_id, "@alice:example.com");
        assert_eq!(active.worker_id, "@worker:example.com");
        assert!(active.start_time > 0);
    }

    #[test]
    fn cannot_claim_duplicate_uuid() {
        let mut mgr = SessionManager::new("@worker:example.com".to_string());
        mgr.claim(make_task("dup-1")).unwrap();
        let err = mgr.claim(make_task("dup-1")).unwrap_err();
        assert!(
            err.to_string().contains("already exists"),
            "expected 'already exists' error, got: {err}"
        );
    }

    #[test]
    fn cannot_mark_running_on_non_claimed_session() {
        let mut mgr = SessionManager::new("@worker:example.com".to_string());
        mgr.claim(make_task("s-3")).unwrap();

        let tmux = make_tmux_session("s-3");
        mgr.mark_running("s-3", Some(100), tmux).unwrap();

        // Now in Running state, trying mark_running again should fail
        let tmux2 = make_tmux_session("s-3-2");
        let err = mgr.mark_running("s-3", Some(200), tmux2).unwrap_err();
        assert!(
            err.to_string().contains("not in Claimed state"),
            "expected state error, got: {err}"
        );
    }

    #[test]
    fn cannot_complete_already_completed_session() {
        let mut mgr = SessionManager::new("@worker:example.com".to_string());
        mgr.claim(make_task("s-4")).unwrap();
        let tmux = make_tmux_session("s-4");
        mgr.mark_running("s-4", Some(100), tmux).unwrap();
        mgr.complete("s-4", SessionStatus::Success, Some(0))
            .unwrap();

        let err = mgr
            .complete("s-4", SessionStatus::Failed, Some(1))
            .unwrap_err();
        assert!(
            err.to_string().contains("already completed"),
            "expected 'already completed' error, got: {err}"
        );
    }

    #[test]
    fn active_sessions_filters_out_completed() {
        let mut mgr = SessionManager::new("@worker:example.com".to_string());
        mgr.claim(make_task("a-1")).unwrap();
        mgr.claim(make_task("a-2")).unwrap();
        mgr.claim(make_task("a-3")).unwrap();

        // Complete one
        let tmux = make_tmux_session("a-2");
        mgr.mark_running("a-2", Some(100), tmux).unwrap();
        mgr.complete("a-2", SessionStatus::Success, Some(0))
            .unwrap();

        let active = mgr.active_sessions();
        assert_eq!(active.len(), 2);
        let uuids: Vec<&str> = active.iter().map(|s| s.uuid.as_str()).collect();
        assert!(uuids.contains(&"a-1"));
        assert!(uuids.contains(&"a-3"));
        assert!(!uuids.contains(&"a-2"));
    }

    #[test]
    fn get_returns_correct_session() {
        let mut mgr = SessionManager::new("@worker:example.com".to_string());
        mgr.claim(make_task("g-1")).unwrap();
        mgr.claim(make_task("g-2")).unwrap();

        let s = mgr.get("g-1").unwrap();
        assert_eq!(s.uuid, "g-1");
        assert_eq!(s.task.bin, "echo");

        assert!(mgr.get("nonexistent").is_none());
    }

    #[test]
    fn complete_returns_correct_completed_state_with_duration() {
        let mut mgr = SessionManager::new("@worker:example.com".to_string());
        mgr.claim(make_task("d-1")).unwrap();
        let tmux = make_tmux_session("d-1");
        mgr.mark_running("d-1", Some(100), tmux).unwrap();

        let completed = mgr.complete("d-1", SessionStatus::Failed, Some(1)).unwrap();

        // Duration should be >= 0 (essentially instant in tests)
        assert!(completed.completion_time > 0);
        assert_eq!(completed.exit_code, Some(1));
        // duration_seconds should be 0 or very small since claim and complete happen instantly
        assert!(completed.duration_seconds <= 1);
    }
}
