use mxdx_types::events::session::SessionTask;

/// Build a SessionTask from user inputs
pub fn build_task(
    bin: &str,
    args: &[String],
    interactive: bool,
    no_room_output: bool,
    timeout_seconds: Option<u64>,
    heartbeat_interval_seconds: u64,
    sender_id: &str,
    cwd: Option<&str>,
) -> SessionTask {
    SessionTask {
        uuid: uuid::Uuid::new_v4().to_string(),
        sender_id: sender_id.to_string(),
        bin: bin.to_string(),
        args: args.to_vec(),
        env: None,
        cwd: cwd.map(|s| s.to_string()),
        interactive,
        no_room_output,
        timeout_seconds,
        heartbeat_interval_seconds,
        plan: None,
        required_capabilities: vec![],
        routing_mode: None,
        on_timeout: None,
        on_heartbeat_miss: None,
    }
}

/// Submit result -- the UUID and thread root event ID
pub struct SubmitResult {
    pub session_uuid: String,
    pub event_id: String,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn build_task_produces_correct_fields() {
        let args = vec!["hello".to_string(), "world".to_string()];
        let task = build_task(
            "echo",
            &args,
            false,
            true,
            Some(60),
            15,
            "@alice:example.com",
            None,
        );
        assert_eq!(task.bin, "echo");
        assert_eq!(task.args, vec!["hello", "world"]);
        assert!(!task.interactive);
        assert!(task.no_room_output);
        assert_eq!(task.timeout_seconds, Some(60));
        assert_eq!(task.heartbeat_interval_seconds, 15);
        assert_eq!(task.sender_id, "@alice:example.com");
        assert!(!task.uuid.is_empty());
    }

    #[test]
    fn build_task_uuid_is_unique() {
        let t1 = build_task("echo", &[], false, false, None, 30, "@a:b", None);
        let t2 = build_task("echo", &[], false, false, None, 30, "@a:b", None);
        assert_ne!(t1.uuid, t2.uuid);
    }

    #[test]
    fn build_task_interactive_flag_propagates() {
        let task = build_task("bash", &[], true, false, None, 30, "@u:h", None);
        assert!(task.interactive);

        let task2 = build_task("bash", &[], false, false, None, 30, "@u:h", None);
        assert!(!task2.interactive);
    }

    #[test]
    fn build_task_defaults_are_none() {
        let task = build_task("ls", &[], false, false, None, 30, "@u:h", None);
        assert!(task.env.is_none());
        assert!(task.cwd.is_none());
        assert!(task.plan.is_none());
        assert!(task.required_capabilities.is_empty());
        assert!(task.routing_mode.is_none());
        assert!(task.on_timeout.is_none());
        assert!(task.on_heartbeat_miss.is_none());
    }
}
