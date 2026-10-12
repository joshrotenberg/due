#![cfg(all(unix, feature = "workflow-lua-spike"))]
#[path = "support/workflow.rs"]
mod support;
use serde_json::{Value, json};
use std::{fs, os::unix::fs::PermissionsExt};
use support::*;
fn lua(source: &str) -> Fixture {
    let f = Fixture::new("");
    f.write("main.lua", source);
    f.edit(|m| m["entry"] = json!("main.lua"));
    f
}

#[test]
fn bundled_local_and_higher_order_compositions_keep_child_contracts_and_privacy() {
    for local in [false, true] {
        let f = lua(&include_str!("../examples/workflow/review.lua").replace(
            "local review = require(\"patterns/v1\")",
            if local {
                "local review = require(\"local/review\")"
            } else {
                "local review = require(\"patterns/v1\")"
            },
        ));
        f.write(
            "local.lua",
            include_str!("../examples/workflow/local-review.lua"),
        );
        f.edit(|m| m["modules"] = json!({"local/review":"local.lua"}));
        f.run(true, 0);
        assert!(f.calls().is_empty());
        let v = f.run(false, 0);
        assert_eq!(v["steps"].as_array().unwrap().len(), 4);
        assert_eq!(
            v["steps"][1]["phase"],
            if local { "local-review" } else { "review" }
        );
        for step in v["steps"].as_array().unwrap() {
            let id = step["id"].as_str().unwrap();
            assert_eq!(step["ok"], true);
            let raw = f.read(&format!("artifacts/steps/{id}/result.json"));
            assert_eq!(raw["answer_schema_validation"], "not_performed");
            assert_eq!(
                f.read(&format!("artifacts/steps/{id}/step.json"))["child_result"],
                raw
            );
            assert_eq!(
                fs::metadata(
                    f.root
                        .path()
                        .join(format!("artifacts/steps/{id}/result.json"))
                )
                .unwrap()
                .permissions()
                .mode()
                    & 0o777,
                0o600
            );
        }
        assert_eq!(
            fs::metadata(f.root.path().join("artifacts"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o700
        );
        assert_eq!(f.read("artifacts/workflow.json"), v);
    }
    let f = lua(r#"
        local component=require("local/component")
        local p=require("patterns/v1")
        local run=component.in_phase("wrapped", function(task)
            return p.answer(due.run(p.job("wrapped", "reader", task)))
        end)
        return run(input.task)
    "#);
    f.write(
        "component.lua",
        include_str!("../examples/workflow/component.lua"),
    );
    f.edit(|m| m["modules"] = json!({"local/component":"component.lua"}));
    let v = f.run(false, 0);
    assert_eq!(v["steps"][0]["phase"], "wrapped");
    assert_eq!(v["output"]["approved"], true);
}

#[test]
fn parallel_order_conditional_followup_and_independent_answer_gates() {
    let f = lua(r#"
        local p=require("patterns/v1")
        local rs=due.parallel("g", {p.job("a","reader","first"),p.job("b","reader","second")})
        local answer=p.answer(due.run(p.job("s","reader",json.encode(p.answers(rs)))))
        if answer.approved then p.answer(due.run(p.job("follow","reader","follow"))) end
        return {order={rs[1].id,rs[2].id},answer=answer}
    "#);
    assert_eq!(f.run(false, 0)["output"]["order"], json!(["a", "b"]));
    assert_eq!(f.calls().len(), 4);
    for prompt in ["invalid", "fail", "missing"] {
        let f = lua(&format!(
            r#"local p=require("patterns/v1"); p.answer(due.run(p.job("a","reader","{prompt}"))); return due.run(p.job("s","reader","never"))"#
        ));
        let v = f.run(false, 1);
        assert_eq!(f.calls(), vec![prompt]);
        assert_eq!(v["steps"][0]["ok"], false);
    }
    let f = lua(
        r#"local p=require("patterns/v1"); local failed=due.run(p.job("a","reader","fail")); if not failed.ok then return p.answer(due.run(p.job("b","reader","fallback"))) end"#,
    );
    assert_eq!(f.run(false, 0)["output"]["approved"], true);
}

#[test]
fn atomic_admission_and_sticky_budget_cannot_be_bypassed_by_pcall() {
    for jobs in [
        r#"{{id="a",profile="reader",prompt="one"},{id="a",profile="reader",prompt="two"}}"#,
        r#"{{id="a",profile="reader",prompt="one"},{id="b",profile="missing",prompt="two"}}"#,
        r#"{{id="a",profile="reader",prompt="one",native_args={}}}"#,
        r#"{{id="a",profile="reader",prompt="one"},{id="b",profile="reader",prompt="two"},{id="c",profile="reader",prompt="three"}}"#,
    ] {
        let f = lua(&format!("return due.parallel('g', {jobs})"));
        f.edit(|m| m["max_runs"] = json!(2));
        f.run(false, 1);
        assert!(f.calls().is_empty());
    }
    let f = lua(r#"
        local p=require("patterns/v1")
        due.run(p.job("a","reader","one"))
        pcall(function() due.run(p.job("b","reader","two")) end)
        pcall(function() due.run(p.job("c","reader","three")) end)
        return true
    "#);
    f.edit(|m| {
        m["max_runs"] = json!(1);
        m["max_concurrency"] = json!(1);
    });
    assert!(
        f.run(false, 1)["error"]
            .as_str()
            .unwrap()
            .contains("max_runs")
    );
    assert_eq!(f.calls(), vec!["one"]);
    let f = lua(
        r#"local p=require("patterns/v1"); due.run(p.job("a","reader","one")); return due.run(p.job("a","reader","two"))"#,
    );
    f.run(false, 1);
    assert_eq!(f.calls(), vec!["one"]);
}

#[test]
fn checked_json_preserves_null_absence_shapes_and_signed_integer_endpoints() {
    let f = lua(r#"
        assert(input.absent == nil and input.present == json.null)
        return {input=input,empty_array=json.array(),empty_object=json.object(),plain_object={},
          null=json.null,encoded=json.encode(input),lo=math.mininteger,hi=math.maxinteger}
    "#);
    let input = json!({"present":null,"a":[],"o":{},"lo":i64::MIN,"hi":i64::MAX,"fraction":1.25});
    f.json("input.schema.json", json!(true));
    f.json("input.json", input.clone());
    let v = f.run(false, 0);
    assert_eq!(v["output"]["input"], input);
    assert_eq!(v["output"]["empty_array"], json!([]));
    assert_eq!(v["output"]["empty_object"], json!({}));
    assert_eq!(v["output"]["plain_object"], json!({}));
    assert_eq!(v["output"]["lo"], json!(i64::MIN));
    assert_eq!(v["output"]["hi"], json!(i64::MAX));
    assert_eq!(
        serde_json::from_str::<Value>(v["output"]["encoded"].as_str().unwrap()).unwrap(),
        input
    );
    let f = lua(
        r#"local s=due.run({id="a",profile="reader",prompt="null"}); assert(s.answer==json.null); return {present=s.answer_present,answer=s.answer}"#,
    );
    f.json("answer.schema.json", json!({"type":"null"}));
    assert_eq!(
        f.run(false, 0)["output"],
        json!({"present":true,"answer":null})
    );
    for value in [json!(u64::MAX), json!(9007199254740992.0)] {
        let f = lua("return input");
        f.json("input.schema.json", json!(true));
        f.json("input.json", value);
        f.run(true, 1);
        assert!(f.calls().is_empty());
    }
}

#[test]
fn malformed_json_is_rejected_without_loss_or_recursion_escape() {
    for source in [
        "return nil",
        "return 0/0",
        "return 1/0",
        "return 9007199254740992.0",
        "return {1,a=2}",
        "return {[2]=1}",
        "return {[0]=1}",
        "return {[true]=1}",
        "return json.array({a=1})",
        "return json.object({1})",
        "return function() end",
        "local t={}; t.self=t; return t",
        "return string.char(255)",
        "local t={}; for i=1,4097 do t[i]=i end; return t",
        "local t={}; for i=1,257 do t[tostring(i)]=i end; return t",
        "local t={}; for i=1,34 do t={t} end; return t",
        "local t={}; for i=1,24 do t={t,t} end; return t",
    ] {
        let f = lua(source);
        f.run(false, 1);
        assert!(f.calls().is_empty(), "{source}");
    }
    for handles in ["{[2]=h}", "{h,x=h}", "json.object({x=h})"] {
        let f = lua(&format!(
            r#"local h=due.start({{id="a",profile="reader",prompt="hold"}}); return due.wait_any({handles})"#
        ));
        assert_eq!(f.run(false, 1)["steps"].as_array().unwrap().len(), 1);
    }
}

#[test]
fn check_executes_no_initializers_and_run_initialization_is_effect_disabled() {
    for source in [
        "while true do end",
        "due.run({id='init',profile='reader',prompt='never'}); return {}",
        "pcall(function() due.log('effect') end); return {}",
        "return 42",
    ] {
        let f = lua("return require('local/m')");
        f.write("module.lua", source);
        f.edit(|m| m["modules"] = json!({"local/m":"module.lua"}));
        f.run(true, 0);
        assert!(!f.root.path().join("artifacts").exists());
        f.run(false, 1);
        assert!(f.calls().is_empty());
    }
    let f = lua("return 42");
    f.write("bad.lua", "local =");
    f.edit(|m| m["modules"] = json!({"local/bad":"bad.lua"}));
    assert!(
        f.run(true, 1)["error"]
            .as_str()
            .unwrap()
            .contains("local/bad")
    );
    let f = lua("return require('local/m').get()");
    f.write(
        "module.lua",
        "local constant=40+2; return {get=function() return constant end}",
    );
    f.edit(|m| m["modules"] = json!({"local/m":"module.lua"}));
    assert_eq!(f.run(false, 0)["output"], 42);
    let f = lua("return require('ambient')");
    f.write("ambient.lua", "return {} ");
    f.run(false, 1);
    let f = lua("return require('local/m')");
    f.write("module.lua", "return require('local/m')");
    f.edit(|m| m["modules"] = json!({"local/m":"module.lua"}));
    f.run(false, 1);
    assert!(f.calls().is_empty());
}

#[test]
fn ordinary_module_errors_are_recoverable_only_in_plain_v1_lua() {
    for version in [1, 2] {
        for (module, expected) in [
            (None, "undeclared module local/m"),
            (
                Some("return require('local/m')"),
                "cyclic module initialization",
            ),
            (Some("error('bad initializer')"), "bad initializer"),
            (Some("return 42"), "table"),
            (Some("return {bad=42}"), "function table"),
            (Some("return json.array()"), "plain function table"),
        ] {
            let body = "local ok=pcall(require,'local/m'); assert(not ok); return require('local/good').answer()";
            let f = lua(&if version == 1 {
                body.to_owned()
            } else {
                format!("return function(input) {body} end")
            });
            f.write("good.lua", "return {answer=function() return 42 end}");
            f.edit(|m| {
                m["schema_version"] = json!(version);
                m["modules"] = json!({"local/good":"good.lua"});
                if let Some(source) = module {
                    f.write("module.lua", source);
                    m["modules"]["local/m"] = json!("module.lua");
                }
            });
            f.run(true, 0);
            let v = f.run(false, if version == 1 { 0 } else { 1 });
            if version == 1 {
                assert_eq!(v["output"], 42);
            } else {
                assert!(v["error"].as_str().unwrap().contains(expected), "{v}");
            }
            assert!(f.calls().is_empty());
        }
    }
    // Failed initializers must also clear the loading marker for a later retry.
    for failure in ["error('first attempt')", "require('local/m')"] {
        let f = lua("assert(not pcall(require,'local/m')); return require('local/m').answer()");
        f.write("module.lua", &format!("attempt=(attempt or 0)+1; if attempt==1 then {failure} end; return {{answer=function() return 42 end}}"));
        f.edit(|m| m["modules"] = json!({"local/m":"module.lua"}));
        assert_eq!(f.run(false, 0)["output"], 42);
    }
}

#[test]
fn v1_module_loading_effects_remain_sticky_when_caught() {
    for effect in [
        "due.log('init')",
        "due.start({id='init',profile='reader',prompt='never'})",
    ] {
        let f = lua("pcall(require,'local/m'); return true");
        f.write(
            "module.lua",
            &format!("pcall(function() {effect} end); return {{}}"),
        );
        f.edit(|m| m["modules"] = json!({"local/m":"module.lua"}));
        f.run(true, 0);
        assert!(
            f.run(false, 1)["error"]
                .as_str()
                .unwrap()
                .contains("effects unavailable")
        );
        assert!(f.calls().is_empty());
    }
}

#[test]
fn c_table_loops_are_bounded_and_errors_join_active_children() {
    let f = lua(r#"
        local t={1,3}; table.insert(t,2,2)
        assert(table.remove(t,3)==3)
        local destination={}; assert(table.move(t,1,2,1,destination)==destination)
        assert(table.move(t,2,1,1)==t)
        assert(string.rep("",math.maxinteger)=="")
        assert(string.rep("",math.maxinteger,"")=="")
        assert(string.rep("a",3,"-")=="a-a-a")
        return destination
    "#);
    assert_eq!(f.run(false, 0)["output"], json!([1, 2]));
    for operation in [
        "table.move({},1,math.maxinteger-1,1)",
        "table.move({},math.mininteger,math.maxinteger,1)",
        "local t={}; for i=0,50 do t[2^i]=true; t[2^i+1]=true end; table.insert(t,1,true)",
        "local t={}; for i=0,50 do t[2^i]=true; t[2^i+1]=true end; table.remove(t,1)",
    ] {
        let f = lua(&format!(
            r#"
            local slow=due.start({{id="slow",profile="reader",prompt="slow"}})
            due.run({{id="fast",profile="reader",prompt="fast"}})
            {operation}; return true
        "#
        ));
        let v = f.run(false, 1);
        assert!(v["error"].as_str().unwrap().contains("4096"), "{v}");
        assert_eq!(v["steps"].as_array().unwrap().len(), 2);
        assert_eq!(v["steps"][0]["exit"]["code"], 130);
        assert_native_cleanup(&f);
    }
    let f = lua("while true do pcall(function() table.move({},1,math.maxinteger-1,1) end) end");
    assert!(
        f.run(false, 1)["error"]
            .as_str()
            .unwrap()
            .contains("instruction budget")
    );
}

#[test]
fn vm_budget_memory_and_restricted_libraries() {
    for source in [
        "while true do end",
        "while true do pcall(function() while true do end end) end",
        "while true do pcall(function() error('caught') end) end",
        "local t={}; while true do t[#t+1]=string.rep('x', 1024*1024) end",
        "return string.rep('x',32*1024*1024)",
    ] {
        let f = lua(source);
        f.run(false, 1);
        assert!(f.calls().is_empty());
    }
    let f = lua(r#"
        assert(io==nil and os==nil and debug==nil and package==nil and coroutine==nil)
        assert(load==nil and loadfile==nil and dofile==nil and collectgarbage==nil)
        assert(setmetatable==nil and getmetatable==nil and xpcall==nil)
        assert(string.dump==nil and string.gsub==nil and table.sort==nil)
        return true
    "#);
    assert_eq!(f.run(false, 0)["output"], true);
}

#[test]
fn validation_snapshots_capture_bounds_and_output_gates() {
    for schema in [
        json!({"$ref":"https://invalid.example"}),
        json!({"format":"email"}),
    ] {
        let f = lua("return input");
        f.json("input.schema.json", schema);
        f.run(true, 1);
        assert!(f.calls().is_empty());
    }
    let f = lua("return input");
    f.json("input.json", json!({"task":42}));
    f.run(true, 1);
    let f = lua("return 42");
    f.json("output.schema.json", json!({"type":"string"}));
    assert!(
        f.run(false, 1)["error"]
            .as_str()
            .unwrap()
            .contains("output:")
    );
    let f = lua("return due.run({id='a',profile='reader',prompt='mutate-schema'})");
    let schema = f.read("answer.schema.json");
    f.run(false, 0);
    assert_eq!(f.read("artifacts/schemas/reader.json"), schema);
    assert_eq!(f.read("answer.schema.json"), json!(false));
    for prompt in ["huge-result", "huge-stderr", "huge-result-bad-exit"] {
        let f = lua(&format!(
            "due.run({{id='a',profile='reader',prompt='{prompt}'}}); return due.run({{id='b',profile='reader',prompt='never'}})"
        ));
        let v = f.run(false, 1);
        assert!(
            v["error"]
                .as_str()
                .unwrap()
                .contains("step a: child output exceeded 1 MiB"),
            "{v}"
        );
        assert_eq!(f.calls(), vec![prompt]);
        for file in ["result.json", "stderr.txt"] {
            assert!(
                fs::metadata(f.root.path().join("artifacts/steps/a").join(file))
                    .unwrap()
                    .len()
                    <= 1024 * 1024
            );
        }
    }
}
