use finocchio_spike::{
    compiler, runtime,
    semantic::{self, Sources},
};
use mlua::Value;
use serde_json::{Value as Json, json};

fn sources() -> Sources {
    [
        (
            "math".into(),
            include_str!("../examples/semantic/math.fnl").into(),
        ),
        (
            "main".into(),
            include_str!("../examples/semantic/main.fnl").into(),
        ),
    ]
    .into()
}
fn check(s: &Sources) -> Json {
    semantic::request(s, json!({"op":"check"})).unwrap()
}
fn rename(s: &Sources, file: &str, offset: usize, name: &str) -> Json {
    semantic::request(
        s,
        json!({"op":"rename","file":file,"offset":offset,"name":name}),
    )
    .unwrap()
}
fn evaluate(s: &Sources) -> i64 {
    let lua = runtime().unwrap();
    let math = compiler::compile(&s["math"], "math").unwrap();
    let exports: Value = lua.load(math).eval().unwrap();
    lua.globals()
        .set(
            "require",
            lua.create_function(move |_, name: String| {
                assert_eq!(name, "math");
                Ok(exports.clone())
            })
            .unwrap(),
        )
        .unwrap();
    let main = compiler::compile(&s["main"], "main").unwrap();
    lua.load(main).eval().unwrap()
}

#[test]
fn cross_module_definition_references_and_binding_rename_preserve_exports() {
    let s = sources();
    let report = check(&s);
    assert_eq!(report["supported"], true, "{report}");
    let offset = s["main"].find("alias.bump").unwrap() + 6;
    let definition =
        semantic::request(&s, json!({"op":"definition","file":"main","offset":offset})).unwrap();
    assert_eq!(definition["definition"]["name"], "bump", "{definition}");
    assert_eq!(definition["definition"]["file"], "math");
    assert!(
        definition["references"]
            .as_array()
            .unwrap()
            .iter()
            .any(|r| r["file"] == "main")
    );
    let renamed = rename(&s, "main", offset, "raise");
    assert_eq!(renamed["supported"], true, "{renamed}");
    let changed: Sources = serde_json::from_value(renamed["sources"].clone()).unwrap();
    assert!(changed["math"].contains("(defn raise"));
    assert!(changed["math"].contains(":bump raise"));
    assert!(changed["math"].contains("(fn shadow [bump] (+ bump 1))"));
    assert!(changed["math"].contains(";; bump in this comment"));
    assert!(changed["math"].contains("(local label \"bump\")"));
    assert_eq!(changed["main"], s["main"]); // Stable explicit export keys.
    assert_eq!(evaluate(&s), 5);
    assert_eq!(evaluate(&changed), 5);
    let alias = rename(&s, "main", s["main"].find("alias").unwrap(), "tools");
    assert_eq!(alias["supported"], true, "{alias}");
    assert!(
        alias["sources"]["main"]
            .as_str()
            .unwrap()
            .contains("(tools.bump 3)")
    );
}

#[test]
fn bad_calls_have_specific_diagnostics_and_unknown_forms_fail_closed() {
    let mut s = sources();
    s.insert(
        "main".into(),
        "(local m (require :math))\n(m.bump :wrong)\n(m.bump 1 2)".into(),
    );
    let report = check(&s);
    assert_eq!(report["supported"], true, "{report}");
    let diagnostics = report["diagnostics"].as_array().unwrap();
    assert_eq!(diagnostics.len(), 2);
    assert_eq!(diagnostics[0]["line"], 2);
    assert!(
        diagnostics[0]["message"]
            .as_str()
            .unwrap()
            .contains("expected integer, got string")
    );
    assert_eq!(diagnostics[1]["line"], 3);
    for (diagnostic, call) in diagnostics.iter().zip(["(m.bump :wrong)", "(m.bump 1 2)"]) {
        let start = s["main"].find(call).unwrap();
        assert_eq!(diagnostic["start"], start);
        assert_eq!(diagnostic["finish"], start + call.len());
    }
    assert!(
        diagnostics[1]["message"]
            .as_str()
            .unwrap()
            .contains("expected 1 arguments, got 2")
    );
    for body in [
        "(mystery x)",
        "(macro surprise [] 1) (surprise)",
        "(import-macros {: strange} :unknown)",
        "(local [a b] [1 2])",
        "(local x 1) `(unknown ,x)",
        "(local name :math) (require name)",
        "(local m (require :math)) (m.unknown 1)",
        "(fn f [...] ...)",
        "(if true (local x 1) (local y 2))",
        "(local f (fn named [] 1))",
        "(fn when [x] x) (when 1)",
        "(import-macros {: defn} :finocchio) (defn f [x :mystery] \"doc\" x)",
    ] {
        s.insert("main".into(), body.into());
        let report = check(&s);
        assert_eq!(report["supported"], false, "{body}: {report}");
        assert!(report.get("definitions").is_none());
        assert_eq!(rename(&s, "main", 0, "new")["supported"], false);
    }
    s.insert("main".into(), "(require :main)".into());
    assert!(check(&s)["error"].as_str().unwrap().contains("cycle"));
}

