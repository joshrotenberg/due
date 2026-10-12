//! Pinned compiler front end. A disposable process bounds non-yieldable compiler
//! work; the workflow itself still uses the existing asynchronous Lua adapter.
use super::bundle::Bundle;
use mlua::{Function, HookTriggers, Lua, LuaOptions, StdLib, Table, VmState};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    cell::Cell,
    collections::BTreeMap,
    io::{Read, Seek, SeekFrom},
    process::{Child, Command, ExitCode, Stdio},
    rc::Rc,
    sync::{
        Arc,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const LIBRARY: &str = include_str!("../../assets/vendor/fennel/fennel-1.6.1.lua");
const DRIVER: &str = include_str!("../../assets/workflow/compile-fennel.lua");
pub const HASH: &str = "c3d45602041e7d8ef8a212563573df040c48a85c648a29fb4597ebed4bc38ec2";
const SOURCE_BYTES: usize = 128 * 1024;
const BUNDLE_BYTES: usize = 512 * 1024;
const GENERATED_BYTES: usize = 512 * 1024;
const RESPONSE_BYTES: u64 = 8 * 1024 * 1024;
const REQUEST_BYTES: u64 = 6 * BUNDLE_BYTES as u64 + 65536;
const MEMORY: usize = 32 * 1024 * 1024;
const OPERATIONS: u64 = 20_000_000;
const DEADLINE: Duration = Duration::from_secs(5);
type Sources = BTreeMap<String, String>;
type SourceMap = Vec<Option<(String, usize)>>;
#[derive(Deserialize, Serialize)]
struct Compiled {
    lua: String,
    map: SourceMap,
}
type Response = Result<BTreeMap<String, Compiled>, String>;

fn compiler_vm(memory: usize, operations: u64) -> mlua::Result<(Lua, Rc<Cell<bool>>)> {
    let lua = Lua::new_with(
        StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8,
        LuaOptions::default(),
    )?;
    lua.set_memory_limit(memory)?;
    // No real package library/searchers, io, os, environment, debug, or native
    // loader. The unmodified library only needs its own preloaded Lua modules.
    lua.load(
        r#"
        package = {preload={}, loaded={}, config="/\n;\n?\n!\n-", path="", cpath="", searchers={}}
        function require(name)
            if package.loaded[name] then return package.loaded[name] end
            local loader = package.preload[name]
            if not loader then error("compiler module unavailable: " .. name, 0) end
            local value = loader(name)
            package.loaded[name] = value
            return value
        end
        dofile=nil; loadfile=nil; print=nil; warn=nil; collectgarbage=nil
    "#,
    )
    .exec()?;
    let count = Rc::new(Cell::new(0u64));
    let exhausted = Rc::new(Cell::new(false));
    let stopped = exhausted.clone();
    lua.set_global_hook(
        HookTriggers::new().every_nth_instruction(1000),
        move |_, _| {
            count.set(count.get().saturating_add(1000));
            if count.get() >= operations {
                stopped.set(true);
                return Err(mlua::Error::runtime(
                    "Fennel compiler operation budget exhausted",
                ));
            }
            Ok(VmState::Continue)
        },
    )?;
    Ok((lua, exhausted))
}
fn validate_sources(sources: &Sources) -> Result<(), String> {
    if sources.len() > 33 || sources.values().map(String::len).sum::<usize>() > BUNDLE_BYTES {
        return Err("Fennel bundle input exceeds 512 KiB or 33 files".into());
    }
    for (name, source) in sources {
        if name.len() > 240 || name.chars().any(char::is_control) {
            return Err(
                "Fennel source filename must be at most 240 bytes without control characters"
                    .into(),
            );
        }
        if source.len() > SOURCE_BYTES {
            return Err(format!("{name}: Fennel source exceeds 128 KiB"));
        }
    }
    Ok(())
}
fn compile_sources(sources: Sources) -> Response {
    validate_sources(&sources)?;
    let (lua, exhausted) = compiler_vm(MEMORY, OPERATIONS).map_err(|e| e.to_string())?;
    let result = (|| -> mlua::Result<_> {
        let fennel: Table = lua
            .load(LIBRARY)
            .set_name("@vendor/fennel-1.6.1.lua")
            .eval()?;
        let driver: Function = lua
            .load(DRIVER)
            .set_name("@due/compile-fennel.lua")
            .eval()?;
        let mut output = BTreeMap::new();
        let mut total = 0usize;
        let mut mapping_bytes = 0usize;
        for (name, source) in sources {
            let (generated, map): (String, Table) = driver
                .call((fennel.clone(), source, name.clone()))
                .map_err(|e| mlua::Error::runtime(format!("{name}: {e}")))?;
            total += generated.len();
            if generated.len() > GENERATED_BYTES || total > 2 * GENERATED_BYTES {
                return Err(mlua::Error::runtime(format!(
                    "{name}: Fennel generated output budget exhausted"
                )));
            }
            // Pinned 1.6.1 compileString's second return, not the public correlate
            // approximation. Copy it out before discarding the compiler state.
            let mut lines = Vec::new();
            for index in 1..=generated.lines().count() {
                let line: Option<Table> = map.raw_get(index)?;
                let mapped = match line {
                    Some(line) => {
                        let file: Option<String> = line.raw_get(1)?;
                        let original: Option<usize> = line.raw_get(2)?;
                        file.zip(original)
                    }
                    None => None,
                };
                mapping_bytes += mapped.as_ref().map_or(4, |(file, _)| 6 * file.len() + 32);
                if mapping_bytes as u64 > RESPONSE_BYTES / 2 {
                    return Err(mlua::Error::runtime(
                        "Fennel source map output budget exhausted",
                    ));
                }
                lines.push(mapped);
            }
            output.insert(
                name,
                Compiled {
                    lua: generated,
                    map: lines,
                },
            );
        }
        Ok(output)
    })();
    // A caught hook error never restores the budget. If compiler code keeps
    // catching forever (or is in C), the parent's process deadline kills it.
    if exhausted.get() {
        return Err("Fennel compiler operation budget exhausted (sticky)".into());
    }
    result.map_err(|e| e.to_string())
}

/// Private pipe protocol. This path never parses manifests or admits jobs.
pub(crate) fn worker() -> ExitCode {
    let response = (|| {
        let mut input = Vec::new();
        std::io::stdin()
            .take(REQUEST_BYTES + 1)
            .read_to_end(&mut input)
            .map_err(|e| e.to_string())?;
        if input.len() as u64 > REQUEST_BYTES {
            return Err("compiler request too large".into());
        }
        let sources =
            serde_json::from_slice(&input).map_err(|e| format!("compiler request: {e}"))?;
        compile_sources(sources)
    })();
    match serde_json::to_writer(std::io::stdout().lock(), &response) {
        Ok(()) => ExitCode::SUCCESS,
        Err(_) => ExitCode::FAILURE,
    }
}

struct JoinedChild(Child);
impl Drop for JoinedChild {
    fn drop(&mut self) {
        // Also handles a dropped parent future. No detached blocking thread.
        let _ = self.0.kill();
        let _ = self.0.wait();
    }
}
async fn join_compiler(
    child: &mut JoinedChild,
    cancelled: &AtomicBool,
    deadline: Duration,
) -> Result<(), String> {
    let start = Instant::now();
    loop {
        if cancelled.load(Ordering::SeqCst) {
            return Err("Fennel compilation cancelled; compiler joined".into());
        }
        if start.elapsed() >= deadline {
            return Err("Fennel compiler wall-time budget exhausted; compiler joined".into());
        }
        if let Some(status) = child.0.try_wait().map_err(|e| e.to_string())? {
            return if status.success() {
                Ok(())
            } else {
                Err(format!("Fennel compiler exited {status}"))
            };
        }
        tokio::time::sleep(Duration::from_millis(5)).await;
    }
}
async fn isolated_compile(sources: Sources, cancelled: Arc<AtomicBool>) -> Response {
    validate_sources(&sources)?;
    if cancelled.load(Ordering::SeqCst) {
        return Err("Fennel compilation cancelled".into());
    }
    // Anonymous, private files avoid pipe deadlocks and unjoined pipe threads.
    // They contain only the bounded request/reply, never ambient search paths.
    let mut input = tempfile::tempfile().map_err(|e| e.to_string())?;
    serde_json::to_writer(&mut input, &sources).map_err(|e| e.to_string())?;
    input.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut output = tempfile::tempfile().map_err(|e| e.to_string())?;
    let mut child = JoinedChild(
        Command::new(std::env::current_exe().map_err(|e| e.to_string())?)
            .arg("--workflow-fennel-compiler")
            .env_clear()
            .stdin(input)
            .stdout(output.try_clone().map_err(|e| e.to_string())?)
            .stderr(Stdio::null())
            .spawn()
            .map_err(|e| format!("start isolated Fennel compiler: {e}"))?,
    );
    let result = join_compiler(&mut child, &cancelled, DEADLINE).await;
    drop(child); // kill and join before returning on every path
    result?;
    output.seek(SeekFrom::Start(0)).map_err(|e| e.to_string())?;
    let mut bytes = Vec::new();
    output
        .take(RESPONSE_BYTES + 1)
        .read_to_end(&mut bytes)
        .map_err(|e| e.to_string())?;
    if bytes.len() as u64 > RESPONSE_BYTES {
        return Err("Fennel compiler response exceeds 8 MiB".into());
    }
    serde_json::from_slice(&bytes).map_err(|e| format!("Fennel compiler response: {e}"))?
}

pub async fn prepare(mut bundle: Bundle, cancelled: Arc<AtomicBool>) -> Result<Bundle, String> {
    let is_fennel = |name: &str| name.ends_with(".fnl");
    if !is_fennel(&bundle.entry_name) && !bundle.module_names.values().any(|n| is_fennel(n)) {
        return Ok(bundle); // No compiler initialization/process for plain Lua.
    }
    let total = bundle.entry.len() + bundle.modules.values().map(String::len).sum::<usize>();
    if total > BUNDLE_BYTES {
        return Err("Fennel total bundle source input exceeds 512 KiB".into());
    }
    let mut sources = BTreeMap::new();
    if is_fennel(&bundle.entry_name) {
        sources.insert(bundle.entry_name.clone(), bundle.entry.clone());
    }
    for (key, name) in &bundle.module_names {
        if is_fennel(name) {
            sources.insert(name.clone(), bundle.modules[key].clone());
        }
    }
    let original = sources.clone();
    let compiled = isolated_compile(sources, cancelled).await?;
    if let Some(entry) = compiled.get(&bundle.entry_name) {
        bundle.entry.clone_from(&entry.lua);
    }
    for (key, name) in &bundle.module_names {
        if let Some(module) = compiled.get(name) {
            bundle.modules.insert(key.clone(), module.lua.clone());
        }
    }
    bundle.source_maps = compiled
        .iter()
        .map(|(name, c)| (name.clone(), c.map.clone()))
        .collect();
    bundle.snapshot["fennel"] = json!({
        "version":"1.6.1", "commit":"9af5e47fa7ddea806544d15507412ddc51bd046d", "sha256":HASH,
        "source_url":"https://fennel-lang.org/downloads/fennel-1.6.1.lua",
        "options":{"correlate":false,"useMetadata":false,"requireAsInclude":false,"compilerEnv":"empty","subset":"due-functions-v1"},
        "limits":{"source_bytes":SOURCE_BYTES,"bundle_bytes":BUNDLE_BYTES,"memory_bytes":MEMORY,"instructions":OPERATIONS,"wall_ms":DEADLINE.as_millis(),"generated_bytes":GENERATED_BYTES,"generated_total_bytes":2*GENERATED_BYTES,"ast_depth":64,"ast_nodes":32768,"compiler_events":4096},
        "original":original,"compiled":compiled,"driver":DRIVER,
        "mapping":"pinned compileString second return; generated Lua line -> [source filename, source line]"
    });
    Ok(bundle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn caught_compiler_hook_errors_remain_sticky() {
        for source in [
            "return pcall(function() while true do end end)",
            "return xpcall(function() while true do end end, function(e) return 'caught' end)",
        ] {
            let (lua, exhausted) = compiler_vm(MEMORY, 10_000).unwrap();
            let _ = lua.load(source).eval::<mlua::Value>();
            assert!(exhausted.get());
            assert!(
                lua.load("local n=0; for i=1,10000 do n=n+i end; return n")
                    .eval::<u64>()
                    .is_err()
            );
        }
    }

    #[test]
    fn compiler_memory_and_capability_boundary() {
        let (lua, _) = compiler_vm(1024 * 1024, OPERATIONS).unwrap();
        lua.load("assert(io==nil and os==nil and debug==nil and due==nil and package.loadlib==nil); assert(not pcall(require, 'ambient'))").exec().unwrap();
        assert!(matches!(
            lua.load("return string.rep('x', 2*1024*1024)")
                .eval::<String>(),
            Err(mlua::Error::MemoryError(_))
        ));
    }

    // Test-binary-only entry, deliberately bypassing the supported-subset gate
    // to probe the process boundary with compiler C library paths. No equivalent
    // source injection or environment switch exists in the shipped helper.
    #[cfg(unix)]
    #[test]
    fn compiler_process_probe() {
        let Some(probe) = std::env::var_os("DUE_FENNEL_TEST_PROBE") else {
            return;
        };
        let ready = std::env::var_os("DUE_FENNEL_TEST_READY").unwrap();
        let (lua, _) = compiler_vm(MEMORY, OPERATIONS).unwrap();
        std::fs::write(ready, "ready").unwrap();
        let source = match probe.to_str().unwrap() {
            "move" => "table.move({}, 1, math.maxinteger-1, 1)",
            "rep" => "string.rep('', math.maxinteger)",
            "pattern" => {
                "string.match(string.rep('a',500), string.rep('a?',50)..string.rep('a',50)..'b')"
            }
            _ => panic!("unknown test probe"),
        };
        lua.load(source).exec().unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn c_paths_are_interruptible_and_compiler_processes_are_joined() {
        for probe in ["move", "rep", "pattern"] {
            for mode in ["wall-time", "cancelled", "signal"] {
                let dir = tempfile::tempdir().unwrap();
                let ready = dir.path().join("ready");
                let mut child = JoinedChild(
                    Command::new(std::env::current_exe().unwrap())
                        .args(["--exact", "workflow::fennel::tests::compiler_process_probe"])
                        .env("DUE_FENNEL_TEST_PROBE", probe)
                        .env("DUE_FENNEL_TEST_READY", &ready)
                        .stdout(Stdio::null())
                        .stderr(Stdio::null())
                        .spawn()
                        .unwrap(),
                );
                let pid = child.0.id();
                let start = Instant::now();
                while !ready.exists() {
                    assert!(start.elapsed() < Duration::from_secs(3));
                    assert!(child.0.try_wait().unwrap().is_none());
                    tokio::time::sleep(Duration::from_millis(5)).await;
                }
                let stopped = AtomicBool::new(mode == "cancelled");
                let result = if mode == "signal" {
                    let signal = crate::shutdown_signal().unwrap();
                    let work = join_compiler(&mut child, &stopped, Duration::from_secs(2));
                    tokio::pin!(work);
                    assert_eq!(
                        unsafe { libc::kill(std::process::id() as i32, libc::SIGTERM) },
                        0
                    );
                    tokio::select! {
                        result = &mut work => result,
                        _ = signal => {
                            stopped.store(true, Ordering::SeqCst);
                            work.await
                        }
                    }
                } else {
                    join_compiler(&mut child, &stopped, Duration::from_millis(100)).await
                };
                assert!(result.unwrap_err().contains(if mode == "signal" {
                    "cancelled"
                } else {
                    mode
                }));
                drop(child);
                assert_ne!(
                    unsafe { libc::kill(pid as i32, 0) },
                    0,
                    "compiler was not joined"
                );
            }
        }
    }
}
