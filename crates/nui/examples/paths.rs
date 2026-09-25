//! Paths demo: filled triangles and stars (earcut), plus stroked bezier
//! curves through the capsule pipeline. Everything below renders with the
//! FUTURE batch 2 path pipeline — no host code, just the `d` attribute.
//!
//! Run: `cargo run -p nui --example paths`
//! (requires a display; headless environments print an error and exit).

use nui::{AppConfig, Application};
use nui_core::Size;

const DEMO: &str = r#"
component Paths {
    Window(id = root) {
        Column(id = content, spacing = 20dp, padding = 24dp) {
            Text(content = "path fill + stroke", font.size = 14dp)

            Path(id = triangle, width = 292dp, height = 140dp,
                 d = "M 10 10 L 282 10 L 146 130 Z",
                 fill = #ff5544, stroke.width = 3dp, stroke.color = #ffffff)

            Path(id = star, width = 292dp, height = 120dp,
                 d = "M 146 10 L 157.8 43.8 L 193.6 44.5 L 165 66.2 L 175.4 100.5 L 146 80 L 116.6 100.5 L 127 66.2 L 98.4 44.5 L 134.2 43.8 Z",
                 fill = #7bc47f)

            Path(id = curve, width = 292dp, height = 110dp,
                 d = "M 40 70 C 40 25 100 25 146 45 S 240 100 252 60 Q 260 20 200 18 T 60 40 Z",
                 fill = #55aaee, stroke.width = 2dp, stroke.color = #ddeeff)
        }
    }
}
"#;

fn main() {
    let config = AppConfig::new(DEMO, "nui — paths", Size::new(340.0, 560.0));
    Application::new(config).run();
}
