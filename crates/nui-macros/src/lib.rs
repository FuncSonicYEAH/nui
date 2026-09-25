//! nui proc macros: [`include_ui!`] — the release-mode fallback for
//! shipping `.nui` documents (plan §8, D1: development loads at runtime,
//! release embeds).
//!
//! `include_ui!("ui/app.nui")` runs nui-compiler **at expansion time**: a
//! document with diagnostics fails the host build with a rendered
//! rustc-style error, and a clean document is embedded as a `&'static str`
//! (via `include_str!`, so cargo rebuilds the host crate when the file
//! changes). The returned source is fed to the runtime exactly like an
//! inline string.
//!
//! Bundle serialization (embedding the compiled Document IR instead of the
//! source) stays release-optional future work per plan §3.4.

use std::path::{Path, PathBuf};

use proc_macro::{TokenStream, TokenTree};

/// Embeds a `.nui` document with compile-time validation. The argument is
/// a path relative to the invoking crate's manifest directory (like
/// `include_str!`).
///
/// ```
/// const SOURCE: &str = nui_macros::include_ui!("tests/fixtures/valid.nui");
/// assert!(SOURCE.contains("component"));
/// ```
#[proc_macro]
pub fn include_ui(input: TokenStream) -> TokenStream {
    let path = match parse_path_argument(input) {
        Ok(path) => path,
        Err(message) => return compile_error(&message),
    };
    let resolved = resolve_manifest_path(&path);
    let display = resolved.display().to_string();
    let Ok(source) = std::fs::read_to_string(&resolved) else {
        return compile_error(&format!("include_ui!: cannot read `{display}`"));
    };
    if let Err(rendered) = nui_compiler::validate_source(&source, &display) {
        return compile_error(&format!(
            "include_ui!: `{display}` does not compile\n{rendered}"
        ));
    }
    // include_str! keeps the file in cargo's rebuild graph. The generated
    // code embeds the resolved absolute path (include_str! anchors at the
    // *containing file*, not the crate root, so the original relative
    // literal would resolve elsewhere).
    let resolved_literal = proc_macro::Literal::string(&display);
    return format!(
        "{{ const NUI_SOURCE: &str = include_str!({}); NUI_SOURCE }}",
        resolved_literal
    )
    .parse()
    .expect("invariant: a token stream of `include_str!(<literal>)` always parses");
}

/// Parses the `.nui` path argument into its unescaped value (for reading
/// the file at expansion time).
fn parse_path_argument(input: TokenStream) -> Result<String, String> {
    let tokens: Vec<TokenTree> = input.into_iter().collect();
    let [TokenTree::Literal(literal)] = tokens.as_slice() else {
        return Err("include_ui! expects a single string literal path".to_string());
    };
    let text = literal.to_string();
    let Some(value) = unescape_string_literal(&text) else {
        return Err(format!("include_ui!: unsupported string literal `{text}`"));
    };
    return Ok(value);
}

/// Resolves the path the same way `include_str!` in the generated code
/// will: relative paths anchor at the invoking crate's manifest directory.
fn resolve_manifest_path(path: &str) -> PathBuf {
    let candidate = Path::new(path);
    if candidate.is_absolute() {
        return candidate.to_path_buf();
    }
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_default();
    return Path::new(&manifest_dir).join(candidate);
}

/// Unescapes a `"..."` literal (the stable proc-macro API exposes no value
/// accessor without `syn`); returns `None` for raw strings, byte strings,
/// or unknown escapes.
fn unescape_string_literal(literal: &str) -> Option<String> {
    let inner = literal.strip_prefix('"')?.strip_suffix('"')?;
    let mut value = String::with_capacity(inner.len());
    let mut characters = inner.chars();
    while let Some(character) = characters.next() {
        if character != '\\' {
            value.push(character);
            continue;
        }
        match characters.next()? {
            'n' => value.push('\n'),
            't' => value.push('\t'),
            'r' => value.push('\r'),
            '0' => value.push('\0'),
            '\\' => value.push('\\'),
            '"' => value.push('"'),
            _ => return None,
        }
    }
    return Some(value);
}

fn compile_error(message: &str) -> TokenStream {
    let literal = proc_macro::Literal::string(message);
    return format!("::core::compile_error!({literal})")
        .parse()
        .expect("invariant: `compile_error!(<string literal>)` always parses");
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn unescapes_common_escapes_and_rejects_others() {
        assert_eq!(
            unescape_string_literal(r#""ui/app.nui""#).unwrap(),
            "ui/app.nui"
        );
        assert_eq!(unescape_string_literal(r#""a\tb""#).unwrap(), "a\tb");
        assert!(unescape_string_literal("r\"raw\"").is_none());
        assert!(unescape_string_literal(r#""a\qb""#).is_none());
    }
}
