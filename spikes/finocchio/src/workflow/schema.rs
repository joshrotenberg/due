// Adapted from root src/workflow/contract.rs; see docs/integrated-workflow.md.
use serde_json::Value;
pub const DIALECT: &str = "https://json-schema.org/draft/2020-12/schema";
pub const MAX_BYTES: usize = 1024 * 1024;

pub fn validator(schema: &Value) -> Result<jsonschema::Validator, String> {
    supported(schema, 0)?;
    jsonschema::options()
        .with_draft(jsonschema::Draft::Draft202012)
        .build(schema)
        .map_err(|e| format!("invalid schema: {e}"))
}

fn supported(schema: &Value, depth: usize) -> Result<(), String> {
    if depth > 32 {
        return Err("schema nesting exceeds 32".into());
    }
    if schema.is_boolean() {
        return Ok(());
    }
    let object = schema
        .as_object()
        .ok_or("schema must be an object or boolean")?;
    for (key, value) in object {
        match key.as_str() {
            "$schema" if value.as_str() == Some(DIALECT) => (),
            "type" | "enum" | "const" | "required" | "minimum" | "maximum" | "exclusiveMinimum"
            | "exclusiveMaximum" | "minLength" | "maxLength" | "minItems" | "maxItems"
            | "minProperties" | "maxProperties" | "uniqueItems" | "title" | "description" => (),
            "properties" => {
                for sub in value
                    .as_object()
                    .ok_or("properties must be an object")?
                    .values()
                {
                    supported(sub, depth + 1)?;
                }
            }
            "items" | "additionalProperties" | "not" => supported(value, depth + 1)?,
            "allOf" | "anyOf" | "oneOf" => {
                for sub in value
                    .as_array()
                    .ok_or("schema composition must be an array")?
                {
                    supported(sub, depth + 1)?;
                }
            }
            _ => {
                return Err(format!(
                    "unsupported schema keyword {key:?}; spike uses a no-$ref Draft 2020-12 subset"
                ));
            }
        }
    }
    Ok(())
}

pub fn validate(schema: &jsonschema::Validator, value: &Value) -> Result<(), String> {
    schema
        .validate(value)
        .map_err(|e| format!("{} at {}", e, e.instance_path()))
}
