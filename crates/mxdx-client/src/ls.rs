use mxdx_types::events::session::{ActiveSessionState, CompletedSessionState};

/// A session entry for display in the "ls" command
#[derive(Debug, Clone)]
pub struct SessionEntry {
    pub uuid: String,
    pub bin: String,
    pub args: Vec<String>,
    pub worker_id: String,
    pub interactive: bool,
    pub status: SessionEntryStatus,
}

#[derive(Debug, Clone)]
pub enum SessionEntryStatus {
    Active { pid: Option<u32>, start_time: u64 },
    Completed { exit_code: Option<i32>, duration_seconds: u64 },
}

/// Build a SessionEntry from an active state event
pub fn from_active(uuid: String, state: &ActiveSessionState) -> SessionEntry {
    SessionEntry {
        uuid,
        bin: state.bin.clone(),
        args: state.args.clone(),
        worker_id: state.worker_id.clone(),
        interactive: state.interactive,
        status: SessionEntryStatus::Active {
            pid: state.pid,
            start_time: state.start_time,
        },
    }
}

/// Build a SessionEntry from a completed state event (needs the original active state too for bin/args)
pub fn from_completed(uuid: String, active: &ActiveSessionState, completed: &CompletedSessionState) -> SessionEntry {
    SessionEntry {
        uuid,
        bin: active.bin.clone(),
        args: active.args.clone(),
        worker_id: active.worker_id.clone(),
        interactive: active.interactive,
        status: SessionEntryStatus::Completed {
            exit_code: completed.exit_code,
            duration_seconds: completed.duration_seconds,
        },
    }
}

/// Format session entries as a table for display
pub fn format_table(entries: &[SessionEntry]) -> String {
    if entries.is_empty() {
        return "No sessions found.".to_string();
    }
    let mut lines = vec![format!(
        "{:<38} {:<15} {:<10} {:<10} {}",
        "UUID", "COMMAND", "WORKER", "STATUS", "DETAILS"
    )];
    for entry in entries {
        let cmd = if entry.args.is_empty() {
            entry.bin.clone()
        } else {
            format!("{} {}", entry.bin, entry.args.join(" "))
        };
        let cmd_display = if cmd.len() > 13 {
            format!("{}...", &cmd[..12])
        } else {
            cmd
        };
        let worker_display = if entry.worker_id.len() > 8 {
            format!("{}...", &entry.worker_id[..7])
        } else {
            entry.worker_id.clone()
        };
        let (status, details) = match &entry.status {
            SessionEntryStatus::Active { pid, .. } => {
                ("active".to_string(), format!("pid={:?}", pid))
            }
            SessionEntryStatus::Completed {
                exit_code,
                duration_seconds,
            } => (
                "done".to_string(),
                format!("exit={:?} {}s", exit_code, duration_seconds),
            ),
        };
        lines.push(format!(
            "{:<38} {:<15} {:<10} {:<10} {}",
            entry.uuid, cmd_display, worker_display, status, details
        ));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_active_state() -> ActiveSessionState {
        ActiveSessionState {
            bin: "echo".into(),
            args: vec!["hello".into()],
            pid: Some(1234),
            start_time: 1742572800,
            client_id: "@alice:example.com".into(),
            interactive: false,
            worker_id: "@worker:example.com".into(),
        }
    }

    fn make_completed_state() -> CompletedSessionState {
        CompletedSessionState {
            exit_code: Some(0),
            duration_seconds: 120,
            completion_time: 1742572920,
        }
    }

    #[test]
    fn from_active_builds_entry() {
        let state = make_active_state();
        let entry = from_active("uuid-1".into(), &state);
        assert_eq!(entry.uuid, "uuid-1");
        assert_eq!(entry.bin, "echo");
        assert_eq!(entry.args, vec!["hello"]);
        assert_eq!(entry.worker_id, "@worker:example.com");
        assert!(!entry.interactive);
        match &entry.status {
            SessionEntryStatus::Active { pid, start_time } => {
                assert_eq!(*pid, Some(1234));
                assert_eq!(*start_time, 1742572800);
            }
            _ => panic!("expected Active status"),
        }
    }

    #[test]
    fn from_completed_builds_entry() {
        let active = make_active_state();
        let completed = make_completed_state();
        let entry = from_completed("uuid-2".into(), &active, &completed);
        assert_eq!(entry.uuid, "uuid-2");
        assert_eq!(entry.bin, "echo");
        match &entry.status {
            SessionEntryStatus::Completed {
                exit_code,
                duration_seconds,
            } => {
                assert_eq!(*exit_code, Some(0));
                assert_eq!(*duration_seconds, 120);
            }
            _ => panic!("expected Completed status"),
        }
    }

    #[test]
    fn format_table_with_entries() {
        let entries = vec![
            from_active("uuid-1".into(), &make_active_state()),
            from_completed("uuid-2".into(), &make_active_state(), &make_completed_state()),
        ];
        let table = format_table(&entries);
        assert!(table.contains("UUID"));
        assert!(table.contains("COMMAND"));
        assert!(table.contains("uuid-1"));
        assert!(table.contains("uuid-2"));
        assert!(table.contains("active"));
        assert!(table.contains("done"));
    }

    #[test]
    fn format_table_empty() {
        let table = format_table(&[]);
        assert_eq!(table, "No sessions found.");
    }

    #[test]
    fn format_table_truncates_long_command() {
        let mut state = make_active_state();
        state.bin = "very-long-command-name".into();
        state.args = vec!["--with-many-args".into()];
        let entries = vec![from_active("uuid-3".into(), &state)];
        let table = format_table(&entries);
        assert!(table.contains("..."));
    }

    #[test]
    fn format_table_truncates_long_worker_id() {
        let state = make_active_state();
        let entries = vec![from_active("uuid-4".into(), &state)];
        let table = format_table(&entries);
        // worker_id "@worker:example.com" is > 8 chars, should be truncated
        assert!(table.contains("..."));
    }
}
