//! Lexer: source text to tokens.

use crate::diagnostics::Diagnostic;
use crate::span::Span;
use crate::token::{Keyword, NumberUnit, Punct, Token, TokenKind};

/// Outcome of lexing: the token stream plus diagnostics.
///
/// Lexing always produces a full token stream (terminated by `Eof`); errors
/// are recorded and lexing continues, so the parser can report further
/// problems in one pass.
pub struct LexOutcome {
    /// Tokens, terminated by a single `Eof` token.
    pub tokens: Vec<Token>,
    /// Diagnostics collected while lexing.
    pub diagnostics: Vec<Diagnostic>,
}

/// Lexes nui-lang source text into a token stream.
pub fn lex(source: &str) -> LexOutcome {
    let lexer = Lexer {
        source,
        pos: 0,
        tokens: Vec::new(),
        diagnostics: Vec::new(),
    };
    return lexer.run();
}

struct Lexer<'source> {
    source: &'source str,
    pos: usize,
    tokens: Vec<Token>,
    diagnostics: Vec<Diagnostic>,
}

impl<'source> Lexer<'source> {
    fn run(mut self) -> LexOutcome {
        loop {
            self.skip_trivia();
            let start = self.pos;
            let Some(ch) = self.advance() else {
                break;
            };
            match ch {
                '(' => self.push_punct(Punct::LParen, start),
                ')' => self.push_punct(Punct::RParen, start),
                '{' => self.push_punct(Punct::LBrace, start),
                '}' => self.push_punct(Punct::RBrace, start),
                ',' => self.push_punct(Punct::Comma, start),
                ':' => self.push_punct(Punct::Colon, start),
                '.' => self.push_punct(Punct::Dot, start),
                ';' => self.push_punct(Punct::Semi, start),
                '*' => self.push_punct(Punct::Star, start),
                '/' => self.push_punct(Punct::Slash, start),
                '%' => self.push_punct(Punct::Percent, start),
                '?' => self.push_punct(Punct::Question, start),
                '+' => {
                    if self.advance_if('=') {
                        self.push_punct(Punct::PlusEq, start);
                    } else {
                        self.push_punct(Punct::Plus, start);
                    }
                }
                '-' => {
                    if self.advance_if('=') {
                        self.push_punct(Punct::MinusEq, start);
                    } else {
                        self.push_punct(Punct::Minus, start);
                    }
                }
                '=' => {
                    if self.advance_if('=') {
                        self.push_punct(Punct::EqEq, start);
                    } else if self.advance_if('>') {
                        self.push_punct(Punct::Arrow, start);
                    } else {
                        self.push_punct(Punct::Assign, start);
                    }
                }
                '!' => {
                    if self.advance_if('=') {
                        self.push_punct(Punct::NotEq, start);
                    } else {
                        self.push_punct(Punct::Not, start);
                    }
                }
                '>' => {
                    if self.advance_if('=') {
                        self.push_punct(Punct::Ge, start);
                    } else {
                        self.push_punct(Punct::Gt, start);
                    }
                }
                '<' => {
                    if self.current() == Some('=') && self.peek_second() == Some('>') {
                        self.advance();
                        self.advance();
                        self.push_punct(Punct::TwoWay, start);
                    } else if self.advance_if('=') {
                        self.push_punct(Punct::Le, start);
                    } else if self.advance_if('-') {
                        self.push_punct(Punct::Bind, start);
                    } else {
                        self.push_punct(Punct::Lt, start);
                    }
                }
                '&' => {
                    if self.advance_if('&') {
                        self.push_punct(Punct::AndAnd, start);
                    } else {
                        self.error(start, "unexpected `&` (did you mean `&&`?)");
                    }
                }
                '|' => {
                    if self.advance_if('|') {
                        self.push_punct(Punct::OrOr, start);
                    } else {
                        self.error(start, "unexpected `|` (did you mean `||`?)");
                    }
                }
                '"' => self.lex_string(start),
                '#' => self.lex_color(start),
                '0'..='9' => self.lex_number(ch, start),
                first_ident if is_ident_start(first_ident) => self.lex_ident(first_ident, start),
                other => {
                    self.error(start, &format!("unexpected character `{other}`"));
                }
            }
        }
        let end = self.pos as u32;
        self.tokens.push(Token {
            kind: TokenKind::Eof,
            span: Span::new(end, end),
        });
        return LexOutcome {
            tokens: self.tokens,
            diagnostics: self.diagnostics,
        };
    }

    fn current(&self) -> Option<char> {
        return self.source[self.pos..].chars().next();
    }

    fn peek_second(&self) -> Option<char> {
        let mut chars = self.source[self.pos..].chars();
        chars.next()?;
        return chars.next();
    }

