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
}
