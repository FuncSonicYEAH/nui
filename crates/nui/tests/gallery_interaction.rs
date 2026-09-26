//! The gallery's interaction seam, end to end: sidebar click → page
//! switch → pointer click on a field → typing.
//!
//! `tests/gallery.rs` pins the gallery's *structure*; the unit tests pin
//! each layer's *rules*. Neither can catch what this file exists for: a
//! bug that only appears when the layers run **in sequence** against the
//! real document. The one it caught — a second text field could not be
//! clicked into — was `hit_test` handing the click to a *hidden page's
//! stale box* (layout skips an invisible subtree, so its last laid-out
//! boxes stay in the slots and win the smallest-area contest), which then
//! focused nothing and blurred.
//!
//! The window parts of `WindowHost` cannot run headless, so the host's
//! pointer sequence is replicated here from its public halves (`hit_test`,
//! `focus_element`, `WidgetStates`, the frame pipeline order). If
//! `WindowHost`'s pointer path changes shape, this file must follow —
//! that is the price of driving the seam without a display.

#![allow(clippy::unwrap_used)]

use nui_core::{Key, Modifiers, Point, Size, Value};
use nui_runtime::{ElementId, ElementTree, Engine, WidgetStates};

#[path = "../examples/gallery/host.rs"]
mod host;

const VIEWPORT: Size = host::WINDOW;

fn example() -> (ElementTree, Engine) {
    let source = host::document();
    let outcome =
        nui_compiler::compile_with_functions(&source, &host::build_registry().function_names());
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    let mut instance = nui_runtime::instantiate_with(&outcome.document, host::build_registry());
    instance.engine.run_registry_init(&mut instance.tree);
    let mut tree = instance.tree;
    let engine = instance.engine;
    let mut text = nui_text::TextSystem::with_embedded_font();
    for _ in 0..3 {
        nui_layout::layout_with_text(&mut tree, VIEWPORT, Some(&mut text));
    }
    return (tree, engine);
}

fn find(tree: &ElementTree, id_name: &str) -> ElementId {
    return tree
        .lookup_id(id_name)
        .unwrap_or_else(|| panic!("missing {id_name}"));
}

fn focus_hit(tree: &mut ElementTree, engine: &mut Engine, hit: Option<ElementId>) {
    let mut current = hit;
    while let Some(id) = current {
        let target = nui_runtime::widget::label_target(&tree.arena[id])
            .and_then(|name| return tree.lookup_id(name));
        if let Some(target) = target {
            let _ = engine.focus_element(tree, target);
            return;
        }
        if tree.arena[id].is_focusable() {
            let _ = engine.focus_element(tree, id);
            return;
        }
        current = tree.arena[id].parent;
    }
    engine.blur();
}

fn hit_interactive(tree: &ElementTree, position: Point) -> Option<ElementId> {
    let mut current = nui::hit_test(tree, position).map(|hit| return hit.element);
    while let Some(id) = current {
        if nui_runtime::widget::is_enabled(&tree.arena[id]) {
            return Some(id);
        }
        current = tree.arena[id].parent;
    }
    return None;
}

struct Ui {
    tree: ElementTree,
    engine: Engine,
    widgets: WidgetStates,
    cursor: Point,
    text: nui_text::TextSystem,
}

impl Ui {
    fn move_to(&mut self, point: Point) {
        self.cursor = point;
        let input = nui_runtime::PointerInput {
            position: point,
            inside: true,
            down: false,
        };
        let changed = self.widgets.update(&mut self.engine, &mut self.tree, input);
        if !changed.is_empty() {
            self.frame();
        }
    }

    fn frame(&mut self) {
        let mut text =
            std::mem::replace(&mut self.text, nui_text::TextSystem::with_embedded_font());
        let _ = self.engine.propagate(&mut self.tree);
        let rebuilt = self.engine.sync_for_nodes(&mut self.tree);
        if rebuilt > 0 {
            let _ = self.engine.propagate(&mut self.tree);
        }
        let _ = self.engine.apply_when_blocks(&mut self.tree);
        nui_layout::layout_with_text(&mut self.tree, VIEWPORT, Some(&mut text));
        let input = nui_runtime::PointerInput {
            position: self.cursor,
            inside: true,
            down: false,
        };
        let _ = self.widgets.update(&mut self.engine, &mut self.tree, input);
        let _ = self.engine.take_changes();
        self.text = text;
    }

