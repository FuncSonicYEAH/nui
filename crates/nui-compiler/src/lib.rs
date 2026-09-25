//! nui-compiler: type checker and Document IR builder for nui-lang.
//!
//! Pipeline stage after nui-syntax: [`compile`] takes source text and
//! returns the checked [`DocumentIr`] plus all diagnostics (lexer, parser,
//! and checker merged, sorted by source position). Compilation is total —
//! it always returns a best-effort document, so tooling can report every
//! problem in one pass.
//!
//! # Examples
//! ```
//! let outcome = nui_compiler::compile("component A { property n: Int = 0 }");
//! assert!(outcome.diagnostics.is_empty());
//! assert_eq!(outcome.document.components[0].name, "A");
//! ```

pub mod bytecode;
pub mod check;
pub mod document;
pub mod types;

pub use bytecode::{AssignOp, Builtin, Effect, InterpPart, PropertyTarget, TypedExpr};
pub use check::{CheckOutcome, check, check_with};
pub use document::{
    AssignmentIr, ComponentIr, DocumentIr, ForIr, HandlerIr, InitKind, MachineIr, NodeIr,
    PropertyDefaultIr, PropertyIr, StateIr, TransitionIr, WhenIr, assign_op_name,
};
pub use types::{Type, unify};

use nui_syntax::Diagnostic;

/// Outcome of full compilation: Document IR plus merged diagnostics.
pub struct CompileOutcome {
    /// Compiled document (best effort in the presence of errors).
    pub document: DocumentIr,
    /// All diagnostics, sorted by source position.
    pub diagnostics: Vec<Diagnostic>,
}

/// Validates nui source and renders any diagnostics rustc-style; `Ok(())`
/// when the document is clean. The shared check behind `include_ui!`'s
/// compile-time validation and previewer tooling.
pub fn validate_source(source: &str, display_path: &str) -> Result<(), String> {
    let outcome = compile(source);
    if outcome.diagnostics.is_empty() {
        return Ok(());
    }
    let rendered = outcome
        .diagnostics
        .iter()
        .map(|diagnostic| return nui_syntax::render_diagnostic(source, display_path, diagnostic))
        .collect::<Vec<_>>()
        .join("\n");
    return Err(rendered);
}

/// Compiles nui-lang source text to Document IR.
pub fn compile(source: &str) -> CompileOutcome {
    return compile_with_functions(source, &[]);
}

/// Compiles nui-lang source text with a set of host-registered function
/// names: calls to those names are accepted and lower to
/// [`TypedExpr::HostCall`](bytecode::TypedExpr::HostCall) (plan §5 宿主
/// 互操作). Names not in the list still produce unknown-function
/// diagnostics, so typos stay compile-time errors.
pub fn compile_with_functions(source: &str, extern_functions: &[String]) -> CompileOutcome {
    let nui_syntax::ParseOutcome {
        document,
        mut diagnostics,
    } = nui_syntax::parse(source);
    let outcome = check_with(&document, extern_functions);
    diagnostics.extend(outcome.diagnostics);
    diagnostics.sort_by_key(|diagnostic| return diagnostic.span.start);
    return CompileOutcome {
        document: outcome.document,
        diagnostics,
    };
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod validate_tests {
    #[test]
    fn validate_source_passes_clean_documents() {
        assert!(crate::validate_source("component A {}", "a.nui").is_ok());
    }

    #[test]
    fn validate_source_renders_diagnostics_with_path() {
        let error = crate::validate_source("component A { property flag: Bool = 3 }", "ui/a.nui")
            .unwrap_err();
        assert!(error.contains("error:"), "{error}");
        assert!(error.contains("ui/a.nui"), "{error}");
    }
}
