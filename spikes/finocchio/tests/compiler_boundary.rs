use finocchio_spike::{
    compiler::{self, BoundaryProbe, Compiler},
    runtime,
};
use mlua::{Function, Table, Value};
use serde_json::Value as Json;

fn probe(source: &str) -> BoundaryProbe {
    Compiler::new()
        .unwrap()
        .boundary_probe(source, "boundary.fnl")
        .unwrap()
}
fn event<'a>(report: &'a BoundaryProbe, name: &str) -> &'a Json {
    report.events.iter().find(|e| e["name"] == name).unwrap()
}
fn link(event: &Json, id: &Json, role: &str, origin: &str) -> bool {
    event["links"].as_array().unwrap().iter().any(|l| {
        l["expansion"] == *id
            && l["role"] == role
            && l["origin"] == origin
            && l["confidence"] == "exact-identity"
    })
}
fn slice<'a>(source: &'a str, location: &Json) -> &'a str {
    &source[location["start"].as_u64().unwrap() as usize
        ..location["finish"].as_u64().unwrap() as usize]
}

#[test]
fn generated_free_references_capture_and_existing_defn_rejects_shadows() {
    let free = "(macro free [] `(. contract :marker))";
    for (body, expected) in [
        ("(free)", 1),
        ("(let [contract {:marker 2}] (free))", 2),
        (
            "(let [contract {:marker 2}] (let [contract {:marker 3}] (free)))",
            3,
        ),
        (
            "(macro introduced [] `(let [,(sym :contract) {:marker 4}] (free))) (introduced)",
            4,
        ),
        ("(let [_ENV {:contract {:marker 5}}] (free))", 5),
    ] {
        let code = compiler::compile(&format!("{free} {body}"), "capture.fnl").unwrap();
        let lua = runtime().unwrap();
        lua.load("contract.marker=1").exec().unwrap();
        assert_eq!(lua.load(code).eval::<i64>().unwrap(), expected);
    }
    // Stock quasiquote already refuses an un-gensymmed introduced binder.
    let error = compiler::compile(
        &format!(
            "{free} (macro introduced [] `(let [contract {{:marker 4}}] (free))) (introduced)"
        ),
        "quoted.fnl",
    )
    .unwrap_err();
    assert!(error.to_string().contains("without gensym"), "{error}");
    for body in [
        "(let [contract {}] (defn f [] \"f\" 1))",
        "(let [x 0] (let [contract {}] (defn f [] \"f\" x)))",
        "(macro introduced [] `(let [,(sym :contract) {}] (defn ,(sym :f) [] \"f\" 1))) (introduced)",
        "(macro introduced [] `(let [,(sym :_ENV) {}] (defn ,(sym :f) [] \"f\" 1))) (introduced)",
        "(let [_ENV {}] (defn f [] \"f\" 1))",
    ] {
        let error = compiler::compile(
            &format!("(import-macros {{: defn}} :finocchio) {body}"),
            "reject.fnl",
        )
        .unwrap_err();
        assert!(error.to_string().contains("unshadowed"), "{error}");
    }
}

#[test]
fn explicit_builder_is_once_lexical_recursive_and_interoperable() {
    let source = include_str!("../examples/boundary-bindings.fnl");
    // The experimental import must not silently become ordinary/semantic policy.
    assert!(compiler::compile(source, "ordinary.fnl").is_err());
    let s = [("m".into(), source.into())].into();
    assert_eq!(
        finocchio_spike::semantic::request(&s, serde_json::json!({"op":"check"})).unwrap()["supported"],
        false
    );
    let report = probe(source);
    let lua = runtime().unwrap();
    let exports: Table = lua
        .load(
            report
                .compilation
                .lua
                .as_ref()
                .expect(&format!("{report:?}")),
        )
        .eval()
        .unwrap();
    let result: Table = exports.get("result").unwrap();
    let factorial: Function = result.get("factorial").unwrap();
    assert_eq!(factorial.call::<i64>(6).unwrap(), 720);
    assert!(
        factorial
            .call::<i64>("wrong")
            .unwrap_err()
            .to_string()
            .contains("expected integer")
    );
    assert_eq!(result.get::<i64>("bindings").unwrap(), 72);
    assert_eq!(
        result.get::<String>("global").unwrap(),
        "caller-environment"
    );
    assert_eq!(
        result
            .get::<Function>("meanings")
            .unwrap()
            .call::<i64>((1, 2, 3))
            .unwrap(),
        29
    );
    lua.globals().set("sample", factorial).unwrap();
    assert_eq!(
        lua.load("local function apply(f,x) return f(x) end; return apply(sample,5)")
            .eval::<i64>()
            .unwrap(),
        120
    );
    assert_eq!(
        exports
            .get::<Function>("evaluations")
            .unwrap()
            .call::<i64>(())
            .unwrap(),
        1
    );
}

