use mxdx_types::events::worker_info::WorkerInfo;
use std::collections::HashMap;

/// Index of worker capabilities for fast lookup
pub struct CapabilityIndex {
    /// capability_name -> Vec<worker_id>
    by_capability: HashMap<String, Vec<String>>,
    /// worker_id -> WorkerInfo
    workers: HashMap<String, WorkerInfo>,
}

impl CapabilityIndex {
    pub fn new() -> Self {
        Self {
            by_capability: HashMap::new(),
            workers: HashMap::new(),
        }
    }

    /// Add or update a worker's capabilities
    pub fn update(&mut self, info: WorkerInfo) {
        let worker_id = info.worker_id.clone();
        // Remove old entries
        self.remove(&worker_id);
        // Add capability entries
        for cap in &info.capabilities {
            self.by_capability
                .entry(cap.clone())
                .or_default()
                .push(worker_id.clone());
        }
        for tool in &info.tools {
            if tool.healthy {
                self.by_capability
                    .entry(tool.name.clone())
                    .or_default()
                    .push(worker_id.clone());
            }
        }
        self.workers.insert(worker_id, info);
    }

    /// Remove a worker
    pub fn remove(&mut self, worker_id: &str) {
        self.workers.remove(worker_id);
        for workers in self.by_capability.values_mut() {
            workers.retain(|id| id != worker_id);
        }
        self.by_capability.retain(|_, workers| !workers.is_empty());
    }

    /// Find workers with a specific capability
    pub fn workers_with_capability(&self, capability: &str) -> Vec<&str> {
        self.by_capability
            .get(capability)
            .map(|ids| ids.iter().map(|s| s.as_str()).collect())
            .unwrap_or_default()
    }

    /// Find workers with ALL of the given capabilities
    pub fn workers_with_all(&self, capabilities: &[String]) -> Vec<&str> {
        if capabilities.is_empty() {
            return self.workers.keys().map(|s| s.as_str()).collect();
        }
        let mut result: Option<Vec<&str>> = None;
        for cap in capabilities {
            let workers = self.workers_with_capability(cap);
            result = Some(match result {
                None => workers,
                Some(prev) => prev.into_iter().filter(|id| workers.contains(id)).collect(),
            });
        }
        result.unwrap_or_default()
    }

    /// Get worker info
    pub fn get_worker(&self, worker_id: &str) -> Option<&WorkerInfo> {
        self.workers.get(worker_id)
    }

    /// Number of workers in the index
    pub fn worker_count(&self) -> usize {
        self.workers.len()
    }
}

impl Default for CapabilityIndex {
    fn default() -> Self {
        Self::new()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use mxdx_types::events::capability::{InputSchema, WorkerTool};
    use std::collections::HashMap as StdHashMap;

    fn make_worker(id: &str, capabilities: Vec<&str>, tools: Vec<WorkerTool>) -> WorkerInfo {
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

    fn make_tool(name: &str, healthy: bool) -> WorkerTool {
        WorkerTool {
            name: name.into(),
            version: Some("1.0".into()),
            description: format!("{name} tool"),
            healthy,
            input_schema: InputSchema {
                r#type: "object".into(),
                properties: StdHashMap::new(),
                required: vec![],
            },
        }
    }

    #[test]
    fn update_and_lookup_by_capability() {
        let mut index = CapabilityIndex::new();
        index.update(make_worker("w-1", vec!["linux", "gpu"], vec![]));
        index.update(make_worker("w-2", vec!["linux"], vec![]));

        let linux_workers = index.workers_with_capability("linux");
        assert_eq!(linux_workers.len(), 2);
        assert!(linux_workers.contains(&"w-1"));
        assert!(linux_workers.contains(&"w-2"));

        let gpu_workers = index.workers_with_capability("gpu");
        assert_eq!(gpu_workers.len(), 1);
        assert!(gpu_workers.contains(&"w-1"));
    }

    #[test]
    fn remove_worker_cleans_up_capabilities() {
        let mut index = CapabilityIndex::new();
        index.update(make_worker("w-1", vec!["linux", "gpu"], vec![]));
        index.update(make_worker("w-2", vec!["linux"], vec![]));

        index.remove("w-1");
        assert_eq!(index.worker_count(), 1);

        let linux_workers = index.workers_with_capability("linux");
        assert_eq!(linux_workers.len(), 1);
        assert!(linux_workers.contains(&"w-2"));

        let gpu_workers = index.workers_with_capability("gpu");
        assert!(gpu_workers.is_empty());
    }

    #[test]
    fn workers_with_all_intersection() {
        let mut index = CapabilityIndex::new();
        index.update(make_worker("w-1", vec!["linux", "gpu", "docker"], vec![]));
        index.update(make_worker("w-2", vec!["linux", "gpu"], vec![]));
        index.update(make_worker("w-3", vec!["linux"], vec![]));

        let result = index.workers_with_all(&["linux".into(), "gpu".into()]);
        assert_eq!(result.len(), 2);
        assert!(result.contains(&"w-1"));
        assert!(result.contains(&"w-2"));

        let result = index.workers_with_all(&["linux".into(), "gpu".into(), "docker".into()]);
        assert_eq!(result.len(), 1);
        assert!(result.contains(&"w-1"));
    }

    #[test]
    fn workers_with_all_empty_capabilities_returns_all() {
        let mut index = CapabilityIndex::new();
        index.update(make_worker("w-1", vec!["linux"], vec![]));
        index.update(make_worker("w-2", vec!["macos"], vec![]));

        let result = index.workers_with_all(&[]);
        assert_eq!(result.len(), 2);
    }

    #[test]
    fn healthy_tools_indexed_unhealthy_excluded() {
        let mut index = CapabilityIndex::new();
        let healthy_tool = make_tool("docker", true);
        let unhealthy_tool = make_tool("kubectl", false);
        index.update(make_worker(
            "w-1",
            vec!["linux"],
            vec![healthy_tool, unhealthy_tool],
        ));

        let docker_workers = index.workers_with_capability("docker");
        assert_eq!(docker_workers.len(), 1);

        let kubectl_workers = index.workers_with_capability("kubectl");
        assert!(kubectl_workers.is_empty());
    }

    #[test]
    fn update_replaces_existing_worker() {
        let mut index = CapabilityIndex::new();
        index.update(make_worker("w-1", vec!["linux", "gpu"], vec![]));
        assert_eq!(index.workers_with_capability("gpu").len(), 1);

        // Update w-1 without gpu
        index.update(make_worker("w-1", vec!["linux"], vec![]));
        assert_eq!(index.worker_count(), 1);
        assert!(index.workers_with_capability("gpu").is_empty());
        assert_eq!(index.workers_with_capability("linux").len(), 1);
    }

    #[test]
    fn get_worker_info() {
        let mut index = CapabilityIndex::new();
        index.update(make_worker("w-1", vec!["linux"], vec![]));

        let info = index.get_worker("w-1").unwrap();
        assert_eq!(info.worker_id, "w-1");
        assert_eq!(info.os, "linux");

        assert!(index.get_worker("w-nonexistent").is_none());
    }

    #[test]
    fn no_match_returns_empty() {
        let mut index = CapabilityIndex::new();
        index.update(make_worker("w-1", vec!["linux"], vec![]));

        assert!(index.workers_with_capability("windows").is_empty());
        assert!(index
            .workers_with_all(&["linux".into(), "windows".into()])
            .is_empty());
    }
}
