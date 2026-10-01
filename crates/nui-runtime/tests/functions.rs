//! Document-level function (`fn`) integration tests.
//!
//! These drive the whole pipeline — compile, instantiate, evaluate — and
//! assert that a `fn` declared at the top of a document is callable from a
//! binding, from an interpolation hole, and from an effect block, with
//! arguments, defaults, and `return` behaving as the declaration says.
#![allow(clippy::unwrap_used)]

use nui_core::Value;
use nui_runtime::{Engine, instantiate};

type TestResult = Result<(), Box<dyn std::error::Error>>;

fn build(source: &str) -> (nui_runtime::ElementTree, Engine) {
    let outcome = nui_compiler::compile(source);
    assert!(
        outcome.diagnostics.is_empty(),
        "test source must compile cleanly: {:?}",
        outcome.diagnostics
    );
    let instance = instantiate(&outcome.document);
    return (instance.tree, instance.engine);
}

/// A function called from a binding, evaluated at propagation time.
#[test]
fn a_function_result_fills_a_binding() -> TestResult {
    let (mut tree, mut engine) = build(
        r#"
        fn double(value: Int) -> Int {
            return value + value
        }

        component App {
            property seed: Int = 21
            Window(id = root, width = 100dp) {
                Text(id = out, content <- "{double(seed)}")
            }
        }
        "#,
    );
    engine.propagate(&mut tree);
    let out = tree.lookup_id("out").expect("the Text id resolves");
    assert_eq!(
        tree.arena[out].get("content"),
        Some(&Value::String("42".to_string()))
    );

    // Changing the property the argument reads re-runs the call: the
    // argument read is a dependency edge like any other.
    let root = tree.lookup_id("root").expect("the root id resolves");
    tree.arena[root].set("seed", Value::Int(5));
    engine.invalidate(root, "seed");
    engine.propagate(&mut tree);
    assert_eq!(
        tree.arena[out].get("content"),
        Some(&Value::String("10".to_string()))
    );
    return Ok(());
}

/// A function whose body branches and returns from inside an `if`.
#[test]
fn a_return_inside_a_branch_ends_the_body() {
    let (mut tree, mut engine) = build(
        r#"
        fn clamp01(value: Float) -> Float {
            if value < 0.0 { return 0.0 }
            if value > 1.0 { return 1.0 }
            return value
        }

        component App {
            property low: Float = -3.0
            property mid: Float = 0.5
            property high: Float = 9.0
            Window(id = root, width = 100dp) {
                Text(id = a, content <- "{clamp01(low)}")
                Text(id = b, content <- "{clamp01(mid)}")
                Text(id = c, content <- "{clamp01(high)}")
            }
        }
        "#,
    );
    engine.propagate(&mut tree);
    let read = |tree: &nui_runtime::ElementTree, name: &str| {
        let id = tree.lookup_id(name).unwrap();
        return tree.arena[id].get("content").cloned();
    };
    assert_eq!(read(&tree, "a"), Some(Value::String("0".to_string())));
    assert_eq!(read(&tree, "b"), Some(Value::String("0.5".to_string())));
    assert_eq!(read(&tree, "c"), Some(Value::String("1".to_string())));
}

/// An omitted argument falls back to the declaration's default, and a
/// named argument is reordered into position.
#[test]
fn defaults_and_named_arguments_fill_positionally() {
    let (mut tree, mut engine) = build(
        r#"
        fn tag(label: String, count: Int = 0) -> String {
            return "{label}:{count}"
        }

        component App {
            Window(id = root, width = 100dp) {
                Text(id = defaulted, content <- tag("a"))
                Text(id = named, content <- tag(count = 7, label = "b"))
                Text(id = positional, content <- tag("c", 3))
            }
        }
        "#,
    );
    engine.propagate(&mut tree);
    let read = |tree: &nui_runtime::ElementTree, name: &str| {
        let id = tree.lookup_id(name).unwrap();
        return tree.arena[id].get("content").cloned();
    };
    assert_eq!(
        read(&tree, "defaulted"),
        Some(Value::String("a:0".to_string()))
    );
    assert_eq!(read(&tree, "named"), Some(Value::String("b:7".to_string())));
    assert_eq!(
        read(&tree, "positional"),
        Some(Value::String("c:3".to_string()))
    );
}

