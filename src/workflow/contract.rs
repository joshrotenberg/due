//! Deliberately small, offline JSON Schema + checked Rhai JSON boundary.
use rhai::{Array, Dynamic, FLOAT, INT, ImmutableString, Map};
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

pub fn to_dynamic(value: &Value) -> Result<Dynamic, String> {
    fn convert(value: &Value, depth: usize) -> Result<Dynamic, String> {
        if depth > 32 {
            return Err("JSON nesting exceeds 32".into());
        }
        Ok(match value {
            Value::Null => Dynamic::UNIT,
            Value::Bool(v) => (*v).into(),
            Value::String(v) => v.clone().into(),
            Value::Number(n) => {
                if let Some(i) = n.as_i64() {
                    Dynamic::from_int(i)
                } else if n.is_u64() {
                    return Err("JSON integer exceeds signed 64-bit Rhai range".into());
                } else {
                    let f = n.as_f64().ok_or("unrepresentable JSON number")?;
                    if !f.is_finite() || (f.fract() == 0.0 && f.abs() > 9_007_199_254_740_991.0) {
                        return Err("nonfinite or potentially lossy integral float".into());
                    }
                    Dynamic::from_float(f)
                }
            }
            Value::Array(v) => v
                .iter()
                .map(|x| convert(x, depth + 1))
                .collect::<Result<Array, _>>()?
                .into(),
            Value::Object(v) => v
                .iter()
                .map(|(k, v)| Ok((k.clone().into(), convert(v, depth + 1)?)))
                .collect::<Result<Map, String>>()?
                .into(),
        })
    }
    if serde_json::to_vec(value).map_err(|e| e.to_string())?.len() > MAX_BYTES {
        return Err("JSON exceeds 1 MiB".into());
    }
    convert(value, 0)
}

pub fn from_dynamic(value: &Dynamic) -> Result<Value, String> {
    fn convert(v: &Dynamic, depth: usize) -> Result<Value, String> {
        if depth > 32 {
            return Err("JSON nesting exceeds 32".into());
        }
        if v.is_unit() {
            return Ok(Value::Null);
        }
        if v.is::<bool>() {
            return Ok(v.clone_cast::<bool>().into());
        }
        if v.is::<INT>() {
            return Ok(v.clone_cast::<INT>().into());
        }
        if v.is::<FLOAT>() {
            let f = v.clone_cast::<FLOAT>();
            if !f.is_finite() || (f.fract() == 0.0 && f.abs() > 9_007_199_254_740_991.0) {
                return Err("nonfinite or potentially lossy integral float".into());
            }
            return Ok(serde_json::Number::from_f64(f)
                .ok_or("nonfinite float")?
                .into());
        }
        if v.is::<ImmutableString>() {
            return Ok(v.clone_cast::<ImmutableString>().to_string().into());
        }
        if v.is::<Array>() {
            return Ok(Value::Array(
                v.clone_cast::<Array>()
                    .iter()
                    .map(|x| convert(x, depth + 1))
                    .collect::<Result<_, _>>()?,
            ));
        }
        if v.is::<Map>() {
            return Ok(Value::Object(
                v.clone_cast::<Map>()
                    .iter()
                    .map(|(k, v)| Ok((k.to_string(), convert(v, depth + 1)?)))
                    .collect::<Result<_, String>>()?,
            ));
        }
        Err(format!("unsupported JSON Dynamic type {}", v.type_name()))
    }
    let value = convert(value, 0)?;
    if serde_json::to_vec(&value).map_err(|e| e.to_string())?.len() > MAX_BYTES {
        return Err("JSON exceeds 1 MiB".into());
    }
    Ok(value)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn exact_json_boundary_and_no_silent_schema_features() {
        for value in [
            json!(null),
            json!({"present":null}),
            json!(i64::MAX),
            json!(1.25),
        ] {
            assert_eq!(from_dynamic(&to_dynamic(&value).unwrap()).unwrap(), value);
        }
        assert!(to_dynamic(&json!(u64::MAX)).is_err());
        assert!(to_dynamic(&json!(9007199254740992.0)).is_err());
        assert!(from_dynamic(&Dynamic::from_float(f64::NAN)).is_err());
        assert!(from_dynamic(&Dynamic::from('x')).is_err());
        for schema in [
            json!({"$ref":"https://invalid/schema"}),
            json!({"format":"email"}),
            json!({"properties":{"x":{"pattern":"x"}}}),
        ] {
            assert!(validator(&schema).is_err());
        }
        let schema = validator(&json!({"type":"object", "required":["x"], "properties":{"x":{"type":"null"}}, "additionalProperties":false})).unwrap();
        assert!(validate(&schema, &json!({"x":null})).is_ok());
        assert!(validate(&schema, &json!({})).is_err());
    }
}
