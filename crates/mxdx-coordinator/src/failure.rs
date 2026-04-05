use mxdx_types::events::fabric::FailurePolicy;
use mxdx_types::events::session::SessionTask;

/// What to do when a session fails
#[derive(Debug, Clone, PartialEq)]
pub enum FailureAction {
    /// Escalate to human/coordinator operator
    Escalate {
        session_uuid: String,
        reason: String,
    },
    /// Respawn the task on another worker
    Respawn {
        task: SessionTask,
        attempt: u8,
        max_retries: u8,
    },
    /// Respawn with context from previous run (uses plan field)
    RespawnWithContext {
        task: SessionTask,
        context: Option<String>,
    },
    /// Abandon — do nothing
    Abandon,
}

/// Determine action based on failure policy
pub fn apply_policy(
    policy: &FailurePolicy,
    task: &SessionTask,
    failure_reason: &str,
    attempt: u8,
) -> FailureAction {
    match policy {
        FailurePolicy::Escalate => FailureAction::Escalate {
            session_uuid: task.uuid.clone(),
            reason: failure_reason.to_string(),
        },
        FailurePolicy::Respawn { max_retries } => {
            if attempt >= *max_retries {
                FailureAction::Escalate {
                    session_uuid: task.uuid.clone(),
                    reason: format!(
                        "max retries ({}) exceeded: {}",
                        max_retries, failure_reason
                    ),
                }
            } else {
                FailureAction::Respawn {
                    task: task.clone(),
                    attempt: attempt + 1,
                    max_retries: *max_retries,
                }
            }
        }
        FailurePolicy::RespawnWithContext => FailureAction::RespawnWithContext {
            task: task.clone(),
            context: task.plan.clone(),
        },
        FailurePolicy::Abandon => FailureAction::Abandon,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn make_task() -> SessionTask {
        SessionTask {
            uuid: "task-1".into(),
            sender_id: "@alice:example.com".into(),
            bin: "echo".into(),
            args: vec!["hello".into()],
            env: None,
            cwd: None,
            interactive: false,
            no_room_output: false,
            timeout_seconds: None,
            heartbeat_interval_seconds: 30,
            plan: Some("run tests".into()),
            required_capabilities: vec![],
            routing_mode: None,
            on_timeout: None,
            on_heartbeat_miss: None,
        }
    }

    #[test]
    fn escalate_policy_returns_escalate_action() {
        let task = make_task();
        let action = apply_policy(&FailurePolicy::Escalate, &task, "crashed", 0);
        assert_eq!(
            action,
            FailureAction::Escalate {
                session_uuid: "task-1".into(),
                reason: "crashed".into(),
            }
        );
    }

    #[test]
    fn respawn_policy_returns_respawn_when_under_limit() {
        let task = make_task();
        let action = apply_policy(
            &FailurePolicy::Respawn { max_retries: 3 },
            &task,
            "crashed",
            1,
        );
        match action {
            FailureAction::Respawn {
                attempt,
                max_retries,
                ..
            } => {
                assert_eq!(attempt, 2);
                assert_eq!(max_retries, 3);
            }
            _ => panic!("expected Respawn action"),
        }
    }

    #[test]
    fn respawn_policy_escalates_when_max_retries_exceeded() {
        let task = make_task();
        let action = apply_policy(
            &FailurePolicy::Respawn { max_retries: 3 },
            &task,
            "crashed",
            3,
        );
        match action {
            FailureAction::Escalate { reason, .. } => {
                assert!(reason.contains("max retries (3) exceeded"));
                assert!(reason.contains("crashed"));
            }
            _ => panic!("expected Escalate action after max retries"),
        }
    }

    #[test]
    fn respawn_with_context_includes_plan() {
        let task = make_task();
        let action = apply_policy(&FailurePolicy::RespawnWithContext, &task, "crashed", 0);
        match action {
            FailureAction::RespawnWithContext { context, .. } => {
                assert_eq!(context, Some("run tests".into()));
            }
            _ => panic!("expected RespawnWithContext action"),
        }
    }

    #[test]
    fn respawn_with_context_none_when_no_plan() {
        let mut task = make_task();
        task.plan = None;
        let action = apply_policy(&FailurePolicy::RespawnWithContext, &task, "crashed", 0);
        match action {
            FailureAction::RespawnWithContext { context, .. } => {
                assert_eq!(context, None);
            }
            _ => panic!("expected RespawnWithContext action"),
        }
    }

    #[test]
    fn abandon_policy_returns_abandon() {
        let task = make_task();
        let action = apply_policy(&FailurePolicy::Abandon, &task, "crashed", 0);
        assert_eq!(action, FailureAction::Abandon);
    }
}
