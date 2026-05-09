//! Convert an MCP `tools/list` descriptor into a Wilai ToolSpec so it
//! plugs into the existing registry, validator, guard, and audit machinery.
//!
//! Mapping decisions
//! - Name: `mcp.<server>.<tool>` (dotted; lowercase server, original tool name).
//! - Inputs: pulled from the MCP JSON schema. We support type=object with a
//!   flat properties map of primitive types (string/integer/number/boolean
//!   /array/object). Nested object schemas are passed through opaque.
//! - Category and risk: heuristic on the tool name. Names that look like
//!   mutations (write/create/update/delete/move/send/post/patch/put) get
//!   write/medium so they trigger the confirm UX. Everything else gets
//!   read/none. Operators can override per-tool via tools.disabled or by
//!   shipping a wrapper YAML.
//! - Executor: a builtin sentinel `mcp.proxy.v1`; the actual call is
//!   intercepted by the daemon before the executor runs and routed to the
//!   right McpClient.

use anyhow::Result;
use serde_json::Value;
use std::collections::BTreeMap;
use wilai_core::{Category, Risk};
use wilai_tools::spec::{
    AuditSpec, BuiltinSpec, CaptureKind, ExecutorSpec, InputSpec, InputType, OutputSpec, ToolSpec,
};

pub fn descriptor_to_spec(server_name: &str, desc: &crate::wire::McpToolDescriptor) -> Result<ToolSpec> {
    let name = format!("mcp.{}.{}", server_name, desc.name);
    let (category, risk) = classify(&desc.name);
    let inputs = parse_inputs(desc.input_schema.as_ref());

    Ok(ToolSpec {
        name: name.clone(),
        version: 1,
        description: desc
            .description
            .clone()
            .unwrap_or_else(|| format!("MCP tool {} from server {}", desc.name, server_name)),
        category,
        risk,
        tags: vec!["mcp".to_string(), server_name.to_string()],
        mode_required: None,
        mode_forbidden: None,
        inputs,
        guards: Vec::new(),
        executor: ExecutorSpec::Builtin(BuiltinSpec {
            function: "mcp.proxy.v1".to_string(),
            timeout_s: 120,
            output: OutputSpec {
                capture: CaptureKind::Stdout,
                max_bytes: 524_288,
                redact: Vec::new(),
            },
        }),
        audit: AuditSpec::default(),
        examples: Vec::new(),
    })
}

fn classify(tool_name: &str) -> (Category, Risk) {
    let n = tool_name.to_ascii_lowercase();
    let mutating = [
        "write", "create", "update", "delete", "remove", "move", "rename",
        "send", "post", "patch", "put", "comment", "merge", "close", "reopen",
        "publish", "push", "fork", "branch", "tag",
    ];
    if mutating.iter().any(|m| n.contains(m)) {
        return (Category::Network, Risk::Medium);
    }
    let destructive = ["destroy", "drop", "purge", "wipe"];
    if destructive.iter().any(|m| n.contains(m)) {
        return (Category::Destructive, Risk::High);
    }
    (Category::Network, Risk::None)
}

fn parse_inputs(schema: Option<&Value>) -> BTreeMap<String, InputSpec> {
    let mut out = BTreeMap::new();
    let Some(schema) = schema else { return out };
    let Some(obj) = schema.as_object() else { return out };
    let required: Vec<String> = obj
        .get("required")
        .and_then(|r| r.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| v.as_str().map(|s| s.to_string()))
                .collect()
        })
        .unwrap_or_default();
    let props = match obj.get("properties").and_then(|p| p.as_object()) {
        Some(p) => p,
        None => return out,
    };
    for (name, p) in props {
        out.insert(name.clone(), parse_property(p, required.contains(name)));
    }
    out
}

fn parse_property(prop: &Value, required: bool) -> InputSpec {
    let obj = prop.as_object().cloned().unwrap_or_default();
    let kind = match obj.get("type").and_then(|t| t.as_str()).unwrap_or("string") {
        "integer" => InputType::Integer,
        "number" => InputType::Number,
        "boolean" => InputType::Boolean,
        "array" => InputType::Array,
        "object" => InputType::Object,
        _ => {
            if obj.get("enum").is_some() {
                InputType::Enum
            } else {
                InputType::String
            }
        }
    };
    let description = obj.get("description").and_then(|d| d.as_str()).map(|s| s.to_string());
    let default = obj.get("default").cloned();
    let values = obj
        .get("enum")
        .and_then(|e| e.as_array().cloned());
    let items = match (kind, obj.get("items")) {
        (InputType::Array, Some(item_schema)) => Some(Box::new(parse_property(item_schema, false))),
        _ => None,
    };
    let min = obj.get("minimum").and_then(|n| n.as_f64());
    let max = obj.get("maximum").and_then(|n| n.as_f64());

    InputSpec {
        kind,
        description,
        required,
        default,
        pattern: None,
        min,
        max,
        values,
        items,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn schema(s: &str) -> Value {
        serde_json::from_str(s).unwrap()
    }

    #[test]
    fn classifies_read_vs_write() {
        let d = crate::wire::McpToolDescriptor {
            name: "search_repositories".into(),
            description: None,
            input_schema: None,
        };
        let spec = descriptor_to_spec("github", &d).unwrap();
        assert_eq!(spec.category, Category::Network);
        assert_eq!(spec.risk, Risk::None);

        let d = crate::wire::McpToolDescriptor {
            name: "create_issue".into(),
            description: None,
            input_schema: None,
        };
        let spec = descriptor_to_spec("github", &d).unwrap();
        assert_eq!(spec.risk, Risk::Medium);
    }

    #[test]
    fn parses_object_schema() {
        let d = crate::wire::McpToolDescriptor {
            name: "search_code".into(),
            description: Some("search source".into()),
            input_schema: Some(schema(
                r#"{
                "type":"object",
                "properties":{
                    "query":{"type":"string","description":"q"},
                    "limit":{"type":"integer","minimum":1,"maximum":100,"default":20}
                },
                "required":["query"]
            }"#,
            )),
        };
        let spec = descriptor_to_spec("github", &d).unwrap();
        assert_eq!(spec.name, "mcp.github.search_code");
        assert_eq!(spec.inputs.len(), 2);
        let q = spec.inputs.get("query").unwrap();
        assert!(q.required);
        assert!(matches!(q.kind, InputType::String));
        let l = spec.inputs.get("limit").unwrap();
        assert!(matches!(l.kind, InputType::Integer));
        assert_eq!(l.min, Some(1.0));
        assert_eq!(l.max, Some(100.0));
        assert_eq!(l.default, Some(serde_json::json!(20)));
    }
}

