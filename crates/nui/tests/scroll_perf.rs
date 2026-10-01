//! Where the time goes in one scroll frame.
//!
//! A scrollbar drag writes `scroll_y` on every pointer move, and the host
//! answers each write with a full `run_frame_pipeline` — timers,
//! animations, binding propagation, `ListView` row reconciliation, taffy
//! layout and text shaping. That is a lot of machinery for what is
//! *conceptually* a viewport translation: `scroll_y` is not a layout
//! input at all (`nui-layout` never reads it), so most of that work may be
//! provably unnecessary.
//!
//! This is not a pass/fail test but a measurement, printed with
//! `--nocapture`. It exists so the optimisation is aimed at a number
//! rather than a hunch, and so the number is reproducible afterwards.

#![allow(clippy::unwrap_used)]

use std::time::Instant;

use nui_core::{Point, Size, Value};
use nui_runtime::{ElementId, ElementTree, Engine, WidgetStates};

#[path = "../examples/gallery/host.rs"]
mod host;

const VIEWPORT: Size = host::WINDOW;

fn example() -> (ElementTree, Engine) {
    let source = host::document();
    let outcome = nui_compiler::compile_with_host(&source, &host::build_registry().vocabulary());
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    let mut instance = nui_runtime::instantiate_with(&outcome.document, host::build_registry());
    instance.engine.run_registry_init(&mut instance.tree);
    let mut tree = instance.tree;
    let mut engine = instance.engine;
    let mut text = nui_text::TextSystem::with_embedded_font();

    // The gallery shows one page at a time and hides the rest with
    // `visible <- root.page == <slot>`. A hidden page is skipped by
    // layout, so the `ListView` would have no geometry and nothing to
    // scroll. Select the list page the way the sidebar does, then settle.
    let root: ElementId = tree.lookup_id("root").expect("the gallery has a root");
    let slot = host::PAGES
        .iter()
        .position(|page| return page.key == "list")
        .expect("the gallery has a list page") as i64;
    let _ = engine.set_direct(&mut tree, root, "page", Value::Int(slot));
    for _ in 0..3 {
        let _ = engine.propagate(&mut tree);
        nui_layout::layout_with_text(&mut tree, VIEWPORT, Some(&mut text));
    }
    return (tree, engine);
}

fn element_count(tree: &ElementTree) -> usize {
    return tree.arena.len();
}

/// Times each stage of the host's frame pipeline separately.
struct Stage {
    label: &'static str,
    total: std::time::Duration,
}

impl Stage {
    fn new(label: &'static str) -> Stage {
        return Stage {
            label,
            total: std::time::Duration::ZERO,
        };
    }

    fn time<T>(&mut self, body: impl FnOnce() -> T) -> T {
        let start = Instant::now();
        let out = body();
        self.total += start.elapsed();
        return out;
    }

    fn report(&self, frames: u32) {
        let per = self.total.as_secs_f64() * 1000.0 / f64::from(frames);
        println!("  {:<28} {:>8.3} ms/frame", self.label, per);
    }
}

#[test]
fn scroll_drag_frame_cost_by_stage() {
    let (mut tree, mut engine) = example();
    let list: ElementId = tree
        .lookup_id("list_view")
        .expect("the gallery's list page has a ListView");
    let mut widgets = WidgetStates::new();
    let mut text = nui_text::TextSystem::with_embedded_font();

    let limit = nui_runtime::widget::max_scroll_y(&engine, &tree, list);
    println!(
        "\nthe list page: {} elements, {} dp of travel",
        element_count(&tree),
        limit
    );
    assert!(
        limit > 0.0,
        "the list must overflow for this to measure anything"
    );

    const FRAMES: u32 = 60;
    let mut timers = std::collections::HashMap::new();
    let mut propagate = Stage::new("binding propagate");
    let mut sync = Stage::new("ListView row reconcile");
    let mut layout = Stage::new("taffy layout + text");
    let mut widget = Stage::new("widget states");
    let mut full = Stage::new("FULL pipeline");
    let mut scroll_only = Stage::new("SCROLL-ONLY pipeline");

    for frame in 0..FRAMES {
        // Sweep the offset the way a drag would: a step per frame.
        let scroll_y = f64::from(limit) * (f64::from(frame) / f64::from(FRAMES));
        engine.set_direct(&mut tree, list, "scroll_y", Value::Float(scroll_y));

        let start = Instant::now();
        let _ = engine.tick_timers(&mut tree, nui_core::Duration::ZERO, &mut timers);
        engine.tick_animations(&mut tree, nui_core::Duration::ZERO);
        let _ = propagate.time(|| return engine.propagate(&mut tree));
        let rebuilt = sync.time(|| return engine.sync_for_nodes(&mut tree));
        if rebuilt > 0 {
            let _ = propagate.time(|| return engine.propagate(&mut tree));
        }
        let _ = engine.apply_when_blocks(&mut tree);
        layout.time(|| {
            nui_layout::layout_with_text(&mut tree, VIEWPORT, Some(&mut text));
        });
        widget.time(|| {
            let input = nui_runtime::PointerInput {
                position: Point::ZERO,
                inside: true,
                down: true,
            };
            return widgets.update(&mut engine, &mut tree, input);
        });
        let _ = engine.take_changes();
        full.total += start.elapsed();

        // The path the host actually takes for a drag or a wheel notch
        // (`run_scroll_pipeline`, D38): propagate + row reconcile + widget
        // state, no layout. Measured on the *same* offset the full run
        // just settled, so the two numbers describe the same frame.
        let start = Instant::now();
        let _ = engine.propagate(&mut tree);
        let rebuilt = engine.sync_for_nodes(&mut tree);
        if rebuilt > 0 {
            let _ = engine.propagate(&mut tree);
        }
        let _ = engine.take_changes();
        scroll_only.total += start.elapsed();
    }

    println!("\none scroll frame, broken down:");
    propagate.report(FRAMES);
    sync.report(FRAMES);
    layout.report(FRAMES);
    widget.report(FRAMES);
    println!("  {}", "-".repeat(42));
    full.report(FRAMES);
    scroll_only.report(FRAMES);

    let before = full.total.as_secs_f64() * 1000.0 / f64::from(FRAMES);
    let after = scroll_only.total.as_secs_f64() * 1000.0 / f64::from(FRAMES);
    println!(
        "\n  drag/wheel frame: {before:.3} ms -> {after:.3} ms  ({:.1}x faster)\n",
        before / after.max(f64::MIN_POSITIVE)
    );
}
