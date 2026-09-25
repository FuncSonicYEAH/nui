//! Login form demo (M7 acceptance): `TextInput` with two-way bindings,
//! Tab focus cycling, keyboard editing, IME commits, and the `accepted`
//! signal on Enter.
//!
//! Run: `cargo run -p nui --example form`
//! (requires a display; headless environments print an error and exit).

use nui::{AppConfig, Application};
use nui_core::Size;

const LOGIN: &str = r#"
component Login {
    property userName: String = ""
    property password: String = ""
    property submitted: Bool = false

    Window(id = root) {
        Column(id = content, spacing = 12dp, padding = 36dp) {
            Rectangle(id = panel, fill = #1d222b, radius = 10dp, width = 360dp, height = 240dp)
            Text(id = title, content <- submitted ? "Signed in as {userName}" : "Welcome back",
                 font.size = 20dp, color = #e8ecf2)
            TextInput(id = user, width = 320dp, height = 40dp,
                      placeholder = "username", text <=> root.userName) {
                on accepted => submitted = true
            }
            TextInput(id = pass, width = 320dp, height = 40dp,
                      placeholder = "password", text <=> root.password) {
                on accepted => submitted = true
            }
            Button(id = go, label = "Sign in", width = 320dp, height = 44dp) {
                on click => submitted = true
            }
        }

        when submitted {
            panel.opacity = 0.55
        }
    }
}
"#;

fn main() {
    let config = AppConfig::new(LOGIN, "nui — login form", Size::new(420.0, 360.0));
    Application::new(config).run();
}
