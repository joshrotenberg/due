use super::{Host, bundle::Bundle, contract};
use rhai::{
    AST, Array, Dynamic, Engine, EvalAltResult, Module, OptimizationLevel, Scope,
    module_resolvers::StaticModuleResolver,
};
use std::{
    cell::RefCell,
    collections::BTreeMap,
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

type ScriptResult<T> = Result<T, Box<EvalAltResult>>;

pub(super) fn engine(host: Option<Rc<RefCell<Host>>>, cancelled: Arc<AtomicBool>) -> Engine {
    // Select pure data packages explicitly. The standard package also exposes
    // blocking sleep and Rhai's permissive parse_json; neither belongs here.
    use rhai::packages::*;
    let mut engine = Engine::new_raw();
    ArithmeticPackage::new().register_into_engine(&mut engine);
    LogicPackage::new().register_into_engine(&mut engine);
    BasicStringPackage::new().register_into_engine(&mut engine);
    MoreStringPackage::new().register_into_engine(&mut engine);
    BasicIteratorPackage::new().register_into_engine(&mut engine);
    BasicFnPackage::new().register_into_engine(&mut engine);
    BasicMathPackage::new().register_into_engine(&mut engine);
    BasicArrayPackage::new().register_into_engine(&mut engine);
    BasicMapPackage::new().register_into_engine(&mut engine);
    engine
        .set_optimization_level(OptimizationLevel::None)
        .set_module_resolver(StaticModuleResolver::new())
        .set_max_operations(100_000)
        .set_max_call_levels(32)
        .set_max_expr_depths(64, 32)
        .set_max_variables(256)
        .set_max_functions(128)
        .set_max_modules(64)
        .set_max_string_size(contract::MAX_BYTES)
        .set_max_array_size(4096)
        .set_max_map_size(256)
        .set_allow_shadowing(false)
        .set_fail_on_invalid_map_property(true)
        .disable_symbol("eval");
    engine.on_progress(move |_| {
        cancelled
            .load(Ordering::SeqCst)
            .then(|| "workflow stopped".into())
    });
    // Silence implicit diagnostics. Explicit due::log is private, bounded JSONL.
    engine.on_print(|_| {}).on_debug(|_, _, _| {});
    #[allow(deprecated)]
    engine.on_def_var(|_, info, _| Ok(!matches!(info.name(), "input" | "due" | "json")));
    let mut due = Module::new();
    let run_host = host.clone();
    due.set_native_fn("run", move |job: Dynamic| -> ScriptResult<Dynamic> {
        let host = run_host
            .as_ref()
            .ok_or("effects unavailable during check")?;
        host.borrow_mut()
            .effect(|host| host.batch("run", vec![job]).map(|mut v| v.remove(0)))
    });
    let batch_host = host.clone();
    due.set_native_fn(
        "parallel",
        move |group: &str, jobs: Array| -> ScriptResult<Array> {
            let host = batch_host
                .as_ref()
                .ok_or("effects unavailable during check")?;
            host.borrow_mut().effect(|host| host.batch(group, jobs))
        },
    );
    let phase_host = host.clone();
    due.set_native_fn("phase", move |phase: &str| -> ScriptResult<()> {
        let host = phase_host
            .as_ref()
            .ok_or("effects unavailable during check")?;
        host.borrow_mut().effect(|host| {
            if phase.len() > 256 {
                return Err("phase exceeds 256 bytes".into());
            }
            host.phase = phase.into();
            host.event("phase", serde_json::json!({}))
        })
    });
    due.set_native_fn("log", move |message: &str| -> ScriptResult<()> {
        let host = host.as_ref().ok_or("effects unavailable during check")?;
        host.borrow_mut().effect(|host| {
            if message.len() > 4096 {
                return Err("log exceeds 4096 bytes".into());
            }
            host.event("log", serde_json::json!({"message":message}))
        })
    });
    engine.register_static_module("due", due.into());
    let mut json = Module::new();
    json.set_native_fn("encode", |value: Dynamic| -> ScriptResult<String> {
        let value = contract::from_dynamic(&value).map_err(Box::<EvalAltResult>::from)?;
        serde_json::to_string(&value).map_err(|e| e.to_string().into())
    });
    engine.register_static_module("json", json.into());
    engine
}

pub struct Compiled {
    pub entry: AST,
    pub modules: BTreeMap<String, AST>,
}
pub fn compile(engine: &Engine, bundle: &Bundle) -> Result<Compiled, String> {
    fn one(engine: &Engine, source: &str, name: &str) -> Result<AST, String> {
        reserved(source).map_err(|e| format!("{name}: {e}"))?;
        let mut ast = engine.compile(source).map_err(|e| format!("{name}: {e}"))?;
        ast.set_source(name);
        for f in ast.iter_functions() {
            if matches!(f.name, "due" | "json" | "input")
                || f.params
                    .iter()
                    .any(|p| matches!(*p, "due" | "json" | "input"))
            {
                return Err(format!("{name}: reserved function or parameter name"));
            }
        }
        Ok(ast)
    }
    let entry = one(engine, &bundle.entry, "entry")?;
    let mut modules = BTreeMap::new();
    for (name, source) in &bundle.modules {
        modules.insert(
            name.clone(),
            one(engine, source, name)?.clone_functions_only(),
        );
    }
    Ok(Compiled { entry, modules })
}
pub fn install_modules(engine: &mut Engine, modules: BTreeMap<String, AST>) -> Result<(), String> {
    let mut resolver = StaticModuleResolver::new();
    for (name, functions) in modules {
        // Only function definitions survive; never evaluate source initializers,
        // including imports/constants. Functions import dependencies internally.
        let module =
            Module::eval_ast_as_new(Scope::new(), &functions, engine).map_err(|e| e.to_string())?;
        resolver.insert(name, module);
    }
    engine.set_module_resolver(resolver);
    Ok(())
}

// A deliberately narrow lexical policy, not a replacement parser: host namespace
// identifiers may only precede `::`. This also forbids import aliases that could
// hide a static host module. Rhai performs actual parsing afterwards. Backtick
// interpolation is explicitly outside this spike (use string + json::encode).
fn reserved(source: &str) -> Result<(), String> {
    let bytes = source.as_bytes();
    let mut tokens = Vec::new();
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_whitespace() {
            i += 1;
            continue;
        }
        if bytes[i..].starts_with(b"//") {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        if bytes[i..].starts_with(b"/*") {
            i += 2;
            let mut depth = 1;
            while i < bytes.len() && depth > 0 {
                if bytes[i..].starts_with(b"/*") {
                    depth += 1;
                    i += 2;
                } else if bytes[i..].starts_with(b"*/") {
                    depth -= 1;
                    i += 2;
                } else {
                    i += 1;
                }
            }
            continue;
        }
        if bytes[i] == b'`' {
            return Err(
                "backtick interpolation unsupported; concatenate strings explicitly".into(),
            );
        }
        if bytes[i] == b'"' || bytes[i] == b'\'' {
            let quote = bytes[i];
            i += 1;
            while i < bytes.len() {
                if bytes[i] == b'\\' {
                    i = (i + 2).min(bytes.len());
                } else if bytes[i] == quote {
                    i += 1;
                    break;
                } else {
                    i += 1;
                }
            }
            tokens.push("literal".to_string());
            continue;
        }
        let start = i;
        if bytes[i].is_ascii_alphabetic() || bytes[i] == b'_' {
            i += 1;
            while i < bytes.len() && (bytes[i].is_ascii_alphanumeric() || bytes[i] == b'_') {
                i += 1;
            }
            tokens.push(source[start..i].to_string());
        } else if bytes[i..].starts_with(b"::") {
            i += 2;
            tokens.push("::".into());
        } else {
            i += 1;
            tokens.push("punctuation".into());
        }
    }
    for (i, token) in tokens.iter().enumerate() {
        if matches!(token.as_str(), "due" | "json")
            && tokens.get(i + 1).map(String::as_str) != Some("::")
        {
            return Err(format!("reserved namespace {token} cannot be shadowed"));
        }
        if token == "input"
            && i > 0
            && matches!(
                tokens[i - 1].as_str(),
                "as" | "for" | "let" | "const" | "fn"
            )
        {
            return Err("input is reserved".into());
        }
    }
    Ok(())
}
