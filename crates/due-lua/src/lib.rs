//! Optional embedded Lua harness for the public due-core engine.

use std::{rc::Rc, thread, time::Duration};

pub use due_harness::core::Options;
use due_harness::{
    MAX_DOCUMENT_BYTES,
    core::{self, Host},
    create_lua_runtime,
};
use mlua::Function;
use serde_json::Value;

/// Run one workflow on a synchronous owner thread, outside a Tokio executor.
/// The return value is available after the engine and its workers settle.
pub fn run(options: Options) -> Result<Value, String> {
    core::run(options, |host, source, args| {
        evaluate(host, source, args).map_err(|error| format!("Lua: {error}"))
    })
}

fn evaluate(host: &Rc<Host>, source: &str, args: &str) -> mlua::Result<Value> {
    let check = Rc::clone(host);
    let lua = create_lua_runtime(move || check.check())?;
    let bridge = lua.create_table()?;
    bridge.set("args", args)?;
    let start = Rc::clone(host);
    bridge.set(
        "start",
        lua.create_function(move |_, request: String| {
            start.start(&request).map_err(mlua::Error::external)
        })?,
    )?;
    let poll = Rc::clone(host);
    bridge.set(
        "poll",
        lua.create_function(move |_, ()| poll.poll().map_err(mlua::Error::external))?,
    )?;
    bridge.set(
        "log",
        lua.create_function(|_, text: String| {
            eprintln!("{text}");
            Ok(())
        })?,
    )?;
    let install = lua
        .load(include_str!("bootstrap.lua"))
        .set_name("due-lua")
        .into_function()?;
    let step = install.call::<Function>(bridge)?;
    lua.load(source).set_name("workflow.lua").exec()?;
    loop {
        host.check().map_err(mlua::Error::external)?;
        if let Some(result) = step.call::<Option<mlua::LuaString>>(())? {
            let bytes = result.as_bytes();
            if bytes.len() > MAX_DOCUMENT_BYTES {
                return Err(mlua::Error::external("workflow result exceeds 1 MiB"));
            }
            return serde_json::from_slice(&bytes).map_err(mlua::Error::external);
        }
        thread::sleep(Duration::from_millis(1));
    }
}
