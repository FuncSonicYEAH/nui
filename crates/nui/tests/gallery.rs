//! End-to-end tests for the gallery itself: the shipped document, driven
//! through the same host-side table the window reads.
//!
//! `examples/gallery/host.rs` — the page table, the document assembly,
//! and the registry — is included here by path, so the test cannot drift
//! from the demo: a page broken in the gallery is broken here, and a page
//! that only exists in one of the two is impossible. What the per-page
//! tests (`list.rs`, `widget_hitboxes.rs`, `containers.rs`,
//! `text_inputs.rs`) check in isolation, this checks *assembled*:
//!
//! - the whole document compiles and instantiates with every page in it;
//! - selecting a slot lays that page out (non-zero box) and exactly one
//!   page is visible at a time;
//! - an unselected page contributes **no draw** — `visible` removes it
//!   from layout, so the scene walk skips its whole subtree, which is
//!   the gallery's reason for using it instead of hiding;
//! - no `.nui` file under `pages/` is orphaned (present but absent from
//!   the table, which would silently never show).
#![allow(clippy::unwrap_used)]

#[path = "../examples/gallery/host.rs"]
mod host;

use nui_core::{Size, Value};
use nui_runtime::widget;
use nui_runtime::{ElementId, ElementTree, Engine};

/// The gallery's viewport.
const VIEWPORT: Size = host::WINDOW;

/// Compiles and settles the shipped document the way the host does.
fn gallery() -> (ElementTree, Engine) {
    let source = host::document();
    let registry = host::build_registry();
    let outcome = nui_compiler::compile_with_host(&source, &registry.vocabulary());
    assert!(
        outcome.diagnostics.is_empty(),
        "the shipped gallery must compile clean:\n{}",
        outcome
            .diagnostics
            .iter()
            .map(|diagnostic| return nui_syntax::render_diagnostic(
                &source,
                "gallery.nui",
                diagnostic
            ))
            .collect::<Vec<_>>()
            .join("")
    );
    let instance = nui_runtime::instantiate_with(&outcome.document, registry);
    let (mut tree, mut engine) = (instance.tree, instance.engine);
    engine.run_registry_init(&mut tree);
    settle(&mut tree, &mut engine);
    settle(&mut tree, &mut engine);
    return (tree, engine);
}

/// One host frame: propagate, sync `For` rows, lay out.
fn settle(tree: &mut ElementTree, engine: &mut Engine) {
    let mut text = nui_text::TextSystem::with_embedded_font();
    let _ = engine.propagate(tree);
    let rebuilt = engine.sync_for_nodes(tree);
    if rebuilt > 0 {
        let _ = engine.propagate(tree);
    }
    let _ = engine.apply_when_blocks(tree);
    nui_layout::layout_with_text(tree, VIEWPORT, Some(&mut text));
}

/// Selects a page the way the sidebar's `on click` ends up doing.
fn select(tree: &mut ElementTree, engine: &mut Engine, slot: usize) {
    let root = tree.lookup_id("root").expect("the gallery has a root");
    engine.set_direct(tree, root, "page", Value::Int(slot as i64));
    settle(tree, engine);
}

fn number(tree: &ElementTree, id: ElementId, name: &str) -> f32 {
    return match tree.arena[id].get(name) {
        Some(Value::Float(inner)) => *inner as f32,
        Some(Value::Int(inner)) => *inner as f32,
        Some(Value::Length(nui_core::Length::Dp(inner))) => *inner,
        other => panic!("expected a numeric {name}, got {other:?}"),
    };
}

/// The page roots, in slot order: the content `Scroll`'s direct children.
fn page_roots(tree: &ElementTree) -> Vec<ElementId> {
    let content = tree.lookup_id("content").expect("the content area exists");
    let roots = tree.arena[content].children.clone();
    assert_eq!(
        roots.len(),
        host::PAGES.len(),
        "one child per page fragment"
    );
    return roots;
}

#[test]
fn the_document_compiles_and_instantiates() {
    let (tree, _engine) = gallery();
    let sidebar = tree.lookup_id("sidebar").expect("the sidebar exists");
    // The title plus one nav rect per page.
    assert_eq!(
        tree.arena[sidebar].children.len(),
        host::PAGES.len() + 1,
        "the sidebar lists every page"
    );
    for id in tree.arena[sidebar].children.clone() {
        assert!(
            number(&tree, id, "height") > 0.0,
            "sidebar entry `{}` is laid out",
            tree.arena[id].id.as_deref().unwrap_or("?")
        );
    }
}

