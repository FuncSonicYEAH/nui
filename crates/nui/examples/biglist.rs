//! Big-list demo (M10 acceptance): a `ListView` with 10,000 model rows.
//! Only the visible window (~14 rows) exists as elements at any time —
//! scrolling re-instantiates window rows while element count stays bound.
//!
//! Run: `cargo run -p nui --example biglist`
//! (requires a display; headless environments print an error and exit).

use nui::{AppConfig, Application};
use nui_core::{Size, Value};
use nui_runtime::{ModelRow, Registry, VecModel};

const BIG_LIST: &str = r#"
component BigList {
    property rows: Model

    Window(id = root) {
        Column(id = content, spacing = 12dp, padding = 20dp) {
            Text(id = title, content = "10,000 rows — scroll me", font.size = 16dp,
                 width = 200dp, height = 24dp, color = #e8ecf2)
            ListView(item in root.rows, id = list, width = 360dp, height = 320dp,
                     row_height = 36dp) {
                Rectangle(id = row, width = 344dp, height = 32dp, radius = 6dp,
                          fill <- item.hot ? #3d6a9e : #2b3240)
            }
        }
    }
}
"#;

fn main() {
    let registry = Registry::new().on_attach(Box::new(|engine, tree| {
        let rows: Vec<ModelRow> = (0..10_000)
            .map(|index| {
                return vec![
                    ("label".to_string(), Value::String(format!("row {index}"))),
                    ("hot".to_string(), Value::Bool(index % 25 == 0)),
                ];
            })
            .collect();
        let model = engine.add_model(Box::new(VecModel::from_rows(rows)));
        let root = tree.lookup_id("root").expect("root id exists");
        engine.set_direct(tree, root, "rows", Value::Model(model.0));
    }));

    let config = AppConfig::new(BIG_LIST, "nui — 10k rows", Size::new(420.0, 460.0));
    Application::new(config).with_registry(registry).run();
}
