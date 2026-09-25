//! Unified error type for nui-core.

use thiserror::Error;

/// Error type for nui-core.
///
/// Shared by every fallible operation in this crate; higher-level crates
/// define their own errors and wrap this type via `#[from]`.
#[derive(Debug, Error)]
pub enum Error {
    /// A color literal failed to parse (an invalid nui-lang form such as a
    /// malformed `#336699`).
    #[error("invalid color literal `{literal}`: {reason}")]
    InvalidColorLiteral {
        /// The original literal, echoed back in diagnostics.
        literal: String,
        /// Failure reason (a static description, independent of input).
        reason: &'static str,
    },

    /// A dynamic value was read as the wrong type.
    #[error("type mismatch: expected {expected}, found {found}")]
    TypeMismatch {
        /// The expected type name.
        expected: &'static str,
        /// The type actually held.
        found: &'static str,
    },
}

/// `Result` alias for nui-core.
pub type Result<T> = std::result::Result<T, Error>;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn display_includes_literal_and_reason() {
        let error = Error::InvalidColorLiteral {
            literal: "#xyz".to_string(),
            reason: "invalid hex digit",
        };
        let message = error.to_string();
        assert!(message.contains("#xyz"));
        assert!(message.contains("invalid hex digit"));
    }

    #[test]
    fn type_mismatch_names_both_sides() {
        let error = Error::TypeMismatch {
            expected: "Bool",
            found: "String",
        };
        let message = error.to_string();
        assert!(message.contains("Bool"));
        assert!(message.contains("String"));
    }
}
