use serde::{Deserialize, Serialize};
use std::collections::HashMap;

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct SchemaProperty {
    pub r#type: String,
    pub description: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct InputSchema {
    pub r#type: String,
    pub properties: HashMap<String, SchemaProperty>,
    pub required: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct WorkerTool {
    pub name: String,
    pub version: Option<String>,
    pub description: String,
    pub healthy: bool,
    pub input_schema: InputSchema,
}

#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CapabilityAdvertisement {
    pub worker_id: String,
    pub host: String,
    pub tools: Vec<WorkerTool>,
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json;

    #[test]
    fn capability_advertisement_round_trips_json() {
        let ad = CapabilityAdvertisement {
            worker_id: "@bel-worker:ca1-beta.mxdx.dev".into(),
            host: "belthanior".into(),
            tools: vec![WorkerTool {
                name: "jcode".into(),
                version: Some("0.7.2".into()),
                description: "Rust coding agent (Claude Max OAuth)".into(),
                healthy: true,
                input_schema: InputSchema {
                    r#type: "object".into(),
                    properties: HashMap::from([
                        (
                            "prompt".into(),
                            SchemaProperty {
                                r#type: "string".into(),
                                description: "Task prompt".into(),
                            },
                        ),
                        (
                            "cwd".into(),
                            SchemaProperty {
                                r#type: "string".into(),
                                description: "Absolute working directory path".into(),
                            },
                        ),
                    ]),
                    required: vec!["prompt".into()],
                },
            }],
        };
        let json = serde_json::to_string(&ad).unwrap();
        let parsed: CapabilityAdvertisement = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.worker_id, ad.worker_id);
        assert_eq!(parsed.host, "belthanior");
        assert_eq!(parsed.tools.len(), 1);
        assert_eq!(parsed.tools[0].name, "jcode");
        assert_eq!(parsed.tools[0].version, Some("0.7.2".into()));
        assert!(parsed.tools[0].healthy);
        assert_eq!(parsed.tools[0].input_schema.r#type, "object");
        assert_eq!(parsed.tools[0].input_schema.properties.len(), 2);
        assert!(parsed.tools[0]
            .input_schema
            .properties
            .contains_key("prompt"));
        assert_eq!(parsed.tools[0].input_schema.required, vec!["prompt"]);
    }

    #[test]
    fn capability_advertisement_json_uses_camel_case() {
        let ad = CapabilityAdvertisement {
            worker_id: "@worker:example.com".into(),
            host: "test-host".into(),
            tools: vec![WorkerTool {
                name: "tool".into(),
                version: None,
                description: "A test tool".into(),
                healthy: true,
                input_schema: InputSchema {
                    r#type: "object".into(),
                    properties: HashMap::new(),
                    required: vec![],
                },
            }],
        };
        let json = serde_json::to_string(&ad).unwrap();
        assert!(
            json.contains("workerId"),
            "expected camelCase workerId, got: {json}"
        );
        assert!(
            !json.contains("worker_id"),
            "unexpected snake_case worker_id in: {json}"
        );
        assert!(
            json.contains("inputSchema"),
            "expected camelCase inputSchema, got: {json}"
        );
        assert!(
            !json.contains("input_schema"),
            "unexpected snake_case input_schema in: {json}"
        );
    }

    #[test]
    fn capability_advertisement_deserializes_from_wire_json() {
        let wire = r#"{
            "workerId": "@bel-worker:ca1-beta.mxdx.dev",
            "host": "belthanior",
            "tools": [{
                "name": "jcode",
                "version": "0.7.2",
                "description": "Rust coding agent",
                "healthy": true,
                "inputSchema": {
                    "type": "object",
                    "properties": {
                        "prompt": {"type": "string", "description": "Task prompt"}
                    },
                    "required": ["prompt"]
                }
            }]
        }"#;
        let ad: CapabilityAdvertisement = serde_json::from_str(wire).unwrap();
        assert_eq!(ad.worker_id, "@bel-worker:ca1-beta.mxdx.dev");
        assert_eq!(ad.host, "belthanior");
        assert_eq!(ad.tools.len(), 1);
        assert_eq!(ad.tools[0].name, "jcode");
        assert_eq!(ad.tools[0].version, Some("0.7.2".into()));
        assert!(ad.tools[0].healthy);
        assert_eq!(ad.tools[0].input_schema.required, vec!["prompt"]);
    }

    #[test]
    fn capability_advertisement_empty_tools() {
        let ad = CapabilityAdvertisement {
            worker_id: "@worker:example.com".into(),
            host: "idle-host".into(),
            tools: vec![],
        };
        let json = serde_json::to_string(&ad).unwrap();
        let parsed: CapabilityAdvertisement = serde_json::from_str(&json).unwrap();
        assert!(parsed.tools.is_empty());
        assert_eq!(parsed.host, "idle-host");
    }

    #[test]
    fn capability_advertisement_multiple_tools() {
        let ad = CapabilityAdvertisement {
            worker_id: "@multi:example.com".into(),
            host: "multi-host".into(),
            tools: vec![
                WorkerTool {
                    name: "jcode".into(),
                    version: Some("0.7.2".into()),
                    description: "Coding agent".into(),
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
                },
                WorkerTool {
                    name: "opencode".into(),
                    version: None,
                    description: "Go coding agent".into(),
                    healthy: false,
                    input_schema: InputSchema {
                        r#type: "object".into(),
                        properties: HashMap::new(),
                        required: vec![],
                    },
                },
            ],
        };
        let json = serde_json::to_string(&ad).unwrap();
        let parsed: CapabilityAdvertisement = serde_json::from_str(&json).unwrap();
        assert_eq!(parsed.tools.len(), 2);
        assert_eq!(parsed.tools[0].name, "jcode");
        assert!(parsed.tools[0].healthy);
        assert_eq!(parsed.tools[1].name, "opencode");
        assert!(!parsed.tools[1].healthy);
        assert_eq!(parsed.tools[1].version, None);
    }

    #[test]
    fn capability_advertisement_version_null_in_json() {
        let wire = r#"{
            "workerId": "@worker:example.com",
            "host": "test",
            "tools": [{
                "name": "jcode",
                "version": null,
                "description": "Agent",
                "healthy": false,
                "inputSchema": {
                    "type": "object",
                    "properties": {},
                    "required": []
                }
            }]
        }"#;
        let ad: CapabilityAdvertisement = serde_json::from_str(wire).unwrap();
        assert_eq!(ad.tools[0].version, None);
        assert!(!ad.tools[0].healthy);
    }

    #[test]
    fn capability_advertisement_equality() {
        let make = || CapabilityAdvertisement {
            worker_id: "@w:example.com".into(),
            host: "h".into(),
            tools: vec![WorkerTool {
                name: "t".into(),
                version: Some("1.0".into()),
                description: "d".into(),
                healthy: true,
                input_schema: InputSchema {
                    r#type: "object".into(),
                    properties: HashMap::new(),
                    required: vec![],
                },
            }],
        };
        assert_eq!(make(), make());
    }
}
