use crate::Host;
use mlua::{HookTriggers, Lua, LuaOptions, LuaSerdeExt, StdLib, Table, Value, VmState};
use serde_json::{Map, Value as Json};
use std::{collections::HashSet, rc::Rc};

pub(crate) fn run(host: Rc<Host>, source: &str, args: &str) -> Result<(), String> {
    execute(host, source, args).map_err(|e| format!("Lua: {e}"))
}

fn execute(host: Rc<Host>, source: &str, args: &str) -> mlua::Result<()> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE,
        LuaOptions::default(),
    )?;
    lua.set_memory_limit(32 * 1024 * 1024)?;
    for name in ["dofile", "loadfile"] {
        lua.globals().set(name, Value::Nil)?;
    }
    let check = Rc::clone(&host);
    lua.set_global_hook(
        HookTriggers::new().every_nth_instruction(10000),
        move |_, _| {
            check.check().map_err(mlua::Error::external)?;
            Ok(VmState::Continue)
        },
    )?;
    let bridge = lua.create_table()?;
    bridge.set("args", args)?;
    let sender = Rc::clone(&host);
    bridge.set(
        "send",
        lua.create_function(move |_, line: String| {
            sender.send(&line).map_err(mlua::Error::external)
        })?,
    )?;
    let receiver = Rc::clone(&host);
    bridge.set(
        "recv",
        lua.create_function(move |_, ms: u64| receiver.recv(ms).map_err(mlua::Error::external))?,
    )?;
    let clock = Rc::clone(&host);
    bridge.set(
        "now_ms",
        lua.create_function(move |_, ()| Ok(clock.now_ms()))?,
    )?;
    bridge.set(
        "log",
        lua.create_function(|_, text: String| {
            eprintln!("{text}");
            Ok(())
        })?,
    )?;
    lua.globals().set("bridge", bridge)?;
    install_json(&lua)?;
    lua.load(include_str!("../clients/client.lua"))
        .set_name("client.lua")
        .exec()?;
    lua.load(source).set_name("workflow.lua").exec()?;
    Ok(())
}

fn install_json(lua: &Lua) -> mlua::Result<()> {
    let json = lua.create_table()?;
    let object_mt = lua.create_table()?;
    let array_mt = lua.array_metatable();
    json.set("null", lua.null())?;
    json.set(
        "decode",
        lua.create_function(|lua, raw: String| {
            let value: Json = serde_json::from_str(&raw).map_err(mlua::Error::external)?;
            lua.to_value(&value)
        })?,
    )?;
    let arrays = array_mt.clone();
    json.set(
        "array",
        lua.create_function(move |lua, table: Option<Table>| {
            let table = table.unwrap_or(lua.create_table()?);
            table.set_metatable(Some(arrays.clone()))?;
            Ok(table)
        })?,
    )?;
    let objects = object_mt.clone();
    json.set(
        "object",
        lua.create_function(move |lua, table: Option<Table>| {
            let table = table.unwrap_or(lua.create_table()?);
            table.set_metatable(Some(objects.clone()))?;
            Ok(table)
        })?,
    )?;
    json.set(
        "encode",
        lua.create_function(move |_, value: Value| {
            let mut encoder = Encoder {
                arrays: &array_mt,
                objects: &object_mt,
                visiting: HashSet::new(),
                remaining: 100_000,
            };
            let value = encoder.convert(value, 0)?;
            serde_json::to_string(&value).map_err(mlua::Error::external)
        })?,
    )?;
    lua.globals().set("json", json)
}

struct Encoder<'a> {
    arrays: &'a Table,
    objects: &'a Table,
    visiting: HashSet<usize>,
    remaining: usize,
}

impl Encoder<'_> {
    fn convert(&mut self, value: Value, depth: usize) -> mlua::Result<Json> {
        let invalid = |message| mlua::Error::external(message);
        if depth > 64 || self.remaining == 0 {
            return Err(invalid("JSON value exceeds depth/node limit"));
        }
        self.remaining -= 1;
        match value {
            Value::Boolean(v) => Ok(Json::Bool(v)),
            Value::Integer(v) => Ok(Json::from(v)),
            Value::Number(v)
                if v.is_finite() && (v.fract() != 0.0 || v.abs() <= 9_007_199_254_740_991.0) =>
            {
                Ok(Json::from(v))
            }
            Value::String(v) => Ok(Json::String(v.to_str()?.to_owned())),
            Value::LightUserData(v) if v.0.is_null() => Ok(Json::Null),
            Value::Table(table) => {
                let key = table.to_pointer() as usize;
                if !self.visiting.insert(key) {
                    return Err(invalid("cyclic JSON table"));
                }
                let mt = table.metatable();
                let is_array = mt
                    .as_ref()
                    .is_some_and(|m| m.to_pointer() == self.arrays.to_pointer());
                if mt
                    .as_ref()
                    .is_some_and(|m| !is_array && m.to_pointer() != self.objects.to_pointer())
                {
                    return Err(invalid("unexpected JSON table metatable"));
                }
                let result = if is_array {
                    let len = table.raw_len();
                    let mut count = 0;
                    for pair in table.clone().pairs::<Value, Value>() {
                        let (index, _) = pair?;
                        if !matches!(index, Value::Integer(i) if i > 0 && i as usize <= len) {
                            return Err(invalid(
                                "JSON arrays must be dense and contain only sequence keys",
                            ));
                        }
                        count += 1;
                    }
                    if count != len {
                        return Err(invalid("sparse JSON array"));
                    }
                    let mut entries = Vec::with_capacity(len);
                    for i in 1..=len {
                        entries.push(self.convert(table.raw_get(i)?, depth + 1)?);
                    }
                    Json::Array(entries)
                } else {
                    let mut entries = Map::new();
                    for pair in table.pairs::<Value, Value>() {
                        let (key, value) = pair?;
                        let Value::String(key) = key else {
                            return Err(invalid(
                                "JSON object keys must be strings; use json.array for arrays",
                            ));
                        };
                        entries.insert(key.to_str()?.to_owned(), self.convert(value, depth + 1)?);
                    }
                    Json::Object(entries)
                };
                self.visiting.remove(&key);
                Ok(result)
            }
            _ => Err(invalid("unsupported JSON value (use json.null for null)")),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn json_preserves_shapes_and_signed_integers_and_rejects_loss() {
        let lua = Lua::new();
        install_json(&lua).unwrap();
        lua.load(
            r#"
            local v = json.decode('{"n":null,"a":[],"o":{},"i":9007199254740993}')
            assert(v.n == json.null and v.i == 9007199254740993)
            assert(json.encode(v) == '{"a":[],"i":9007199254740993,"n":null,"o":{}}')
            assert(json.encode(json.array({json.null})) == '[null]')
            assert(not pcall(json.encode, json.array({[1]=1,[3]=3})))
            assert(not pcall(json.encode, json.array({a=1})))
            assert(not pcall(json.encode, {1,2}))
            local cycle = {}; cycle.self = cycle
            assert(not pcall(json.encode, cycle))
            assert(not pcall(json.encode, 0/0))
            assert(not pcall(json.encode, function() end))
        "#,
        )
        .exec()
        .unwrap();
    }
}
