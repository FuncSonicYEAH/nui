//! Debug probe: ListView deep-scroll layout positions.
#![allow(clippy::unwrap_used)]
use nui_core::{Size, Value};
use nui_runtime::{ModelRow, Registry, VecModel};

const DOC: &str = r#"
component Big {
    property rows: Model

    Window(id = root) {
        ListView(item in root.rows, id = list, height = 210dp, row_height = 30dp, width = 448dp) {
            Rectangle(height = 24dp, width = 432dp, fill = #3d6a9e)
        }
    }
}
"#;

fn main() {
    let registry = Registry::new().on_attach(Box::new(|engine, tree| {
        let rows: Vec<ModelRow> = (0..10_000)
            .map(|index| {
                return vec![("label".to_string(), Value::String(format!("row {index}")))];
            })
            .collect();
        let model = engine.add_model(Box::new(VecModel::from_rows(rows)));
        let root = tree.lookup_id("root").unwrap();
        engine.set_direct(tree, root, "rows", Value::Model(model.0));
    }));
    let outcome = nui_compiler::compile(DOC);
    eprintln!("diags: {}", outcome.diagnostics.len());
    let instance = nui_runtime::instantiate_with(&outcome.document, registry);
    let (mut tree, mut engine) = (instance.tree, instance.engine);
    engine.run_registry_init(&mut tree);
    let mut text = nui_text::TextSystem::with_embedded_font();

    let mut settle = |engine: &mut nui_runtime::Engine, tree: &mut nui_runtime::ElementTree| {
        let _ = engine.propagate(tree);
        let rebuilt = engine.sync_for_nodes(tree);
        if rebuilt > 0 {
            let _ = engine.propagate(tree);
        }
        nui_layout::layout_with_text(tree, Size::new(480.0, 620.0), Some(&mut text));
    };
    settle(&mut engine, &mut tree);

    let list_id = tree.lookup_id("list").unwrap();
    let dump = |tree: &nui_runtime::ElementTree, tag: &str| {
        let list = tree.lookup_id("list").unwrap();
        eprintln!(
            "{tag}: children={} scroll_y={:?}",
            tree.arena[list].children.len(),
            tree.arena[list].get("scroll_y")
        );
        for (index, child) in tree.arena[list].children.iter().enumerate().take(3) {
            let element = &tree.arena[*child];
            eprintln!(
                "  [{index}] ty={} y={:?} h={:?} scope_row={:?}",
                element.ty,
                element.get("y"),
                element.get("height"),
                element.for_scope.as_ref().map(|scope| return scope.row)
            );
        }
    };
    dump(&tree, "scroll=0");

    engine.set_direct(&mut tree, list_id, "scroll_y", Value::Float(30_000.0));
    settle(&mut engine, &mut tree);
    dump(&tree, "scroll=30000");
}
