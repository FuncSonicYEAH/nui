//! Waveline demo: an infinitely looping `Timer` drives `phase`, which feeds
//! two procedural waves (one breathing via `sin(phase)`, one mirrored) and
//! a static Cava-style `levels` line. Everything renders through the stroke
//! pipeline from FUTURE batch 1 — no new GPU code.
//!
//! Run: `cargo run -p nui --example waveline`
//! (requires a display; headless environments print an error and exit).

use nui::{AppConfig, Application};
use nui_core::Size;

const DEMO: &str = r#"
component WavelineDemo {
    property tick: Float = 0

    Window(id = root) {
        Column(id = content, spacing = 16dp, padding = 24dp) {
            Text(content = "waveline — an infinite timer drives the phase", font.size = 14dp)

            Timer(id = ticker, interval = 33ms, running = true) {
                on timer => tick += 11
            }

            Waveline(id = wave, width = 292dp, height = 80dp,
                     amplitude <- 16 + 7 * sin(tick * 0.017),
                     frequency = 3, phase <- tick,
                     stroke.width = 3dp, color = #55aaee)

            Waveline(id = twin, width = 292dp, height = 120dp,
                     amplitude = 26, frequency = 2.5, phase <- tick,
                     mirror = true, stroke.width = 2dp, color = #e05555)

            Waveline(id = samples, width = 292dp, height = 70dp,
                     levels = "0.2 0.45 0.85 0.5 0.75 0.3 0.6 0.25 0.55 0.4 0.7 0.3 0.5 0.65 0.35 0.55",
                     amplitude = 26, stroke.width = 2dp, color = #7bc47f)
        }
    }
}
"#;

fn main() {
    let config = AppConfig::new(DEMO, "nui — waveline", Size::new(340.0, 480.0));
    Application::new(config).run();
}
