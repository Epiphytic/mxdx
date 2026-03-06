use serde::{Deserialize, Serialize};

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
}