#[test]
fn every_page_lays_out_when_selected() {
    let (mut tree, mut engine) = gallery();
    let roots = page_roots(&tree);
    for (slot, page) in host::PAGES.iter().enumerate() {
        select(&mut tree, &mut engine, slot);
        let root = roots[slot];
        assert!(
            number(&tree, root, "width") > 0.0,
            "page `{}` gets a box when selected",
            page.key
        );
        assert!(
            number(&tree, root, "height") > 0.0,
            "page `{}` has content height",
            page.key
        );
        // ...and it is the *only* page with a box: `visible` pulls the
        // others out of layout entirely.
        let visible: Vec<ElementId> = roots
            .iter()
            .copied()
            .filter(|id| return widget::is_visible(&tree.arena[*id]))
            .collect();
        assert_eq!(
            visible,
            vec![root],
            "exactly page `{}` is visible at slot {slot}",
            page.key
        );
    }
}

/// Every element at or below `id`, `id` first.
fn subtree_of(tree: &ElementTree, id: ElementId) -> Vec<ElementId> {
    let mut out = vec![id];
    let mut index = 0;
    while index < out.len() {
        out.extend(tree.arena[out[index]].children.iter().copied());
        index += 1;
    }
    return out;
}

#[test]
fn an_unselected_page_paints_nothing() {
    let (mut tree, mut engine) = gallery();
    let roots = page_roots(&tree);
    select(&mut tree, &mut engine, 0);

    // The elements of every hidden page, collected up front.
    let mut hidden: Vec<ElementId> = Vec::new();
    for root in roots.iter().skip(1) {
        hidden.extend(subtree_of(&tree, *root));
    }
    assert!(!hidden.is_empty(), "the other pages have elements");

    // The frame the window would draw: the scene walk must not attribute
    // a single rect to any of them.
    let mut text = nui_text::TextSystem::with_embedded_font();
    let scene = nui_render::SceneBuilder::build_with_context(
        &tree,
        &mut text,
        nui_render::SceneContext {
            focused: engine.focused(),
            image_keys: &std::collections::HashMap::new(),
        },
    );
    assert!(
        !scene.rects.is_empty(),
        "the selected page paints something"
    );
    for source in &scene.sources {
        assert!(
            !hidden.contains(source),
            "a hidden page's element drew a rect: {}",
            tree.arena[*source].id.as_deref().unwrap_or("?")
        );
    }
    // ...and nothing of the hidden pages is reachable by pointer either.
    let content = tree.lookup_id("content").unwrap();
    let (x, width) = (number(&tree, content, "x"), number(&tree, content, "width"));
    for probe in [0.25, 0.5, 0.75] {
        let point = nui_core::Point::new(x + width * probe, 40.0);
        if let Some(hit) = nui::hit_test(&tree, point) {
            assert!(
                !hidden.contains(&hit.element),
                "a hidden page answered a pointer probe at {point:?}"
            );
        }
    }
}

#[test]
fn no_page_file_is_orphaned() {
    // `include_str!` already fails the build when a PAGES entry has no
    // file; this is the other direction — a file with no entry would
    // silently never appear in the gallery.
    //
    // A page may have two files: `<key>.nui` (the fragment) and
    // `<key>_defs.nui` (the top-level component declarations it uses).
    let keys: Vec<&str> = host::PAGES.iter().map(|page| return page.key).collect();
    let dir = std::fs::read_dir("examples/gallery/pages")
        .expect("tests run from the crate root, so the pages dir is reachable");
    for entry in dir {
        let path = entry.expect("readable dir entry").path();
        if path.extension().and_then(|ext| return ext.to_str()) != Some("nui") {
            continue;
        }
        let stem = path.file_stem().unwrap().to_string_lossy().to_string();
        let owner = stem.strip_suffix("_defs").unwrap_or(&stem);
        assert!(
            keys.contains(&owner),
            "`pages/{stem}.nui` is not in the gallery's PAGES table — it will never be shown"
        );
    }
}

