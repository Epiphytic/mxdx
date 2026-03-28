use anyhow::Result;
use mxdx_types::events::capability::{InputSchema, WorkerTool};
use mxdx_types::events::worker_info::WorkerInfo;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use sysinfo::{Disks, System};

pub struct TelemetryCollector {
    worker_id: String,
    refresh_seconds: u64,
    extra_capabilities: Vec<String>,
}

impl TelemetryCollector {
    pub fn new(worker_id: String, refresh_seconds: u64, extra_capabilities: Vec<String>) -> Self {
        Self {
            worker_id,
            refresh_seconds,
            extra_capabilities,
        }
    }

    /// Collect current system info and build a WorkerInfo event.
    pub fn collect_info(&self) -> Result<WorkerInfo> {
        let mut sys = System::new_all();
        sys.refresh_all();

        let host = hostname::get()?.to_string_lossy().to_string();
        let os = System::name().unwrap_or_else(|| "unknown".into());
        let arch = std::env::consts::ARCH.to_string();
        let cpu_count = sys.cpus().len() as u32;
        let memory_total_mb = sys.total_memory() / (1024 * 1024);

        let disks = Disks::new_with_refreshed_list();
        let disk_available_mb = disks
            .iter()
            .map(|d| d.available_space())
            .sum::<u64>()
            / (1024 * 1024);

        let tools = self.probe_tools();
        let timestamp = SystemTime::now().duration_since(UNIX_EPOCH)?.as_secs();

        Ok(WorkerInfo {
            worker_id: self.worker_id.clone(),
            host,
            os,
            arch,
            cpu_count,
            memory_total_mb,
            disk_available_mb,
            tools,
            capabilities: self.extra_capabilities.clone(),
            updated_at: timestamp,
        })
    }

    /// Probe for available tools by checking if common binaries exist on PATH.
    fn probe_tools(&self) -> Vec<WorkerTool> {
        let bins_to_check = ["bash", "python3", "node", "git", "docker", "tmux", "curl"];
        let mut tools = vec![];
        for bin in &bins_to_check {
            if let Ok(output) = std::process::Command::new("which").arg(bin).output() {
                if output.status.success() {
                    let version = self.probe_version(bin);
                    tools.push(WorkerTool {
                        name: bin.to_string(),
                        version,
                        description: format!("{bin} binary"),
                        healthy: true,
                        input_schema: InputSchema {
                            r#type: "object".into(),
                            properties: HashMap::new(),
                            required: vec![],
                        },
                    });
                }
            }
        }
        tools
    }

    fn probe_version(&self, bin: &str) -> Option<String> {
        let output = std::process::Command::new(bin)
            .arg("--version")
            .output()
            .ok()?;
        if output.status.success() {
            let stdout = String::from_utf8_lossy(&output.stdout);
            Some(stdout.lines().next()?.trim().to_string())
        } else {
            None
        }
    }

    pub fn refresh_seconds(&self) -> u64 {
        self.refresh_seconds
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn collect_info_returns_non_empty_host_and_arch() {
        let collector = TelemetryCollector::new(
            "@test-worker:example.com".into(),
            60,
            vec![],
        );
        let info = collector.collect_info().expect("collect_info failed");
        assert!(!info.host.is_empty(), "host should not be empty");
        assert!(!info.arch.is_empty(), "arch should not be empty");
        assert_eq!(info.worker_id, "@test-worker:example.com");
        assert!(info.cpu_count > 0, "should have at least one CPU");
        assert!(info.memory_total_mb > 0, "should have some memory");
    }

    #[test]
    fn probe_tools_finds_bash() {
        let collector = TelemetryCollector::new(
            "@test-worker:example.com".into(),
            60,
            vec![],
        );
        let tools = collector.probe_tools();
        let bash = tools.iter().find(|t| t.name == "bash");
        assert!(bash.is_some(), "bash should be found on this system");
        assert!(bash.unwrap().healthy);
    }

    #[test]
    fn extra_capabilities_included_in_result() {
        let collector = TelemetryCollector::new(
            "@test-worker:example.com".into(),
            120,
            vec!["gpu".into(), "linux".into()],
        );
        let info = collector.collect_info().expect("collect_info failed");
        assert_eq!(info.capabilities, vec!["gpu", "linux"]);
    }

    #[test]
    fn refresh_seconds_accessor() {
        let collector = TelemetryCollector::new("w".into(), 45, vec![]);
        assert_eq!(collector.refresh_seconds(), 45);
    }
}
