use serde::{Deserialize, Serialize};

use super::capability::WorkerTool;

/// State event representing a worker's current info and capabilities.
/// Merges capability advertisement with telemetry data into a single state event.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub struct WorkerInfo {
    pub worker_id: String,
    pub host: String,
    pub os: String,
    pub arch: String,
    pub cpu_count: u32,
    pub memory_total_mb: u64,
    pub disk_available_mb: u64,
    pub tools: Vec<WorkerTool>,
    pub capabilities: Vec<String>,
    pub updated_at: u64,
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::events::capability::{InputSchema, SchemaProperty};
    use std::collections::HashMap;

    fn sample_worker_info() -> WorkerInfo {
        WorkerInfo {
            worker_id: "@bel-worker:ca1-beta.mxdx.dev".into(),
            host: "belthanior".into(),
            os: "linux".into(),
            arch: "x86_64".into(),
            cpu_count: 16,
            memory_total_mb: 32768,
            disk_available_mb: 102400,
            tools: vec![WorkerTool {
                name: "jcode".into(),
                version: Some("0.7.2".into()),
                description: "Rust coding agent".into(),
                healthy: true,
                input_schema: InputSchema {
                    r#type: "object".into(),
                    properties: HashMap::from([(
                        "prompt".into(),
                        SchemaProperty {
                            r#type: "string".into(),
                            description: "Task prompt".into(),
                        },
                    )]),
                    required: vec!["prompt".into()],
                },
            }],
            capabilities: vec!["linux".into(), "gpu".into()],
            updated_at: 1742572800,
        }
    }

    #[test]
    fn worker_info_roundtrip() {
        let info = sample_worker_info();
        let json = serde_json::to_string(&info).unwrap();
        let back: WorkerInfo = serde_json::from_str(&json).unwrap();
        assert_eq!(back.worker_id, "@bel-worker:ca1-beta.mxdx.dev");
        assert_eq!(back.host, "belthanior");
        assert_eq!(back.os, "linux");
        assert_eq!(back.arch, "x86_64");
        assert_eq!(back.cpu_count, 16);
        assert_eq!(back.memory_total_mb, 32768);
        assert_eq!(back.disk_available_mb, 102400);
        assert_eq!(back.tools.len(), 1);
        assert_eq!(back.tools[0].name, "jcode");
        assert_eq!(back.capabilities, vec!["linux", "gpu"]);
        assert_eq!(back.updated_at, 1742572800);
    }

    #[test]
    fn worker_info_snake_case_fields() {
        let info = sample_worker_info();
        let json = serde_json::to_string(&info).unwrap();
        assert!(json.contains("worker_id"), "expected snake_case worker_id in: {json}");
        assert!(json.contains("cpu_count"), "expected snake_case cpu_count in: {json}");
        assert!(json.contains("memory_total_mb"), "expected snake_case memory_total_mb in: {json}");
        assert!(json.contains("disk_available_mb"), "expected snake_case disk_available_mb in: {json}");
        assert!(json.contains("updated_at"), "expected snake_case updated_at in: {json}");
        // Ensure no camelCase leakage for WorkerInfo's own fields
        assert!(!json.contains("workerId"), "unexpected camelCase workerId in: {json}");
        assert!(!json.contains("cpuCount"), "unexpected camelCase cpuCount in: {json}");
        assert!(!json.contains("memoryTotalMb"), "unexpected camelCase memoryTotalMb in: {json}");
    }
}
