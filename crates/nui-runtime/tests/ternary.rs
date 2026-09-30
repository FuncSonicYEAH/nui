//! A ternary whose branches are host calls must evaluate to a value.
#![allow(clippy::unwrap_used)]

use nui_compiler::compile_with_functions;
use nui_core::Value;
use nui_runtime::{Registry, instantiate_with};

/// Builds a document with one element whose `fill` is `expr`, and returns the
/// value `fill` holds after one propagate.
fn fill_of(expr: &str) -> Value {
    let mut registry = Registry::new();
    registry.register_function(
        "col",
        Box::new(|arguments: &[Value]| {
            return match arguments {
                [Value::String(name)] => Ok(Value::Color(nui_core::Color::from_rgb8(
                    name.len() as u8,
                    0,
                    0,
                ))),
                _ => Err(nui_runtime::registry::FunctionError::new("col(name)")),
            };
        }),
    );
    let source = format!(
        r#"
        component App {{
            property flag: Bool = true
            Window(id = shell, width = 100dp, height = 100dp) {{
                Rectangle(id = probe, width = 10dp, height = 10dp,
                          fill <- {expr}) {{ }}
            }}
        }}
        "#
    );
    let outcome = compile_with_functions(&source, &registry.function_names());
    assert!(
        outcome.diagnostics.is_empty(),
        "should compile: {:?}",
        outcome.diagnostics
    );
    let instance = instantiate_with(&outcome.document, registry);
    let (mut tree, mut engine) = (instance.tree, instance.engine);
    engine.run_registry_init(&mut tree);
    let errors = engine.propagate(&mut tree);
    assert!(errors.is_empty(), "{errors:?}");
    let probe = tree.lookup_id("probe").expect("the probe rectangle");
    return tree.arena[probe].get("fill").cloned().expect("fill is set");
}

#[test]
fn a_single_host_call_evaluates() {
    let value = fill_of("col(\"red\")");
    assert!(matches!(value, Value::Color(_)), "got {value:?}");
}

#[test]
fn a_ternary_of_host_calls_evaluates() {
    // The shape every Material 3 button state uses.
    let value = fill_of("shell.flag ? col(\"red\") : col(\"blue\")");
    assert!(matches!(value, Value::Color(_)), "got {value:?}");
}

#[test]
fn a_nested_ternary_of_host_calls_evaluates() {
    let value =
        fill_of("shell.flag ? (shell.flag ? col(\"red\") : col(\"blue\")) : col(\"green\")");
    assert!(matches!(value, Value::Color(_)), "got {value:?}");
}

#[test]
fn a_ternary_of_host_calls_and_a_literal_evaluates() {
    let value = fill_of("shell.flag ? col(\"red\") : #00000000");
    assert!(matches!(value, Value::Color(_)), "got {value:?}");
}

/// The gallery's exact shape: a registered `Interaction` whose *child* carries a
/// ternary-of-host-calls fill.
#[test]
fn a_control_components_skin_binding_evaluates() {
    use nui_runtime::widget::Interaction;
    let mut registry = Registry::new();
    registry.register_function(
        "col",
        Box::new(|arguments: &[Value]| {
            return match arguments {
                [Value::String(name)] => Ok(Value::Color(nui_core::Color::from_rgb8(
                    name.len() as u8,
                    0,
                    0,
                ))),
                _ => Err(nui_runtime::registry::FunctionError::new("col(name)")),
            };
        }),
    );
    registry.register_component(
        nui_runtime::ComponentDesc::new("Probe").with_interaction(Interaction::momentary()),
        None,
    );
    let source = r#"
        component Probe {
            property enabled: Bool = true
            Stack(id = self, width = 100dp, height = 30dp) {
                Rectangle(id = skin, width = 100dp, height = 30dp, radius = 4dp,
                          fill <- enabled
                                ? (self.pressed ? col("aaaa") : col("bb"))
                                : col("c")) { }
            }
        }
        component App {
            Window(id = shell, width = 200dp, height = 100dp) {
                Probe(enabled = true)
            }
        }
    "#;
    let outcome = compile_with_functions(source, &registry.function_names());
    assert!(
        outcome.diagnostics.is_empty(),
        "should compile: {:?}",
        outcome.diagnostics
    );
    let instance = instantiate_with(&outcome.document, registry);
    let (mut tree, mut engine) = (instance.tree, instance.engine);
    engine.run_registry_init(&mut tree);
    let errors = engine.propagate(&mut tree);
    assert!(errors.is_empty(), "{errors:?}");
    // The id is namespaced into the instance (`i1::skin`), which is the whole
    // point of the rewrite -- so find it by shape rather than by name.
    let mut fills: Vec<Value> = Vec::new();
    tree.visit_pre_order(|_id, element| {
        if element.ty == "Rectangle"
            && let Some(value) = element.get("fill")
        {
            fills.push(value.clone());
        }
    });
    assert_eq!(fills.len(), 1, "one rectangle, one fill: {fills:?}");
    assert!(
        matches!(fills[0], Value::Color(_)),
        "the skin never filled: {:?}",
        fills[0]
    );
}
