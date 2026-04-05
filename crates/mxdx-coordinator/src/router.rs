use mxdx_types::events::session::SessionTask;
use mxdx_types::events::worker_info::WorkerInfo;

/// A known worker and its capabilities.
#[derive(Debug, Clone)]
pub struct WorkerEntry {
    pub room_id: String,
    pub info: WorkerInfo,
}

/// Router decides which worker should handle a task based on required capabilities.
pub struct Router {
    workers: Vec<WorkerEntry>,
}

impl Router {
    pub fn new() -> Self {
        Self { workers: vec![] }
    }

    /// Register or update a worker's capabilities.
    pub fn update_worker(&mut self, room_id: String, info: WorkerInfo) {
        if let Some(entry) = self
            .workers
            .iter_mut()
            .find(|w| w.info.worker_id == info.worker_id)
        {
            entry.info = info;
            entry.room_id = room_id;
        } else {
            self.workers.push(WorkerEntry { room_id, info });
        }
    }

    /// Remove a worker by its worker_id.
    pub fn remove_worker(&mut self, worker_id: &str) {
        self.workers.retain(|w| w.info.worker_id != worker_id);
    }

    /// Route a task to the best worker based on required capabilities.
    ///
    /// Returns `None` if no worker matches. When a task has no required capabilities,
    /// any available worker is acceptable.
    pub fn route(&self, task: &SessionTask) -> Option<&WorkerEntry> {
        if task.required_capabilities.is_empty() {
            // No requirements — pick first available worker
            return self.workers.first();
        }

        // Find workers that have ALL required capabilities
        let candidates: Vec<_> = self
            .workers
            .iter()
            .filter(|w| {
                task.required_capabilities.iter().all(|req| {
                    w.info.capabilities.contains(req)
                        || w.info.tools.iter().any(|t| t.name == *req && t.healthy)
                })
            })
            .collect();

        // Return best candidate (first match for now — could add scoring later)
        candidates.first().copied()
    }

    /// List all registered workers.
    pub fn workers(&self) -> &[WorkerEntry] {
        &self.workers
    }
}

impl Default for Router {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxdx_types::events::capability::{InputSchema, WorkerTool};
    use std::collections::HashMap;

    fn make_worker_info(id: &str, capabilities: Vec<&str>, tools: Vec<WorkerTool>) -> WorkerInfo {
        WorkerInfo {
            worker_id: id.into(),
            host: "test-host".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            cpu_count: 4,
            memory_total_mb: 8192,
            disk_available_mb: 50000,
            tools,
            capabilities: capabilities.into_iter().map(String::from).collect(),
            updated_at: 1742572800,
        }
    }

    fn make_task(required_capabilities: Vec<&str>) -> SessionTask {
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
            plan: None,
            required_capabilities: required_capabilities
                .into_iter()
                .map(String::from)
                .collect(),
            routing_mode: None,
            on_timeout: None,
            on_heartbeat_miss: None,
        }
    }

    fn make_tool(name: &str, healthy: bool) -> WorkerTool {
        WorkerTool {
            name: name.into(),
            version: Some("1.0".into()),
            description: format!("{name} tool"),
            healthy,
            input_schema: InputSchema {
                r#type: "object".into(),
                properties: HashMap::new(),
                required: vec![],
            },
        }
    }

    #[test]
    fn route_with_no_requirements_returns_first_worker() {
        let mut router = Router::new();
        router.update_worker(
            "!room1:example.com".into(),
            make_worker_info("worker-1", vec!["linux"], vec![]),
        );
        router.update_worker(
            "!room2:example.com".into(),
            make_worker_info("worker-2", vec!["macos"], vec![]),
        );

        let task = make_task(vec![]);
        let result = router.route(&task);
        assert!(result.is_some());
        assert_eq!(result.unwrap().info.worker_id, "worker-1");
    }

    #[test]
    fn route_with_capability_match() {
        let mut router = Router::new();
        router.update_worker(
            "!room1:example.com".into(),
            make_worker_info("worker-1", vec!["linux"], vec![]),
        );
        router.update_worker(
            "!room2:example.com".into(),
            make_worker_info("worker-2", vec!["linux", "gpu"], vec![]),
        );

        let task = make_task(vec!["gpu"]);
        let result = router.route(&task);
        assert!(result.is_some());
        assert_eq!(result.unwrap().info.worker_id, "worker-2");
    }

    #[test]
    fn route_with_tool_match() {
        let mut router = Router::new();
        let tool = make_tool("docker", true);
        router.update_worker(
            "!room1:example.com".into(),
            make_worker_info("worker-1", vec!["linux"], vec![tool]),
        );

        let task = make_task(vec!["docker"]);
        let result = router.route(&task);
        assert!(result.is_some());
        assert_eq!(result.unwrap().info.worker_id, "worker-1");
    }

    #[test]
    fn route_unhealthy_tool_does_not_match() {
        let mut router = Router::new();
        let tool = make_tool("docker", false);
        router.update_worker(
            "!room1:example.com".into(),
            make_worker_info("worker-1", vec!["linux"], vec![tool]),
        );

        let task = make_task(vec!["docker"]);
        let result = router.route(&task);
        assert!(result.is_none());
    }

    #[test]
    fn route_no_matching_worker() {
        let mut router = Router::new();
        router.update_worker(
            "!room1:example.com".into(),
            make_worker_info("worker-1", vec!["linux"], vec![]),
        );

        let task = make_task(vec!["gpu", "arm64"]);
        let result = router.route(&task);
        assert!(result.is_none());
    }

    #[test]
    fn route_empty_router_returns_none() {
        let router = Router::new();
        let task = make_task(vec!["linux"]);
        let result = router.route(&task);
        assert!(result.is_none());
    }

    #[test]
    fn route_empty_router_no_requirements_returns_none() {
        let router = Router::new();
        let task = make_task(vec![]);
        let result = router.route(&task);
        assert!(result.is_none());
    }

    #[test]
    fn update_worker_replaces_existing() {
        let mut router = Router::new();
        router.update_worker(
            "!room1:example.com".into(),
            make_worker_info("worker-1", vec!["linux"], vec![]),
        );
        assert_eq!(router.workers().len(), 1);
        assert_eq!(router.workers()[0].info.capabilities, vec!["linux"]);

        // Update same worker with new capabilities
        router.update_worker(
            "!room2:example.com".into(),
            make_worker_info("worker-1", vec!["linux", "gpu"], vec![]),
        );
        assert_eq!(router.workers().len(), 1);
        assert_eq!(
            router.workers()[0].info.capabilities,
            vec!["linux", "gpu"]
        );
        assert_eq!(router.workers()[0].room_id, "!room2:example.com");
    }

    #[test]
    fn remove_worker() {
        let mut router = Router::new();
        router.update_worker(
            "!room1:example.com".into(),
            make_worker_info("worker-1", vec!["linux"], vec![]),
        );
        router.update_worker(
            "!room2:example.com".into(),
            make_worker_info("worker-2", vec!["macos"], vec![]),
        );
        assert_eq!(router.workers().len(), 2);

        router.remove_worker("worker-1");
        assert_eq!(router.workers().len(), 1);
        assert_eq!(router.workers()[0].info.worker_id, "worker-2");
    }

    #[test]
    fn remove_nonexistent_worker_is_no_op() {
        let mut router = Router::new();
        router.update_worker(
            "!room1:example.com".into(),
            make_worker_info("worker-1", vec!["linux"], vec![]),
        );
        router.remove_worker("worker-999");
        assert_eq!(router.workers().len(), 1);
    }
}
