//! Canvas demo (FUTURE batch 3): the imperative escape hatch. The `.nui`
//! side is only a shell; the host registers a `CanvasBehavior` whose
//! painter records commands. Fill goes through ear clipping, strokes reuse
//! the capsule pipeline, and the result composites via the offscreen layer
//! pipeline — zero new GPU code.
//!
//! Run: `cargo run -p nui --example canvas_demo`
//! (requires a display; headless environments print an error and exit).

use nui::{AppConfig, Application};
use nui_core::Size;
use nui_runtime::canvas::{CanvasCap, CanvasPainter};
use nui_runtime::{
    BehaviorContext, BehaviorFactory, ComponentDesc, ElementBehavior, ElementId, Registry,
};

/// Host behavior painting a small chart: an area line (fill + stroke) and
/// a stroked arc gauge, drawn once at instantiation. Repainting on demand
/// (a `repaint()` call from effects) is a FUTURE item — the scene builder
/// re-interprets the buffer every frame, so an updated buffer shows up
/// next frame automatically.
#[derive(Debug)]
struct ChartBehavior {
    painter: CanvasPainter,
}

impl ElementBehavior for ChartBehavior {
    fn on_signal(
        &mut self,
        _context: &mut BehaviorContext<'_>,
        _element: ElementId,
        _signal: &str,
    ) {
    }

    fn canvas(&self) -> Option<CanvasPainter> {
        return Some(self.painter.clone());
    }
}

fn chart_canvas_registry() -> Registry {
    let mut registry = Registry::new();
    let factory: BehaviorFactory = Box::new(|| {
        let painter = CanvasPainter::new();
        // Area chart: a polyline through sample points, filled to the
        // baseline and stroked on top.
        painter.move_to(0.0, 90.0);
        painter.line_to(0.0, 60.0);
        painter.cubic_to(40.0, 20.0, 70.0, 80.0, 105.0, 45.0);
        painter.cubic_to(140.0, 10.0, 180.0, 55.0, 210.0, 30.0);
        painter.line_to(240.0, 50.0);
        painter.line_to(240.0, 90.0);
        painter.close();
        painter.fill(nui_core::Color::from_rgba8(85, 170, 238, 90));
        painter.clear();
        painter.move_to(0.0, 60.0);
        painter.cubic_to(40.0, 20.0, 70.0, 80.0, 105.0, 45.0);
        painter.cubic_to(140.0, 10.0, 180.0, 55.0, 210.0, 30.0);
        painter.line_to(240.0, 50.0);
        painter.stroke(3.0, CanvasCap::Round, nui_core::Color::from_rgb8(85, 170, 238));
        painter.clear();
        // Gauge: a 270-degree arc, opened at the bottom.
        let segments = 36;
        let start_deg = 135.0f32;
        let sweep_deg = 270.0f32;
        painter.move_to(
            120.0 + 60.0 * (start_deg * std::f32::consts::PI / 180.0).cos(),
            170.0 + 60.0 * (start_deg * std::f32::consts::PI / 180.0).sin(),
        );
        for step in 1..=segments {
            let angle = (start_deg + sweep_deg * step as f32 / segments as f32)
                * std::f32::consts::PI
                / 180.0;
            painter.line_to(120.0 + 60.0 * angle.cos(), 170.0 + 60.0 * angle.sin());
        }
        painter.stroke(8.0, CanvasCap::Round, nui_core::Color::from_rgb8(123, 196, 127));
        return Box::new(ChartBehavior { painter });
    });
    registry.register_component(
        ComponentDesc {
            name: "Canvas".to_string(),
            properties: Vec::new(),
        },
        Some(factory),
    );
    return registry;
}

fn main() {
    let config = AppConfig::new(DEMO, "nui — canvas", Size::new(340.0, 400.0));
    Application::new(config)
        .with_registry(chart_canvas_registry())
        .run();
}

const DEMO: &str = r#"
component CanvasDemo {
    Window(id = root) {
        Column(id = content, spacing = 16dp, padding = 24dp) {
            Text(content = "canvas — the host paints through a command buffer", font.size = 14dp)

            Canvas(id = chart, width = 240dp, height = 230dp, clip = true)
        }
    }
}
"#;
