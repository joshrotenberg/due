use mlua::{Function, HookTriggers, Lua, LuaSerdeExt, MultiValue, Table, VmState};
use serde::{Deserialize, Serialize};
use std::{cell::Cell, rc::Rc};

pub const VENDOR: &str = include_str!("../../../assets/vendor/fennel/fennel-1.6.1.lua");
pub const MACROS: &str = include_str!("../lua/macros.fnl");

#[derive(Debug, Serialize, Deserialize)]
pub struct Expansion {
    pub id: usize,
    pub parent: Option<usize>,
    pub name: String,
    pub file: Option<String>,
    pub line: Option<usize>,
    pub start: Option<usize>,
    pub end: Option<usize>,
    pub input: String,
    pub output: String,
}

#[derive(Debug, Serialize, Deserialize)]
pub struct Compilation {
    pub lua: Option<String>,
    pub error: Option<String>,
    pub expansions: Vec<Expansion>,
    /// Experimental, deliberately labelled pinned implementation-dependent data.
    pub pinned_line_map: Vec<(usize, String, usize)>,
}

pub struct Compiler {
    pub(crate) lua: Lua,
    pub(crate) fennel: Table,
    exhausted: Rc<Cell<bool>>,
}

fn check_budget(exhausted: &Cell<bool>) -> mlua::Result<()> {
    if exhausted.get() {
        Err(mlua::Error::runtime(
            "compiler instruction budget exhausted (sticky)",
        ))
    } else {
        Ok(())
    }
}

impl Compiler {
    pub fn new() -> mlua::Result<Self> {
        let lua = crate::bare_vm()?;
        // Only the vendor's embedded preload modules. Never filesystem searchers.
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
        "#,
        )
        .exec()?;
        // Fennel's macro error handler calls debug.traceback. Expose only mlua's
        // read-only stack formatting, never Lua's debug mutation facilities.
        let debug = lua.create_table()?;
        debug.set(
            "traceback",
            lua.create_function(|lua, (message, skip): (String, Option<usize>)| {
                lua.traceback(Some(&message), skip.unwrap_or(1))
            })?,
        )?;
        lua.globals().set("debug", debug)?;
        let exhausted = Rc::new(Cell::new(false));
        // Ordinary hook errors ARE catchable. Guard every protected-call exit,
        // including vendor-internal calls, before loading the compiler so no
        // compiler closure captures a raw pcall/xpcall. The originals live only
        // in Rust closures, inaccessible through macro tables/metatables.
        for name in ["pcall", "xpcall"] {
            let raw: Function = lua.globals().get(name)?;
            let stopped = exhausted.clone();
            lua.globals().set(
                name,
                lua.create_function(move |_, args: MultiValue| {
                    check_budget(&stopped)?;
                    let result = raw.call::<MultiValue>(args);
                    check_budget(&stopped)?;
                    result
                })?,
            )?;
        }
        let fennel: Table = lua.load(VENDOR).set_name("@fennel-1.6.1.lua").eval()?;
        let stopped = exhausted.clone();
        let count = Cell::new(0usize);
        lua.set_global_hook(
            HookTriggers::new().every_nth_instruction(1000),
            move |_, _| {
                count.set(count.get() + 1000);
                if count.get() > 10_000_000 {
                    stopped.set(true);
                    return Err(mlua::Error::runtime(
                        "compiler instruction budget exhausted",
                    ));
                }
                Ok(VmState::Continue)
            },
        )?;
        Ok(Self {
            lua,
            fennel,
            exhausted,
        })
    }

    pub fn compile(&self, source: &str, filename: &str) -> mlua::Result<Compilation> {
        check_budget(&self.exhausted)?;
        if source.len() > 128 * 1024 {
            return Err(mlua::Error::runtime("source exceeds 128 KiB"));
        }
        let driver: Function = self.lua.load(include_str!("../lua/compile.lua")).eval()?;
        let value = driver.call((self.fennel.clone(), source, filename, MACROS));
        check_budget(&self.exhausted)?;
        self.lua.from_value(value?)
    }
}

pub fn compile(source: &str, filename: &str) -> mlua::Result<String> {
    let result = Compiler::new()?.compile(source, filename)?;
    result
        .lua
        .ok_or_else(|| mlua::Error::runtime(result.error.unwrap_or_default()))
}
