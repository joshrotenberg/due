use finocchio_spike::{compiler, runtime};
use mlua::{Function, MultiValue, Table, Value};

#[test]
fn native_fn_arity_and_checked_trailing_nil_multiple_returns() {
    let source = r#"(import-macros {: defn} :finocchio)
      (fn native [x] (values x nil))
      (defn checked [a :nil b :string?] "Exact nil slots." (values a b nil))
      {:native native :checked checked}"#;
    let lua = runtime().unwrap();
    let exports: Table = lua
        .load(compiler::compile(source, "arity.fnl").unwrap())
        .eval()
        .unwrap();
    let native: Function = exports.get("native").unwrap();
    let missing = native.call::<MultiValue>(()).unwrap();
    assert_eq!(missing.len(), 2);
    assert!(missing.iter().all(Value::is_nil));
    let extra = native.call::<MultiValue>((7, 8)).unwrap();
    assert_eq!(extra.len(), 2);
    assert_eq!(extra[0], Value::Integer(7));
    assert!(extra[1].is_nil());
    let checked: Function = exports.get("checked").unwrap();
    let output = checked
        .call::<MultiValue>((Value::Nil, Value::Nil))
        .unwrap();
    assert_eq!(output.len(), 3);
    assert!(output.iter().all(Value::is_nil));
    assert!(
        checked
            .call::<MultiValue>(Value::Nil)
            .unwrap_err()
            .to_string()
            .contains("expected 2 arguments, got 1")
    );
    assert!(
        checked
            .call::<MultiValue>((Value::Nil, Value::Nil, Value::Nil))
            .unwrap_err()
            .to_string()
            .contains("expected 2 arguments, got 3")
    );
}

#[test]
fn native_tables_remain_mutable_identity_values_and_nil_removes_keys() {
    let source = r#"(local a {:value 1}) (local b {:value 1})
      (assert (not= a b)) (local alias a) (tset alias :value 2)
      (assert (= a.value 2)) (tset a :value nil)
      (assert (= a.value nil))
      (assert (= (type []) :table)) (assert (= (type {}) :table))
      (assert (= 1 1.0)) (assert (= (math.type 1) :integer))
      (assert (= (math.type 1.0) :float)) true"#;
    assert!(
        runtime()
            .unwrap()
            .load(compiler::compile(source, "native.fnl").unwrap())
            .eval::<bool>()
            .unwrap()
    );
}
