//! Lua adapter: real mlua async continuations, checked JSON, no native loader.
use super::{
    bundle::Bundle,
    contract,
    host::{RunHandle, SharedHost},
};
use mlua::{
    Error, FromLua, Function, HookTriggers, Lua, LuaOptions, LuaSerdeExt, MultiValue, StdLib,
    Table, Value, VmState, chunk::ChunkMode, debug::DebugEvent,
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
    // host manages every continuation. Hook suspension is uncatchable by pcall.
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
fn has_fennel(bundle: &Bundle) -> bool {
    #[cfg(feature = "workflow-fennel-spike")]
    return !bundle.source_maps.is_empty();
    #[cfg(not(feature = "workflow-fennel-spike"))]
    {
        let _ = bundle;
        false
    }
}
fn chunk_name(bundle: &Bundle, name: &str) -> String {
    #[cfg(feature = "workflow-fennel-spike")]
    if bundle.source_maps.contains_key(name) {
        return format!("@{name} (generated Lua)");
    }
    let _ = bundle;
    format!("@{name}")
}
pub fn check(bundle: &Bundle) -> Result<(), String> {
    let lua = vm().map_err(|e| e.to_string())?;
    compile(&lua, &bundle.entry, &chunk_name(bundle, &bundle.entry_name))
        .map_err(|e| map_error(bundle, &e.to_string()))?;
    for (name, source) in &bundle.modules {
        compile(
            &lua,
            source,
            &chunk_name(bundle, &bundle.module_names[name]),
        )
        .map_err(|e| map_error(bundle, &format!("{name}: {e}")))?;
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
    current: Rc<Cell<usize>>,
    initializing: Rc<RefCell<HashMap<usize, usize>>>,
    budget: Rc<Cell<u64>>,
    object_mt: Table,
}
impl Adapter {
    fn host(&self) -> SharedHost {
        self.host.in_scope(self.current.get())
    }
    fn stopped(&self) -> mlua::Result<()> {
        if self.budget.get() >= INSTRUCTIONS {
            return Err(err("Lua instruction budget exhausted"));
        }
        if self.host.lock().cancelled.load(Ordering::SeqCst) {
            return Err(err("workflow stopped"));
        }
        Ok(())
    }
    fn enabled(&self) -> mlua::Result<()> {
        if self
            .initializing
            .borrow()
            .get(&self.current.get())
            .copied()
            .unwrap_or(0)
            > 0
        {
            return Err(err(self.host().fail(
                "effects unavailable during Lua module initialization or entry loading".into(),
            )));
        }
        Ok(())
    }
    fn json(&self, lua: &Lua, value: Value) -> mlua::Result<Json> {
        self.checked(from_lua(lua, value, &self.object_mt))
    }
    fn checked<T>(&self, result: mlua::Result<T>) -> mlua::Result<T> {
        result.map_err(|e| err(self.host().fail(e.to_string())))
    }
    fn text(&self, lua: &Lua, value: Value) -> mlua::Result<String> {
        self.checked(String::from_lua(value, lua))
    }
    fn outcome(&self, lua: &Lua, value: &Json) -> mlua::Result<Value> {
        self.checked(to_lua(lua, value))
    }
    fn handle(&self, value: Value) -> mlua::Result<RunHandle> {
        if let Value::UserData(u) = value
            && let Ok(h) = u.borrow::<RunHandle>()
        {
            return Ok(h.clone());
        }
        Err(err(self.host().fail("expected opaque RunHandle".into())))
    }
    fn handles(&self, value: Value) -> mlua::Result<Vec<RunHandle>> {
        let invalid = || {
            err(self
                .host()
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
            let (key, _) = self.checked(pair)?;
            if !matches!(key, Value::Integer(i) if i > 0 && i <= len as i64) {
                return Err(invalid());
            }
            count += 1;
        }
        if count != len {
            return Err(invalid());
        }
        (1..=len)
            .map(|i| self.handle(self.checked(t.raw_get(i))?))
            .collect()
    }
}
// Poll-local identity, restored even when a nested composition returns Pending.
// No thread-local state and no host-global phase mutation across await points.
struct CurrentScope {
    current: Rc<Cell<usize>>,
    previous: usize,
}
impl CurrentScope {
    fn enter(current: &Rc<Cell<usize>>, scope: usize) -> Self {
        Self {
            current: current.clone(),
            previous: current.replace(scope),
        }
    }
}
impl Drop for CurrentScope {
    fn drop(&mut self) {
        self.current.set(self.previous);
    }
}
struct Initializing {
    counts: Rc<RefCell<HashMap<usize, usize>>>,
    scope: usize,
}
impl Initializing {
    fn enter(a: &Adapter) -> Self {
        let scope = a.current.get();
        *a.initializing.borrow_mut().entry(scope).or_default() += 1;
        Self {
            counts: a.initializing.clone(),
            scope,
        }
    }
}
impl Drop for Initializing {
    fn drop(&mut self) {
        let mut counts = self.counts.borrow_mut();
        let n = counts.get_mut(&self.scope).unwrap();
        *n -= 1;
        if *n == 0 {
            counts.remove(&self.scope);
        }
    }
}
fn functions(value: Value) -> mlua::Result<Vec<Function>> {
    let invalid = || err("concurrent expects a plain dense array of 1..32 functions");
    let Value::Table(t) = value else {
        return Err(invalid());
    };
    let len = t.raw_len();
    if t.metatable().is_some() || len == 0 || len > 32 {
        return Err(invalid());
    }
    let mut count = 0;
    for pair in t.clone().pairs::<Value, Value>() {
        let (key, value) = pair?;
        if !matches!(key, Value::Integer(i) if i > 0 && i <= len as i64)
            || !matches!(value, Value::Function(_))
        {
            return Err(invalid());
        }
        count += 1;
    }
    if count != len {
        return Err(invalid());
    }
    (1..=len).map(|i| t.raw_get(i)).collect()
}
// Armed from reservation through aggregate conversion. Dropping a pending Lua
// async wrapper must not silently discard its component tree, even without jobs.
// This guard owns no tasks: pending futures drop normally and JoinBack returns
// any borrowed native joins to the host for outer finish.
struct PendingGroup {
    parent: SharedHost,
    complete: bool,
}
impl Drop for PendingGroup {
    fn drop(&mut self) {
        if !self.complete {
            self.parent.abandon_group();
        }
    }
}
async fn concurrent(lua: Lua, a: Adapter, group: Value, value: Value) -> mlua::Result<Value> {
    a.enabled()?;
    a.stopped()?;
    let parent = a.host();
    let Value::String(group) = group else {
        return Err(err(parent.fail("concurrent group must be a string".into())));
    };
    let functions = functions(value).map_err(|e| err(parent.fail(e.to_string())))?;
    let group = group
        .to_str()
        .map_err(|e| err(parent.fail(e.to_string())))?;
    let scopes = parent.open_scopes(&group, functions.len()).map_err(err)?;
    let mut guard = PendingGroup {
        parent: parent.clone(),
        complete: false,
    };
    let scope_ids: Vec<_> = scopes.iter().map(SharedHost::scope_id).collect();
    let mut pending = Vec::new();
    for (function, host) in functions.into_iter().zip(scopes) {
        let a = a.clone();
        let lua = lua.clone();
        // Construct call_async under its own identity as well as polling it.
        let _identity = CurrentScope::enter(&a.current, host.scope_id());
        let call = function.call_async::<MultiValue>(());
        let future = async move {
            let result = async {
                let mut values = call.await?;
                if values.len() != 1 {
                    return Err(err(
                        "concurrent function must return exactly one JSON value",
                    ));
                }
                from_lua(&lua, values.pop_front().unwrap(), &a.object_mt)
            }
            .await;
            if let Err(e) = &result {
                host.scope_failure(format!("scope {}: {e}", host.scope_id()));
            }
            host.finish_scope().await.map_err(err)?;
            result
        };
        pending.push(Some(Box::pin(future)));
    }
    let mut results = vec![Json::Null; pending.len()];
    let mut result_bytes = 2usize;
    // A single bounded tree of futures on one VM; Rust jobs run independently.
    // Poll every ready sibling once per outer poll, never drive a yielded hook
    // again here. The outer budget check runs even through nested pcall.
    poll_fn(|cx| {
        for (i, future) in pending.iter_mut().enumerate() {
            if a.stopped().is_err() {
                return Poll::Pending;
            }
            if let Some(f) = future {
                let scope = scope_ids[i];
                let _identity = CurrentScope::enter(&a.current, scope);
                if let Poll::Ready(value) = f.as_mut().poll(cx) {
                    let Ok(value) = value else {
                        return Poll::Pending;
                    };
                    result_bytes += serde_json::to_vec(&value)
                        .map_err(|e| err(parent.fail(e.to_string())))?
                        .len()
                        + 1;
                    if result_bytes > contract::MAX_BYTES {
                        parent.fail("concurrent results exceed 1 MiB".into());
                        return Poll::Pending;
                    }
                    results[i] = value;
                    *future = None;
                }
            }
            if a.stopped().is_err() {
                return Poll::Pending;
            }
        }
        if pending.iter().all(Option::is_none) {
            Poll::Ready(Ok::<(), Error>(()))
        } else {
            Poll::Pending
        }
    })
    .await?;
    let result =
        to_lua(&lua, &Json::Array(results)).map_err(|e| err(parent.fail(e.to_string())))?;
    guard.complete = true;
    Ok(result)
}
fn install(lua: &Lua, a: Adapter) -> mlua::Result<()> {
    let due = lua.create_table()?;
    let h = a.clone();
    due.set(
        "concurrent",
        lua.create_async_function(move |lua, (group, value): (Value, Value)| {
            concurrent(lua, h.clone(), group, value)
        })?,
    )?;
    let h = a.clone();
    due.set(
        "start",
        lua.create_function(move |lua, value: Value| {
            h.enabled()?;
            let job = h.json(lua, value)?;
            let handle = h.host().start("run", vec![job]).map_err(err)?.remove(0);
            h.checked(lua.create_userdata(handle))
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
                            let handle = h.host().start("run", vec![job]).map_err(err)?.remove(0);
                            h.host().wait(handle).await
                        }
                        "wait_any" => h.host().wait_any(h.handles(value)?).await,
                        "cancel" => h.host().cancel(h.handle(value)?).await,
                        _ => h.host().wait(h.handle(value)?).await,
                    }
                    .map_err(err)?;
                    h.outcome(&lua, &outcome)
                }
            })?,
        )?;
    }
    let h = a.clone();
    due.set(
        "parallel",
        lua.create_async_function(move |lua, (group, value): (Value, Value)| {
            let h = h.clone();
            async move {
                h.enabled()?;
                let group = h.text(&lua, group)?;
                let Json::Array(jobs) = h.json(&lua, value)? else {
                    return Err(err(h.host().fail("parallel expects array".into())));
                };
                let value = h.host().parallel(&group, jobs).await.map_err(err)?;
                h.outcome(&lua, &value)
            }
        })?,
    )?;
    for op in ["phase", "log"] {
        let h = a.clone();
        due.set(
            op,
            lua.create_function(move |lua, text: Value| {
                h.enabled()?;
                let text = h.text(lua, text)?;
                if op == "phase" {
                    h.host().phase(&text)
                } else {
                    h.host().log(&text)
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
    let (modules, names, sticky_loading) = {
        let h = a.host.lock();
        (
            h.bundle.modules.clone(),
            h.bundle
                .module_names
                .iter()
                .map(|(key, name)| (key.clone(), chunk_name(&h.bundle, name)))
                .collect::<HashMap<_, _>>(),
            h.bundle.manifest.schema_version == 2 || has_fennel(&h.bundle),
        )
    };
    let cache = Rc::new(RefCell::new(HashMap::<String, Table>::new()));
    let loading = Rc::new(RefCell::new(HashSet::<String>::new()));
    let module_gate = Rc::new(tokio::sync::Mutex::new(()));
    lua.globals().set(
        "require",
        lua.create_async_function(move |lua, name: Value| {
            let a = a.clone();
            let module_gate = module_gate.clone();
            let cache = cache.clone();
            let loading = loading.clone();
            let name = String::from_lua(name, &lua);
            let source = name
                .as_ref()
                .ok()
                .and_then(|name| modules.get(name))
                .cloned();
            let filename = name.as_ref().ok().and_then(|name| names.get(name)).cloned();
            async move {
                // Only the legacy plain v1 root may recover from ordinary
                // require errors, including argument conversion errors.
                // Due effects still call host.fail in enabled(), in every version.
                let load_error = |e: Error| {
                    if sticky_loading || a.current.get() != 0 {
                        err(a.host().fail(e.to_string()))
                    } else {
                        e
                    }
                };
                let name = name.map_err(load_error)?;
                // Serialize only uncached loading. Nested require in the same
                // initializer reenters without locking; cycle checks still apply.
                if let Some(module) = cache.borrow().get(&name) {
                    return Ok(module.clone());
                }
                let outer = a
                    .initializing
                    .borrow()
                    .get(&a.current.get())
                    .copied()
                    .unwrap_or(0)
                    == 0;
                let _gate = if outer {
                    Some(module_gate.lock().await)
                } else {
                    None
                };
                if let Some(module) = cache.borrow().get(&name) {
                    return Ok(module.clone());
                }
                let source =
                    source.ok_or_else(|| load_error(err(format!("undeclared module {name}"))))?;
                if !loading.borrow_mut().insert(name.clone()) {
                    return Err(load_error(err("cyclic module initialization")));
                }
                let _initializing = Initializing::enter(&a);
                let result = async {
                    let function = compile(
                        &lua,
                        &source,
                        &filename.unwrap_or_else(|| format!("@{name}")),
                    )?;
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

                loading.borrow_mut().remove(&name);
                let module = result.map_err(load_error)?;
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
    let track_locations = has_fennel(&host.lock().bundle);
    if track_locations {
        // Fennel sequence-rest emits this call. Never return a real metatable,
        // including JSON markers, string methods or opaque handle internals.
        lua.globals().set(
            "getmetatable",
            lua.create_function(|_, _: Value| Ok(Value::Nil))?,
        )?;
    }
    let budget = Rc::new(Cell::new(0u64));
    let hook_budget = budget.clone();
    let cancelled = host.lock().cancelled.clone();
    let diagnostic_host = host.clone();
    let current = Rc::new(Cell::new(0));
    let hook_current = current.clone();
    let triggers = HookTriggers::new().every_nth_instruction(1);
    let triggers = if track_locations {
        triggers.every_line()
    } else {
        triggers
    };
    lua.set_global_hook(triggers, move |_, debug| {
        // This pinned mlua implementation chunk polls Rust futures, rather than
        // executing workflow logic. Charging it makes quiet native waits consume
        // the script budget. Yielding inside it can also interrupt the transition
        // from polling a nested future to yielding its Pending sentinel, consuming
        // the child's wake before the next poll reaches that child. Keep this
        // private wrapper atomic; actual user/module chunks always have @ names.
        if debug.source().source.as_deref() == Some("=__mlua_async_poll") {
            return Ok(VmState::Continue);
        }
        // A host-only line hook survives async wait + hook suspension and
        // tail calls, where mlua callback tracebacks may lose the caller.
        if track_locations
            && matches!(debug.event(), DebugEvent::Line)
            && let (Some(source), Some(line)) = (debug.source().source, debug.current_line())
        {
            let name = source.strip_prefix('@').unwrap_or(&source);
            let h = diagnostic_host.lock();
            // Only generated chunks carry this suffix; leave ordinary Lua
            // filenames alone (including names that happen to end this way).
            let name = name
                .strip_suffix(" (generated Lua)")
                .filter(|n| {
                    #[cfg(feature = "workflow-fennel-spike")]
                    return h.bundle.source_maps.contains_key(*n);
                    #[cfg(not(feature = "workflow-fennel-spike"))]
                    {
                        let _ = n;
                        false
                    }
                })
                .unwrap_or(name);
            if name == h.bundle.entry_name || h.bundle.module_names.values().any(|n| n == name) {
                let location = source_location(&h.bundle, name, line);
                // Generated epilogues can have no source-map entry. Keep the
                // last available workflow site in this scope in that case.
                #[cfg(feature = "workflow-fennel-spike")]
                let fallback = h.bundle.source_maps.get(name).is_some_and(|map| {
                    line.checked_sub(1)
                        .and_then(|index| map.get(index))
                        .is_none_or(Option::is_none)
                });
                #[cfg(not(feature = "workflow-fennel-spike"))]
                let fallback = false;
                drop(h);
                diagnostic_host
                    .in_scope(hook_current.get())
                    .location(location, fallback);
            }
        }
        if !matches!(debug.event(), DebugEvent::Count) {
            return Ok(VmState::Continue);
        }
        hook_budget.set(hook_budget.get() + 1);
        if hook_budget.get() == INSTRUCTIONS {
            diagnostic_host
                .in_scope(hook_current.get())
                .scope_failure("Lua instruction budget exhausted".into());
        }
        // Always yield control to the outer future, including inside pcall.
        // That future terminates without resuming once the sticky budget trips.
        Ok(if hook_budget.get().is_multiple_of(1000) {
            VmState::Yield
        } else {
            VmState::Continue
        })
    })?;
    let object_mt = lua.create_table()?;
    let initializing = Rc::new(RefCell::new(HashMap::new()));
    let adapter = Adapter {
        host: host.clone(),
        initializing: initializing.clone(),
        object_mt: object_mt.clone(),
        current,
        budget: budget.clone(),
    };
    install(&lua, adapter.clone())?;
    let (entry, input, name, version) = {
        let h = host.lock();
        (
            h.bundle.entry.clone(),
            h.bundle.input.clone(),
            chunk_name(&h.bundle, &h.bundle.entry_name),
            h.bundle.manifest.schema_version,
        )
    };
    lua.globals().set("input", to_lua(&lua, &input)?)?;
    let function = compile(&lua, &entry, &name)?;
    let future = async {
        if version == 1 {
            return function.call_async::<Value>(()).await;
        }
        let loading = Initializing::enter(&adapter);
        let entry = function.call_async::<Value>(()).await;
        drop(loading);
        let entry = entry.map_err(|e| err(host.fail(e.to_string())))?;
        if let Some(error) = host.lock().fatal_cause() {
            return Err(err(error));
        }
        let Value::Function(entry) = entry else {
            return Err(err(
                host.fail("manifest v2 entry must return a function".into())
            ));
        };
        entry.call_async::<Value>(to_lua(&lua, &input)?).await
    };
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
    let result = evaluate(host.clone()).await;
    result.map_err(|e| {
        // Signals stay cause-less for the outer invocation's cancelled receipt.
        if host.lock().cancelled.load(Ordering::SeqCst) {
            e.to_string()
        } else {
            host.scope_failure(e.to_string())
        }
    })
}

fn source_location(bundle: &Bundle, name: &str, line: usize) -> String {
    #[cfg(feature = "workflow-fennel-spike")]
    if let Some(map) = bundle.source_maps.get(name) {
        return match line.checked_sub(1).and_then(|index| map.get(index)) {
            Some(Some((file, original))) => {
                format!("{file}:{original} (generated Lua {name}:{line})")
            }
            _ => format!("generated Lua {name}:{line} (unmapped)"),
        };
    }
    let _ = bundle;
    format!("{name}:{line}")
}

pub fn map_error(bundle: &Bundle, error: &str) -> String {
    #[cfg(feature = "workflow-fennel-spike")]
    {
        let mut locations = Vec::new();
        for name in bundle.source_maps.keys() {
            let needle = format!("{name} (generated Lua):");
            for (i, matched) in error.match_indices(&needle) {
                // Whitespace and punctuation are legal in filenames too. A
                // known longer chunk must win over any matching filename suffix.
                if bundle.source_maps.keys().any(|other| {
                    other
                        .strip_suffix(name.as_str())
                        .is_some_and(|prefix| !prefix.is_empty() && error[..i].ends_with(prefix))
                }) {
                    continue;
                }
                // A path suffix (a.fnl inside lib/a.fnl) is not a chunk match.
                if i > 0
                    && !error[..i].chars().next_back().is_some_and(|c| {
                        c.is_whitespace() || matches!(c, ':' | '\'' | '"' | '[' | '(')
                    })
                {
                    continue;
                }
                let rest = &error[i + matched.len()..];
                let number: String = rest.chars().take_while(char::is_ascii_digit).collect();
                if !rest[number.len()..].starts_with(':') {
                    continue;
                }
                if let Ok(line) = number.parse() {
                    let location = source_location(bundle, name, line);
                    if !locations.contains(&location) {
                        locations.push(location);
                    }
                }
            }
        }
        if !locations.is_empty() {
            return format!("{}\nraw Lua error: {error}", locations.join("\n"));
        }
    }
    let _ = bundle;
    error.to_owned()
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[tokio::test]
    async fn abandoned_pending_group_is_sticky_without_jobs_and_preserves_existing_stop() {
        for stop in ["none", "signal", "earlier"] {
            let (host, dir) = super::super::host::tests::fixture();
            let lua = vm().unwrap();
            let adapter = Adapter {
                host: host.clone(),
                current: Rc::new(Cell::new(0)),
                initializing: Rc::new(RefCell::new(HashMap::new())),
                budget: Rc::new(Cell::new(0)),
                object_mt: lua.create_table().unwrap(),
            };
            let live = Rc::new(Cell::new(0));
            let counted = live.clone();
            lua.globals()
                .set(
                    "suspend",
                    lua.create_async_function(move |_, ()| {
                        let counted = counted.clone();
                        async move {
                            struct Live(Rc<Cell<usize>>);
                            impl Drop for Live {
                                fn drop(&mut self) {
                                    self.0.set(self.0.get() - 1);
                                }
                            }
                            counted.set(counted.get() + 1);
                            let _live = Live(counted);
                            std::future::pending::<mlua::Result<Value>>().await
                        }
                    })
                    .unwrap(),
                )
                .unwrap();
            install(&lua, adapter.clone()).unwrap();
            let functions: Value = lua.load("return {function() return due.concurrent('inner',{function() return suspend() end}) end}")
                .eval().unwrap();
            let mut future = Box::pin(concurrent(
                lua.clone(),
                adapter,
                Value::String(lua.create_string("outer").unwrap()),
                functions,
            ));
            poll_fn(|cx| {
                assert!(future.as_mut().poll(cx).is_pending());
                Poll::Ready(())
            })
            .await;
            assert_eq!(live.get(), 1);
            assert!(host.lock().fatal_cause().is_none());
            host.location("root.fnl:5".into(), false);
            match stop {
                "signal" => host.lock().cancelled.store(true, Ordering::SeqCst),
                "earlier" => {
                    host.scope_failure("original failure".into());
                }
                _ => (),
            }
            // Deterministically abandon the real Rust composition future. This
            // models lost ownership, not an observed Lua allocator failure.
            drop(future);
            assert_eq!(live.get(), 0, "pending mlua child future leaked");
            let error = host.finish(Ok(json!(true))).await.unwrap_err();
            match stop {
                "signal" => assert!(host.lock().fatal_cause().is_none(), "{error}"),
                "earlier" => {
                    assert_eq!(error, "original failure");
                    assert_eq!(host.lock().script_location.as_deref(), Some("root.fnl:5"));
                }
                _ => assert!(error.contains("abandoned"), "{error}"),
            }
            assert!(host.lock().steps().is_empty());
            assert!(
                host.start(
                    "after",
                    vec![json!({"id":"after","profile":"none","prompt":"never"})]
                )
                .is_err()
            );
            let events: Vec<Json> = std::fs::read_to_string(dir.path().join("events"))
                .unwrap()
                .lines()
                .map(|s| serde_json::from_str(s).unwrap())
                .collect();
            let finished: Vec<_> = events
                .iter()
                .filter(|v| v["event"] == "scope_finished")
                .map(|v| v["scope"]["id"].as_u64().unwrap())
                .collect();
            assert_eq!(finished, vec![2, 1]);
        }
    }

    #[cfg(feature = "workflow-fennel-spike")]
    #[test]
    fn mapped_locations_require_exact_generated_chunks_and_known_map_entries() {
        let bundle = Bundle {
            manifest: serde_json::from_value(json!({
                "schema_version":2,"entry":"main.lua","input_schema":"schema.json",
                "output_schema":"schema.json","max_runs":1,"max_concurrency":1,"profiles":{}
            }))
            .unwrap(),
            language: super::super::Language::Lua,
            entry: String::new(),
            entry_name: "main.lua".into(),
            modules: Default::default(),
            module_names: Default::default(),
            source_maps: [
                ("a.fnl".into(), vec![Some(("a.fnl".into(), 111))]),
                (
                    "lib/a.fnl".into(),
                    vec![Some(("lib/a.fnl".into(), 27)), None],
                ),
                ("lib a.fnl".into(), vec![Some(("lib a.fnl".into(), 39))]),
                ("lib:a.fnl".into(), vec![Some(("lib:a.fnl".into(), 41))]),
            ]
            .into(),
            input: Json::Null,
            output_validator: contract::validator(&json!(true)).unwrap(),
            profiles: Default::default(),
            snapshot: Json::Null,
        };
        let raw = "runtime error: lib/a.fnl (generated Lua):1: failure";
        assert_eq!(
            map_error(&bundle, raw),
            format!("lib/a.fnl:27 (generated Lua lib/a.fnl:1)\nraw Lua error: {raw}")
        );
        for (name, original) in [("lib a.fnl", 39), ("lib:a.fnl", 41)] {
            let raw = format!("runtime error: {name} (generated Lua):1: failure");
            assert_eq!(
                map_error(&bundle, &raw),
                format!("{name}:{original} (generated Lua {name}:1)\nraw Lua error: {raw}")
            );
        }
        for line in [0, 2, 99] {
            let raw = format!("lib/a.fnl (generated Lua):{line}: failure");
            assert_eq!(
                map_error(&bundle, &raw),
                format!("generated Lua lib/a.fnl:{line} (unmapped)\nraw Lua error: {raw}")
            );
        }
        for raw in [
            "lib/a.fnl:1: unmarked",
            "otherlib/a.fnl (generated Lua):1: failure",
            "a.fnl (generated Lua):1suffix",
        ] {
            assert_eq!(map_error(&bundle, raw), raw);
        }
        assert_eq!(chunk_name(&bundle, "a.fnl"), "@a.fnl (generated Lua)");
        assert_eq!(chunk_name(&bundle, "main.lua"), "@main.lua");
    }
}
