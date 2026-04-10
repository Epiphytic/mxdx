use anyhow::Result;
use mxdx_types::events::capability::{InputSchema, WorkerTool};
use mxdx_types::events::telemetry::WorkerTelemetryState;
use mxdx_types::events::worker_info::WorkerInfo;
use std::collections::HashMap;
use std::time::{SystemTime, UNIX_EPOCH};
use sysinfo::{Disks, System};

pub struct TelemetryCollector {
    worker_id: String,
    worker_uuid: String,
    refresh_seconds: u64,
    extra_capabilities: Vec<String>,
}

impl TelemetryCollector {
    pub fn new(worker_id: String, refresh_seconds: u64, extra_capabilities: Vec<String>) -> Self {
        let worker_uuid = uuid::Uuid::new_v4().to_string();
        Self {
            worker_id,
            worker_uuid,
            refresh_seconds,
            extra_capabilities,
        }
    }

    pub fn worker_uuid(&self) -> &str {
        &self.worker_uuid
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

    /// Collect a `WorkerTelemetryState` for posting as a Matrix state event.
    /// Compatible with the npm launcher's `org.mxdx.host_telemetry` format.
    pub fn collect_telemetry_state(
        &self,
        session_count: u32,
        status: &str,
    ) -> Result<WorkerTelemetryState> {
        let mut sys = System::new_all();
        sys.refresh_all();

        let host = hostname::get()?.to_string_lossy().to_string();
        let platform = System::name().unwrap_or_else(|| std::env::consts::OS.into());
        let arch = std::env::consts::ARCH.to_string();
        let cpu_count = sys.cpus().len() as u32;
        let total_memory_mb = sys.total_memory() / (1024 * 1024);
        let free_memory_mb = sys.available_memory() / (1024 * 1024);

        let tmux_available = std::process::Command::new("which")
            .arg("tmux")
            .output()
            .map(|o| o.status.success())
            .unwrap_or(false);

        Ok(WorkerTelemetryState {
            timestamp: iso8601_now(),
            heartbeat_interval_ms: self.refresh_seconds * 1000,
            worker_uuid: Some(self.worker_uuid.clone()),
            hostname: host,
            platform,
            arch,
            cpus: Some(cpu_count),
            total_memory_mb: Some(total_memory_mb),
            free_memory_mb: Some(free_memory_mb),
            status: status.to_string(),
            capabilities: self.extra_capabilities.clone(),
            session_count: Some(session_count),
            tmux_available,
            session_persistence: tmux_available,
        })
    }
}

/// Generate an ISO 8601 UTC timestamp string (e.g. "2026-03-31T12:00:00Z").
fn iso8601_now() -> String {
    let dur = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default();
    let secs = dur.as_secs();

    // Convert epoch seconds to date/time components (UTC)
    let days = secs / 86400;
    let time_of_day = secs % 86400;
    let hours = time_of_day / 3600;
    let minutes = (time_of_day % 3600) / 60;
    let seconds = time_of_day % 60;

    // Days since 1970-01-01
    let (year, month, day) = days_to_ymd(days);

    format!(
        "{:04}-{:02}-{:02}T{:02}:{:02}:{:02}Z",
        year, month, day, hours, minutes, seconds
    )
}

/// Convert days since Unix epoch to (year, month, day).
fn days_to_ymd(days: u64) -> (u64, u64, u64) {
    // Algorithm based on Howard Hinnant's civil_from_days
    let z = days as i64 + 719468;
    let era = if z >= 0 { z } else { z - 146096 } / 146097;
    let doe = (z - era * 146097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36524 - doe / 146096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };
    (y as u64, m, d)
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

    #[test]
    fn collect_telemetry_state_online() {
        let collector = TelemetryCollector::new(
            "@test-worker:example.com".into(),
            60,
            vec!["docker".into()],
        );
        let state = collector
            .collect_telemetry_state(2, "online")
            .expect("collect_telemetry_state failed");
        assert_eq!(state.status, "online");
        assert_eq!(state.heartbeat_interval_ms, 60000);
        assert_eq!(state.session_count, Some(2));
        assert!(!state.hostname.is_empty());
        assert!(!state.arch.is_empty());
        assert!(state.cpus.unwrap() > 0);
        assert!(state.total_memory_mb.unwrap() > 0);
        assert_eq!(state.capabilities, vec!["docker"]);
        // Timestamp should be ISO 8601 format
        assert!(state.timestamp.ends_with('Z'));
        assert!(state.timestamp.contains('T'));
    }

    #[test]
    fn collect_telemetry_state_offline() {
        let collector = TelemetryCollector::new(
            "@test-worker:example.com".into(),
            60,
            vec![],
        );
        let state = collector
            .collect_telemetry_state(0, "offline")
            .expect("collect_telemetry_state failed");
        assert_eq!(state.status, "offline");
        assert_eq!(state.session_count, Some(0));
    }

    #[test]
    fn iso8601_now_format() {
        let ts = super::iso8601_now();
        // Should look like "YYYY-MM-DDTHH:MM:SSZ"
        assert_eq!(ts.len(), 20);
        assert!(ts.ends_with('Z'));
        assert_eq!(&ts[4..5], "-");
        assert_eq!(&ts[7..8], "-");
        assert_eq!(&ts[10..11], "T");
        assert_eq!(&ts[13..14], ":");
        assert_eq!(&ts[16..17], ":");
    }

    #[test]
    fn days_to_ymd_epoch() {
        // 1970-01-01
        let (y, m, d) = super::days_to_ymd(0);
        assert_eq!((y, m, d), (1970, 1, 1));
    }

    #[test]
    fn days_to_ymd_known_date() {
        // 2026-03-31 is day 20543 since epoch
        // (calculated: 56 years * 365 + 14 leap days + 31 + 28 + 31 - 1 = 20543)
        // Actually let's just verify a known date
        // 2000-01-01 = day 10957
        let (y, m, d) = super::days_to_ymd(10957);
        assert_eq!((y, m, d), (2000, 1, 1));
    }
}
