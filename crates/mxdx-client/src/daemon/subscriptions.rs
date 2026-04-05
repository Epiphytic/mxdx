use std::collections::HashMap;
use crate::protocol::methods::EventFilter;

pub struct SubscriptionRegistry {
    subscriptions: HashMap<String, Subscription>,
    next_id: u64,
}

struct Subscription {
    event_patterns: Vec<String>,
    filter: Option<EventFilter>,
    sink: tokio::sync::mpsc::UnboundedSender<String>,
}

impl SubscriptionRegistry {
    pub fn new() -> Self {
        Self {
            subscriptions: HashMap::new(),
            next_id: 0,
        }
    }

    pub fn subscribe(
        &mut self,
        event_patterns: Vec<String>,
        filter: Option<EventFilter>,
        sink: tokio::sync::mpsc::UnboundedSender<String>,
    ) -> String {
        self.next_id += 1;
        let id = format!("sub-{:04}", self.next_id);
        self.subscriptions.insert(id.clone(), Subscription {
            event_patterns,
            filter,
            sink,
        });
        id
    }

    pub fn unsubscribe(&mut self, id: &str) -> bool {
        self.subscriptions.remove(id).is_some()
    }

    pub fn dispatch(&self, event_type: &str, event_json: &str) {
        for sub in self.subscriptions.values() {
            if self.matches_patterns(event_type, &sub.event_patterns) {
                let _ = sub.sink.send(event_json.to_string());
            }
        }
    }

    fn matches_patterns(&self, event_type: &str, patterns: &[String]) -> bool {
        patterns.iter().any(|p| {
            if p.ends_with(".*") {
                let prefix = &p[..p.len() - 2];
                event_type.starts_with(prefix)
            } else {
                p == event_type
            }
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn subscribe_and_dispatch() {
        let mut registry = SubscriptionRegistry::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let id = registry.subscribe(vec!["session.*".into()], None, tx);
        assert!(id.starts_with("sub-"));
        registry.dispatch("session.output", r#"{"data":"test"}"#);
        assert_eq!(rx.try_recv().unwrap(), r#"{"data":"test"}"#);
        registry.dispatch("daemon.status", r#"{}"#);
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn exact_pattern_match() {
        let mut registry = SubscriptionRegistry::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        registry.subscribe(vec!["session.result".into()], None, tx);
        registry.dispatch("session.output", r#"{"data":"test"}"#);
        assert!(rx.try_recv().is_err());
        registry.dispatch("session.result", r#"{"exit_code":0}"#);
        assert!(rx.try_recv().is_ok());
    }

    #[test]
    fn unsubscribe_stops_delivery() {
        let mut registry = SubscriptionRegistry::new();
        let (tx, mut rx) = tokio::sync::mpsc::unbounded_channel();
        let id = registry.subscribe(vec!["session.*".into()], None, tx);
        registry.dispatch("session.output", r#"{"test":1}"#);
        assert!(rx.try_recv().is_ok());
        assert!(registry.unsubscribe(&id));
        registry.dispatch("session.output", r#"{"test":2}"#);
        assert!(rx.try_recv().is_err());
    }
}
