use finocchio_spike::{compiler, runtime};
use mlua::{Table, Value};

#[test]
fn faithful_json_tags_nested_immutability_equality_and_iteration() {
    let lua = runtime().unwrap();
    let code = compiler::compile(include_str!("../examples/data.fnl"), "data.fnl").unwrap();
    let out: Table = lua.load(code).eval().unwrap();
    assert_eq!(out.get::<i64>("selected").unwrap(), 4);
    assert_eq!(
        out.get::<Vec<String>>("kinds").unwrap(),
        ["null", "map", "array", "map"]
    );
    lua.globals()
        .set("value", out.get::<Value>("value").unwrap())
        .unwrap();
    lua.load(
        r#"
        local items=data.get(value,'items')
        assert(data.get(value,'absent')==nil)
        assert(data.get(items,1)~=nil and data.kind(data.get(items,1))=='null')
        assert(data.parse('null')~=data.parse('[]'))
        assert(data.parse('[]')~=data.parse('{}'))
        assert(data.parse('{"a":[null,{}]}')==data.parse('{"a":[null,{}]}'))
        assert(data.parse('{"a":1}')~=data.parse('{"a":2}'))
        assert(not rawequal(data.parse('{}'),data.parse('{}')))
        local keys={} for key,child in data.iter(value) do keys[key]=data.kind(child) end
        assert(keys.items=='array' and keys.label=='string')
        assert(data.scalar(data.get(data.get(items,4),'x'))==7)
        assert(not pcall(function() value.items=3 end))
        assert(not pcall(function() items[1]=3 end))
        local nested=data.get(items,4)
        assert(not pcall(function() nested.x=8 end))
        assert(not pcall(rawset,nested,'x',8))
        assert(not pcall(setmetatable,nested,{}))
        assert(not pcall(table.insert,items,8))
        assert(data.scalar(data.get(nested,'x'))==7)
        assert(not pcall(data.get,items,0))
        assert(not pcall(data.json,{a=1}))
        assert(not pcall(data.scalar,data.parse('null')))
        assert(not pcall(data.parse,'[NaN]'))
        assert(data.json(data.parse('1e999'))=='1e+999')
        assert(not pcall(data.scalar,data.parse('1e999')))
        assert(not pcall(data.scalar,data.parse('18446744073709551615')))
        assert(not pcall(data.scalar,data.parse('0.1234567890123456789012345')))
        assert(data.scalar(data.parse('1.25'))==1.25)
        -- Conservative spelling policy, even though these values fit exactly.
        assert(not pcall(data.scalar,data.parse('1e2')))
        assert(not pcall(data.scalar,data.parse('2.50')))
        assert(data.scalar(data.parse('100.0'))==100.0)
        assert(data.scalar(data.parse('2.5'))==2.5)
    "#,
    )
    .exec()
    .unwrap();
    for source in [
        "null",
        "{}",
        "[]",
        "[null,{},[],true,false,\"café\\u0000\"]",
        "-9223372036854775808",
        "18446744073709551615",
        "1.25",
        "1e2",
        "2.50",
        "123456789012345678901234567890123456789",
        "0.1234567890123456789012345",
        "1e999",
    ] {
        let expected: serde_json::Value = serde_json::from_str(source).unwrap();
        lua.globals().set("json_source", source).unwrap();
        let actual: String = lua
            .load("return data.json(data.parse(json_source))")
            .eval()
            .unwrap();
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&actual).unwrap(),
            expected
        );
    }
}

#[test]
fn native_table_patterns_and_destructuring_are_not_userdata_protocols() {
    let lua = runtime().unwrap();
    for source in [
        "(local [a] (data.parse \"[7]\")) a",
        "(local {: x} (data.parse \"{\\\"x\\\":7}\")) x",
    ] {
        let code = compiler::compile(source, "destructure.fnl").unwrap();
        let error = lua.load(code).eval::<Value>().unwrap_err().to_string();
        assert!(
            error.contains("index") || error.contains("field"),
            "{error}"
        );
    }
    let code = compiler::compile(
        "(match (data.parse \"[7]\") [x] x _ :unsupported)",
        "match.fnl",
    )
    .unwrap();
    assert_eq!(lua.load(code).eval::<String>().unwrap(), "unsupported");
    lua.load("assert(not pcall(function() for k,v in pairs(data.parse('{}')) do end end)); assert(not pcall(function() return #data.parse('[]') end))").exec().unwrap();
}
