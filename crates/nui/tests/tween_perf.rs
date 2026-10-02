//! Where the time goes in one tween frame.
//!
//! A `tween` binding retargets on every state change and then advances a
//! little on every frame until it arrives. The host answers each frame
//! with the full `run_frame_pipeline` — animations, binding propagation,
//! row reconciliation, taffy layout and text shaping. For a tween on
//! `opacity` most of that may be provably unnecessary: `opacity` is not a
//! layout input (`nui-layout` never reads it), so re-running the layout
//! cannot change any box while the tween runs.
//!
//! This is not a pass/fail test but a measurement, printed with
//! `--nocapture`. It exists so the optimisation is aimed at a number
//! rather than a hunch, and so the number is reproducible afterwards.

#![allow(clippy::unwrap_used)]

use std::time::Instant;

use nui_core::{Point, Size, Value};
use nui_runtime::{ElementId, ElementTree, Engine, ModelRow, VecModel, WidgetStates};

const VIEWPORT: Size = Size::new(800.0, 600.0);

/// A page of rows whose swatch opacity tweens; `on` toggles them all at
/// once the way a selection change would.
const SOURCE: &str = r#"
component Bench {
    property rows: Model

    Window(id = root) {
        Column(id = bench_page, spacing = 10dp, padding = 16dp) {
            state lit: Bool = false
            For(item in root.rows) {
                Row(height = 44dp, spacing = 10dp) {
                    Rectangle(width = 18dp, height = 18dp, radius = 9dp,
                              fill = #336699,
                              opacity <- tween(bench_page.lit ? 1.0 : 0.4,
                                               duration = 200ms, easing = ease-out))
                    Text(content <- item.label, color = #e8ecf2, font.size = 14dp)
                }
            }
        }
    }
}
"#;

fn example(rows: usize) -> (ElementTree, Engine, ElementId) {
    let outcome = nui_compiler::compile(SOURCE);
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    let instance = nui_runtime::instantiate(&outcome.document);
    let mut tree = instance.tree;
    let mut engine = instance.engine;

    // Seed the model the way a registry attach hook would, then let the
    // `For` build its rows and a few passes settle the boxes.
    let model_rows: Vec<ModelRow> = (0..rows)
        .map(|index| return vec![("label".to_string(), Value::String(format!("row {index}")))])
        .collect();
    let model = engine.add_model(Box::new(VecModel::from_rows(model_rows)));
    let root: ElementId = tree.lookup_id("root").expect("the bench has a root");
    engine.set_direct(&mut tree, root, "rows", Value::Model(model.0));
    let mut text = nui_text::TextSystem::with_embedded_font();
    for _ in 0..3 {
        let _ = engine.propagate(&mut tree);
        engine.sync_for_nodes(&mut tree);
        nui_layout::layout_with_text(&mut tree, VIEWPORT, Some(&mut text));
    }
    let page: ElementId = tree.lookup_id("bench_page").expect("bench page root");
    return (tree, engine, page);
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
fn tween_frame_cost_by_stage() {
    let rows = 24;
    let (mut tree, mut engine, page) = example(rows);
    let mut widgets = WidgetStates::new();
    let mut text = nui_text::TextSystem::with_embedded_font();
    let mut timers = std::collections::HashMap::new();

    println!(
        "\nthe bench page: {} elements, {rows} tweened swatches",
        tree.arena.len()
    );

    // Toggle once: every swatch's `tween` binding retargets and the clock
    // starts. The tween runs 200 ms; a frame is 16 ms. One settle frame
    // consumes the `lit` write itself, so the measured frames below see
    // only what *tween frames* write.
    engine.set_direct(&mut tree, page, "lit", Value::Bool(true));
    let _ = engine.propagate(&mut tree);
    let _ = engine.take_changes();

    const FRAMES: u32 = 12;
    let mut animate = Stage::new("animation tick");
    let mut propagate = Stage::new("binding propagate");
    let mut sync = Stage::new("For row reconcile");
    let mut layout = Stage::new("taffy layout + text");
    let mut widget = Stage::new("widget states");
    let mut full = Stage::new("FULL pipeline");
    let mut all_writes_paint_only = true;

    for _ in 0..FRAMES {
        let start = Instant::now();
        let _ = engine.tick_timers(
            &mut tree,
            nui_core::Duration::from_millis(16.0),
            &mut timers,
        );
        animate.time(|| {
            engine.tick_animations(&mut tree, nui_core::Duration::from_millis(16.0));
        });
        let _ = propagate.time(|| return engine.propagate(&mut tree));
        let rebuilt = sync.time(|| return engine.sync_for_nodes(&mut tree));
        if rebuilt > 0 {
            let _ = propagate.time(|| return engine.propagate(&mut tree));
        }
        let _ = engine.apply_when_blocks(&mut tree);
        // The claim the optimisation rests on, sampled mid-frame before
        // the drain: nothing written this frame is a layout input, so the
        // layout stage below is provably idle work here.
        all_writes_paint_only &= engine.pending_changes().iter().all(|change| {
            return nui_core::props::is_paint_only_property(&change.property);
        });
        layout.time(|| {
            nui_layout::layout_with_text(&mut tree, VIEWPORT, Some(&mut text));
        });
        widget.time(|| {
            let input = nui_runtime::PointerInput {
                position: Point::ZERO,
                inside: true,
                down: false,
            };
            return widgets.update(&mut engine, &mut tree, input);
        });
        let _ = engine.take_changes();
        full.total += start.elapsed();
    }

    println!("\none tween frame, broken down:");
    animate.report(FRAMES);
    propagate.report(FRAMES);
    sync.report(FRAMES);
    layout.report(FRAMES);
    widget.report(FRAMES);
    println!("  {}", "-".repeat(42));
    full.report(FRAMES);
    println!(
        "\n  every write across all {FRAMES} tween frames is paint-only: {all_writes_paint_only}\n"
    );
    assert!(
        all_writes_paint_only,
        "a tween on opacity must only write paint-only properties, \
         or the D40 layout skip would pin stale boxes"
    );
}
