//! `AppConfig` defaults and builder semantics (M11 window attributes).

use nui::{AppConfig, Application};
use nui_core::Size;

#[test]
fn config_defaults_to_resizable() {
    let config = AppConfig::new(
        "component A { Window(id = root) {} }",
        "a",
        Size::new(420.0, 300.0),
    );
    assert!(config.resizable);
}

#[test]
fn builder_can_fix_the_window_size() {
    let config = AppConfig::new(
        "component A { Window(id = root) {} }",
        "a",
        Size::new(420.0, 300.0),
    )
    .with_resizable(false);
    assert!(!config.resizable);
}

#[test]
fn builder_can_reenable_resizing() {
    let config = AppConfig::new(
        "component A { Window(id = root) {} }",
        "a",
        Size::new(420.0, 300.0),
    )
    .with_resizable(false)
    .with_resizable(true);
    assert!(config.resizable);
}

#[test]
fn builder_keeps_other_fields() {
    let source = "component A { Window(id = root) {} }";
    let config = AppConfig::new(source, "title", Size::new(42.0, 30.0)).with_resizable(false);
    assert_eq!(config.source, source);
    assert_eq!(config.title, "title");
    assert_eq!(config.size.width, 42.0);
    assert_eq!(config.size.height, 30.0);
}

/// Compile-level smoke: the builder chain type-checks end to end and
/// `Application` still accepts the configured value (no window opened).
#[test]
fn application_accepts_a_fixed_size_config() {
    let config = AppConfig::new(
        "component A { Window(id = root) {} }",
        "a",
        Size::new(420.0, 300.0),
    )
    .with_resizable(false);
    // `Application::new` only stores the config; dropping it is fine.
    let _app = Application::new(config);
}
