use anyhow::{Result, bail, ensure};
use std::sync::OnceLock;

/// The checked-in catalogue is the contract for tool discovery and validation.
pub fn catalogue() -> &'static serde_json::Value {
    static CATALOGUE: OnceLock<serde_json::Value> = OnceLock::new();
    CATALOGUE.get_or_init(|| {
        serde_json::from_str(include_str!("../protocol/tools.json"))
            .expect("checked-in tool catalogue")
    })
}
pub fn tool(name: &str) -> Option<&'static serde_json::Value> {
    catalogue()["tools"]
        .as_array()?
        .iter()
        .find(|t| t["name"] == name)
}
pub fn read_only(name: &str) -> bool {
    tool(name).is_some_and(|t| t["annotations"]["readOnlyHint"] == true)
}
pub fn validate(name: &str, args: &serde_json::Value) -> Result<()> {
    let schema = &tool(name).ok_or_else(|| anyhow::anyhow!("unknown tool"))?["inputSchema"];
    validate_schema(schema, args)
}
/// Validates the small JSON-schema subset used by the catalogue: objects with
/// string, integer, boolean and string-array properties.
pub fn validate_schema(schema: &serde_json::Value, args: &serde_json::Value) -> Result<()> {
    let object = args
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("arguments must be object"))?;
    for required in schema["required"].as_array().into_iter().flatten() {
        let key = required.as_str().unwrap_or_default();
        ensure!(object.contains_key(key), "missing required argument {key}");
    }
    let properties = schema["properties"]
        .as_object()
        .ok_or_else(|| anyhow::anyhow!("invalid checked-in schema"))?;
    for (name, value) in object {
        if name == "_meta" {
            continue;
        }
        let spec = properties
            .get(name)
            .ok_or_else(|| anyhow::anyhow!("unexpected argument {name}"))?;
        value_matches(name, spec, value)?;
    }
    Ok(())
}
fn value_matches(name: &str, spec: &serde_json::Value, value: &serde_json::Value) -> Result<()> {
    match spec["type"].as_str().unwrap_or_default() {
        "string" => {
            let text = value
                .as_str()
                .ok_or_else(|| anyhow::anyhow!("{name} must be string"))?;
            ensure!(!text.contains('\0'), "{name} contains NUL");
            if let Some(max) = spec["maxLength"].as_u64() {
                ensure!(text.len() <= max as usize, "{name} exceeds byte limit");
            }
            if let Some(min) = spec["minLength"].as_u64() {
                ensure!(text.len() >= min as usize, "{name} is too short");
            }
        }
        "integer" => {
            let n = value
                .as_u64()
                .ok_or_else(|| anyhow::anyhow!("{name} must be nonnegative integer"))?;
            ensure!(
                n >= spec["minimum"].as_u64().unwrap_or(0)
                    && n <= spec["maximum"].as_u64().unwrap_or(u64::MAX),
                "{name} out of range"
            );
        }
        "boolean" => ensure!(value.is_boolean(), "{name} must be boolean"),
        "array" => {
            let items = value
                .as_array()
                .ok_or_else(|| anyhow::anyhow!("{name} must be array"))?;
            ensure!(
                items.len() >= spec["minItems"].as_u64().unwrap_or(0) as usize
                    && items.len() <= spec["maxItems"].as_u64().unwrap_or(u64::MAX) as usize,
                "{name} item count out of range"
            );
            for item in items {
                value_matches(name, &spec["items"], item)?;
            }
        }
        _ => bail!("unsupported checked-in schema"),
    }
    Ok(())
}
