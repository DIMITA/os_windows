use crate::spec::{InputSpec, InputType, ToolSpec};
use anyhow::{bail, Result};
use serde_json::{Map, Value};

pub fn validate_args(spec: &ToolSpec, args: &Value) -> Result<Value> {
    let obj = args
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("arguments must be a JSON object"))?;
    let mut out = Map::new();

    for (name, ispec) in &spec.inputs {
        match obj.get(name) {
            Some(v) => {
                let validated = validate_one(name, ispec, v)?;
                out.insert(name.clone(), validated);
            }
            None => {
                if let Some(d) = &ispec.default {
                    out.insert(name.clone(), d.clone());
                } else if ispec.required {
                    bail!("missing required argument: {name}");
                }
            }
        }
    }

    for k in obj.keys() {
        if !spec.inputs.contains_key(k) {
            bail!("unknown argument: {k}");
        }
    }

    Ok(Value::Object(out))
}

fn validate_one(name: &str, spec: &InputSpec, value: &Value) -> Result<Value> {
    match spec.kind {
        InputType::String => {
            let s = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{name}: expected string"))?;
            if let Some(p) = &spec.pattern {
                crate::patterns::validate(p, s)
                    .map_err(|e| anyhow::anyhow!("{name}: {e}"))?;
            }
            Ok(value.clone())
        }
        InputType::Integer => {
            let i = value
                .as_i64()
                .ok_or_else(|| anyhow::anyhow!("{name}: expected integer"))?;
            if let Some(min) = spec.min {
                if (i as f64) < min {
                    bail!("{name}: below min ({min})");
                }
            }
            if let Some(max) = spec.max {
                if (i as f64) > max {
                    bail!("{name}: above max ({max})");
                }
            }
            Ok(value.clone())
        }
        InputType::Number => {
            let f = value
                .as_f64()
                .ok_or_else(|| anyhow::anyhow!("{name}: expected number"))?;
            if let Some(min) = spec.min {
                if f < min {
                    bail!("{name}: below min");
                }
            }
            if let Some(max) = spec.max {
                if f > max {
                    bail!("{name}: above max");
                }
            }
            Ok(value.clone())
        }
        InputType::Boolean => {
            value
                .as_bool()
                .ok_or_else(|| anyhow::anyhow!("{name}: expected bool"))?;
            Ok(value.clone())
        }
        InputType::Enum => {
            let values = spec
                .values
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("{name}: enum without values"))?;
            if !values.contains(value) {
                bail!("{name}: not in allowed values");
            }
            Ok(value.clone())
        }
        InputType::Array => {
            let arr = value
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("{name}: expected array"))?;
            let item_spec = spec
                .items
                .as_ref()
                .ok_or_else(|| anyhow::anyhow!("{name}: array without item spec"))?;
            let mut out = Vec::with_capacity(arr.len());
            for (i, v) in arr.iter().enumerate() {
                let label = format!("{name}[{i}]");
                out.push(validate_one(&label, item_spec, v)?);
            }
            Ok(Value::Array(out))
        }
        InputType::Object => Ok(value.clone()),
    }
}
