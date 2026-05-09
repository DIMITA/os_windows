use serde_json::{json, Map, Value};
use wilai_tools::spec::{InputType, ToolSpec};

pub fn tool_to_schema(spec: &ToolSpec) -> wilai_providers::ToolSchema {
    let mut props = Map::new();
    let mut required = Vec::new();
    for (name, ispec) in &spec.inputs {
        let mut prop = Map::new();
        let ty = match ispec.kind {
            InputType::String => "string",
            InputType::Integer => "integer",
            InputType::Number => "number",
            InputType::Boolean => "boolean",
            InputType::Enum => "string",
            InputType::Array => "array",
            InputType::Object => "object",
        };
        prop.insert("type".to_string(), Value::String(ty.to_string()));
        if let Some(desc) = &ispec.description {
            prop.insert("description".to_string(), Value::String(desc.clone()));
        }
        if let Some(values) = &ispec.values {
            prop.insert("enum".to_string(), Value::Array(values.clone()));
        }
        if ispec.kind == InputType::Array {
            if let Some(items) = &ispec.items {
                let item_ty = match items.kind {
                    InputType::String => "string",
                    InputType::Integer => "integer",
                    InputType::Number => "number",
                    InputType::Boolean => "boolean",
                    _ => "string",
                };
                prop.insert("items".to_string(), json!({ "type": item_ty }));
            }
        }
        props.insert(name.clone(), Value::Object(prop));
        if ispec.required {
            required.push(Value::String(name.clone()));
        }
    }

    let parameters = json!({
        "type": "object",
        "properties": Value::Object(props),
        "required": Value::Array(required),
    });

    wilai_providers::ToolSchema {
        name: spec.name.clone(),
        description: spec.description.clone(),
        parameters,
    }
}
