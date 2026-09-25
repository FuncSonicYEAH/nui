//! Rotation & gradient demo: click the diamond to spin it (bindings drive
//! `rotation`), and two gradient bars show `gradient.from/to/angle`. The
//! bottom label exercises the math builtins (`sin`, `floor`) inside an
//! interpolated string.
//!
//! Run: `cargo run -p nui --example rotation_gradient`
//! (requires a display; headless environments print an error and exit).

use nui::{AppConfig, Application};
use nui_core::Size;

const DEMO: &str = r#"
component RotationGradient {
    property spin: Float = 45

    Window(id = root) {
        Column(id = content, spacing = 16dp, padding = 24dp) {
            Text(content = "rotation + gradient + math builtins", font.size = 14dp)

            Rectangle(id = diamond, width = 80dp, height = 80dp, radius = 14dp,
                      fill = #e05555, rotation <- spin) {
                on click => spin += 15
            }

            Rectangle(id = bar, height = 40dp,
                      gradient.from = #ff5544, gradient.to = #4466ff,
                      gradient.angle = 0)

            Rectangle(id = pill, height = 40dp, radius = 20dp,
                      gradient.from = #ffb347, gradient.to = #7a4c9e)

            Text(content <- "floor(sin(1.57) * 100) / 100 = {floor(sin(1.57) * 100.0) / 100.0}")
        }
    }
}
"#;

fn main() {
    let config = AppConfig::new(DEMO, "nui — rotation & gradient", Size::new(360.0, 420.0));
    Application::new(config).run();
}