/// A function calling another function, and reading its own parameter.
#[test]
fn functions_may_call_earlier_functions() {
    let (mut tree, mut engine) = build(
        r#"
        fn base() -> Int { return 10 }
        fn plus(value: Int) -> Int { return value + base() }

        component App {
            Window(id = root, width = 100dp) {
                Text(id = out, content <- "{plus(5)}")
            }
        }
        "#,
    );
    engine.propagate(&mut tree);
    let out = tree.lookup_id("out").unwrap();
    assert_eq!(
        tree.arena[out].get("content"),
        Some(&Value::String("15".to_string()))
    );
}

/// A function's arguments do not leak into the caller's `let` frame.
#[test]
fn a_function_frame_is_popped_on_return() {
    let (mut tree, mut engine) = build(
        r#"
        fn inner(value: Int) -> Int { return value * 2 }

        component App {
            Window(id = root, width = 100dp) {
                Text(id = out, content = "start")
                Button(id = go, label = "go") {
                    on click => {
                        let value = 3
                        let doubled = inner(value)
                        let result = value + doubled
                        out.content = "{result}"
                    }
                }
            }
        }
        "#,
    );
    engine.propagate(&mut tree);
    let go = tree.lookup_id("go").unwrap();
    let out = tree.lookup_id("out").unwrap();
    engine.emit_signal(&mut tree, go, "click").unwrap();
    // `value` is still 3 after `inner` ran: the callee's parameter vanished
    // with the frame. 3 + 6 = 9.
    assert_eq!(
        tree.arena[out].get("content"),
        Some(&Value::String("9".to_string()))
    );
}

/// A `Void` function is callable as a statement; its body runs and its
/// (absent) value is discarded.
#[test]
fn a_void_function_runs_as_an_effect_call() -> TestResult {
    let (mut tree, mut engine) = build(
        r#"
        fn noop(steps: Int) {
            let total = steps + 1
        }

        component App {
            property count: Int = 0
            Window(id = root, width = 100dp) {
                Text(id = out, content <- "{count}")
                Button(id = go, label = "go") {
                    on click => {
                        noop(3)
                        count += 1
                    }
                }
            }
        }
        "#,
    );
    engine.propagate(&mut tree);
    let go = tree.lookup_id("go").unwrap();
    let out = tree.lookup_id("out").unwrap();
    assert_eq!(
        tree.arena[out].get("content"),
        Some(&Value::String("0".to_string()))
    );
    // The statement runs its body and the following statement still runs:
    // a Void call is not a `return`.
    engine.emit_signal(&mut tree, go, "click")?;
    engine.propagate(&mut tree);
    assert_eq!(
        tree.arena[out].get("content"),
        Some(&Value::String("1".to_string()))
    );
    return Ok(());
}

/// The recursion guard still fires: a function may not call a *later* one,
/// so the only way to express depth is `MAX_EVAL_DEPTH` through host code —
/// but a deeply nested chain of calls still returns the right answer.
#[test]
fn a_deep_call_chain_resolves() {
    let (mut tree, mut engine) = build(
        r#"
        fn step0() -> Int { return 0 }
        fn step1() -> Int { return step0() + 1 }
        fn step2() -> Int { return step1() + 1 }
        fn step3() -> Int { return step2() + 1 }
        fn step4() -> Int { return step3() + 1 }

        component App {
            Window(id = root, width = 100dp) {
                Text(id = out, content <- "{step4()}")
            }
        }
        "#,
    );
    engine.propagate(&mut tree);
    let out = tree.lookup_id("out").unwrap();
    assert_eq!(
        tree.arena[out].get("content"),
        Some(&Value::String("4".to_string()))
    );
}
