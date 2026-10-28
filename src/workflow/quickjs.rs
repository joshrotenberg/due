//! Experimental JavaScript adapter over the same Rust-owned job supervisor.
use super::{
    bundle::Bundle,
    contract,
    host::{RunHandle, SharedHost},
};
use rquickjs::{
    AsyncContext, AsyncRuntime, Context, Ctx, Error, Exception, Function, JsLifetime, Module,
    Object, Persistent, Promise, Runtime, Value,
    function::This,
    loader::{BuiltinLoader, ImportAttributes, Resolver},
};
use serde_json::{Value as Json, json};
use std::{
    cell::RefCell,
    collections::{BTreeMap, HashSet},
    rc::Rc,
    sync::{
        Arc, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::Duration,
};

const MEMORY: usize = 16 * 1024 * 1024;
const BUDGET: u64 = 100_000;
struct Names(BTreeMap<String, String>);
impl Resolver for Names {
    fn resolve<'js>(
        &mut self,
        _: &Ctx<'js>,
        base: &str,
        name: &str,
        _: Option<ImportAttributes<'js>>,
    ) -> rquickjs::Result<String> {
        self.0
            .get(name)
            .cloned()
            .ok_or_else(|| Error::new_resolving(base, name))
    }
}
fn loader(bundle: &Bundle) -> (Names, BuiltinLoader) {
    let mut names = BTreeMap::new();
    let mut loader = BuiltinLoader::default();
    for (key, source) in &bundle.modules {
        let filename = bundle.module_names[key].clone();
        names.insert(key.clone(), filename.clone());
        loader.add_module(filename, source.clone());
    }
    (Names(names), loader)
}
// QuickJS stores its native Error stack behind a built-in getter. Capture that
// exact function before evaluating user code; calling it directly reads only the
// engine's Error data, bypassing overridden stack accessors and toString hooks.
struct StackGetter<'js>(Function<'js>);
unsafe impl<'js> JsLifetime<'js> for StackGetter<'js> {
    type Changed<'to> = StackGetter<'to>;
}
fn capture_stack_getter(ctx: &Ctx<'_>) -> rquickjs::Result<()> {
    let getter = ctx.eval("Object.getOwnPropertyDescriptor(Error.prototype, 'stack').get")?;
    ctx.store_userdata(StackGetter(getter))
        .map_err(|_| Error::Unknown)?;
    Ok(())
}
fn native_stack<'js>(ctx: &Ctx<'js>, thrown: &Value<'js>) -> Option<String> {
    let getter = ctx.userdata::<StackGetter>()?;
    let stack: Value = getter.0.call((This(thrown.clone()),)).ok()?;
    stack
        .as_string()?
        .to_string()
        .ok()
        .map(|s| s.chars().take(4096).collect())
}
// Read own data properties through the C API: formatting an error must not
// execute a user-defined message/stack getter or toString and admit more jobs.
fn own_string<'js>(ctx: &Ctx<'js>, value: &Value<'js>, key: &str) -> Option<String> {
    unsafe {
        let atom = rquickjs::qjs::JS_NewAtomLen(
            ctx.as_raw().as_ptr(),
            key.as_ptr().cast(),
            key.len() as _,
        );
        let mut descriptor = std::mem::MaybeUninit::<rquickjs::qjs::JSPropertyDescriptor>::uninit();
        let found = rquickjs::qjs::JS_GetOwnProperty(
            ctx.as_raw().as_ptr(),
            descriptor.as_mut_ptr(),
            value.as_raw(),
            atom,
        );
        rquickjs::qjs::JS_FreeAtom(ctx.as_raw().as_ptr(), atom);
        if found != 1 {
            return None;
        }
        let descriptor = descriptor.assume_init();
        // GetOwnProperty returns owned references, including getter and setter.
        let data = Value::from_raw(ctx.clone(), descriptor.value);
        let _getter = Value::from_raw(ctx.clone(), descriptor.getter);
        let _setter = Value::from_raw(ctx.clone(), descriptor.setter);
        if descriptor.flags & rquickjs::qjs::JS_PROP_GETSET as i32 != 0 {
            return None;
        }
        data.as_string()?
            .to_string()
            .ok()
            .map(|s| s.chars().take(4096).collect())
    }
}
fn diagnostic(ctx: &Ctx<'_>, error: Error) -> String {
    if !error.is_exception() {
        return error.to_string();
    }
    let thrown = ctx.catch();
    if unsafe { rquickjs::qjs::JS_IsProxy(thrown.as_raw()) } {
        return "JavaScript threw a Proxy (diagnostic access suppressed)".into();
    }
    if thrown.is_object() {
        let message =
            own_string(ctx, &thrown, "message").unwrap_or_else(|| "JavaScript exception".into());
        let stack = native_stack(ctx, &thrown)
            .or_else(|| own_string(ctx, &thrown, "stack"))
            .unwrap_or_default();
        return format!("{message}\n{stack}");
    }
    if let Some(text) = thrown.as_string() {
        return text
            .to_string()
            .unwrap_or_else(|_| "JavaScript exception".into());
    }
    format!("JavaScript exception: {thrown:?}")
}
pub fn check(bundle: &Bundle) -> Result<(), String> {
    let runtime = Runtime::new().map_err(|e| e.to_string())?;
    runtime.set_memory_limit(MEMORY);
    runtime.set_max_stack_size(512 * 1024);
    let (resolver, loader) = loader(bundle);
    runtime.set_loader(resolver, loader);
    let ctx = Context::full(&runtime).map_err(|e| e.to_string())?;
    ctx.with(|ctx| {
        capture_stack_getter(&ctx).map_err(|e| diagnostic(&ctx, e))?;
        for (name, source) in &bundle.modules {
            Module::declare(
                ctx.clone(),
                bundle.module_names[name].clone(),
                source.clone(),
            )
            .map_err(|e| diagnostic(&ctx, e))?;
        }
        Module::declare(ctx.clone(), bundle.entry_name.clone(), bundle.entry.clone())
            .map_err(|e| diagnostic(&ctx, e))?;
        Ok(())
    })
}
fn safe_json(v: &Json) -> Result<(), String> {
    contract::check_value(v)?;
    fn visit(v: &Json) -> Result<(), String> {
        match v {
            Json::Number(n)
                if n.as_f64()
                    .is_some_and(|f| f.fract() == 0.0 && f.abs() > 9_007_199_254_740_991.0) =>
            {
                Err("JavaScript JSON integer exceeds safe integer range".into())
            }
            Json::Array(a) => a.iter().try_for_each(visit),
            Json::Object(o) => o.values().try_for_each(visit),
            _ => Ok(()),
        }
    }
    visit(v)
}
fn js_error(ctx: &Ctx<'_>, message: impl AsRef<str>) -> Error {
    Exception::throw_message(ctx, message.as_ref())
}
struct State {
    scopes: Vec<SharedHost>,
    handles: Vec<RunHandle>,
}
fn install<'js>(ctx: &Ctx<'js>, host: SharedHost) -> rquickjs::Result<Object<'js>> {
    let state = Rc::new(RefCell::new(State {
        scopes: vec![host.clone()],
        handles: vec![],
    }));
    let sync_state = state.clone();
    let sync_host = host.clone();
    let call = Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>,
              scope: usize,
              op: String,
              payload: String|
              -> rquickjs::Result<String> {
            let h = sync_state
                .borrow()
                .scopes
                .get(scope)
                .cloned()
                .ok_or_else(|| js_error(&ctx, "unknown scope"))?;
            let result = (|| -> Result<Json, String> {
                let data: Json = serde_json::from_str(&payload).map_err(|e| e.to_string())?;
                safe_json(&data)?;
                match op.as_str() {
                    "fail" => Err(data.as_str().unwrap_or("JavaScript failure").to_owned()),
                    "phase" => {
                        h.phase(data.as_str().ok_or("phase must be string")?)?;
                        Ok(Json::Null)
                    }
                    "log" => {
                        h.log(data.as_str().ok_or("log must be string")?)?;
                        Ok(Json::Null)
                    }
                    "start" => {
                        let group = data[0].as_str().ok_or("group must be string")?;
                        let jobs = data[1].as_array().ok_or("jobs must be array")?.clone();
                        let handles = h.start(group, jobs)?;
                        let mut state = sync_state.borrow_mut();
                        Ok(Json::Array(
                            handles
                                .into_iter()
                                .map(|handle| {
                                    let id = state.handles.len();
                                    state.handles.push(handle);
                                    json!(id)
                                })
                                .collect(),
                        ))
                    }
                    "open" => {
                        let children = h.open_scopes(
                            data[0].as_str().ok_or("group must be string")?,
                            data[1].as_u64().ok_or("count must be integer")? as usize,
                        )?;
                        let mut state = sync_state.borrow_mut();
                        Ok(Json::Array(
                            children
                                .into_iter()
                                .map(|child| {
                                    let id = state.scopes.len();
                                    state.scopes.push(child);
                                    json!(id)
                                })
                                .collect(),
                        ))
                    }
                    _ => Err("unknown host operation".into()),
                }
            })();
            result
                .and_then(|value| {
                    safe_json(&value)?;
                    serde_json::to_string(&value).map_err(|e| e.to_string())
                })
                .map_err(|e| {
                    let e = if h.lock().cancelled.load(Ordering::SeqCst) {
                        e
                    } else {
                        sync_host.in_scope(h.scope_id()).fail(e)
                    };
                    js_error(&ctx, e)
                })
        },
    )?;
    let async_state = state;
    let async_host = host.clone();
    let wait = Function::new(
        ctx.clone(),
        move |ctx: Ctx<'js>,
              scope: usize,
              op: String,
              payload: String|
              -> rquickjs::Result<Promise<'js>> {
            let h = async_state
                .borrow()
                .scopes
                .get(scope)
                .cloned()
                .ok_or_else(|| js_error(&ctx, "unknown scope"))?;
            let ids: Vec<usize> = serde_json::from_str(&payload)
                .map_err(|e| js_error(&ctx, h.fail(e.to_string())))?;
            let handles = ids
                .iter()
                .map(|id| {
                    async_state
                        .borrow()
                        .handles
                        .get(*id)
                        .cloned()
                        .ok_or("foreign run handle".to_string())
                })
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| js_error(&ctx, h.fail(e)))?;
            let (promise, resolve, reject) = Promise::new(&ctx)?;
            let child_ctx = ctx.clone();
            let failure_host = async_host.clone();
            ctx.spawn(async move {
                let result = match op.as_str() {
                    "close" => h.finish_scope().await.map(|_| Json::Null),
                    "wait" => h.wait_any(handles).await,
                    "cancel" if handles.len() == 1 => h.cancel(handles[0].clone()).await,
                    _ => Err("unknown async host operation".into()),
                }
                .and_then(|v| {
                    safe_json(&v)?;
                    serde_json::to_string(&v).map_err(|e| e.to_string())
                });
                let delivered = match result {
                    Ok(v) => resolve.call::<_, ()>((v,)),
                    Err(e) => {
                        if !h.lock().cancelled.load(Ordering::SeqCst) {
                            h.fail(e.clone());
                        }
                        reject.call::<_, ()>((e,))
                    }
                };
                if let Err(error) = delivered
                    && !failure_host.lock().cancelled.load(Ordering::SeqCst)
                {
                    failure_host.fail(diagnostic(&child_ctx, error));
                }
            });
            Ok(promise)
        },
    )?;
    let is_proxy = Function::new(ctx.clone(), |value: Value<'js>| unsafe {
        rquickjs::qjs::JS_IsProxy(value.as_raw())
    })?;
    let bootstrap: Function =
        ctx.eval(include_str!("../../assets/workflow/quickjs-bootstrap.js"))?;
    bootstrap.call((call, wait, is_proxy))
}
async fn evaluate(host: SharedHost) -> Result<Json, String> {
    let runtime = AsyncRuntime::new().map_err(|e| e.to_string())?;
    runtime.set_memory_limit(MEMORY).await;
    runtime.set_max_stack_size(512 * 1024).await;
    let budget = Arc::new(AtomicU64::new(0));
    let counter = budget.clone();
    let stop = host.lock().cancelled.clone();
    let hook_host = host.clone();
    runtime
        .set_interrupt_handler(Some(Box::new(move || {
            if stop.load(Ordering::SeqCst) {
                return true;
            }
            if counter.fetch_add(1, Ordering::SeqCst) >= BUDGET {
                hook_host.fail("JavaScript compute budget exhausted".into());
                return true;
            }
            false
        })))
        .await;
    let unhandled = Arc::new(Mutex::new(HashSet::new()));
    let rejected = unhandled.clone();
    runtime
        .set_host_promise_rejection_tracker(Some(Box::new(move |_, promise, _, handled| {
            // Promise rejection is initially reported before an immediately attached catch.
            let id = unsafe { rquickjs::qjs::JS_VALUE_GET_PTR(promise.as_raw()) } as usize;
            let mut set = rejected.lock().unwrap();
            if handled {
                set.remove(&id);
            } else {
                set.insert(id);
            }
        })))
        .await;
    let (resolver, load) = {
        let h = host.lock();
        loader(&h.bundle)
    };
    runtime.set_loader(resolver, load).await;
    let context = AsyncContext::full(&runtime)
        .await
        .map_err(|e| e.to_string())?;
    let (namespace, initializing, api) = context
        .with(|ctx| {
            capture_stack_getter(&ctx).map_err(|e| diagnostic(&ctx, e))?;
            let api = install(&ctx, host.clone()).map_err(|e| diagnostic(&ctx, e))?;
            let h = host.lock();
            safe_json(&h.bundle.input)?;
            let m = Module::declare(
                ctx.clone(),
                h.bundle.entry_name.clone(),
                h.bundle.entry.clone(),
            )
            .map_err(|e| diagnostic(&ctx, e))?;
            drop(h);
            let (module, promise) = m.eval().map_err(|e| diagnostic(&ctx, e))?;
            Ok::<_, String>((
                Persistent::save(&ctx, module.namespace().map_err(|e| diagnostic(&ctx, e))?),
                Persistent::save(&ctx, promise),
                Persistent::save(&ctx, api),
            ))
        })
        .await?;
    drive(&runtime, &context, &host, initializing, &budget).await?;
    let workflow = context
        .with(|ctx| {
            let namespace = namespace.restore(&ctx).map_err(|e| diagnostic(&ctx, e))?;
            let entry: Function = namespace.get("default").map_err(|e| diagnostic(&ctx, e))?;
            let api = api.restore(&ctx).map_err(|e| diagnostic(&ctx, e))?;
            let invoke: Function = api.get("invoke").map_err(|e| diagnostic(&ctx, e))?;
            let input =
                serde_json::to_string(&host.lock().bundle.input).map_err(|e| e.to_string())?;
            let promise: Promise = invoke
                .call((entry, input))
                .map_err(|e| diagnostic(&ctx, e))?;
            Ok::<_, String>(Persistent::save(&ctx, promise))
        })
        .await?;
    let result = drive(&runtime, &context, &host, workflow, &budget).await?;
    if !unhandled.lock().unwrap().is_empty() {
        return Err("unhandled JavaScript promise rejection".into());
    }
    if runtime.is_job_pending().await {
        return Err("workflow returned with pending JavaScript or host work".into());
    }
    let raw = context
        .with(|ctx| {
            let value = result.restore(&ctx).map_err(|e| diagnostic(&ctx, e))?;
            value
                .as_string()
                .ok_or("workflow output must be JSON string".to_owned())?
                .to_string()
                .map_err(|e| e.to_string())
        })
        .await?;
    let value: Json = serde_json::from_str(&raw).map_err(|e| e.to_string())?;
    safe_json(&value)?;
    Ok(value)
}
async fn drive(
    runtime: &AsyncRuntime,
    context: &AsyncContext,
    host: &SharedHost,
    promise: Persistent<Promise<'static>>,
    budget: &AtomicU64,
) -> Result<Persistent<Value<'static>>, String> {
    loop {
        if host.lock().cancelled.load(Ordering::SeqCst) {
            return Err("workflow stopped".into());
        }
        if budget.load(Ordering::SeqCst) >= BUDGET {
            return Err(host.fail("JavaScript compute budget exhausted".into()));
        }
        let ready = context
            .with(|ctx| {
                let p = promise
                    .clone()
                    .restore(&ctx)
                    .map_err(|e| diagnostic(&ctx, e))?;
                p.result::<Value>()
                    .map(|r| {
                        r.map(|value| Persistent::save(&ctx, value))
                            .map_err(|e| diagnostic(&ctx, e))
                    })
                    .transpose()
            })
            .await;
        if budget.load(Ordering::SeqCst) >= BUDGET {
            return Err(host.fail("JavaScript compute budget exhausted".into()));
        }
        if let Some(result) = ready? {
            return Ok(result);
        }
        if !runtime.is_job_pending().await {
            return Err("JavaScript promise cannot settle: no pending jobs or host work".into());
        }
        if budget.fetch_add(1, Ordering::SeqCst) >= BUDGET {
            return Err(host.fail("JavaScript compute budget exhausted".into()));
        }
        match runtime.execute_pending_job().await {
            Ok(true) => (),
            Ok(false) => {
                budget.fetch_sub(1, Ordering::SeqCst);
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            Err(error) => {
                if budget.load(Ordering::SeqCst) >= BUDGET {
                    return Err(host.fail("JavaScript compute budget exhausted".into()));
                }
                return Err(error.0.with(|ctx| diagnostic(&ctx, Error::Exception)).await);
            }
        }
    }
}
pub async fn run(host: SharedHost) -> Result<Json, String> {
    let runtime = tokio::runtime::Handle::current();
    tokio::task::spawn_blocking(move || runtime.block_on(evaluate(host)))
        .await
        .map_err(|e| format!("JavaScript worker: {e}"))?
}
