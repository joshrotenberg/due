// Narrow integrated VM controls adapted from root src/workflow/lua.rs.
use mlua::{Error, Function, Lua, LuaOptions, MultiValue, StdLib, Table, Value};
fn err(e: impl ToString) -> Error {
    Error::runtime(e.to_string())
}
pub(super) fn vm() -> mlua::Result<Lua> {
    // Omit user-created coroutines and C functions that can call arbitrary Lua
    // callbacks across a non-yieldable C boundary. The host's async coroutine is
    // host manages every continuation. Hook suspension is uncatchable by pcall.
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )?;
    lua.set_memory_limit(32 * 1024 * 1024)?;
    crate::data::install(&lua)?;
    let contract: Table = lua
        .load(include_str!("../../lua/contract.lua"))
        .set_name("@finocchio/contract.lua")
        .eval()?;
    lua.globals().set("contract", contract)?;
    let globals = lua.globals();
    for name in [
        "xpcall",
        "dofile",
        "loadfile",
        "load",
        "collectgarbage",
        "setmetatable",
        "getmetatable",
        "print",
        "warn",
    ] {
        globals.set(name, Value::Nil)?;
    }
    let string: Table = globals.get("string")?;
    for name in ["dump", "gsub", "gmatch", "match", "find"] {
        string.set(name, Value::Nil)?;
    }
    let repeat: Function = string.get("rep")?;
    string.set(
        "rep",
        lua.create_function(
            move |lua, (text, count, separator): (mlua::LuaString, i64, Option<mlua::LuaString>)| {
                // Lua 5.4 loops count times even when every copy is empty, so neither
                // its output allocation nor our allocator ceiling bounds that case.
                if text.as_bytes().is_empty()
                    && separator.as_ref().is_none_or(|s| s.as_bytes().is_empty())
                {
                    return lua.create_string("");
                }
                repeat.call::<mlua::LuaString>((text, count, separator))
            },
        )?,
    )?;
    let table: Table = globals.get("table")?;
    table.set("sort", Value::Nil)?;
    // These C loops do not execute VM instructions. Sparse integer keys can
    // produce an enormous length without allocating an enormous table.
    for name in ["insert", "remove"] {
        let original: Function = table.get(name)?;
        table.set(
            name,
            lua.create_function(move |_, args: MultiValue| {
                if let Some(Value::Table(t)) = args.front() {
                    let limit = if name == "insert" { 4095 } else { 4096 };
                    if t.raw_len() > limit {
                        return Err(err("table operation exceeds 4096 items"));
                    }
                }
                original.call::<Value>(args)
            })?,
        )?;
    }
    let original: Function = table.get("move")?;
    table.set(
        "move",
        lua.create_function(
            move |_,
                  (source, first, last, target, destination): (
                Table,
                i64,
                i64,
                i64,
                Option<Table>,
            )| {
                if last >= first
                    && last
                        .checked_sub(first)
                        .and_then(|n| n.checked_add(1))
                        .is_none_or(|n| n > 4096)
                {
                    return Err(err("table.move exceeds 4096 items"));
                }
                original.call::<Table>((source, first, last, target, destination))
            },
        )?,
    )?;
    Ok(lua)
}
