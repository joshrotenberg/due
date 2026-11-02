use crate::Host;
use rquickjs::{
    Context, Ctx, Exception, Function, IntoJs, Object, Persistent, Runtime, Value,
    promise::{PromiseHookType, PromiseState},
};
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    rc::Rc,
    time::Duration,
};

struct Frame(Option<String>);

impl<'js> IntoJs<'js> for Frame {
    fn into_js(self, ctx: &Ctx<'js>) -> rquickjs::Result<Value<'js>> {
        match self.0 {
            Some(line) => line.into_js(ctx),
            None => Ok(Value::new_null(ctx.clone())),
        }
    }
}

fn exception(ctx: &Ctx<'_>, error: rquickjs::Error) -> String {
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
}

pub(crate) fn run(
    host: Rc<Host>,
    source: &str,
    args: &str,
    async_mode: bool,
) -> Result<serde_json::Value, String> {
    let runtime = Runtime::new().map_err(|e| e.to_string())?;
    runtime.set_memory_limit(32 * 1024 * 1024);
    runtime.set_max_stack_size(512 * 1024);
    let used_promise = Rc::new(Cell::new(false));
    let promises: Rc<RefCell<HashSet<Persistent<Value<'static>>>>> = Rc::default();
    let rejected: Rc<RefCell<HashMap<Persistent<Value<'static>>, String>>> = Rc::default();
    let promise_tracker = Rc::clone(&used_promise);
    let live_promises = Rc::clone(&promises);
    let limits = Rc::clone(&host);
    runtime.set_promise_hook(Some(Box::new(move |ctx, kind, promise, _| {
        promise_tracker.set(true);
        if !async_mode {
            return;
        }
        let saved = Persistent::save(&ctx, promise);
        match kind {
            PromiseHookType::Init => {
                let mut live = live_promises.borrow_mut();
                if live.len() >= 4096 {
                    limits.fail("too many unsettled JavaScript Promises".into());
                } else {
                    live.insert(saved);
                }
            }
            PromiseHookType::Resolve => {
                live_promises.borrow_mut().remove(&saved);
            }
            _ => {}
        }
    })));
    if async_mode {
        let unhandled = Rc::clone(&rejected);
        runtime.set_host_promise_rejection_tracker(Some(Box::new(
            move |ctx, promise, reason, handled| {
                let saved = Persistent::save(&ctx, promise);
                if handled {
                    unhandled.borrow_mut().remove(&saved);
                } else {
                    let detail = reason
                        .as_exception()
                        .and_then(|value| value.message())
                        .or_else(|| reason.as_string().and_then(|value| value.to_string().ok()))
                        .unwrap_or_else(|| format!("{reason:?}"));
                    unhandled.borrow_mut().insert(saved, detail);
                }
            },
        )));
    }
    let interrupt = Rc::clone(&host);
    runtime.set_interrupt_handler(Some(Box::new(move || interrupt.check().is_err())));
    let context = Context::full(&runtime).map_err(|e| e.to_string())?;
    let result = (|| {
        context.with(|ctx| {
            let execute = || -> rquickjs::Result<()> {
                let bridge = Object::new(ctx.clone())?;
                bridge.set("args", args)?;
                let sender = Rc::clone(&host);
                bridge.set(
                    "send",
                    Function::new(ctx.clone(), move |ctx: Ctx<'_>, line: String| {
                        sender
                            .send(&line)
                            .map_err(|e| Exception::throw_message(&ctx, &e))
                    })?,
                )?;
                let receiver = Rc::clone(&host);
                bridge.set(
                    "recv",
                    Function::new(ctx.clone(), move |ctx: Ctx<'_>, ms: f64| {
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
                    Function::new(ctx.clone(), |text: String| eprintln!("{text}"))?,
                )?;
                if async_mode {
                    let executor = Rc::clone(&host);
                    bridge.set(
                        "exec_start",
                        Function::new(ctx.clone(), move |ctx: Ctx<'_>, spec: String| {
                            executor
                                .exec_start(&spec)
                                .map_err(|e| Exception::throw_message(&ctx, &e))
                        })?,
                    )?;
                    let executor = Rc::clone(&host);
                    bridge.set(
                        "exec_poll",
                        Function::new(ctx.clone(), move |ctx: Ctx<'_>, id: u64| {
                            executor
                                .exec_poll(id)
                                .map(Frame)
                                .map_err(|e| Exception::throw_message(&ctx, &e))
                        })?,
                    )?;
                    let executor = Rc::clone(&host);
                    bridge.set(
                        "exec_cancel",
                        Function::new(ctx.clone(), move |ctx: Ctx<'_>, id: u64| {
                            executor
                                .exec_cancel(id)
                                .map_err(|e| Exception::throw_message(&ctx, &e))
                        })?,
                    )?;
                }
                ctx.globals().set("bridge", bridge)?;
                ctx.eval::<(), _>(include_str!("../clients/client.js"))?;
                if async_mode {
                    ctx.eval::<(), _>(include_str!("../clients/async.js"))?;
                }
                let result: Value = ctx.eval(source)?;
                if !async_mode && (used_promise.get() || result.is_promise()) {
                    return Err(Exception::throw_message(
                        &ctx,
                        "async workflows are not supported by this cooperative client spike",
                    ));
                }
                Ok(())
            };
            execute().map_err(|error| exception(&ctx, error))
        })?;
        if !async_mode {
            return Ok(serde_json::Value::Null);
        }
        loop {
            host.check()?;
            // A runaway microtask chain must not starve RPC items or process cancellation.
            for _ in 0..64 {
                match runtime.execute_pending_job() {
                    Ok(true) => {}
                    Ok(false) => break,
                    Err(error) => {
                        return Err(error
                            .0
                            .with(|ctx| exception(&ctx, rquickjs::Error::Exception)));
                    }
                }
            }
            // QuickJS emits Resolve only for fulfillment, not rejection. Reap handled
            // rejections too, including promises whose handlers preceded rejection.
            context.with(|ctx| {
                promises.borrow_mut().retain(|saved| {
                    saved.clone().restore(&ctx).map_or(true, |value| {
                        value
                            .as_promise()
                            .is_some_and(|promise| promise.state() == PromiseState::Pending)
                    })
                });
            });
            let checkpoint = !runtime.is_job_pending();
            let unhandled = if checkpoint {
                rejected.borrow().values().next().cloned()
            } else {
                None
            };
            let state: serde_json::Value = context
                .with(|ctx| {
                    let step = || -> rquickjs::Result<String> {
                        if let Some(detail) = unhandled {
                            let fail: Function = ctx.globals().get("__dueFail")?;
                            fail.call::<_, ()>(
                                (format!("unhandled Promise rejection: {detail}"),),
                            )?;
                        }
                        let tick: Function = ctx.globals().get("__dueTick")?;
                        tick.call::<_, ()>(())?;
                        let state: Function = ctx.globals().get("__dueState")?;
                        state.call(())
                    };
                    step().map_err(|error| exception(&ctx, error))
                })
                .and_then(|value| serde_json::from_str(&value).map_err(|e| e.to_string()))?;
            if state["started"] != true {
                return Err("async workflow must call Due.run".into());
            }
            if state["done"] == true && !runtime.is_job_pending() {
                if let Some(error) = state["error"].as_str() {
                    return Err(format!("JavaScript async: {error}"));
                }
                if !promises.borrow().is_empty() {
                    return Err("JavaScript async workflow left unsettled Promises".into());
                }
                let encoded: String = context.with(|ctx| {
                    let retrieve = || -> rquickjs::Result<String> {
                        let result: Function = ctx.globals().get("__dueResult")?;
                        result.call(())
                    };
                    retrieve().map_err(|error| exception(&ctx, error))
                })?;
                if encoded.len() > crate::MAX_DOCUMENT_BYTES {
                    return Err("workflow result exceeds 1 MiB".into());
                }
                return serde_json::from_str(&encoded)
                    .map_err(|error| format!("workflow result JSON: {error}"));
            }
            std::thread::sleep(Duration::from_millis(1));
        }
    })();
    // Persistent hook values must be released before their QuickJS runtime.
    runtime.set_promise_hook(None);
    runtime.set_host_promise_rejection_tracker(None);
    promises.borrow_mut().clear();
    rejected.borrow_mut().clear();
    result
}