/// The first element of type `ty`, in document order.
fn first_of_type(tree: &ElementTree, ty: &str) -> Option<ElementId> {
    let mut found = None;
    tree.visit_pre_order(|id, element| {
        if found.is_none() && element.ty == ty {
            found = Some(id);
        }
    });
    return found;
}

/// The first button carrying `label`. Fragments do not give every control an
/// id, so a label is the only handle a test has on them.
fn button_labelled(tree: &ElementTree, label: &str) -> ElementId {
    let mut found = None;
    tree.visit_pre_order(|id, element| {
        if found.is_some() || element.ty != "Button" {
            return;
        }
        if element
            .get("label")
            .and_then(|value| return value.as_str().ok())
            == Some(label)
        {
            found = Some(id);
        }
    });
    return found.unwrap_or_else(|| panic!("no Button labelled `{label}` in the gallery"));
}

fn scene_of(tree: &ElementTree, engine: &Engine) -> nui_render::Scene {
    let mut text = nui_text::TextSystem::with_embedded_font();
    return nui_render::SceneBuilder::build_with_context(
        tree,
        &mut text,
        nui_render::SceneContext {
            focused: engine.focused(),
            image_keys: &std::collections::HashMap::new(),
        },
    );
}

/// The widgets page's dialog, opened and closed through the buttons that
/// drive it.
///
/// This is the end-to-end shape of a modal, and it exercises two things the
/// per-crate tests do not:
///
/// - `Dialog(open <=> page.sheetOpen)` is a `<=>` whose *declaring* side is
///   never written by the document — the page writes `sheetOpen` and the
///   dialog has to follow, which is the reverse direction of the pair;
/// - the effect write that does it (`on click => page.sheetOpen = true`)
///   goes through the effect path, not a host or widget write.
///
/// The dialog must also end up in an overlay pass of its own: its scrim is
/// a rect and the page behind it is full of glyphs, and glyphs are the last
/// pipeline of a pass, so a scrim folded into the main scene would sit
/// *under* the text it exists to dim.
#[test]
fn the_widgets_page_dialog_opens_and_occludes() {
    let (mut tree, mut engine) = gallery();
    let widgets = host::PAGES
        .iter()
        .position(|page| return page.key == "widgets")
        .expect("the gallery has a widgets page");
    select(&mut tree, &mut engine, widgets);

    let dialog = first_of_type(&tree, "Dialog").expect("the widgets page declares a Dialog");
    assert!(
        !nui_runtime::widget::is_open(&tree.arena[dialog]),
        "the dialog starts closed"
    );
    assert!(
        scene_of(&tree, &engine).overlays.is_empty(),
        "a closed dialog contributes no overlay pass"
    );

    let open_button = button_labelled(&tree, "Open dialog");
    let _ = engine.emit_bubble(&mut tree, open_button, "click");
    settle(&mut tree, &mut engine);
    assert!(
        nui_runtime::widget::is_open(&tree.arena[dialog]),
        "the dialog's `open` follows the page's `sheetOpen` through the `<=>` pair"
    );

    let scene = scene_of(&tree, &engine);
    assert_eq!(scene.overlays.len(), 1, "the open dialog owns one pass");
    let inner = &scene.overlays[0].scene;
    let scrim = inner.rects.first().expect("the backdrop is the first part");
    assert_eq!(
        (scrim.geometry.size.width, scrim.geometry.size.height),
        (host::WINDOW.width, host::WINDOW.height),
        "the scrim covers the whole window"
    );
    assert!(
        scrim.fill.alpha() > 0.0 && scrim.fill.alpha() < 1.0,
        "and dims it rather than blanking it: {:?}",
        scrim.fill
    );
    // The page's own text stays in the main scene, which is the point:
    // the renderer paints the whole main scene first and this pass after,
    // so those glyphs are dimmed instead of floating above the scrim.
    assert!(
        !scene.texts.is_empty(),
        "the page's glyphs are still in the main scene"
    );

    let cancel_button = button_labelled(&tree, "Cancel");
    let _ = engine.emit_bubble(&mut tree, cancel_button, "click");
    settle(&mut tree, &mut engine);
    assert!(
        !nui_runtime::widget::is_open(&tree.arena[dialog]),
        "Cancel closes it again"
    );
    assert!(
        scene_of(&tree, &engine).overlays.is_empty(),
        "and the pass goes away with it"
    );
}
