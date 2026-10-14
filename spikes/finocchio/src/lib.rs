//! Isolated language experiment. No Due runtime changes or ambient loaders.
pub mod compiler;
pub mod data;
pub mod semantic;
#[cfg(unix)]
pub mod workflow;

use mlua::{Lua, LuaOptions, StdLib, Value};

pub(crate) fn bare_vm() -> mlua::Result<Lua> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )?;
    lua.set_memory_limit(32 * 1024 * 1024)?;
    for name in ["dofile", "loadfile", "print", "warn", "collectgarbage"] {
        lua.globals().set(name, Value::Nil)?;
    }
    Ok(lua)
}

pub fn runtime() -> mlua::Result<Lua> {
    let lua = bare_vm()?;
    lua.globals().set("load", Value::Nil)?;
    data::install(&lua)?;
    let contract: mlua::Table = lua.load(include_str!("../lua/contract.lua")).eval()?;
    lua.globals().set("contract", contract)?;
    Ok(lua)
}
