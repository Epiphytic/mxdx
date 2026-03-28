use mxdx_types::events::session::ActiveSessionState;

/// Check for sessions this client previously started
pub fn find_reconnectable_sessions<'a>(
    active_sessions: &'a [(String, ActiveSessionState)],
    client_id: &str,
) -> Vec<(String, &'a ActiveSessionState)> {
    active_sessions
        .iter()
        .filter(|(_, state)| state.client_id == client_id)
        .map(|(uuid, state)| (uuid.clone(), state))
        .collect()
}

/// Format reconnectable sessions for user display
pub fn format_reconnectable(sessions: &[(String, &ActiveSessionState)]) -> String {
    if sessions.is_empty() {
        return "No active sessions to reconnect to.".to_string();
    }
    let mut lines = vec!["Active sessions from this client:".to_string()];
    for (uuid, state) in sessions {
        let cmd = if state.args.is_empty() {
            state.bin.clone()
        } else {
            format!("{} {}", state.bin, state.args.join(" "))
        };
        lines.push(format!("  {} — {} (worker: {})", uuid, cmd, state.worker_id));
    }
    lines.join("\n")
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_active(client_id: &str, bin: &str, worker_id: &str) -> ActiveSessionState {
        ActiveSessionState {
            bin: bin.into(),
            args: vec![],
            pid: Some(1234),
            start_time: 1742572800,
            client_id: client_id.into(),
            interactive: false,
            worker_id: worker_id.into(),
        }
    }

    #[test]
    fn find_reconnectable_filters_by_client_id() {
        let sessions = vec![
            ("uuid-1".into(), make_active("@alice:h", "echo", "@w1:h")),
            ("uuid-2".into(), make_active("@bob:h", "ls", "@w2:h")),
            ("uuid-3".into(), make_active("@alice:h", "cat", "@w3:h")),
        ];
        let result = find_reconnectable_sessions(&sessions, "@alice:h");
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].0, "uuid-1");
        assert_eq!(result[1].0, "uuid-3");
    }

    #[test]
    fn find_reconnectable_returns_empty_when_no_match() {
        let sessions = vec![
            ("uuid-1".into(), make_active("@bob:h", "echo", "@w1:h")),
        ];
        let result = find_reconnectable_sessions(&sessions, "@alice:h");
        assert!(result.is_empty());
    }

    #[test]
    fn format_reconnectable_empty() {
        let sessions: Vec<(String, &ActiveSessionState)> = vec![];
        let output = format_reconnectable(&sessions);
        assert_eq!(output, "No active sessions to reconnect to.");
    }

    #[test]
    fn format_reconnectable_with_sessions() {
        let state1 = make_active("@alice:h", "echo", "@w1:h");
        let state2 = make_active("@alice:h", "ls", "@w2:h");
        let sessions = vec![
            ("uuid-1".into(), &state1),
            ("uuid-2".into(), &state2),
        ];
        let output = format_reconnectable(&sessions);
        assert!(output.contains("Active sessions from this client:"));
        assert!(output.contains("uuid-1"));
        assert!(output.contains("uuid-2"));
        assert!(output.contains("echo"));
        assert!(output.contains("ls"));
        assert!(output.contains("@w1:h"));
        assert!(output.contains("@w2:h"));
    }

    #[test]
    fn format_reconnectable_shows_args() {
        let mut state = make_active("@alice:h", "echo", "@w1:h");
        state.args = vec!["hello".into(), "world".into()];
        let sessions = vec![("uuid-1".into(), &state)];
        let output = format_reconnectable(&sessions);
        assert!(output.contains("echo hello world"));
    }
}
