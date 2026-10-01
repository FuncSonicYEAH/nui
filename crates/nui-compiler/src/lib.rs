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
pub use check::{CheckOutcome, check, check_with, check_with_host, check_with_vocabulary};
pub use document::{
    AssignmentIr, ComponentIr, DocumentIr, ForIr, FunctionIr, HandlerIr, InitKind, MachineIr,
    NodeIr, ParameterIr, PropertyDefaultIr, PropertyIr, StateIr, TransitionIr, WhenIr,
    assign_op_name,
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
    let host = HostVocabulary::new().with_functions(extern_functions.iter().cloned());
    return compile_with_host(source, &host);
}

/// What a host can offer a document: the names it may call, and which of
/// them are commands.
///
/// One type rather than two parallel parameters because the two lists
/// belong together — a command *is* one of the callable names — and because
/// every future addition to the host's surface (types, properties) belongs
/// in the same place.
#[derive(Debug, Clone, Default)]
pub struct HostVocabulary {
    /// Every name callable from a document, in any position.
    pub functions: Vec<String>,
    /// The subset of [`Self::functions`] that acts instead of returning:
    /// legal as a statement, an error in a value position.
    pub commands: Vec<String>,
    /// The names of types the host registered with `register_component`.
    ///
    /// A document may name one of these as an `extends` parent. The host
    /// supplies the *name* so the checker can accept the clause; the
    /// inherited members come from the host's own descriptor, which the
    /// runtime already holds.
    ///
    /// Unlike `functions` / `commands`, an empty list here does **not**
    /// mean "nothing is allowed": the built-in element types are always
    /// available (see [`nui_core::props::is_builtin_type`]). This slot only
    /// adds names the compiler does not know on its own.
    pub components: Vec<String>,
}

impl HostVocabulary {
    /// A vocabulary that knows no host names at all.
    pub fn new() -> HostVocabulary {
        return HostVocabulary::default();
    }

    /// Builder: the names a document may call.
    pub fn with_functions(mut self, functions: impl IntoIterator<Item = String>) -> HostVocabulary {
        self.functions.extend(functions);
        return self;
    }

    /// Builder: declares commands, which are also callable names — a
    /// command is registered once, not in both lists.
    pub fn with_commands(mut self, commands: impl IntoIterator<Item = String>) -> HostVocabulary {
        for command in commands {
            self.functions.push(command.clone());
            self.commands.push(command);
        }
        return self;
    }

    /// Builder: the type names a document may `extends`.
    pub fn with_components(
        mut self,
        components: impl IntoIterator<Item = String>,
    ) -> HostVocabulary {
        self.components.extend(components);
        return self;
    }
}

/// Compiles nui-lang source text against a host vocabulary; see
/// [`HostVocabulary`].
pub fn compile_with_host(source: &str, host: &HostVocabulary) -> CompileOutcome {
    let nui_syntax::ParseOutcome {
        document,
        mut diagnostics,
    } = nui_syntax::parse(source);
    let outcome =
        check::check_with_vocabulary(&document, &host.functions, &host.commands, &host.components);
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