    fn click(&mut self, point: Point) -> Option<ElementId> {
        self.cursor = point;
        let hit = hit_interactive(&self.tree, point);
        self.widgets.capture(hit);
        focus_hit(&mut self.tree, &mut self.engine, hit);
        let input = nui_runtime::PointerInput {
            position: point,
            inside: true,
            down: true,
        };
        let changed = self.widgets.update(&mut self.engine, &mut self.tree, input);
        if !changed.is_empty() {
            let mut text =
                std::mem::replace(&mut self.text, nui_text::TextSystem::with_embedded_font());
            let _ = self.engine.propagate(&mut self.tree);
            let rebuilt = self.engine.sync_for_nodes(&mut self.tree);
            if rebuilt > 0 {
                let _ = self.engine.propagate(&mut self.tree);
            }
            let _ = self.engine.apply_when_blocks(&mut self.tree);
            nui_layout::layout_with_text(&mut self.tree, VIEWPORT, Some(&mut text));
            let down_input = nui_runtime::PointerInput {
                position: point,
                inside: true,
                down: true,
            };
            let _ = self
                .widgets
                .update(&mut self.engine, &mut self.tree, down_input);
            let _ = self.engine.take_changes();
            self.text = text;
        }
        let captured = self.widgets.captured();
        self.widgets.release_capture();
        let inside = captured
            .is_some_and(|id| return nui::element_bounds(&self.tree, id).unwrap().contains(point));
        if let Some(target) = captured
            && inside
        {
            let _ = self.engine.emit_bubble(&mut self.tree, target, "click");
            let _ = nui_runtime::widget::activate(&mut self.engine, &mut self.tree, target);
            self.frame();
        }
        return hit;
    }

    fn wheel(&mut self, dy: f64, point: Point) {
        let hit = nui::hit_test(&self.tree, point).map(|hit| return hit.element);
        let mut current = hit;
        while let Some(id) = current {
            if self.tree.arena[id].ty == "Scroll" || self.tree.arena[id].ty == "ListView" {
                let scroll_y = match self.tree.arena[id].get("scroll_y") {
                    Some(Value::Float(value)) => *value,
                    _ => 0.0,
                };
                let limit = nui_runtime::widget::max_scroll_y(&self.engine, &self.tree, id);
                let next = (scroll_y + dy).clamp(0.0, f64::from(limit));
                self.engine
                    .set_direct(&mut self.tree, id, "scroll_y", Value::Float(next));
                self.frame();
                return;
            }
            current = self.tree.arena[id].parent;
        }
    }
}

fn viewport_point(tree: &ElementTree, id: ElementId) -> Point {
    let bounds = nui::element_bounds(tree, id).unwrap();
    return Point::new(
        bounds.origin.x + 10.0,
        bounds.origin.y + bounds.size.height / 2.0,
    );
}

#[test]
fn switching_pages_then_clicking_each_field_focuses_and_types() {
    let (tree, engine) = example();
    let mut ui = Ui {
        tree,
        engine,
        widgets: WidgetStates::new(),
        cursor: Point::ZERO,
        text: nui_text::TextSystem::with_embedded_font(),
    };

    // 1. The sidebar click switches to the text fields page through the
    //    real click path (press → focus walk → release → `click` signal).
    let nav = find(&ui.tree, "nav_text_fields");
    let nav_point = viewport_point(&ui.tree, nav);
    ui.move_to(nav_point);
    ui.click(nav_point);
    assert_eq!(
        ui.tree.arena[find(&ui.tree, "root")].get("page"),
        Some(&Value::Int(2)),
        "text_fields is slot 2"
    );

    let name = find(&ui.tree, "text_name");
    let secret = find(&ui.tree, "text_secret");
    let bio = find(&ui.tree, "text_bio");
    let mail = find(&ui.tree, "text_mail");

    // 2. Click the first field and type into it.
    let pa = viewport_point(&ui.tree, name);
    ui.move_to(pa);
    ui.click(pa);
    assert_eq!(ui.engine.focused(), Some(name), "the first field focuses");
    assert!(
        ui.engine.handle_text_input(&mut ui.tree, "Ada"),
        "the first field takes input"
    );
    ui.frame();

    // 3. The regression: clicking a *second* field used to land on a
    //    hidden page's stale box, focus nothing, and blur. Every field
    //    must take over focus and input.
    for (id, typed) in [(secret, "hunter2"), (bio, "hello\nworld"), (mail, "a@b.co")] {
        let point = viewport_point(&ui.tree, id);
        ui.move_to(point);
        ui.click(point);
        assert_eq!(ui.engine.focused(), Some(id), "the field takes focus");
        assert!(
            ui.engine.handle_text_input(&mut ui.tree, typed),
            "the field takes input"
        );
    }

    // 4. Scroll, then click the fields that moved; the boxes the hit test
    //    sees must track the scroll.
    ui.wheel(160.0, Point::new(600.0, 400.0));
    let scrolled = viewport_point(&ui.tree, bio);
    ui.move_to(scrolled);
    ui.click(scrolled);
    assert_eq!(ui.engine.focused(), Some(bio), "a scrolled field focuses");
    assert!(
        ui.engine.handle_text_input(&mut ui.tree, "more"),
        "it types"
    );

    // 5. Tab still walks from wherever the pointer left focus.
    ui.engine
        .handle_key(&mut ui.tree, Key::Tab, Modifiers::NONE);
    assert!(ui.engine.focused().is_some(), "tab keeps focus somewhere");
}