    fn advance(&mut self) -> Option<char> {
        let ch = self.current()?;
        self.pos += ch.len_utf8();
        return Some(ch);
    }

    fn advance_if(&mut self, expected: char) -> bool {
        if self.current() == Some(expected) {
            self.pos += expected.len_utf8();
            return true;
        }
        return false;
    }

    fn push_punct(&mut self, punct: Punct, start: usize) {
        self.push(TokenKind::Punct(punct), start);
    }

    fn push(&mut self, kind: TokenKind, start: usize) {
        self.tokens.push(Token {
            kind,
            span: Span::new(start as u32, self.pos as u32),
        });
    }

    fn error(&mut self, start: usize, message: &str) {
        self.diagnostics.push(Diagnostic::error(
            Span::new(start as u32, self.pos as u32),
            message,
        ));
    }

    /// Skips whitespace and comments (`//` line, `/* */` block).
    fn skip_trivia(&mut self) {
        loop {
            match self.current() {
                Some(ch) if ch.is_whitespace() => {
                    self.advance();
                    continue;
                }
                Some('/') if self.peek_second() == Some('/') => {
                    while self.current().is_some_and(|ch| return ch != '\n') {
                        self.advance();
                    }
                    continue;
                }
                Some('/') if self.peek_second() == Some('*') => {
                    let start = self.pos;
                    self.advance();
                    self.advance();
                    let mut closed = false;
                    while let Some(ch) = self.advance() {
                        if ch == '*' && self.advance_if('/') {
                            closed = true;
                            break;
                        }
                    }
                    if !closed {
                        self.error(start, "unterminated block comment");
                    }
                    continue;
                }
                _ => return,
            }
        }
    }

    fn lex_number(&mut self, first: char, start: usize) {
        let mut text = String::new();
        text.push(first);
        while let Some(digit) = self.advance_if_digit() {
            text.push(digit);
        }
        let mut is_float = false;
        if self.current() == Some('.')
            && self
                .peek_second()
                .is_some_and(|c| return c.is_ascii_digit())
        {
            is_float = true;
            self.advance();
            text.push('.');
            while let Some(digit) = self.advance_if_digit() {
                text.push(digit);
            }
        }
        let unit = self.lex_number_unit();
        if is_float {
            let Ok(value) = text.parse::<f64>() else {
                self.error(start, &format!("malformed float literal `{text}`"));
                return;
            };
            self.push(TokenKind::Float { value, unit }, start);
        } else {
            let Ok(value) = text.parse::<i64>() else {
                self.error(start, &format!("integer literal `{text}` is out of range"));
                return;
            };
            self.push(TokenKind::Int { value, unit }, start);
        }
    }

    fn advance_if_digit(&mut self) -> Option<char> {
        let digit = self.current()?;
        if !digit.is_ascii_digit() {
            return None;
        }
        self.pos += 1;
        return Some(digit);
    }

    fn lex_number_unit(&mut self) -> NumberUnit {
        if self.advance_if('%') {
            return NumberUnit::Percent;
        }
        if !self
            .current()
            .is_some_and(|ch| return ch.is_ascii_alphabetic())
        {
            return NumberUnit::None;
        }
        let suffix_start = self.pos;
        while self
            .current()
            .is_some_and(|ch| return ch.is_ascii_alphabetic())
        {
            self.advance();
        }
        let suffix = &self.source[suffix_start..self.pos];
        return match suffix {
            "dp" => NumberUnit::Dp,
            "ms" => NumberUnit::Millis,
            other => {
                self.error(
                    suffix_start,
                    &format!("unknown number unit `{other}` (expected `dp` or `ms`)"),
                );
                NumberUnit::None
            }
        };
    }

    fn lex_color(&mut self, start: usize) {
        while self
            .current()
            .is_some_and(|ch| return ch.is_ascii_hexdigit())
        {
            self.advance();
        }
        if self.pos == start + 1 {
            self.error(start, "expected hex digits after `#`");
            return;
        }
        let digits = self.source[start + 1..self.pos].to_string();
        self.push(TokenKind::Color(digits), start);
    }

    fn lex_string(&mut self, start: usize) {
        let mut value = String::new();
        loop {
            match self.advance() {
                None => {
                    self.error(start, "unterminated string literal");
                    break;
                }
                Some('"') => break,
                Some('\n') => {
                    self.error(start, "newline in string literal (write `\\n` instead)");
                    break;
                }
                Some('\\') => {
                    let escape_start = self.pos - 1;
                    match self.advance() {
                        Some('n') => value.push('\n'),
                        Some('t') => value.push('\t'),
                        Some('r') => value.push('\r'),
                        Some('"') => value.push('"'),
                        Some('\\') => value.push('\\'),
                        Some(other) => {
                            self.diagnostics.push(Diagnostic::error(
                                Span::new(escape_start as u32, self.pos as u32),
                                format!("unknown escape `\\{other}`"),
                            ));
                        }
                        None => {
                            self.error(start, "unterminated string literal");
                            break;
                        }
                    }
                }
                Some(ch) => value.push(ch),
            }
        }
        self.push(TokenKind::Str(value), start);
    }

