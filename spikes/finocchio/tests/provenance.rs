use finocchio_spike::compiler::Compiler;

#[test]
fn nested_expansions_record_real_ast_edges_and_user_callsite() {
    let source = include_str!("../examples/provenance.fnl");
    let result = Compiler::new()
        .unwrap()
        .compile(source, "user.fnl")
        .unwrap();
    let error = result.error.as_ref().unwrap();
    assert!(error.contains("not-a-real-function"), "{error}");
    let outer = result
        .expansions
        .iter()
        .find(|e| e.name == "outer")
        .unwrap();
    let inner = result
        .expansions
        .iter()
        .find(|e| e.name == "inner")
        .unwrap();
    assert_eq!(outer.file.as_deref(), Some("user.fnl"));
    assert_eq!(outer.line, Some(5));
    let start = source.find("(outer 42)").unwrap();
    assert_eq!(outer.start, Some(start));
    assert_eq!(outer.end, Some(start + "(outer 42)".len()));
    assert_eq!(
        &source[outer.start.unwrap()..outer.end.unwrap()],
        "(outer 42)"
    );
    assert!(outer.input.contains("outer 42"));
    assert!(outer.output.contains("inner 42"));
    assert!(inner.input.contains("inner 42"));
    assert!(inner.output.contains("not-a-real-function 42"));
    assert_eq!(inner.parent, Some(outer.id));
    // Do not require the generated inner node to retain the user's location:
    // same-file quasiquote can keep the macro definition's location instead.
    assert_eq!(inner.line, Some(3));
    assert!(error.starts_with("user.fnl:2:"), "{error}");
    let shifted = Compiler::new()
        .unwrap()
        .compile(&format!(";; café\n\n{source}"), "shifted.fnl")
        .unwrap();
    assert_eq!(
        shifted
            .expansions
            .iter()
            .find(|e| e.name == "outer")
            .unwrap()
            .line,
        Some(7)
    );
    assert_eq!(
        shifted
            .expansions
            .iter()
            .find(|e| e.name == "inner")
            .unwrap()
            .line,
        Some(5)
    );
    let shifted_outer = shifted
        .expansions
        .iter()
        .find(|e| e.name == "outer")
        .unwrap();
    assert_eq!(shifted_outer.start, Some(start + ";; café\n\n".len()));
    assert_eq!(
        shifted_outer.end,
        Some(start + ";; café\n\n(outer 42)".len())
    );
}

#[test]
fn failed_macro_body_has_no_successful_expansion_event() {
    let source =
        "(macro inner [x] (error :macro-exploded))\n(macro outer [x] `(inner ,x))\n(outer 7)";
    let result = Compiler::new()
        .unwrap()
        .compile(source, "body.fnl")
        .unwrap();
    let error = result.error.unwrap();
    assert!(error.contains("macro-exploded"), "{error}");
    assert!(
        result
            .expansions
            .iter()
            .any(|e| e.name == "outer" && e.line == Some(3))
    );
    assert!(!result.expansions.iter().any(|e| e.name == "inner"));
}

#[test]
fn pinned_map_is_observed_separately_from_public_macro_events() {
    let source = "(macro inner [x] `(+ ,x 1))\n(macro outer [x] `(inner ,x))\n(outer 7)";
    let result = Compiler::new().unwrap().compile(source, "map.fnl").unwrap();
    assert!(result.error.is_none(), "{result:?}");
    assert!(!result.pinned_line_map.is_empty());
    assert_eq!(
        result
            .expansions
            .iter()
            .filter(|e| e.name == "outer" || e.name == "inner")
            .count(),
        2
    );
    let lua = finocchio_spike::runtime().unwrap();
    assert_eq!(lua.load(result.lua.unwrap()).eval::<i64>().unwrap(), 8);
}

#[test]
fn runtime_error_mapping_is_pinned_and_still_points_to_macro_definition() {
    let source = "(macro inner [] `(error :nested-runtime))\n(macro outer [] `(inner))\n(outer)";
    let result = Compiler::new()
        .unwrap()
        .compile(source, "runtime.fnl")
        .unwrap();
    let code = result.lua.as_ref().unwrap();
    let line = code
        .lines()
        .position(|l| l.contains("nested-runtime"))
        .unwrap()
        + 1;
    assert!(
        result
            .pinned_line_map
            .contains(&(line, "runtime.fnl".into(), 1)),
        "{result:?}"
    );
    let lua = finocchio_spike::runtime().unwrap();
    let error = lua
        .load(code)
        .set_name("@generated.lua")
        .eval::<mlua::Value>()
        .unwrap_err()
        .to_string();
    assert!(
        error.contains(&format!("generated.lua:{line}: nested-runtime")),
        "{error}"
    );
    let outer = result
        .expansions
        .iter()
        .find(|e| e.name == "outer")
        .unwrap();
    let inner = result
        .expansions
        .iter()
        .find(|e| e.name == "inner")
        .unwrap();
    assert_eq!(outer.line, Some(3));
    assert_eq!(inner.parent, Some(outer.id));
}
