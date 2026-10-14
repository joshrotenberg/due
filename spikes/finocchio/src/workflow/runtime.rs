use super::{Mode, config::Metadata, host::Host, schema, vm};
use crate::{
    compiler::Compiler,
    data::{self, Data},
};
use mlua::{Function, HookTriggers, Lua, MultiValue, Table, Value, VmState};
use serde_json::{Value as Json, json};
use std::{
    cell::{Cell, RefCell},
    collections::{BTreeMap, BTreeSet},
    future::{Future, poll_fn},
    rc::Rc,
    task::Poll,
};

fn err(e: impl ToString) -> mlua::Error {
    mlua::Error::runtime(e.to_string())
}
#[derive(Clone)]
struct Adapter {
    host: Host,
    enabled: Rc<Cell<bool>>,
    depth: Rc<Cell<usize>>,
    active: Rc<Cell<usize>>,
    total: Rc<Cell<usize>>,
}
impl Adapter {
    fn enabled(&self) -> mlua::Result<()> {
        if !self.enabled.get() {
            return Err(err(self.host.stop.fail(
                "effects unavailable during module initialization or inspection".into(),
            )));
        }
        if let Some(e) = self.host.stop.error() {
            return Err(err(e));
        }
        Ok(())
    }
    fn checked<T>(&self, r: mlua::Result<T>) -> mlua::Result<T> {
        r.map_err(|e| err(self.host.stop.fail(e.to_string())))
    }
}
struct Depth(Rc<Cell<usize>>, usize);
impl Drop for Depth {
    fn drop(&mut self) {
        self.0.set(self.1);
    }
}
struct Active(Rc<Cell<usize>>, usize);
impl Drop for Active {
    fn drop(&mut self) {
        self.0.set(self.0.get() - self.1);
    }
}
fn functions(v: Value) -> mlua::Result<Vec<Function>> {
    let Value::Table(t) = v else {
        return Err(err("concurrent needs a dense array of functions"));
    };
    let len = t.raw_len();
    if !(1..=32).contains(&len) || t.metatable().is_some() {
        return Err(err("concurrent needs 1..32 functions"));
    }
    let mut count = 0;
    for pair in t.clone().pairs::<Value, Value>() {
        let (k, v) = pair?;
        if !matches!(k, Value::Integer(i) if i > 0 && i as usize <= len)
            || !matches!(v, Value::Function(_))
        {
            return Err(err("concurrent needs a dense array of functions"));
        }
        count += 1;
    }
    if count != len {
        return Err(err("concurrent array has holes"));
    }
    (1..=len).map(|i| t.raw_get(i)).collect()
}
fn one(mut values: MultiValue) -> mlua::Result<Value> {
    if values.len() != 1 {
        return Err(err("expected exactly one value"));
    }
    Ok(values.pop_front().unwrap())
}
async fn concurrent(lua: Lua, a: Adapter, values: MultiValue) -> mlua::Result<Value> {
    a.enabled()?;
    let functions = a.checked(one(values).and_then(functions))?;
    let depth = a.depth.get() + 1;
    if depth > 8 || a.active.get() + functions.len() > 64 || a.total.get() + functions.len() > 256 {
        return Err(err(a.host.stop.fail(
            "concurrent depth/active/total continuation budget exceeded (8/64/256)".into(),
        )));
    }
    a.active.set(a.active.get() + functions.len());
    a.total.set(a.total.get() + functions.len());
    let _active = Active(a.active.clone(), functions.len());
    let mut pending = functions
        .into_iter()
        .map(|f| Some(Box::pin(f.call_async::<MultiValue>(()))))
        .collect::<Vec<_>>();
    let mut output = vec![Json::Null; pending.len()];
    let mut bytes = 2;
    poll_fn(|cx| {
        for (i, f) in pending.iter_mut().enumerate() {
            if a.host.stop.stopped() {
                return Poll::Pending;
            }
            if let Some(future) = f {
                let _depth = Depth(a.depth.clone(), a.depth.replace(depth));
                if let Poll::Ready(value) = future.as_mut().poll(cx) {
                    let value = a.checked(value.and_then(one).and_then(data::from_lua))?;
                    bytes += value.to_string().len() + 1;
                    if bytes > schema::MAX_BYTES {
                        return Poll::Ready(Err(err(a
                            .host
                            .stop
                            .fail("concurrent output exceeds 1 MiB".into()))));
                    }
                    output[i] = value;
                    *f = None;
                }
            }
        }
        if pending.iter().all(Option::is_none) {
            Poll::Ready(Ok(()))
        } else {
            Poll::Pending
        }
    })
    .await?;
    let output = Json::Array(output);
    a.checked(data::check(&output))?;
    Ok(Value::UserData(lua.create_userdata(Data(output))?))
}

