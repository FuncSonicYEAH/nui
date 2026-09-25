//! Todo-list demo (M4 acceptance): `For` rows driven by a host-registered
//! `VecModel`, custom Rust components (`Checkbox` / `RemoveButton`) that
//! mutate the model from their behavior, `when`-style done-state colors,
//! and an animated `tween` on each row's opacity.
//!
//! Run: `cargo run -p nui --example todo`
//! (requires a display; headless environments print an error and exit).
//!
//! Rendering note: v1 has the rect pipeline only, so rows appear as colored
//! rectangles — the point of this demo is the M4 engine machinery (models,
//! per-row scopes, registry behaviors, animation), not visual fidelity.

use nui::{AppConfig, Application};
use nui_core::Value;
use nui_runtime::element::ElementId;
use nui_runtime::{BehaviorContext, ElementBehavior, ModelRow, Registry, VecModel};

const TODO_APP: &str = r#"
component TodoApp {
    property items: Model

    Window(id = root) {
        Column(id = content, spacing = 10dp, padding = 16dp) {
            For(item in root.items) {
                Row(id = row, spacing = 10dp, height = 44dp) {
                    Rectangle(
                        id = swatch,
                        width = 18dp, height = 18dp, radius = 9dp,
                        fill <- item.done ? #3f9d55 : #64748b,
                        opacity <- tween(item.done ? 1.0 : 0.4, duration = 200ms, easing = ease-out),
                    )
                    Text(id = label, content <- item.label, width = 220dp, height = 24dp)
                    Checkbox(id = check, width = 24dp, height = 24dp, fill = #475569)
                    RemoveButton(id = remove, width = 24dp, height = 24dp, fill = #b45454)
                }
            }
        }
    }
}
"#;

/// `Checkbox`: clicking toggles the row's `done` field in the model, which
/// re-drives every row binding that reads it.
#[derive(Debug)]
struct Checkbox;

impl ElementBehavior for Checkbox {
    fn on_signal(&mut self, context: &mut BehaviorContext<'_>, element: ElementId, signal: &str) {
        if signal != "click" {
            return;
        }
        let Some(scope) = context.row_scope(element) else {
            return;
        };
        let done = context
            .model_field(&scope, "done")
            .and_then(|value| return value.as_bool().ok())
            .unwrap_or(false);
        context.set_model_field(&scope, "done", Value::Bool(!done));
    }
}

/// `RemoveButton`: clicking removes the row from the model; the next frame
/// rebuilds the `For` rows.
#[derive(Debug)]
struct RemoveButton;

impl ElementBehavior for RemoveButton {
    fn on_signal(&mut self, context: &mut BehaviorContext<'_>, element: ElementId, signal: &str) {
        if signal != "click" {
            return;
        }
        if let Some(scope) = context.row_scope(element) {
            context.remove_model_row(&scope);
        }
    }
}

fn todo_row(label: &str, done: bool) -> ModelRow {
    return vec![
        ("label".to_string(), Value::String(label.to_string())),
        ("done".to_string(), Value::Bool(done)),
    ];
}

fn main() {
    let registry = Registry::new().on_attach(Box::new(|engine, tree| {
        let model = engine.add_model(Box::new(VecModel::from_rows(vec![
            todo_row("buy oat milk", true),
            todo_row("wire up For + Model", true),
            todo_row("ship the todo demo", false),
            todo_row("hot reload (M5)", false),
        ])));
        let root = tree.lookup_id("root").expect("root id exists");
        engine.set_direct(tree, root, "items", Value::Model(model.0));
    }));
    let mut registry = registry;
    registry.register_component(
        nui_runtime::ComponentDesc {
            name: "Checkbox".to_string(),
            properties: Vec::new(),
        },
        Some(Box::new(|| return Box::new(Checkbox))),
    );
    registry.register_component(
        nui_runtime::ComponentDesc {
            name: "RemoveButton".to_string(),
            properties: Vec::new(),
        },
        Some(Box::new(|| return Box::new(RemoveButton))),
    );

    let config = AppConfig::new(TODO_APP, "nui — todo", nui_core::Size::new(420.0, 480.0));
    Application::new(config).with_registry(registry).run();
}
