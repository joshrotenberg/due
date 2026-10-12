//! Lua adapter: real mlua async continuations, checked JSON, no native loader.
use super::{
    bundle::Bundle,
    contract,
    host::{RunHandle, SharedHost},
};
use mlua::{
    Error, Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, MultiValue, StdLib, Table, Value,
    VmState, chunk::ChunkMode,
};
use serde_json::Value as Json;
use std::{
    cell::{Cell, RefCell},
    collections::{HashMap, HashSet},
    future::{Future, poll_fn},
    rc::Rc,
    sync::atomic::Ordering,
    task::Poll,
};

const MEMORY: usize = 16 * 1024 * 1024;
const INSTRUCTIONS: u64 = 100_000;
impl mlua::UserData for RunHandle {}
fn err(e: impl ToString) -> Error {
    Error::runtime(e.to_string())
}

fn vm() -> mlua::Result<Lua> {
    // Omit user-created coroutines and C functions that can call arbitrary Lua
    // callbacks across a non-yieldable C boundary. The host's async coroutine is
    // the sole continuation. This makes hook suspension uncatchable by pcall.
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )?;
    lua.set_memory_limit(MEMORY)?;
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
fn compile(lua: &Lua, source: &str, name: &str) -> mlua::Result<Function> {
    lua.load(source)
        .set_name(name)
        .set_mode(ChunkMode::Text)
        .into_function()
}
pub fn check(bundle: &Bundle) -> Result<(), String> {
    let lua = vm().map_err(|e| e.to_string())?;
    compile(&lua, &bundle.entry, "entry").map_err(|e| e.to_string())?;
    for (name, source) in &bundle.modules {
        compile(&lua, source, name).map_err(|e| format!("{name}: {e}"))?;
    }
    Ok(())
}
fn to_lua(lua: &Lua, value: &Json) -> mlua::Result<Value> {
    contract::check_value(value).map_err(err)?;
    lua.to_value(value)
}
// Serde's default table heuristic can discard mixed keys. Validate raw entries
// ourselves, using its null sentinel and array metatable for explicit identity.
fn from_lua(lua: &Lua, value: Value, object_mt: &Table) -> mlua::Result<Json> {
    struct Converter<'a> {
        lua: &'a Lua,
        object_mt: &'a Table,
        path: HashSet<usize>,
        bytes: usize,
    }
    impl Converter<'_> {
        fn charge(&mut self, n: usize) -> mlua::Result<()> {
            self.bytes = self.bytes.saturating_add(n);
            if self.bytes > contract::MAX_BYTES {
                return Err(err("JSON exceeds 1 MiB"));
            }
            Ok(())
        }
        fn convert(&mut self, value: Value, depth: usize) -> mlua::Result<Json> {
            if depth > 32 {
                return Err(err("JSON nesting exceeds 32"));
            }
            self.charge(1)?;
            Ok(match value {
                Value::Boolean(v) => v.into(),
                Value::Integer(v) => v.into(),
                Value::Number(v) => {
                    if !v.is_finite() || (v.fract() == 0.0 && v.abs() > 9_007_199_254_740_991.0) {
                        return Err(err("nonfinite or potentially lossy integral float"));
                    }
                    serde_json::Number::from_f64(v)
                        .ok_or_else(|| err("invalid float"))?
                        .into()
                }
                Value::String(v) => {
                    let v = v.to_str()?;
                    self.charge(v.len())?;
                    v.to_string().into()
                }
                Value::LightUserData(v) if Value::LightUserData(v) == self.lua.null() => Json::Null,
                Value::Table(t) => {
                    let pointer = t.to_pointer() as usize;
                    if !self.path.insert(pointer) {
                        return Err(err("cyclic JSON table"));
                    }
                    let mt = t.metatable();
                    let array = mt.as_ref() == Some(&self.lua.array_metatable());
                    let object = mt.as_ref() == Some(self.object_mt);
                    if mt.is_some() && !array && !object {
                        return Err(err("unsupported JSON metatable"));
                    }
                    let mut strings = serde_json::Map::new();
                    let mut numbers = std::collections::BTreeMap::new();
                    for pair in t.pairs::<Value, Value>() {
                        let (key, value) = pair?;
                        match key {
                            Value::String(k) if !array => {
                                if strings.len() >= 256 {
                                    return Err(err("object exceeds 256 entries"));
                                }
                                let k = k.to_str()?.to_string();
                                self.charge(k.len())?;
                                strings.insert(k, self.convert(value, depth + 1)?);
                            }
                            Value::Integer(k) if k > 0 && k <= 4096 && !object => {
                                numbers.insert(k, self.convert(value, depth + 1)?);
                            }
                            _ => return Err(err("malformed JSON table key")),
                        }
                    }
                    self.path.remove(&pointer);
                    if !strings.is_empty() && !numbers.is_empty() {
                        return Err(err("mixed JSON table"));
                    }
                    if array || !numbers.is_empty() {
                        if numbers.keys().copied().ne(1..=numbers.len() as i64) {
                            return Err(err("sparse JSON array"));
                        }
                        Json::Array(numbers.into_values().collect())
                    } else {
                        Json::Object(strings)
                    }
                }
                _ => {
                    return Err(err(
                        "unsupported JSON value (use json.null for explicit null)",
                    ));
                }
            })
        }
    }
    let value = Converter {
        lua,
        object_mt,
        path: HashSet::new(),
        bytes: 0,
    }
    .convert(value, 0)?;
    contract::check_value(&value).map_err(err)?;
    Ok(value)
}
#[derive(Clone)]
struct Adapter {
    host: SharedHost,
    initializing: Rc<Cell<usize>>,
    object_mt: Table,
}
impl Adapter {
    fn enabled(&self) -> mlua::Result<()> {
        if self.initializing.get() > 0 {
            return Err(err(self.host.fail(
                "effects unavailable during Lua module initialization".into(),
            )));
        }
        Ok(())
    }
    fn json(&self, lua: &Lua, value: Value) -> mlua::Result<Json> {
        from_lua(lua, value, &self.object_mt).map_err(|e| err(self.host.fail(e.to_string())))
    }
    fn handle(&self, value: Value) -> mlua::Result<RunHandle> {
        if let Value::UserData(u) = value
            && let Ok(h) = u.borrow::<RunHandle>()
        {
            return Ok(h.clone());
        }
        Err(err(self.host.fail("expected opaque RunHandle".into())))
    }
    fn handles(&self, value: Value) -> mlua::Result<Vec<RunHandle>> {
        let invalid = || {
            err(self
                .host
                .fail("expected dense nonempty handle array".into()))
        };
        let Value::Table(t) = value else {
            return Err(invalid());
        };
        if t.metatable() == Some(self.object_mt.clone()) {
            return Err(invalid());
        }
        let len = t.raw_len();
        if len == 0 || len > 64 {
            return Err(invalid());
        }
        let mut count = 0;
        for pair in t.clone().pairs::<Value, Value>() {
            let (key, _) = pair?;
            if !matches!(key, Value::Integer(i) if i > 0 && i <= len as i64) {
                return Err(invalid());
            }
            count += 1;
        }
        if count != len {
            return Err(invalid());
        }
        (1..=len).map(|i| self.handle(t.raw_get(i)?)).collect()
    }
}
fn install(lua: &Lua, a: Adapter) -> mlua::Result<()> {
    let due = lua.create_table()?;
    let h = a.clone();
    due.set(
        "start",
        lua.create_function(move |lua, value: Value| {
            h.enabled()?;
            let job = h.json(lua, value)?;
            let handle = h.host.start("run", vec![job]).map_err(err)?.remove(0);
            lua.create_userdata(handle)
        })?,
    )?;
    for op in ["wait", "cancel", "wait_any", "run"] {
        let h = a.clone();
        due.set(
            op,
            lua.create_async_function(move |lua, value: Value| {
                let h = h.clone();
                async move {
                    h.enabled()?;
                    let outcome = match op {
                        "run" => {
                            let job = h.json(&lua, value)?;
                            let handle = h.host.start("run", vec![job]).map_err(err)?.remove(0);
                            h.host.wait(handle).await
                        }
                        "wait_any" => h.host.wait_any(h.handles(value)?).await,
                        "cancel" => h.host.cancel(h.handle(value)?).await,
                        _ => h.host.wait(h.handle(value)?).await,
                    }
                    .map_err(err)?;
                    to_lua(&lua, &outcome)
                }
            })?,
        )?;
    }
    let h = a.clone();
    due.set(
        "parallel",
        lua.create_async_function(move |lua, (group, value): (String, Value)| {
            let h = h.clone();
            async move {
                h.enabled()?;
                let Json::Array(jobs) = h.json(&lua, value)? else {
                    return Err(err(h.host.fail("parallel expects array".into())));
                };
                let value = h.host.parallel(&group, jobs).await.map_err(err)?;
                to_lua(&lua, &value)
            }
        })?,
    )?;
    for op in ["phase", "log"] {
        let h = a.clone();
        due.set(
            op,
            lua.create_function(move |_, text: String| {
                h.enabled()?;
                if op == "phase" {
                    h.host.phase(&text)
                } else {
                    h.host.log(&text)
                }
                .map_err(err)
            })?,
        )?;
    }
    lua.globals().set("due", due)?;
    let json = lua.create_table()?;
    json.set("null", lua.null())?;
    let mt = a.object_mt.clone();
    json.set(
        "encode",
        lua.create_function(move |lua, value| {
            serde_json::to_string(&from_lua(lua, value, &mt)?).map_err(err)
        })?,
    )?;
    for array in [true, false] {
        let mt = a.object_mt.clone();
        json.set(
            if array { "array" } else { "object" },
            lua.create_function(move |lua, table: Option<Table>| {
                let table = table.unwrap_or(lua.create_table()?);
                if table.metatable().is_some() {
                    return Err(err("table already has JSON marker"));
                }
                table.set_metatable(Some(if array {
                    lua.array_metatable()
                } else {
                    mt.clone()
                }))?;
                from_lua(lua, Value::Table(table.clone()), &mt)?;
                Ok(table)
            })?,
        )?;
    }
    lua.globals().set("json", json)?;
    let modules = a.host.lock().bundle.modules.clone();
    let cache = Rc::new(RefCell::new(HashMap::<String, Table>::new()));
    let loading = Rc::new(RefCell::new(HashSet::<String>::new()));
    lua.globals().set(
        "require",
        lua.create_async_function(move |lua, name: String| {
            let a = a.clone();
            let cache = cache.clone();
            let loading = loading.clone();
            let source = modules.get(&name).cloned();
            async move {
                if let Some(module) = cache.borrow().get(&name) {
                    return Ok(module.clone());
                }
                let source = source.ok_or_else(|| err(format!("undeclared module {name}")))?;
                if !loading.borrow_mut().insert(name.clone()) {
                    return Err(err("cyclic module initialization"));
                }
                a.initializing.set(a.initializing.get() + 1);
                let result = async {
                    let function = compile(&lua, &source, &name)?;
                    let module: Table = function.call_async(()).await?;
                    if module.metatable().is_some() {
                        return Err(err("module must return a plain function table"));
                    }
                    let mut count = 0;
                    for pair in module.clone().pairs::<Value, Value>() {
                        let (k, v) = pair?;
                        count += 1;
                        if count > 128
                            || !matches!(k, Value::String(_))
                            || !matches!(v, Value::Function(_))
                        {
                            return Err(err("module must return a bounded function table"));
                        }
                    }
                    Ok(module)
                }
                .await;
                a.initializing.set(a.initializing.get() - 1);
                loading.borrow_mut().remove(&name);
                let module = result?;
                cache.borrow_mut().insert(name, module.clone());
                Ok(module)
            }
        })?,
    )?;
    // mlua loads coroutine while creating async wrappers and captures yield
    // privately in them. Remove its public table only after registration.
    lua.globals().set("coroutine", Value::Nil)?;
    Ok(())
}
async fn evaluate(host: SharedHost) -> mlua::Result<Json> {
    let lua = vm()?;
    let budget = Rc::new(Cell::new(0u64));
    let hook_budget = budget.clone();
    let cancelled = host.lock().cancelled.clone();
    lua.set_global_hook(
        HookTriggers::new().every_nth_instruction(1000),
        move |_, _| {
            hook_budget.set(hook_budget.get() + 1000);
            // Always yield control to the outer future, including inside pcall.
            // That future terminates without resuming once the sticky budget trips.
            Ok(VmState::Yield)
        },
    )?;
    let object_mt = lua.create_table()?;
    install(
        &lua,
        Adapter {
            host: host.clone(),
            initializing: Rc::new(Cell::new(0)),
            object_mt: object_mt.clone(),
        },
    )?;
    let (entry, input) = {
        let h = host.lock();
        (h.bundle.entry.clone(), h.bundle.input.clone())
    };
    lua.globals().set("input", to_lua(&lua, &input)?)?;
    let function = compile(&lua, &entry, "entry")?;
    let future = function.call_async::<Value>(());
    tokio::pin!(future);
    let value = poll_fn(|cx| {
        let stopped = || {
            if budget.get() >= INSTRUCTIONS {
                Some(err("Lua instruction budget exhausted"))
            } else if cancelled.load(Ordering::SeqCst) {
                Some(err("workflow stopped"))
            } else {
                None
            }
        };
        if let Some(error) = stopped() {
            return Poll::Ready(Err(error));
        }
        let result = future.as_mut().poll(cx);
        if let Some(error) = stopped() {
            return Poll::Ready(Err(error));
        }
        result
    })
    .await?;
    from_lua(&lua, value, &object_mt)
}
pub async fn run(host: SharedHost) -> Result<Json, String> {
    evaluate(host).await.map_err(|e| e.to_string())
}
