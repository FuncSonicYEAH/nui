//! Showcase demo (M9 acceptance): `Scroll` viewport (wheel + clipped
//! content), `layer.opacity` group compositing, and `layer.blur` (two-pass
//! gaussian offscreen layer).
//!
//! Run: `cargo run -p nui --example showcase`
//! (requires a display; headless environments print an error and exit).

use nui::{AppConfig, Application};
use nui_core::{Size, Value};
use nui_runtime::{ModelRow, Registry, VecModel};

const SHOWCASE: &str = r#"
component Showcase {
    property rows: Model

    Window(id = root) {
        Column(id = content, spacing = 16dp, padding = 24dp) {
            Rectangle(
                id = card,
                width = 340dp, height = 84dp, radius = 12dp,
                fill = #2b3240,
                shadow.dx = 0dp, shadow.dy = 8dp,
                shadow.blur = 14dp, shadow.color = #000000aa,
            ) {
                layer.opacity = 0.85
            }
            Rectangle(
                id = pill,
                width = 180dp, height = 36dp, radius = 9dp,
                fill = #336699,
            ) {
                layer.blur = 3dp
            }
            Scroll(
                id = list,
                width = 340dp, height = 200dp, radius = 10dp,
                fill = #161a22,
            ) {
                For(item in root.rows) {
                    Rectangle(
                        id = row,
                        width = 316dp, height = 36dp, radius = 6dp,
                        fill <- item.hot ? #3d6a9e : #2b3240,
                    )
                }
            }
        }
    }
}
"#;

fn row(index: usize, hot: bool) -> ModelRow {
    return vec![
        ("label".to_string(), Value::String(format!("row {}", index))),
        ("hot".to_string(), Value::Bool(hot)),
    ];
}

fn main() {
    let registry = Registry::new().on_attach(Box::new(|engine, tree| {
        let rows: Vec<ModelRow> = (0..24)
            .map(|index| return row(index, index % 4 == 0))
            .collect();
        let model = engine.add_model(Box::new(VecModel::from_rows(rows)));
        let root = tree.lookup_id("root").expect("root id exists");
        engine.set_direct(tree, root, "rows", Value::Model(model.0));
    }));

    let config = AppConfig::new(
        SHOWCASE,
        "nui — showcase (scroll + layers)",
        Size::new(420.0, 460.0),
    );
    Application::new(config).with_registry(registry).run();
}
