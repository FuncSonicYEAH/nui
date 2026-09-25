//! Stroke demo (FUTURE batch 1): a polyline "chart" (round caps), a hairline
//! divider with butt caps, and an arc progress ring — `start`/`end` sweep
//! clockwise in screen coordinates (y down), so `-90` starts at the top.
//!
//! Run: `cargo run -p nui --example strokes`
//! (requires a display; headless environments print an error and exit).

use nui::{AppConfig, Application};
use nui_core::Size;

const DEMO: &str = r#"
component Strokes {
    Window(id = root) {
        Column(id = content, spacing = 20dp, padding = 24dp) {
            Text(content = "polyline + arc stroking", font.size = 14dp)

            Polyline(id = chart, width = 292dp, height = 110dp,
                     points = "0,95 48,60 96,74 144,28 192,46 240,14 292,30",
                     stroke.width = 3dp, color = #55aaee)

            Polyline(id = divider, width = 292dp, height = 2dp,
                     points = "0,0 292,0",
                     stroke.width = 2dp, stroke.cap = "butt", color = #444a55)

            Arc(id = ring, width = 150dp, height = 150dp,
                cx = 75, cy = 75, radius = 60,
                start = -90, end = 180,
                stroke.width = 8dp, stroke.cap = "round", color = #e05555)
        }
    }
}
"#;

fn main() {
    let config = AppConfig::new(DEMO, "nui — strokes", Size::new(340.0, 440.0));
    Application::new(config).run();
}
