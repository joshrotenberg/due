use super::{
    bundle::Bundle,
    contract,
    host::{RunHandle, SharedHost},
};
use rhai::{
    AST, Array, Dynamic, Engine, EvalAltResult, Module, OptimizationLevel, Scope,
    module_resolvers::StaticModuleResolver,
};
use std::{
    collections::BTreeMap,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
};

type ScriptResult<T> = Result<T, Box<EvalAltResult>>;

pub(super) fn engine(host: Option<SharedHost>, cancelled: Arc<AtomicBool>) -> Engine {
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
    engine.register_type_with_name::<RunHandle>("RunHandle");
    let mut due = Module::new();
    let run_host = host.clone();
    due.set_native_fn("start", move |job: Dynamic| -> ScriptResult<RunHandle> {
        let h = run_host
            .as_ref()
            .ok_or("effects unavailable during check")?;
        let job = contract::from_dynamic(&job).map_err(|e| h.fail(e))?;
        Ok(h.start("run", vec![job])?.remove(0))
    });
    let run_host = host.clone();
    due.set_native_fn("run", move |job: Dynamic| -> ScriptResult<Dynamic> {
        let h = run_host
            .as_ref()
            .ok_or("effects unavailable during check")?;
        let job = contract::from_dynamic(&job).map_err(|e| h.fail(e))?;
        let value = tokio::runtime::Handle::current().block_on(h.parallel("run", vec![job]))?;
        contract::to_dynamic(&value[0]).map_err(Into::into)
    });
    let batch_host = host.clone();
    due.set_native_fn(
        "parallel",
        move |group: &str, jobs: Array| -> ScriptResult<Dynamic> {
            let h = batch_host
                .as_ref()
                .ok_or("effects unavailable during check")?;
            let jobs = jobs
                .iter()
                .map(contract::from_dynamic)
                .collect::<Result<Vec<_>, _>>()
                .map_err(|e| h.fail(e))?;
            let value = tokio::runtime::Handle::current().block_on(h.parallel(group, jobs))?;
            contract::to_dynamic(&value).map_err(Into::into)
        },
    );
    for op in ["wait", "cancel"] {
        let wait_host = host.clone();
        due.set_native_fn(op, move |handle: Dynamic| -> ScriptResult<Dynamic> {
            let h = wait_host
                .as_ref()
                .ok_or("effects unavailable during check")?;
            let handle = handle
                .try_cast::<RunHandle>()
                .ok_or_else(|| h.fail("expected opaque RunHandle".into()))?;
            let runtime = tokio::runtime::Handle::current();
            let value = if op == "cancel" {
                runtime.block_on(h.cancel(handle))
            } else {
                runtime.block_on(h.wait(handle))
            }?;
            contract::to_dynamic(&value).map_err(Into::into)
        });
    }
    let any_host = host.clone();
    due.set_native_fn("wait_any", move |handles: Array| -> ScriptResult<Dynamic> {
        let h = any_host
            .as_ref()
            .ok_or("effects unavailable during check")?;
        let handles = handles
            .into_iter()
            .map(|v| {
                v.try_cast::<RunHandle>()
                    .ok_or_else(|| h.fail("expected opaque RunHandle".into()))
            })
            .collect::<Result<Vec<_>, _>>()?;
        let value = tokio::runtime::Handle::current().block_on(h.wait_any(handles))?;
        contract::to_dynamic(&value).map_err(Into::into)
    });
    let phase_host = host.clone();
    due.set_native_fn("phase", move |phase: &str| -> ScriptResult<()> {
        phase_host
            .as_ref()
            .ok_or("effects unavailable during check")?
            .phase(phase)
            .map_err(Into::into)
    });
    due.set_native_fn("log", move |message: &str| -> ScriptResult<()> {
        host.as_ref()
            .ok_or("effects unavailable during check")?
            .log(message)
            .map_err(Into::into)
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
        let mut ast = engine.compile(source).map_err(|e| format!("{name}: {e}"))?;
        ast.set_source(name);
        let mut reserved = false;
        ast.walk(&mut |nodes| {
            if let Some(rhai::ASTNode::Stmt(stmt)) = nodes.last() {
                let ident = match stmt {
                    rhai::Stmt::Import(pair, _) => Some(&pair.1),
                    rhai::Stmt::Var(var, _, _) => Some(&var.0),
                    _ => None,
                };
                reserved |= ident.is_some_and(|id| matches!(id.as_str(), "due" | "json" | "input"));
            }
            !reserved
        });
        if reserved {
            return Err(format!("{name}: reserved host binding"));
        }

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

pub fn run(host: SharedHost, runtime: tokio::runtime::Handle) -> Result<serde_json::Value, String> {
    let _runtime = runtime.enter();
    let cancelled = host.lock().cancelled.clone();
    let mut engine = engine(Some(host.clone()), cancelled);
    let compiled = compile(&engine, &host.lock().bundle)?;
    install_modules(&mut engine, compiled.modules)?;
    let mut scope = Scope::new();
    scope.push_constant_dynamic("input", contract::to_dynamic(&host.lock().bundle.input)?);
    let value = engine
        .eval_ast_with_scope::<Dynamic>(&mut scope, &compiled.entry)
        .map_err(|e| e.to_string())?;
    contract::from_dynamic(&value)
}
