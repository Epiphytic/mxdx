use serde::{Deserialize, Serialize};

/// Matrix state event type for worker/launcher telemetry.
/// Used by both the Rust worker and the npm launcher for cross-ecosystem compatibility.
pub const WORKER_TELEMETRY: &str = "org.mxdx.host_telemetry";

/// Unified telemetry state event posted periodically by workers and launchers.
/// Matches the npm launcher's `org.mxdx.host_telemetry` state event format.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkerTelemetryState {
    pub timestamp: String,              // ISO 8601
    pub heartbeat_interval_ms: u64,     // default 60000
    pub hostname: String,
    pub platform: String,
    pub arch: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cpus: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub total_memory_mb: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_memory_mb: Option<u64>,
    pub status: String,                 // "online" | "offline"
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_count: Option<u32>,
    pub tmux_available: bool,
    pub session_persistence: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct HostTelemetryEvent {
    pub timestamp: String,
    pub hostname: String,
    pub os: String,
    pub arch: String,
    pub uptime_seconds: u64,
    pub load_avg: [f64; 3],
    pub cpu: CpuInfo,
    pub memory: MemoryInfo,
    pub disk: DiskInfo,
    pub network: Option<NetworkInfo>,
    pub services: Option<serde_json::Value>,
    pub devices: Option<serde_json::Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct CpuInfo {
    pub cores: u32,
    pub usage_percent: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct MemoryInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct DiskInfo {
    pub total_bytes: u64,
    pub used_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct NetworkInfo {
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn telemetry_event_round_trips_json() {
        let evt = HostTelemetryEvent {
            timestamp: "2026-03-05T12:00:00Z".into(),
            hostname: "worker-01".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            uptime_seconds: 86400,
            load_avg: [0.5, 0.3, 0.1],
            cpu: CpuInfo {
                cores: 8,
                usage_percent: 45.2,
            },
            memory: MemoryInfo {
                total_bytes: 16_000_000_000,
                used_bytes: 8_000_000_000,
            },
            disk: DiskInfo {
                total_bytes: 500_000_000_000,
                used_bytes: 200_000_000_000,
            },
            network: None,
            services: None,
            devices: None,
        };
        let json = serde_json::to_string(&evt).unwrap();
        let parsed: HostTelemetryEvent = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.hostname, "worker-01");
        assert_eq!(parsed.cpu.cores, 8);
        assert_eq!(parsed.memory.total_bytes, 16_000_000_000);
        assert_eq!(parsed.uptime_seconds, 86400);
    }

    #[test]
    fn telemetry_event_with_optional_fields() {
        let evt = HostTelemetryEvent {
            timestamp: "2026-03-05T12:00:00Z".into(),
            hostname: "worker-02".into(),
            os: "linux".into(),
            arch: "aarch64".into(),
            uptime_seconds: 3600,
            load_avg: [1.0, 0.8, 0.6],
            cpu: CpuInfo {
                cores: 4,
                usage_percent: 90.0,
            },
            memory: MemoryInfo {
                total_bytes: 8_000_000_000,
                used_bytes: 7_000_000_000,
            },
            disk: DiskInfo {
                total_bytes: 100_000_000_000,
                used_bytes: 50_000_000_000,
            },
            network: Some(NetworkInfo {
                rx_bytes: 1_000_000,
                tx_bytes: 500_000,
            }),
            services: None,
            devices: None,
        };
        let json = serde_json::to_string(&evt).unwrap();
        assert!(json.contains("rx_bytes"));
        let parsed: HostTelemetryEvent = serde_json::from_str(&json).unwrap();
        assert!(parsed.network.is_some());
    }

    #[test]
    fn telemetry_event_rejects_missing_required_fields() {
        let json = r#"{"timestamp":"2026-03-05T12:00:00Z","hostname":"x"}"#;
        let result: Result<HostTelemetryEvent, _> = serde_json::from_str(json);
        assert!(result.is_err());
    }

    #[test]
    fn worker_telemetry_state_round_trips_json() {
        let state = WorkerTelemetryState {
            timestamp: "2026-03-31T12:00:00Z".into(),
            heartbeat_interval_ms: 60000,
            hostname: "worker-01".into(),
            platform: "linux".into(),
            arch: "x86_64".into(),
            cpus: Some(8),
            total_memory_mb: Some(16384),
            free_memory_mb: Some(8192),
            status: "online".into(),
            capabilities: vec!["gpu".into(), "docker".into()],
            session_count: Some(3),
            tmux_available: true,
            session_persistence: true,
        };
        let json = serde_json::to_string(&state).unwrap();
        let parsed: WorkerTelemetryState = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed, state);
    }

    #[test]
    fn worker_telemetry_state_omits_none_fields() {
        let state = WorkerTelemetryState {
            timestamp: "2026-03-31T12:00:00Z".into(),
            heartbeat_interval_ms: 60000,
            hostname: "worker-01".into(),
            platform: "linux".into(),
            arch: "x86_64".into(),
            cpus: None,
            total_memory_mb: None,
            free_memory_mb: None,
            status: "online".into(),
            capabilities: vec![],
            session_count: None,
            tmux_available: false,
            session_persistence: false,
        };
        let json = serde_json::to_string(&state).unwrap();
        assert!(!json.contains("cpus"));
        assert!(!json.contains("total_memory_mb"));
        assert!(!json.contains("free_memory_mb"));
        assert!(!json.contains("session_count"));
    }

    #[test]
    fn worker_telemetry_state_offline_minimal() {
        // Matches npm launcher's #postOfflineStatus format
        let json = r#"{
            "timestamp": "2026-03-31T12:00:00Z",
            "heartbeat_interval_ms": 60000,
            "hostname": "",
            "platform": "",
            "arch": "",
            "status": "offline",
            "tmux_available": false,
            "session_persistence": false
        }"#;
        let parsed: WorkerTelemetryState = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.status, "offline");
        assert!(parsed.capabilities.is_empty());
        assert!(parsed.cpus.is_none());
    }

    #[test]
    fn worker_telemetry_state_npm_compat() {
        // JSON matching the npm launcher's telemetry format
        let json = r#"{
            "timestamp": "2026-03-31T10:00:00.000Z",
            "heartbeat_interval_ms": 60000,
            "hostname": "dev-server",
            "platform": "linux",
            "arch": "x64",
            "cpus": 4,
            "total_memory_mb": 8192,
            "free_memory_mb": 4096,
            "tmux_available": true,
            "session_persistence": true,
            "status": "online",
            "capabilities": ["docker"],
            "session_count": 1
        }"#;
        let parsed: WorkerTelemetryState = serde_json::from_str(json).unwrap();
        assert_eq!(parsed.hostname, "dev-server");
        assert_eq!(parsed.heartbeat_interval_ms, 60000);
        assert_eq!(parsed.cpus, Some(4));
        assert_eq!(parsed.session_count, Some(1));
        assert!(parsed.tmux_available);
    }

    #[test]
    fn worker_telemetry_const_matches_npm() {
        assert_eq!(WORKER_TELEMETRY, "org.mxdx.host_telemetry");
    }
}
