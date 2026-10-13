//! Owned immutable trees, with no table proxy or user-writable interior.
use mlua::{AnyUserData, Lua, MetaMethod, UserData, UserDataMethods, Value};
use serde_json::Value as Json;

#[derive(Clone)]
pub struct Data(Json);

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
