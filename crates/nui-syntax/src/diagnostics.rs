//! Compile diagnostics and a rustc-style renderer.

use crate::span::Span;

/// Diagnostic severity.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    /// Compilation error: the document is rejected.
    Error,
    /// Warning: the document compiles, but something looks suspicious.
    Warning,
}

impl Severity {
    /// Lower-case label used by the renderer.
    pub fn as_str(self) -> &'static str {
        return match self {
            Severity::Error => "error",
            Severity::Warning => "warning",
        };
    }
}

/// A single diagnostic anchored to a source span.
#[derive(Debug, Clone)]
pub struct Diagnostic {
    /// Severity.
    pub severity: Severity,
    /// Span the diagnostic points at.
    pub span: Span,
    /// Primary message.
    pub message: String,
    /// Secondary notes (rendered as `note: ...` lines).
    pub notes: Vec<String>,
}

impl Diagnostic {
    /// Creates an error diagnostic.
    pub fn error(span: Span, message: impl Into<String>) -> Diagnostic {
        return Diagnostic {
            severity: Severity::Error,
            span,
            message: message.into(),
            notes: Vec::new(),
        };
    }

    /// Creates a warning diagnostic.
    pub fn warning(span: Span, message: impl Into<String>) -> Diagnostic {
        return Diagnostic {
            severity: Severity::Warning,
            span,
            message: message.into(),
            notes: Vec::new(),
        };
    }

    /// Attaches a note to the diagnostic.
    pub fn with_note(mut self, note: impl Into<String>) -> Diagnostic {
        self.notes.push(note.into());
        return self;
    }
}

/// Renders a diagnostic in rustc style against the source text:
///
/// ```text
/// error: expected an identifier, found `}`
///   --> counter.nui:4:17
///    |
///  4 |     Button(label = ) {
///    |                 ^
/// ```
pub fn render_diagnostic(source: &str, path: &str, diagnostic: &Diagnostic) -> String {
    let span_start = diagnostic.span.start as usize;
    let span_end = diagnostic.span.end.max(diagnostic.span.start) as usize;
    let line_index = source[..span_start.min(source.len())].matches('\n').count();
    let line_start = source[..span_start.min(source.len())]
        .rfind('\n')
        .map(|index| return index + 1)
        .unwrap_or(0);
    let line_end = source[line_start..]
        .find('\n')
        .map(|offset| return line_start + offset)
        .unwrap_or(source.len());
    let line_text = source[line_start..line_end].trim_end_matches('\r');
    let column = source[line_start..span_start.min(source.len())]
        .chars()
        .count()
        + 1;
    let caret_start = column - 1;
    let caret_width_in_chars = source[span_start.min(line_end)..span_end.min(line_end)]
        .chars()
        .count()
        .max(1);
    let line_number = line_index + 1;
    let gutter_width = line_number.to_string().len().max(2);

    let mut rendered = String::new();
    rendered.push_str(&format!(
        "{}: {}\n",
        diagnostic.severity.as_str(),
        diagnostic.message
    ));
    rendered.push_str(&format!(
        "{:>width$}--> {}:{}:{}\n",
        "",
        path,
        line_number,
        column,
        width = gutter_width
    ));
    rendered.push_str(&format!("{:>width$}|\n", "", width = gutter_width));
    rendered.push_str(&format!(
        "{:>width$} | {}\n",
        line_number,
        line_text,
        width = gutter_width
    ));
    rendered.push_str(&format!(
        "{:>width$} | {}{}\n",
        "",
        " ".repeat(caret_start),
        "^".repeat(caret_width_in_chars),
        width = gutter_width
    ));
    for note in &diagnostic.notes {
        rendered.push_str(&format!("note: {note}\n"));
    }
    return rendered;
}

#[cfg(test)]
mod tests {
    use super::*;

    const SOURCE: &str = "component A {\n    property x: Int =\n}\n";

    #[test]
    fn render_shows_location_and_caret() {
        // Span covers the `}` on line 3 (offset 36..37).
        let diagnostic = Diagnostic::error(Span::new(36, 37), "expected an expression");
        let rendered = render_diagnostic(SOURCE, "a.nui", &diagnostic);
        assert!(rendered.contains("error: expected an expression"));
        assert!(rendered.contains("a.nui:3:1"));
        assert!(rendered.contains("^"));
        assert!(rendered.contains("}"));
    }

    #[test]
    fn render_shows_wide_caret_for_multi_char_span() {
        let diagnostic = Diagnostic::error(Span::new(4, 13), "bad component name");
        let rendered = render_diagnostic(SOURCE, "a.nui", &diagnostic);
        assert!(rendered.contains("a.nui:1:5"));
        assert!(rendered.contains("^^^^^^^^^"));
    }

    #[test]
    fn render_appends_notes() {
        let diagnostic = Diagnostic::error(Span::new(0, 1), "msg").with_note("try adding a type");
        let rendered = render_diagnostic(SOURCE, "a.nui", &diagnostic);
        assert!(rendered.contains("note: try adding a type"));
    }
}