#[test]
fn shadowing_capture_unicode_and_generated_symbols() {
    let source =
        ";; café x\n(local x 10)\n(fn f [y] (+ x y))\n(fn g [x] (+ x 2))\n{:x x :f f :g g}";
    let s: Sources = [("m".into(), source.into())].into();
    let at = source.find("x 10").unwrap();
    let r = rename(&s, "m", at, "outer");
    assert_eq!(r["supported"], true, "{r}");
    let changed = r["sources"]["m"].as_str().unwrap();
    assert!(changed.contains("(fn f [y] (+ outer y))"));
    assert!(changed.contains("(fn g [x] (+ x 2))"));
    assert!(changed.contains(";; café x"));
    assert_eq!(rename(&s, "m", at, "y")["supported"], false);
    assert_eq!(rename(&s, "m", at, "f")["supported"], false);
    assert_eq!(rename(&s, "m", at, "a.b")["supported"], false);
    assert_eq!(rename(&s, "m", at, "when")["supported"], false);
    let source = "(import-macros {: twice} :finocchio) (local x 4) (twice x)";
    let s: Sources = [("m".into(), source.into())].into();
    let r = rename(&s, "m", source.find("x 4").unwrap(), "once");
    assert_eq!(r["supported"], true, "{r}");
    let changed = r["sources"]["m"].as_str().unwrap();
    let compilation = compiler::Compiler::new()
        .unwrap()
        .compile(changed, "m")
        .unwrap();
    let expansion = compilation
        .expansions
        .iter()
        .find(|e| e.name == "twice")
        .unwrap();
    assert!(expansion.output.contains("once"));
    let lua = runtime().unwrap();
    assert_eq!(lua.load(compilation.lua.unwrap()).eval::<i64>().unwrap(), 8);
    let report = check(&s);
    assert_eq!(report["definitions"].as_array().unwrap().len(), 1);
}

#[test]
fn native_functions_keep_lua_arity_semantics_and_empty_results_are_arrays() {
    let s: Sources = [("m".into(), "(fn first [x] x) (first 1 2)".into())].into();
    let report = check(&s);
    assert_eq!(report["supported"], true, "{report}");
    assert_eq!(report["diagnostics"], json!([]));
    assert!(
        report["definitions"]
            .as_array()
            .unwrap()
            .iter()
            .all(|d| d.get("value").is_none())
    );
    let lua = runtime().unwrap();
    assert_eq!(
        lua.load(compiler::compile(&s["m"], "m").unwrap())
            .eval::<i64>()
            .unwrap(),
        1
    );
    let invalid = semantic::request(&s, json!({"op":"rename","file":"m"})).unwrap();
    assert_eq!(invalid["supported"], false);
    let empty = check(&[("m".into(), "42".into())].into());
    for field in ["definitions", "references", "diagnostics"] {
        assert_eq!(empty[field], json!([]), "{empty}");
    }
    let unused: Sources = [("m".into(), "(local x 1)".into())].into();
    let definition =
        semantic::request(&unused, json!({"op":"definition","file":"m","offset":7})).unwrap();
    assert_eq!(definition["references"], json!([]));
    assert!(definition["definition"].get("value").is_none());
    let renamed = rename(&unused, "m", 7, "y");
    assert_eq!(renamed["diagnostics"], json!([]));
    assert!(renamed["edits"].is_array());
    assert!(renamed["edits"][0].get("value").is_none());
}

