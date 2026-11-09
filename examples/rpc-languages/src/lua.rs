use crate::Host;
use mlua::{Function, Table};
use std::rc::Rc;

pub(crate) fn run(
    host: Rc<Host>,
    source: &str,
    args: &str,
    async_mode: bool,
) -> Result<(), String> {
    execute(host, source, args, async_mode).map_err(|e| format!("Lua: {e}"))
}

fn execute(host: Rc<Host>, source: &str, args: &str, async_mode: bool) -> mlua::Result<()> {
    let check = Rc::clone(&host);
    let lua = due_harness::create_lua_runtime(move || check.check())?;
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
    if async_mode {
        let executor = Rc::clone(&host);
        bridge.set(
            "exec_start",
            lua.create_function(move |_, spec: String| {
                executor.exec_start(&spec).map_err(mlua::Error::external)
            })?,
        )?;
        let executor = Rc::clone(&host);
        bridge.set(
            "exec_poll",
            lua.create_function(move |_, id: u64| {
                executor.exec_poll(id).map_err(mlua::Error::external)
            })?,
        )?;
        let executor = Rc::clone(&host);
        bridge.set(
            "exec_cancel",
            lua.create_function(move |_, id: u64| {
                executor.exec_cancel(id).map_err(mlua::Error::external)
            })?,
        )?;
    }
    lua.globals().set("bridge", bridge)?;
    lua.load(include_str!("../clients/client.lua"))
        .set_name("client.lua")
        .exec()?;
    if async_mode {
        lua.load(include_str!("../clients/async.lua"))
            .set_name("async.lua")
            .exec()?;
    }
    lua.load(source).set_name("workflow.lua").exec()?;
    if async_mode {
        let due: Table = lua.globals().get("Due")?;
        let step: Function = due.get("_step")?;
        loop {
            host.check().map_err(mlua::Error::external)?;
            if step.call::<bool>(())? {
                break;
            }
            // The background Rust reader remains active between bounded guest ticks.
            std::thread::sleep(std::time::Duration::from_millis(1));
        }
    }
    Ok(())
}
