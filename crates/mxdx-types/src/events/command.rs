#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn command_event_round_trips_json() {
        let cmd = CommandEvent {
            uuid: "550e8400-e29b-41d4-a716-446655440000".into(),
            action: CommandAction::Exec,
            cmd: "cargo build --release".into(),
            args: vec!["--features".into(), "gpu".into()],
            env: [("RUST_LOG".into(), "info".into())].into(),
            cwd: Some("/workspace".into()),
            timeout_seconds: Some(3600),
        };
        let json = serde_json::to_string(&cmd).unwrap();
        let parsed: CommandEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.uuid, cmd.uuid);
        assert_eq!(parsed.action, CommandAction::Exec);
        assert_eq!(parsed.args, cmd.args);
    }

    #[test]
    fn command_event_rejects_unknown_action() {
        let json = r#"{"uuid":"x","action":"fly_to_moon","cmd":"x","args":[],"env":{}}"#;
        let result: Result<CommandEvent, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }
}
