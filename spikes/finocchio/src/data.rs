//! Owned immutable trees, with no table proxy or user-writable interior.
use mlua::{AnyUserData, Lua, MetaMethod, UserData, UserDataMethods, Value};
use serde_json::Value as Json;

#[derive(Clone)]
pub struct Data(pub(crate) Json);

impl UserData for Data {
    fn add_methods<M: UserDataMethods<Self>>(methods: &mut M) {
        methods.add_meta_method(MetaMethod::Eq, |_, this, other: AnyUserData| {
            Ok(other.borrow::<Self>().is_ok_and(|other| this.0 == other.0))
        });
    }
}

fn err(message: &str) -> mlua::Error {
    mlua::Error::runtime(message)
}
fn owned(value: AnyUserData) -> mlua::Result<Json> {
    Ok(value.borrow::<Data>()?.0.clone())
}

fn scalar(lua: &Lua, value: &Json) -> mlua::Result<Value> {
    Ok(match value {
        Json::Bool(b) => Value::Boolean(*b),
        Json::String(s) => Value::String(lua.create_string(s)?),
        Json::Number(n) => {
            if let Some(i) = n.as_i64() {
                Value::Integer(i)
            // Conservative spelling check, not exact decimal-value equality:
            // reject noncanonical spellings even when the value fits a float.
            } else if let Some(float) = n.as_f64().filter(|v| v.is_finite())
                && serde_json::Number::from_f64(float).as_ref() == Some(n)
            {
                Value::Number(float)
            } else {
                return Err(err(
                    "number is out of range or not canonical Lua float spelling; keep as data",
                ));
            }
        }
        _ => {
            return Err(err(
                "scalar expects boolean, string or representable number",
            ));
        }
    })
}

pub fn install(lua: &Lua) -> mlua::Result<()> {
    let api = lua.create_table()?;
    api.set(
        "pack",
        lua.create_function(|_, value: Value| from_lua(value).map(Data))?,
    )?;
    for kind in ["array", "object"] {
        api.set(
            kind,
            lua.create_function(move |_, table: mlua::Table| {
                let value = convert(Value::Table(table), Some(kind), &mut Vec::new(), 0, &mut 0)?;
                check(&value)?;
                Ok(Data(value))
            })?,
        )?;
    }
    api.set(
        "parse",
        lua.create_function(|_, source: String| {
            if source.len() > 1024 * 1024 {
                return Err(err("JSON input exceeds 1 MiB"));
            }
            serde_json::from_str::<Json>(&source)
                .map(Data)
                .map_err(mlua::Error::external)
        })?,
    )?;
    api.set(
        "json",
        lua.create_function(|_, data: AnyUserData| Ok(owned(data)?.to_string()))?,
    )?;
    api.set(
        "kind",
        lua.create_function(|_, data: AnyUserData| {
            Ok(match owned(data)? {
                Json::Null => "null",
                Json::Bool(_) => "boolean",
                Json::Number(_) => "number",
                Json::String(_) => "string",
                Json::Array(_) => "array",
                Json::Object(_) => "map",
            })
        })?,
    )?;
    api.set(
        "get",
        lua.create_function(|lua, (data, key): (AnyUserData, Value)| {
            let value = owned(data)?;
            let found = match (&value, key) {
                (Json::Object(map), Value::String(key)) => map.get(key.to_str()?.as_ref()),
                (Json::Array(array), Value::Integer(index)) if index > 0 => {
                    array.get((index - 1) as usize)
                }
                _ => return Err(err("get expects map/string or array/positive integer")),
            };
            match found {
                Some(value) => Ok(Value::UserData(lua.create_userdata(Data(value.clone()))?)),
                None => Ok(Value::Nil),
            }
        })?,
    )?;
    api.set(
        "scalar",
        lua.create_function(|lua, data: AnyUserData| scalar(lua, &owned(data)?))?,
    )?;
    api.set(
        "len",
        lua.create_function(|_, data: AnyUserData| match owned(data)? {
            Json::Array(v) => Ok(v.len()),
            Json::Object(v) => Ok(v.len()),
            _ => Err(err("len expects array or map")),
        })?,
    )?;
    api.set(
        "iter",
        lua.create_function(|lua, data: AnyUserData| {
            let items: Vec<(Json, Json)> = match owned(data)? {
                Json::Array(v) => v
                    .into_iter()
                    .enumerate()
                    .map(|(i, v)| ((i + 1).into(), v))
                    .collect(),
                Json::Object(v) => v.into_iter().map(|(k, v)| (k.into(), v)).collect(),
                _ => return Err(err("iter expects array or map")),
            };
            let mut items = items.into_iter();
            lua.create_function_mut(move |lua, _: mlua::MultiValue| match items.next() {
                Some((key, value)) => Ok((
                    scalar(lua, &key)?,
                    Value::UserData(lua.create_userdata(Data(value))?),
                )),
                None => Ok((Value::Nil, Value::Nil)),
            })
        })?,
    )?;
    lua.globals().set("data", api)
}

