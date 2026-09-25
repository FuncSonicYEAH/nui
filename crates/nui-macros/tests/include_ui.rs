//! `include_ui!` integration: the macro compiles the fixture at expansion
//! time (a broken fixture would fail this test crate to build) and yields
//! the source for the runtime.
#![allow(clippy::unwrap_used)]

const SOURCE: &str = nui_macros::include_ui!("tests/fixtures/valid.nui");

#[test]
fn embeds_the_document_source() {
    assert!(SOURCE.contains("component Embedded"));
}

#[test]
fn embedded_source_compiles_cleanly_at_runtime() {
    let outcome = nui_compiler::compile(SOURCE);
    assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
}
