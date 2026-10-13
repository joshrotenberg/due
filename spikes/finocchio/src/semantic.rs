//! Bounded prototype; the source scope model is in the Lua module.
use crate::compiler::Compiler;
use mlua::{Function, LuaSerdeExt};
use serde_json::Value;
use std::collections::BTreeMap;

pub type Sources = BTreeMap<String, String>;

/// All operations return JSON; rename returns candidate sources, never writes.
/// Byte offsets are zero-based UTF-8 offsets, ends exclusive.
pub fn request(sources: &Sources, operation: Value) -> mlua::Result<Value> {
    if sources.len() > 32
        || sources.values().any(|s| s.len() > 128 * 1024)
        || sources.values().map(String::len).sum::<usize>() > 512 * 1024
    {
        return Err(mlua::Error::runtime(
            "semantic input exceeds 32 modules, 128 KiB/file or 512 KiB total",
        ));
    }
    let compiler = Compiler::new()?;
    let driver: Function = compiler
        .lua
        .load(include_str!("../lua/semantic.lua"))
        .eval()?;
    // serde_json's arbitrary_precision number wrapper is not a Lua number.
    // Admit the small request schema explicitly instead of serializing that IR.
    let request = compiler.lua.create_table()?;
    for key in ["op", "file", "name"] {
        if let Some(value) = operation.get(key).and_then(Value::as_str) {
            request.set(key, value)?;
        }
    }
    if let Some(offset) = operation.get("offset").and_then(Value::as_u64) {
        request.set("offset", offset)?;
    }
    let answer = driver.call((
        compiler.fennel.clone(),
        compiler.lua.to_value(sources)?,
        request,
        compiler.lua.array_metatable(),
    ))?;
    compiler.lua.from_value(answer)
}