/// No serde-to-Lua conversion: arbitrary_precision's private number encoding
/// must never become a native table. Empty native tables require an explicit tag.
pub fn from_lua(value: Value) -> mlua::Result<Json> {
    let value = convert(value, None, &mut Vec::new(), 0, &mut 0)?;
    check(&value)?;
    Ok(value)
}

pub fn check(value: &Json) -> mlua::Result<()> {
    fn depth(value: &Json, n: usize) -> mlua::Result<()> {
        if n > 32 {
            return Err(err("JSON nesting exceeds 32"));
        }
        match value {
            Json::Array(a) => {
                for v in a {
                    depth(v, n + 1)?;
                }
            }
            Json::Object(o) => {
                for v in o.values() {
                    depth(v, n + 1)?;
                }
            }
            _ => (),
        }
        Ok(())
    }
    depth(value, 0)?;
    if value.to_string().len() > 1024 * 1024 {
        return Err(err("JSON exceeds 1 MiB"));
    }
    Ok(())
}

fn charge(budget: &mut usize, bytes: usize) -> mlua::Result<()> {
    *budget = budget.saturating_add(bytes);
    if *budget > 1024 * 1024 {
        return Err(err("JSON conversion exceeds 1 MiB"));
    }
    Ok(())
}
fn convert(
    value: Value,
    tag: Option<&str>,
    seen: &mut Vec<usize>,
    depth: usize,
    budget: &mut usize,
) -> mlua::Result<Json> {
    if depth > 32 {
        return Err(err("JSON nesting exceeds 32"));
    }
    charge(budget, 1)?;
    Ok(match value {
        Value::UserData(u) => {
            let value = u.borrow::<Data>()?;
            charge(budget, value.0.to_string().len())?;
            value.0.clone()
        }
        Value::Boolean(v) => v.into(),
        Value::String(v) => {
            charge(budget, v.as_bytes().len())?;
            v.to_str()?.to_string().into()
        }
        Value::Integer(v) => v.into(),
        Value::Number(v) => serde_json::Number::from_f64(v)
            .ok_or_else(|| err("non-finite JSON number"))?
            .into(),
        Value::Table(t) => {
            if t.metatable().is_some() {
                return Err(err("JSON table must have no metatable"));
            }
            let id = t.to_pointer() as usize;
            if seen.contains(&id) {
                return Err(err("cyclic JSON table"));
            }
            seen.push(id);
            let pairs = t
                .pairs::<Value, Value>()
                .collect::<mlua::Result<Vec<_>>>()?;
            if pairs.len() > 65536 {
                return Err(err("JSON table exceeds 65536 entries"));
            }
            let array = tag == Some("array")
                || (tag.is_none()
                    && !pairs.is_empty()
                    && pairs.iter().all(|(k, _)| matches!(k, Value::Integer(_))));
            if pairs.is_empty() && tag.is_none() {
                return Err(err("empty table ambiguous; use data.array or data.object"));
            }
            let result = if array {
                let mut a = vec![Json::Null; pairs.len()];
                for (k, v) in pairs {
                    let Value::Integer(k) = k else {
                        return Err(err("array key must be an integer"));
                    };
                    if k <= 0 || k as usize > a.len() {
                        return Err(err("array must be dense"));
                    }
                    a[k as usize - 1] = convert(v, None, seen, depth + 1, budget)?;
                }
                Json::Array(a)
            } else {
                let mut o = serde_json::Map::new();
                for (k, v) in pairs {
                    let Value::String(k) = k else {
                        return Err(err("object key must be a string"));
                    };
                    charge(budget, k.as_bytes().len())?;
                    o.insert(
                        k.to_str()?.to_string(),
                        convert(v, None, seen, depth + 1, budget)?,
                    );
                }
                Json::Object(o)
            };
            seen.pop();
            result
        }
        _ => {
            return Err(err(
                "expected JSON value; nil, functions, threads and foreign userdata are not JSON",
            ));
        }
    })
}
