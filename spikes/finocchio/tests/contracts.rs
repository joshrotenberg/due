use finocchio_spike::{compiler, runtime};
use mlua::{Function, LuaSerdeExt, Table};

#[test]
fn ordinary_recursive_higher_order_and_lua_calls_share_contracts() {
    let code =
        compiler::compile(include_str!("../examples/contracts.fnl"), "contracts.fnl").unwrap();
    let lua = runtime().unwrap();
    let exports: Table = lua.load(code).eval().unwrap();
    assert_eq!(exports.get::<i64>("example").unwrap(), 24);
    let factorial: Function = exports.get("factorial").unwrap();
    assert_eq!(factorial.call::<i64>(6).unwrap(), 720);
    lua.globals().set("sample", exports).unwrap();
    assert_eq!(
        lua.load("return sample['apply-one'](sample.factorial, 5)")
            .eval::<i64>()
            .unwrap(),
        120
    );
    let doc = lua
        .load("return contract.describe(sample.factorial)")
        .eval()
        .unwrap();
    let doc: serde_json::Value = lua.from_value(doc).unwrap();
    assert_eq!(doc["parameters"][0]["name"], "n");
    assert_eq!(doc["parameters"][0]["type"], "integer");
    assert_eq!(doc["arity"], 1);
    assert!(doc["doc"].as_str().unwrap().contains("Factorial"));
    lua.load("contract.describe(sample.factorial).parameters[1].type='any'")
        .exec()
        .unwrap();
    for (args, expected) in [
        ("'bad'", "argument 1 (n): expected integer, got string"),
        ("nil", "argument 1 (n): expected integer, got nil"),
        ("1.5", "argument 1 (n): expected integer, got number"),
        ("", "expected 1 arguments, got 0"),
        ("1,nil", "expected 1 arguments, got 2"),
    ] {
        let error = lua
            .load(format!("return sample.factorial({args})"))
            .eval::<mlua::Value>()
            .unwrap_err()
            .to_string();
        assert!(error.contains(expected), "{error}");
    }
    assert_eq!(
        lua.load("return sample.greeting(nil)")
            .eval::<String>()
            .unwrap(),
        "anonymous"
    );
    assert!(
        lua.load("return sample.greeting()")
            .eval::<String>()
            .is_err()
    );
    // Internal recursive calls also pass through the wrapper.
    let code = compiler::compile("(import-macros {: defn} :finocchio) (defn bad [n :integer] \"recursive\" (if (= n 0) 0 (bad :wrong))) bad", "recursive.fnl").unwrap();
    let bad: Function = lua.load(code).eval().unwrap();
    assert!(
        bad.call::<i64>(1)
            .unwrap_err()
            .to_string()
            .contains("contract bad argument 1")
    );
}

#[test]
fn exact_nil_positions_multiple_returns_and_unknown_types() {
    let lua = runtime().unwrap();
    let code = compiler::compile("(import-macros {: defn} :finocchio) (defn f [a :nil b :string?] \"nil slots\" (values a b)) f", "nil.fnl").unwrap();
    let f: Function = lua.load(code).eval().unwrap();
    assert_eq!(
        f.call::<(Option<String>, Option<String>)>((mlua::Value::Nil, "ok"))
            .unwrap(),
        (None, Some("ok".into()))
    );
    assert!(f.call::<mlua::Value>(mlua::Value::Nil).is_err());
    assert!(
        lua.load(
            "return contract.wrap('bad',{{name='x',type='mystery'}},'',function(x) return x end)"
        )
        .eval::<Function>()
        .is_err()
    );
}

#[test]
fn defn_rejects_contract_capture_without_introducing_a_global_table_capture() {
    for source in [
        "(local contract 1) (defn f [] \"f\" 1)",
        "(fn outer [contract] (defn f [] \"f\" 1) (f))",
        "(defn f [contract :integer] \"f\" contract)",
        "(defn contract [] \"f\" 1)",
        "(local _ENV 1) (defn f [] \"f\" 1)",
        "(fn outer [_ENV] (defn f [] \"f\" 1) (f))",
        "(defn f [_ENV :integer] \"f\" _ENV)",
        "(defn _ENV [] \"f\" 1)",
    ] {
        let error = compiler::compile(
            &format!("(import-macros {{: defn}} :finocchio) {source}"),
            "capture.fnl",
        )
        .unwrap_err()
        .to_string();
        assert!(error.contains("contract"), "{source}: {error}");
    }
    // No _G workaround: shadowing _G in either position cannot capture define.
    for source in [
        "(local _G 1) (defn f [] \"f\" 7) (f)",
        "(fn outer [_G] (defn f [] \"f\" 7) (f)) (outer 1)",
        "(defn f [_G :integer] \"f\" _G) (f 7)",
    ] {
        let code = compiler::compile(
            &format!("(import-macros {{: defn}} :finocchio) {source}"),
            "global-table.fnl",
        )
        .unwrap();
        assert_eq!(runtime().unwrap().load(code).eval::<i64>().unwrap(), 7);
    }
}
