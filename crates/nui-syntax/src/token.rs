//! Token model produced by the lexer.

use std::fmt;

use crate::span::Span;

/// Numeric unit suffixes attached directly to number literals.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NumberUnit {
    /// No unit: a plain number (`42`, `0.5`).
    None,
    /// `dp`: density-independent pixels (`420dp`).
    Dp,
    /// `%`: percent of available space (`50%`).
    Percent,
    /// `ms`: milliseconds (`200ms`).
    Millis,
}

/// Keywords of nui-lang.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Keyword {
    /// `component`
    Component,
    /// `property`
    Property,
    /// `signal`
    Signal,
    /// `machine`
    Machine,
    /// `state`
    State,
    /// `on`
    On,
    /// `when`
    When,
    /// `enter`
    Enter,
    /// `exit`
    Exit,
    /// `let`
    Let,
    /// `if`
    If,
    /// `else`
    Else,
    /// `emit`
    Emit,
    /// `in`
    In,
    /// `true`
    True,
    /// `false`
    False,
    /// `auto`
    Auto,
}

impl Keyword {
    /// Maps an identifier's text to its keyword, if it is one.
    pub fn from_ident(text: &str) -> Option<Keyword> {
        return match text {
            "component" => Some(Keyword::Component),
            "property" => Some(Keyword::Property),
            "signal" => Some(Keyword::Signal),
            "machine" => Some(Keyword::Machine),
            "state" => Some(Keyword::State),
            "on" => Some(Keyword::On),
            "when" => Some(Keyword::When),
            "enter" => Some(Keyword::Enter),
            "exit" => Some(Keyword::Exit),
            "let" => Some(Keyword::Let),
            "if" => Some(Keyword::If),
            "else" => Some(Keyword::Else),
            "emit" => Some(Keyword::Emit),
            "in" => Some(Keyword::In),
            "true" => Some(Keyword::True),
            "false" => Some(Keyword::False),
            "auto" => Some(Keyword::Auto),
            _ => None,
        };
    }
}

impl fmt::Display for Keyword {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        return write!(formatter, "{}", self.as_str());
    }
}

impl Keyword {
    /// The keyword's source text.
    pub fn as_str(&self) -> &'static str {
        return match self {
            Keyword::Component => "component",
            Keyword::Property => "property",
            Keyword::Signal => "signal",
            Keyword::Machine => "machine",
            Keyword::State => "state",
            Keyword::On => "on",
            Keyword::When => "when",
            Keyword::Enter => "enter",
            Keyword::Exit => "exit",
            Keyword::Let => "let",
            Keyword::If => "if",
            Keyword::Else => "else",
            Keyword::Emit => "emit",
            Keyword::In => "in",
            Keyword::True => "true",
            Keyword::False => "false",
            Keyword::Auto => "auto",
        };
    }
}

/// Punctuation and operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Punct {
    /// `(`
    LParen,
    /// `)`
    RParen,
    /// `{`
    LBrace,
    /// `}`
    RBrace,
    /// `,`
    Comma,
    /// `:`
    Colon,
    /// `.`
    Dot,
    /// `;`
    Semi,
    /// `=` (static assignment)
    Assign,
    /// `<-` (reactive binding)
    Bind,
    /// `<=>` (two-way binding)
    TwoWay,
    /// `=>` (handler arrow)
    Arrow,
    /// `+`
    Plus,
    /// `-`
    Minus,
    /// `*`
    Star,
    /// `/`
    Slash,
    /// `%` (modulo operator; the percent unit is part of number tokens)
    Percent,
    /// `+=`
    PlusEq,
    /// `-=`
    MinusEq,
    /// `==`
    EqEq,
    /// `!=`
    NotEq,
    /// `!`
    Not,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `&&`
    AndAnd,
    /// `||`
    OrOr,
    /// `?`
    Question,
}