#[test]
fn explicit_builder_preserves_helper_and_environment_names_but_is_caller_authority() {
    let source = r#"
      (import-macros {: defn-with} :finocchio-boundary)
      (local cap contract.define)
      (var chosen 0)
      (fn outer [builder]
        (let [builder (fn [name specs doc build]
                        (set chosen (+ chosen 1)) (cap name specs doc build))
              contract :shadow _G :shadow define :shadow checked :shadow implementation :shadow
              builder_1_ 41 _ENV {:type (fn [_] :user-global)}]
          (defn-with builder builder [n :integer] "same name, checked recursion"
            (if (= n 0) builder_1_ (builder :bad)))
          (defn-with cap env [_ENV :any] "user environment" (type 1))
          {:f builder :env env :native (fn [_ENV] (type 1)) :chosen chosen}))
      (outer (fn [...] (error :wrong-builder)))"#;
    let report = probe(source);
    let lua = runtime().unwrap();
    let out: Table = lua
        .load(
            report
                .compilation
                .lua
                .as_ref()
                .expect(&format!("{report:?}")),
        )
        .eval()
        .unwrap();
    assert_eq!(out.get::<i64>("chosen").unwrap(), 1);
    let f: Function = out.get("f").unwrap();
    assert_eq!(f.call::<i64>(0).unwrap(), 41);
    assert!(
        f.call::<Value>(1)
            .unwrap_err()
            .to_string()
            .contains("contract builder argument 1")
    );
    lua.globals()
        .set("envfn", out.get::<Function>("env").unwrap())
        .unwrap();
    lua.globals()
        .set("native", out.get::<Function>("native").unwrap())
        .unwrap();
    // Stock Fennel mangles a nested _ENV parameter; compare the user's ordinary
    // fn meaning, rather than assuming that spelling always changes Lua's env.
    lua.load("local arg={type=function() return 'parameter-environment' end}; assert(envfn(arg)=='user-global'); assert(native(arg)=='user-global')").exec().unwrap();
    // Supplying an unchecked builder can bypass checks. This is dependency
    // injection, not capability authentication or definition-site resolution.
    let report = probe(
        r#"(import-macros {: defn-with} :finocchio-boundary)
      (defn-with (fn [_ _ _ build] (build nil)) f [x :integer] "untrusted" x)
      (f :unchecked)"#,
    );
    assert_eq!(
        lua.load(report.compilation.lua.unwrap())
            .eval::<String>()
            .unwrap(),
        "unchecked"
    );
    let report = probe(
        r#"(import-macros {: defn-with} :finocchio-boundary)
      (local cap contract.define)
      (macro surround [dependency body]
        `(let [,(sym :builder) ,dependency ,(sym :contract) 9 ,(sym :_ENV) {}] ,body))
      (surround cap (do (defn-with builder f [n :integer] "introduced names" (+ n contract))
                        (f 1)))"#,
    );
    assert_eq!(
        lua.load(
            report
                .compilation
                .lua
                .as_ref()
                .expect(&format!("{report:?}"))
        )
        .eval::<i64>()
        .unwrap(),
        10
    );
}

#[test]
fn public_scope_queries_observe_use_site_presence_not_definition_site_binding() {
    let source = r#"
      (macro scope-probe []
        (assert (= (type (get-scope)) :table))
        (if (in-scope? (sym :contract)) :local :global))
      [(scope-probe) (let [contract 1] (scope-probe))
       (let [contract 2] (let [contract 3] (scope-probe)))
       (let [_ENV {}] (scope-probe))]"#;
    let lua = runtime().unwrap();
    assert_eq!(
        lua.load(compiler::compile(source, "scope.fnl").unwrap())
            .eval::<Vec<String>>()
            .unwrap(),
        ["global", "local", "local", "global"]
    );
}

#[test]
fn experimental_import_does_not_leak_through_the_public_macro_cache() {
    let compiler = Compiler::new().unwrap();
    let source = "(import-macros {: defn-with} :finocchio-boundary) (defn-with contract.define f [] \"f\" 1) (f)";
    assert!(
        compiler
            .boundary_probe(source, "probe.fnl")
            .unwrap()
            .compilation
            .lua
            .is_some()
    );
    assert!(
        compiler
            .compile(source, "ordinary.fnl")
            .unwrap()
            .error
            .is_some()
    );
    assert!(
        compiler
            .boundary_probe(source, "again.fnl")
            .unwrap()
            .compilation
            .lua
            .is_some()
    );
    // Failed compilation after import must not retain the probe-only module.
    assert!(
        compiler
            .boundary_probe(&format!("{source} (unknown-after-import)"), "failed.fnl")
            .unwrap()
            .compilation
            .error
            .is_some()
    );
    assert!(
        compiler
            .compile(source, "after-failure.fnl")
            .unwrap()
            .error
            .is_some()
    );
}

#[test]
fn cross_file_library_expansions_report_propagated_spans_as_pinned_hints() {
    let source = r#"(import-macros {: defn-with} :finocchio-boundary)
(defn-with contract.define first [] "first" 11)
(defn-with contract.define second [] "second" 22)
[(first) (second)]"#;
    for prefix in ["", ";; café\n\n"] {
        let source = format!("{prefix}{source}");
        let report = probe(&source);
        let events: Vec<_> = report
            .events
            .iter()
            .filter(|e| e["name"] == "defn-with")
            .collect();
        assert_eq!(events.len(), 2);
        for (event, expected) in events.iter().zip([
            "(defn-with contract.define first [] \"first\" 11)",
            "(defn-with contract.define second [] \"second\" 22)",
        ]) {
            assert_eq!(slice(&source, &event["location"]), expected);
            assert!(event["location"]["pinned_from_macro"].is_null());
            let output = event["output_nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["role"] == "root")
                .unwrap();
            let location = &output["location"];
            // The macro is defined in finocchio/boundary-macros.fnl. Fennel
            // has replaced that location before the observer is called.
            assert_eq!(location["file"], "boundary.fnl");
            assert_eq!(slice(&source, location), expected);
            assert_eq!(location["pinned_from_macro"], true);
            assert_eq!(location["confidence"], "reported");
        }
        assert_eq!(
            runtime()
                .unwrap()
                .load(report.compilation.lua.unwrap())
                .eval::<Vec<i64>>()
                .unwrap(),
            [11, 22]
        );
    }
}

#[test]
fn descendants_repeated_invocations_and_passthrough_have_distinct_identity_evidence() {
    let original = include_str!("../examples/boundary-provenance.fnl");
    for prefix in ["", ";; shifted λ\n\n"] {
        let source = format!("{prefix}{original}");
        let report = probe(&source);
        let branches: Vec<_> = report
            .events
            .iter()
            .filter(|e| e["name"] == "branch")
            .collect();
        let leaves: Vec<_> = report
            .events
            .iter()
            .filter(|e| e["name"] == "leaf")
            .collect();
        assert_eq!(branches.len(), 2);
        assert_eq!(leaves.len(), 3);
        assert_ne!(branches[0]["input_node"], branches[1]["input_node"]);
        for (i, branch) in branches.iter().enumerate() {
            assert_eq!(
                slice(&source, &branch["location"]),
                format!("(branch {})", 10 * (i + 1))
            );
            assert!(link(
                leaves[i],
                &branch["id"],
                "descendant",
                "first-observed-output"
            ));
            let location = &leaves[i]["location"];
            assert_eq!(
                location["start"],
                source.find("(leaf ,x)").unwrap(),
                "{location}"
            );
            assert!(
                location["finish"].is_null(),
                "generated form has no end: {location}"
            );
            // The baseline tracks output-root identity only, as before.
            let baseline = report
                .compilation
                .expansions
                .iter()
                .find(|e| e.id == leaves[i]["id"].as_u64().unwrap() as usize)
                .unwrap();
            assert_eq!(baseline.parent, None);
        }
        assert_ne!(leaves[0]["input_node"], leaves[1]["input_node"]);
        assert_eq!(leaves[0]["location"], leaves[1]["location"]); // shared definition span is not identity
        let pass = event(&report, "pass");
        assert!(link(leaves[2], &pass["id"], "descendant", "retained-input"));
        assert_eq!(slice(&source, &leaves[2]["location"]), "(leaf 30)");
        assert_eq!(
            runtime()
                .unwrap()
                .load(report.compilation.lua.unwrap())
                .eval::<Vec<i64>>()
                .unwrap(),
            [11, 21, 31]
        );
    }
}

#[test]
fn whole_output_chains_still_have_exact_identity_links() {
    let source = "(macro inner [x] `(+ ,x 1))\n(macro outer [x] `(inner ,x))\n(outer 7)";
    let report = probe(source);
    let outer = event(&report, "outer");
    let inner = event(&report, "inner");
    assert!(link(inner, &outer["id"], "root", "first-observed-output"));
    assert_eq!(
        runtime()
            .unwrap()
            .load(report.compilation.lua.unwrap())
            .eval::<i64>()
            .unwrap(),
        8
    );
}

#[test]
fn reused_output_objects_keep_multiple_relations_instead_of_inventing_a_parent() {
    let source = r#"(macro leaf [] `(+ 1 2))
      (macros (let [node `(leaf)] {:reuse (fn [] node)}))
      [(reuse) (reuse)]"#;
    let report = probe(source);
    let leaves: Vec<_> = report
        .events
        .iter()
        .filter(|e| e["name"] == "leaf")
        .collect();
    let reuse: Vec<_> = report
        .events
        .iter()
        .filter(|e| e["name"] == "reuse")
        .collect();
    assert_eq!(leaves.len(), 2);
    assert_eq!(reuse.len(), 2);
    assert_eq!(leaves[0]["input_node"], leaves[1]["input_node"]);
    assert!(link(
        leaves[1],
        &reuse[0]["id"],
        "root",
        "first-observed-output"
    ));
    assert!(link(leaves[1], &reuse[1]["id"], "root", "previous-output"));
    assert_eq!(
        runtime()
            .unwrap()
            .load(
                report
                    .compilation
                    .lua
                    .as_ref()
                    .expect(&format!("{report:?}"))
            )
            .eval::<Vec<i64>>()
            .unwrap(),
        [3, 3]
    );
}

#[test]
fn compile_assertions_relate_only_observed_nodes_including_failed_macro_calls() {
    for source in [
        "(macro inner [] `(not-a-real-function))\n(macro outer [] `(do (inner)))\n(outer)",
        "(macro inner [] (error :macro-exploded))\n(macro outer [] `(do (inner)))\n(outer)",
    ] {
        let report = probe(source);
        assert!(report.compilation.lua.is_none());
        let diagnostic = report.diagnostics.last().unwrap();
        assert_eq!(diagnostic["origin"], "public-assert-compile");
        assert_eq!(diagnostic["attribution"], "related");
        let id = &diagnostic["related"][0]["expansion"];
        let related = report.events.iter().find(|e| e["id"] == *id).unwrap();
        assert!(
            related["output_nodes"]
                .as_array()
                .unwrap()
                .iter()
                .any(|n| n["node"] == diagnostic["node"])
        );
        if source.contains("macro-exploded") {
            assert_eq!(related["name"], "outer");
            assert_eq!(
                diagnostic["location"]["start"],
                source.find("(inner)").unwrap()
            );
            assert!(diagnostic["location"]["finish"].is_null());
            assert!(!report.events.iter().any(|e| e["name"] == "inner"));
        } else {
            assert_eq!(related["name"], "inner");
            assert_eq!(diagnostic["location"]["line"], 1);
            assert!(diagnostic["location"]["start"].is_null());
            assert!(diagnostic["location"]["finish"].is_null());
        }
    }
    for source in [
        "(not-a-real-function)",
        "(macro fails [] (error :boom)) (fails)",
    ] {
        let report = probe(source);
        let d = report.diagnostics.last().unwrap();
        assert_eq!(d["attribution"], "unknown");
        assert_eq!(d["related"], serde_json::json!([]));
        assert!(report.events.iter().all(|e| e["name"] != "fails"));
    }
    let report = probe("(macro pass [x] `(do ,x)) (pass (not-a-real-function))");
    let d = report.diagnostics.last().unwrap();
    assert_eq!(d["attribution"], "related");
    assert_eq!(d["related"][0]["origin"], "retained-input");
    assert_eq!(event(&report, "pass")["id"], d["related"][0]["expansion"]);
    // A related expansion did not create this invalid caller expression.
}

#[test]
fn runtime_errors_execute_lua_but_public_events_do_not_identify_runtime_invocation() {
    let source = "(macro failure [] `(error :runtime-boom))\n(fn first [] (failure))\n(fn second [] (failure))\n[first second]";
    let report = probe(source);
    assert!(report.diagnostics.is_empty());
    let events: Vec<_> = report
        .events
        .iter()
        .filter(|e| e["name"] == "failure")
        .collect();
    assert_eq!(events.len(), 2);
    let roots: Vec<_> = events
        .iter()
        .map(|e| {
            e["output_nodes"]
                .as_array()
                .unwrap()
                .iter()
                .find(|n| n["role"] == "root")
                .unwrap()
        })
        .collect();
    assert_ne!(roots[0]["node"], roots[1]["node"]);
    assert_eq!(roots[0]["location"], roots[1]["location"]);
    let code = report.compilation.lua.as_ref().unwrap();
    let lua = runtime().unwrap();
    let functions: Vec<Function> = lua.load(code).set_name("@generated.lua").eval().unwrap();
    let mut runtime_lines = Vec::new();
    for f in functions {
        let error = f.call::<Value>(()).unwrap_err().to_string();
        let line = error
            .split("generated.lua:")
            .nth(1)
            .unwrap()
            .split(':')
            .next()
            .unwrap()
            .parse::<usize>()
            .unwrap();
        assert!(error.contains("runtime-boom"), "{error}");
        assert!(
            report
                .compilation
                .pinned_line_map
                .contains(&(line, "boundary.fnl".into(), 1))
        );
        runtime_lines.push(line);
    }
    assert_ne!(runtime_lines[0], runtime_lines[1]);
    // Both actual runtime PCs map to the same macro definition under the
    // separately labelled internal map. No public PC -> event identity exists.
}
