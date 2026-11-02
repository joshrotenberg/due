use crate::Host;
use rquickjs::{Context, Ctx, Exception, Function, IntoJs, Object, Runtime, Value};
use std::{cell::Cell, rc::Rc};

struct Frame(Option<String>);

impl<'js> IntoJs<'js> for Frame {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<Value<'js>> {
        match self.0 {
            Some(line) => line.into_js(ctx),
            None => Ok(Value::new_null(ctx.clone())),
        }
    }
}

pub(crate) fn run(host: Rc<Host>, source: &str, args: &str) -> Result<(), String> {
    let runtime = Runtime::new().map_err(|e| e.to_string())?;
    runtime.set_memory_limit(32 * 1024 * 1024);
    runtime.set_max_stack_size(512 * 1024);
    // This slice has no job scheduler. Detect even a detached/rejected Promise
    // whose creation leaves no pending job and whose value the caller discards.
    let used_promise = Rc::new(Cell::new(false));
    let promise_tracker = Rc::clone(&used_promise);
    runtime.set_promise_hook(Some(Box::new(move |_, _, _, _| {
        promise_tracker.set(true);
    })));
    let interrupt = Rc::clone(&host);
    runtime.set_interrupt_handler(Some(Box::new(move || interrupt.check().is_err())));
    let context = Context::full(&runtime).map_err(|e| e.to_string())?;
    context.with(|ctx| {
        let execute = || -> rquickjs::Result<()> {
            let bridge = Object::new(ctx.clone())?;
            bridge.set("args", args)?;
            let sender = Rc::clone(&host);
            bridge.set(
                "send",
                Function::new(ctx.clone(), move |ctx: rquickjs::Ctx<'_>, line: String| {
                    sender
                        .send(&line)
                        .map_err(|e| Exception::throw_message(&ctx, &e))
                })?,
            )?;
            let receiver = Rc::clone(&host);
            bridge.set(
                "recv",
                Function::new(ctx.clone(), move |ctx: rquickjs::Ctx<'_>, ms: f64| {
                    if !ms.is_finite() || ms.fract() != 0.0 || !(0.0..=10000.0).contains(&ms) {
                        return Err(Exception::throw_message(
                            &ctx,
                            "receive timeout must be an integer in 0..10000",
                        ));
                    }
                    receiver
                        .recv(ms as u64)
                        .map(Frame)
                        .map_err(|e| Exception::throw_message(&ctx, &e))
                })?,
            )?;
            let clock = Rc::clone(&host);
            bridge.set(
                "now_ms",
                Function::new(ctx.clone(), move || clock.now_ms())?,
            )?;
            bridge.set(
                "log",
                Function::new(ctx.clone(), |text: String| {
                    eprintln!("{text}");
                })?,
            )?;
            ctx.globals().set("bridge", bridge)?;
            ctx.eval::<(), _>(include_str!("../clients/client.js"))?;
            let result: Value = ctx.eval(source)?;
            if used_promise.get() || result.is_promise() {
                return Err(Exception::throw_message(
                    &ctx,
                    "async workflows are not supported by this cooperative client spike",
                ));
            }
            Ok(())
        };
        execute().map_err(|error| {
            if error.is_exception() {
                let caught = ctx.catch();
                if let Some(exception) = caught.as_exception() {
                    format!(
                        "JavaScript: {}\n{}",
                        exception.message().unwrap_or_default(),
                        exception.stack().unwrap_or_default()
                    )
                } else {
                    format!("JavaScript exception: {caught:?}")
                }
            } else {
                format!("JavaScript: {error}")
            }
        })
    })
}