    fn lex_ident(&mut self, first: char, start: usize) {
        let mut text = String::new();
        text.push(first);
        while let Some(current) = self.current() {
            if current.is_ascii_alphanumeric() || current == '_' {
                text.push(current);
                self.advance();
                continue;
            }
            // Kebab merge: an inline `-` followed directly by an identifier
            // character joins the identifier (`ease-out`). Consequence:
            // binary minus must be surrounded by whitespace (`a - b`).
            if current == '-'
                && let Some(after) = self.peek_second()
                && (after.is_ascii_alphanumeric() || after == '_')
            {
                text.push('-');
                self.advance();
                continue;
            }
            break;
        }
        let kind = match Keyword::from_ident(&text) {
            Some(keyword) => TokenKind::Keyword(keyword),
            None => TokenKind::Ident(text),
        };
        self.push(kind, start);
    }
}

/// Whether `ch` may start an identifier.
fn is_ident_start(ch: char) -> bool {
    return ch.is_ascii_alphabetic() || ch == '_';
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn kinds(source: &str) -> Vec<TokenKind> {
        return lex(source)
            .tokens
            .into_iter()
            .map(|t| return t.kind)
            .collect();
    }

    #[test]
    fn lexes_units_and_colors() {
        let kinds = kinds("420dp 50% 200ms 0.5 #336699 auto");
        assert_eq!(
            kinds,
            vec![
                TokenKind::Int {
                    value: 420,
                    unit: NumberUnit::Dp
                },
                TokenKind::Int {
                    value: 50,
                    unit: NumberUnit::Percent
                },
                TokenKind::Int {
                    value: 200,
                    unit: NumberUnit::Millis
                },
                TokenKind::Float {
                    value: 0.5,
                    unit: NumberUnit::None
                },
                TokenKind::Color("336699".to_string()),
                TokenKind::Keyword(Keyword::Auto),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_binding_operators() {
        let kinds = kinds("a <- b <=> c <= d < e");
        assert_eq!(
            kinds,
            vec![
                TokenKind::Ident("a".to_string()),
                TokenKind::Punct(Punct::Bind),
                TokenKind::Ident("b".to_string()),
                TokenKind::Punct(Punct::TwoWay),
                TokenKind::Ident("c".to_string()),
                TokenKind::Punct(Punct::Le),
                TokenKind::Ident("d".to_string()),
                TokenKind::Punct(Punct::Lt),
                TokenKind::Ident("e".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn merges_kebab_idents_but_not_spaced_minus() {
        let kinds = kinds("ease-out a - b a-1");
        assert_eq!(
            kinds,
            vec![
                TokenKind::Ident("ease-out".to_string()),
                TokenKind::Ident("a".to_string()),
                TokenKind::Punct(Punct::Minus),
                TokenKind::Ident("b".to_string()),
                TokenKind::Ident("a-1".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn lexes_strings_with_escapes() {
        let kinds = kinds(r#""a\nb\"c{}""#);
        assert_eq!(
            kinds,
            vec![TokenKind::Str("a\nb\"c{}".to_string()), TokenKind::Eof,]
        );
    }

    #[test]
    fn skips_comments_and_whitespace() {
        let kinds = kinds("a // line\n /* block */ b");
        assert_eq!(
            kinds,
            vec![
                TokenKind::Ident("a".to_string()),
                TokenKind::Ident("b".to_string()),
                TokenKind::Eof,
            ]
        );
    }

    #[test]
    fn reports_unterminated_string() {
        let outcome = lex("\"abc");
        assert_eq!(outcome.diagnostics.len(), 1);
        assert!(outcome.diagnostics[0].message.contains("unterminated"));
    }

    #[test]
    fn reports_unknown_number_unit() {
        let outcome = lex("10px");
        assert_eq!(outcome.diagnostics.len(), 1);
        assert!(
            outcome.diagnostics[0]
                .message
                .contains("unknown number unit")
        );
    }

    #[test]
    fn arrow_is_not_assignment() {
        let kinds = kinds("=> = == +=");
        assert_eq!(
            kinds,
            vec![
                TokenKind::Punct(Punct::Arrow),
                TokenKind::Punct(Punct::Assign),
                TokenKind::Punct(Punct::EqEq),
                TokenKind::Punct(Punct::PlusEq),
                TokenKind::Eof,
            ]
        );
    }
}