pub(super) async fn evaluate(host: Host, mode: Mode, input: Json) -> Result<Json, String> {
    let compiler = Compiler::new().map_err(|e| e.to_string())?;
    let mut sources = BTreeMap::new();
    for (name, path) in &host.config.modules {
        let source =
            std::fs::read_to_string(path).map_err(|e| format!("module {}: {e}", path.display()))?;
        let compiled = compiler
            .compile(&source, &path.display().to_string())
            .map_err(|e| e.to_string())?;
        let source = compiled
            .lua
            .ok_or_else(|| compiled.error.unwrap_or_else(|| "compile failed".into()))?;
        sources.insert(name.clone(), (path.clone(), source));
    }
    let sources = Rc::new(sources);
    let lua = vm::vm().map_err(|e| e.to_string())?;
    // Capture the library inspector before any module can rebind globals.
    // The closure still uses ordinary Lua globals; pure initializer code can
    // rebind those. This snapshot is not a tamper-proof documentation channel.
    let contract: Table = lua.globals().get("contract").map_err(|e| e.to_string())?;
    let describe: Function = contract.get("describe").map_err(|e| e.to_string())?;
    let a = Adapter {
        host: host.clone(),
        enabled: Default::default(),
        depth: Default::default(),
        active: Default::default(),
        total: Default::default(),
    };
    let api = lua.create_table().map_err(|e| e.to_string())?;
    let run = a.clone();
    api.set(
        "run",
        lua.create_async_function(move |lua, args: MultiValue| {
            let a = run.clone();
            async move {
                a.enabled()?; // phase check precedes argument conversion
                let value = a.checked(one(args).and_then(data::from_lua))?;
                let result = a.host.run(value).await.map_err(err)?;
                a.checked(data::check(&result))?;
                lua.create_userdata(Data(result))
            }
        })
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let c = a.clone();
    api.set(
        "concurrent",
        lua.create_async_function(move |lua, args: MultiValue| concurrent(lua, c.clone(), args))
            .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    let v = a.clone();
    api.set(
        "validate",
        lua.create_function(
            move |_, (role, direction, value): (String, String, Value)| {
                v.enabled()?;
                let value = data::from_lua(value)?;
                v.host.validate(&role, &direction, &value).map_err(err)?;
                Ok(true)
            },
        )
        .map_err(|e| e.to_string())?,
    )
    .map_err(|e| e.to_string())?;
    lua.globals().set("host", api).map_err(|e| e.to_string())?;
    // Preload ALL explicitly configured modules. Later require only returns a
    // cached ordinary export table; there is no runtime filesystem resolution.
    let loaded: Rc<RefCell<BTreeMap<String, Table>>> = Default::default();
    let loading: Rc<RefCell<BTreeSet<String>>> = Default::default();
    let require_loaded = loaded.clone();
    let require_host = host.clone();
    lua.globals()
        .set(
            "require",
            lua.create_async_function(move |lua, args: MultiValue| {
                let loaded = require_loaded.clone();
                let loading = loading.clone();
                let sources = sources.clone();
                let host = require_host.clone();
                async move {
                    let operation = async {
                        let Value::String(name) = one(args)? else {
                            return Err(err("require expects a configured module name"));
                        };
                        let name = name.to_str()?.to_string();
                        if let Some(value) = loaded.borrow().get(&name) {
                            return Ok(value.clone());
                        }
                        let (path, source) = sources
                            .get(&name)
                            .ok_or_else(|| err(format!("module not configured: {name}")))?;
                        if !loading.borrow_mut().insert(name.clone()) {
                            return Err(err(format!("module cycle: {name}")));
                        }
                        let values = lua
                            .load(source)
                            .set_name(format!("@{} (generated Lua)", path.display()))
                            .set_mode(mlua::chunk::ChunkMode::Text)
                            .call_async::<MultiValue>(())
                            .await?;
                        let Value::Table(exports) = one(values)? else {
                            return Err(err(format!(
                                "module {name} must return an ordinary export table"
                            )));
                        };
                        loading.borrow_mut().remove(&name);
                        loaded.borrow_mut().insert(name, exports.clone());
                        Ok(exports)
                    }
                    .await;
                    operation.map_err(|e| err(host.stop.fail(e.to_string())))
                }
            })
            .map_err(|e| e.to_string())?,
        )
        .map_err(|e| e.to_string())?;

    let count = Cell::new(0u64);
    let stop = host.stop.clone();
    lua.set_global_hook(
        HookTriggers::new().every_nth_instruction(1000),
        move |_, debug| {
            // Pinned mlua wrapper must remain atomic; don't charge idle async polls.
            if debug.source().source.as_deref() == Some("=__mlua_async_poll") {
                return Ok(VmState::Continue);
            }
            count.set(count.get() + 1000);
            if count.get() >= 1_000_000 {
                stop.fail("integrated Lua instruction budget exhausted".into());
            }
            Ok(VmState::Yield)
        },
    )
    .map_err(|e| e.to_string())?;

    let future = async {
        let require: Function = lua.globals().get("require")?;
        for name in host.config.modules.keys() {
            require.call_async::<Table>(name.clone()).await?;
        }
        let mut description = serde_json::Map::new();
        let exports = loaded.borrow().clone();
        for (name, exports) in &exports {
            let metadata: Value = exports.raw_get("meta")?;
            if !matches!(metadata, Value::Nil) {
                let metadata = data::from_lua(metadata)?;
                let meta: Metadata = serde_json::from_value(metadata.clone()).map_err(err)?;
                meta.validate().map_err(err)?;
                host.metadata.borrow_mut().insert(name.clone(), meta);
                let mut functions = Vec::new();
                for item in exports.clone().pairs::<String, Value>() {
                    let (key, value) = item?;
                    if let Value::Function(function) = value {
                        let doc: Value = describe.call_async(function).await?;
                        // contract.describe returns an owned native-table copy.
                        // Its known parameters field is an array, even at arity 0.
                        if let Value::Table(doc) = &doc {
                            let parameters: Table = doc.raw_get("parameters")?;
                            if parameters.is_empty() {
                                doc.raw_set("parameters", lua.create_userdata(Data(json!([])))?)?;
                            }
                        }
                        functions.push(json!({"name":key,"contract": if matches!(doc,Value::Nil) { Json::Null } else { data::from_lua(doc)? }}));
                    }
                }
                description.insert(name.clone(), json!({"metadata":metadata,"functions":functions,"source":host.config.modules[name]}));
            }
        }
        if mode != Mode::Run {
            let description = Json::Object(description);
            data::check(&description)?;
            return Ok(description);
        }
        data::check(&input)?;
        let meta = host
            .metadata
            .borrow()
            .get(&host.config.entry)
            .cloned()
            .ok_or_else(|| err("entry module needs metadata"))?;
        schema::validate(&schema::validator(&meta.input_schema).map_err(err)?, &input).map_err(
            |e| {
                err(format!(
                    "entry input ({}): {e}",
                    host.config.modules[&host.config.entry].display()
                ))
            },
        )?;
        let entry = loaded.borrow()[&host.config.entry].clone();
        let run: Function = entry.raw_get("run")?;
        a.enabled.set(true);
        let result = run
            .call_async::<MultiValue>(lua.create_userdata(Data(input))?)
            .await?;
        let output = data::from_lua(one(result)?)?;
        schema::validate(
            &schema::validator(&meta.output_schema).map_err(err)?,
            &output,
        )
        .map_err(|e| {
            err(format!(
                "entry output ({}): {e}",
                host.config.modules[&host.config.entry].display()
            ))
        })?;
        Ok(output)
    };
    tokio::pin!(future);
    let driven = poll_fn(|cx| {
        if let Some(e) = host.stop.error() {
            return Poll::Ready(Err(err(e)));
        }
        let value = future.as_mut().poll(cx);
        if let Some(e) = host.stop.error() {
            return Poll::Ready(Err(err(e)));
        }
        value
    });
    tokio::select! {
        value = driven => value.map_err(|e| e.to_string()),
        _ = host.stop.external.notify.notified() => Err("parent cancelled".into()),
        _ = host.stop.notify.notified() => Err(host.stop.error().unwrap_or_else(|| "workflow stopped".into())),
    }
}
