//! Counter demo (M3 acceptance): a clickable counter window driven entirely
//! by an inline `.nui` document — bindings re-render the label, the click
//! handler mutates the component property.
//!
//! Run: `cargo run -p nui --example counter`
//! (requires a display; headless environments print an error and exit).

use nui::{AppConfig, Application};
use nui_core::Size;

const COUNTER: &str = r#"
component Counter {
    property count: Int = 0

    Window(id = root) {
        Column(id = content, spacing = 12dp, padding = 24dp) {
            Rectangle(id = panel, fill = #336699, radius = 12dp, opacity = 0.25)
            Text(id = label, content <- "已点击 {count} 次")
            Button(id = increment, label = "+1") {
                on click => count += 1
            }
        }
    }
}
"#;

fn main() {
    let config = AppConfig::new(COUNTER, "nui — counter", Size::new(420.0, 300.0));
    Application::new(config).run();
}