impl fmt::Display for Punct {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        return write!(formatter, "{}", self.as_str());
    }
}

impl Punct {
    /// The punctuation's source text.
    pub fn as_str(&self) -> &'static str {
        return match self {
            Punct::LParen => "(",
            Punct::RParen => ")",
            Punct::LBrace => "{",
            Punct::RBrace => "}",
            Punct::Comma => ",",
            Punct::Colon => ":",
            Punct::Dot => ".",
            Punct::Semi => ";",
            Punct::Assign => "=",
            Punct::Bind => "<-",
            Punct::TwoWay => "<=>",
            Punct::Arrow => "=>",
            Punct::Plus => "+",
            Punct::Minus => "-",
            Punct::Star => "*",
            Punct::Slash => "/",
            Punct::Percent => "%",
            Punct::PlusEq => "+=",
            Punct::MinusEq => "-=",
            Punct::EqEq => "==",
            Punct::NotEq => "!=",
            Punct::Not => "!",
            Punct::Lt => "<",
            Punct::Le => "<=",
            Punct::Gt => ">",
            Punct::Ge => ">=",
            Punct::AndAnd => "&&",
            Punct::OrOr => "||",
            Punct::Question => "?",
        };
    }
}

/// Token payloads.
#[derive(Debug, Clone, PartialEq)]
pub enum TokenKind {
    /// Integer literal with an optional unit (`42`, `420dp`, `50%`, `200ms`).
    Int {
        /// Literal value.
        value: i64,
        /// Attached unit.
        unit: NumberUnit,
    },
    /// Float literal with an optional unit (`0.5`, `0.5dp`).
    Float {
        /// Literal value.
        value: f64,
        /// Attached unit.
        unit: NumberUnit,
    },
    /// Color literal: the raw hex digits after `#` (validated by the
    /// compiler against the supported lengths).
    Color(String),
    /// String literal with escapes processed; `{...}` interpolation holes
    /// are kept in the raw text and expanded by the parser.
    Str(String),
    /// Identifier, possibly kebab-cased (`ease-out`). Identifiers merge an
    /// inline `-` when surrounded by identifier characters, so binary minus
    /// must be written with surrounding whitespace (`a - b`, never `a-b`).
    Ident(String),
    /// Keyword.
    Keyword(Keyword),
    /// Punctuation or operator.
    Punct(Punct),
    /// End of file.
    Eof,
}

impl fmt::Display for TokenKind {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        return match self {
            TokenKind::Int { .. } => write!(formatter, "an integer literal"),
            TokenKind::Float { .. } => write!(formatter, "a float literal"),
            TokenKind::Color(_) => write!(formatter, "a color literal"),
            TokenKind::Str(value) => write!(formatter, "string literal `{value}`"),
            TokenKind::Ident(name) => write!(formatter, "identifier `{name}`"),
            TokenKind::Keyword(keyword) => write!(formatter, "keyword `{keyword}`"),
            TokenKind::Punct(punct) => write!(formatter, "`{punct}`"),
            TokenKind::Eof => write!(formatter, "end of file"),
        };
    }
}

/// A token with its source span.
#[derive(Debug, Clone, PartialEq)]
pub struct Token {
    /// Token payload.
    pub kind: TokenKind,
    /// Source span of the token.
    pub span: Span,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn keywords_roundtrip_through_text() {
        for keyword in [
            Keyword::Component,
            Keyword::Property,
            Keyword::Machine,
            Keyword::Auto,
        ] {
            assert_eq!(Keyword::from_ident(keyword.as_str()), Some(keyword));
        }
        assert_eq!(Keyword::from_ident("count"), None);
    }

    #[test]
    fn punctuation_displays_as_source_text() {
        assert_eq!(Punct::Bind.to_string(), "<-");
        assert_eq!(Punct::TwoWay.to_string(), "<=>");
        assert_eq!(Punct::Arrow.to_string(), "=>");
    }
}