#[test]
fn shorthand_maps_fail_closed_even_with_comments_and_whitespace() {
    for map in [
        "{: x}",
        "{:  \n\t x}",
        "{: ; x is a value\n x}",
        "{; lead\n :\n; middle\n x}",
    ] {
        for source in [
            format!("(local x 1) {map}"),
            format!("(fn x [] 1) {map}"),
            format!("(fn f [m] m) (local x 1) (f {map})"),
        ] {
            // These are valid Fennel, but the semantic subset must not offer edits.
            let code = compiler::compile(&source, "m").unwrap();
            let lua = runtime().unwrap();
            let table: mlua::Table = lua.load(code).eval().unwrap();
            assert!(!table.get::<Value>("x").unwrap().is_nil());
            let s: Sources = [("m".into(), source.clone())].into();
            let report = check(&s);
            assert_eq!(report["supported"], false, "{source}: {report}");
            assert!(
                report["error"].as_str().unwrap().contains("shorthand"),
                "{report}"
            );
            let renamed = rename(&s, "m", source.find("x ").unwrap(), "y");
            assert_eq!(renamed["supported"], false, "{renamed}");
            assert!(renamed.get("edits").is_none());
            assert!(renamed.get("sources").is_none());
        }
    }
    // Explicit keys stay supported, including comments that look like shorthand.
    for map in ["{:x ; : x\n x}", "{\"x\"\n x}"] {
        let source = format!("(local x 1) {map}");
        let s: Sources = [("m".into(), source)].into();
        let renamed = rename(&s, "m", 7, "y");
        assert_eq!(renamed["supported"], true, "{renamed}");
        let code = compiler::compile(renamed["sources"]["m"].as_str().unwrap(), "m").unwrap();
        let lua = runtime().unwrap();
        let table: mlua::Table = lua.load(code).eval().unwrap();
        assert_eq!(table.get::<i64>("x").unwrap(), 1);
        assert!(table.get::<Value>("y").unwrap().is_nil());
    }
}

#[test]
fn integer_subtype_survives_locals_aliases_and_module_calls() {
    for (literal, diagnostics) in [("5", 0), ("5.0", 1), ("5.5", 1), (":wrong", 1)] {
        let source = format!(
            "(import-macros {{: defn}} :finocchio) (defn f [x :integer] \"f\" x) (local n {literal}) (local alias n) (f alias)"
        );
        let s: Sources = [("m".into(), source)].into();
        let report = check(&s);
        assert_eq!(report["supported"], true, "{report}");
        assert_eq!(
            report["diagnostics"].as_array().unwrap().len(),
            diagnostics,
            "{report}"
        );
        let result = runtime()
            .unwrap()
            .load(compiler::compile(&s["m"], "m").unwrap())
            .eval::<Value>();
        assert_eq!(result.is_ok(), diagnostics == 0);
    }
    let mut s = sources();
    s.insert(
        "main".into(),
        "(local m (require :math)) (local n 5) (m.bump n)".into(),
    );
    assert_eq!(check(&s)["diagnostics"], json!([]));
    assert_eq!(evaluate(&s), 7);
}

#[test]
fn rename_rejects_generated_contract_capture_in_locals_and_parameters() {
    for body in [
        "(local x 1) (defn f [] \"f\" x) (f)",
        "(fn outer [x] (defn f [] \"f\" x) (f)) (outer 1)",
        "(defn f [x :integer] \"f\" x) (f 1)",
    ] {
        let source = format!("(import-macros {{: defn}} :finocchio) {body}");
        let at = source.find("x").unwrap();
        let s: Sources = [("m".into(), source)].into();
        assert_eq!(check(&s)["supported"], true);
        let rejected = rename(&s, "m", at, "contract");
        assert_eq!(rejected["supported"], false, "{rejected}");
        assert!(rejected.get("edits").is_none());
        let renamed = rename(&s, "m", at, "_G");
        assert_eq!(renamed["supported"], true, "{renamed}");
        let code = compiler::compile(renamed["sources"]["m"].as_str().unwrap(), "m").unwrap();
        assert_eq!(runtime().unwrap().load(code).eval::<i64>().unwrap(), 1);
    }
}
