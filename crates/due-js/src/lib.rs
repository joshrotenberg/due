//! Optional embedded JavaScript harness for the public due-core engine.

use std::rc::Rc;

pub use due_harness::core::Options;
use due_harness::{
    core::{self, Host},
    evaluate_js_runtime,
};
use rquickjs::{Ctx, Exception, Function, Object};
use serde_json::Value;

/// Run one workflow on a synchronous owner thread, outside a Tokio executor.
/// The return value is available after the engine and its workers settle.
pub fn run(options: Options) -> Result<Value, String> {
    core::run(options, |host, source, args| {
        let check = Rc::clone(host);
        let fail = Rc::clone(host);
        evaluate_js_runtime(
            source,
            true,
            move || check.check(),
            move |message| fail.fail(message),
            |ctx| install(ctx, host, args),
        )
    })
}

fn install<'js>(ctx: &Ctx<'js>, host: &Rc<Host>, args: &str) -> rquickjs::Result<()> {
    let bridge = Object::new(ctx.clone())?;
    bridge.set("args", args)?;
    let start = Rc::clone(host);
    bridge.set(
        "start",
        Function::new(ctx.clone(), move |ctx: Ctx<'_>, request: String| {
            start
                .start(&request)
                .map_err(|error| Exception::throw_message(&ctx, &error))
        })?,
    )?;
    let poll = Rc::clone(host);
    bridge.set(
        "poll",
        Function::new(ctx.clone(), move |ctx: Ctx<'_>| {
            poll.poll()
                .map_err(|error| Exception::throw_message(&ctx, &error))
        })?,
    )?;
    bridge.set(
        "log",
        Function::new(ctx.clone(), |text: String| eprintln!("{text}"))?,
    )?;
    ctx.globals().set("__dueCore", bridge)?;
    ctx.eval::<(), _>(include_str!("bootstrap.js"))?;
    ctx.globals().remove("__dueCore")?;
    Ok(())
}
