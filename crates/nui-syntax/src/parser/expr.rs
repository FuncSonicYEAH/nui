//! Expression parsing: precedence climbing, string interpolation, call
//! arguments.

use nui_core::{Duration, Length};

use crate::ast::{BinaryOp, CallArg, Expr, StrPart, UnaryOp};
use crate::diagnostics::Diagnostic;
use crate::lexer::{LexOutcome, lex};
use crate::span::Span;
use crate::token::{Keyword, NumberUnit, Punct, TokenKind};

use super::Parser;

/// Binary operator precedence (higher binds tighter).
fn binary_precedence(kind: &TokenKind) -> Option<u8> {
    let TokenKind::Punct(punct) = kind else {
        return None;
    };
    return match punct {
        Punct::OrOr => Some(1),
        Punct::AndAnd => Some(2),
        Punct::EqEq | Punct::NotEq => Some(3),
        Punct::Lt | Punct::Le | Punct::Gt | Punct::Ge => Some(4),
        Punct::Plus | Punct::Minus => Some(5),
        Punct::Star | Punct::Slash | Punct::Percent => Some(6),
        _ => None,
    };
}

fn punct_to_binary(punct: Punct) -> BinaryOp {
    return match punct {
        Punct::Plus => BinaryOp::Add,
        Punct::Minus => BinaryOp::Sub,
        Punct::Star => BinaryOp::Mul,
        Punct::Slash => BinaryOp::Div,
        Punct::Percent => BinaryOp::Rem,
        Punct::EqEq => BinaryOp::Eq,
        Punct::NotEq => BinaryOp::NotEq,
        Punct::Lt => BinaryOp::Lt,
        Punct::Le => BinaryOp::Le,
        Punct::Gt => BinaryOp::Gt,
        Punct::Ge => BinaryOp::Ge,
        Punct::AndAnd => BinaryOp::And,
        Punct::OrOr => BinaryOp::Or,
        other => unreachable!("`{other}` is not a binary operator"),
    };
}

impl Parser {
    /// Parses a full expression (lowest precedence: ternary).
    pub(crate) fn parse_expression(&mut self) -> Expr {
        return self.parse_ternary();
    }

    fn parse_ternary(&mut self) -> Expr {
        let condition = self.parse_binary(1);
        if !self.eat_punct(Punct::Question) {
            return condition;
        }
        let then_expr = self.parse_ternary();
        if !self.expect_punct(Punct::Colon) {
            return then_expr;
        }
        let else_expr = self.parse_ternary();
        return Expr::Ternary {
            span: condition.span().merge(else_expr.span()),
            condition: Box::new(condition),
            then_expr: Box::new(then_expr),
            else_expr: Box::new(else_expr),
        };
    }

    fn parse_binary(&mut self, min_precedence: u8) -> Expr {
        let mut lhs = self.parse_unary();
        while let Some(precedence) =
            binary_precedence(self.peek()).filter(|&precedence| return precedence >= min_precedence)
        {
            let TokenKind::Punct(punct) = self.peek().clone() else {
                break;
            };
            self.bump();
            let rhs = self.parse_binary(precedence + 1);
            let span = lhs.span().merge(rhs.span());
            lhs = Expr::Binary {
                span,
                op: punct_to_binary(punct),
                lhs: Box::new(lhs),
                rhs: Box::new(rhs),
            };
        }
        return lhs;
    }

    fn parse_unary(&mut self) -> Expr {
        if self.eat_punct(Punct::Minus) {
            let operand = self.parse_unary();
            return Expr::Unary {
                span: operand.span(),
                op: UnaryOp::Neg,
                operand: Box::new(operand),
            };
        }
        if self.eat_punct(Punct::Not) {
            let operand = self.parse_unary();
            return Expr::Unary {
                span: operand.span(),
                op: UnaryOp::Not,
                operand: Box::new(operand),
            };
        }
        return self.parse_postfix();
    }

    fn parse_postfix(&mut self) -> Expr {
        let mut expr = self.parse_primary();
        loop {
            if self.eat_punct(Punct::Dot) {
                let Some(name) = self.expect_ident() else {
                    break;
                };
                let span = expr.span().merge(name.span);
                expr = Expr::Member {
                    span,
                    base: Box::new(expr),
                    name,
                };
                continue;
            }
            if self.at_punct(Punct::LParen) {
                self.bump();
                let args = self.parse_call_args();
                self.expect_punct(Punct::RParen);
                let span = expr.span().merge(self.prev_span());
                expr = Expr::Call {
                    span,
                    callee: Box::new(expr),
                    args,
                };
                continue;
            }
            break;
        }
        return expr;
    }

    fn parse_primary(&mut self) -> Expr {
        let token = self.bump();
        let span = token.span;
        return match token.kind {
            TokenKind::Int { value, unit } => match unit {
                NumberUnit::None => Expr::Int { span, value },
                NumberUnit::Dp => Expr::Length {
                    span,
                    length: Length::Dp(value as f32),
                },
                NumberUnit::Percent => Expr::Length {
                    span,
                    length: Length::Percent(value as f32),
                },
                NumberUnit::Millis => Expr::Duration {
                    span,
                    duration: Duration::from_millis(value as f64),
                },
            },
            TokenKind::Float { value, unit } => match unit {
                NumberUnit::None => Expr::Float { span, value },
                NumberUnit::Dp => Expr::Length {
                    span,
                    length: Length::Dp(value as f32),
                },
                NumberUnit::Percent => Expr::Length {
                    span,
                    length: Length::Percent(value as f32),
                },
                NumberUnit::Millis => Expr::Duration {
                    span,
                    duration: Duration::from_millis(value),
                },
            },
            TokenKind::Color(digits) => Expr::Color { span, digits },
            TokenKind::Str(value) => self.parse_string_literal(value, span),
            TokenKind::Keyword(Keyword::True) => Expr::Bool { span, value: true },
            TokenKind::Keyword(Keyword::False) => Expr::Bool { span, value: false },
            TokenKind::Keyword(Keyword::Auto) => Expr::Auto { span },
            TokenKind::Ident(name) => Expr::Ident { span, name },
            TokenKind::Punct(Punct::LParen) => {
                let inner = self.parse_expression();
                self.expect_punct(Punct::RParen);
                return inner;
            }
            kind => {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    format!("expected an expression, found {kind}"),
                ));
                return Expr::Error { span };
            }
        };
    }

    /// Splits a string literal into text and `{expr}` interpolation parts.
    fn parse_string_literal(&mut self, value: String, span: Span) -> Expr {
        let mut parts: Vec<StrPart> = Vec::new();
        let mut text = String::new();
        let mut chars = value.char_indices().peekable();
        while let Some((_index, ch)) = chars.next() {
            if ch != '{' {
                text.push(ch);
                continue;
            }
            if !text.is_empty() {
                parts.push(StrPart::Text(std::mem::take(&mut text)));
            }
            let mut hole = String::new();
            let mut closed = false;
            for (_hole_index, hole_char) in chars.by_ref() {
                if hole_char == '}' {
                    closed = true;
                    break;
                }
                hole.push(hole_char);
            }
            if !closed {
                self.diagnostics.push(Diagnostic::error(
                    span,
                    "unterminated interpolation hole (missing `}`) in string literal",
                ));
                break;
            }
            if hole.trim().is_empty() {
                self.diagnostics
                    .push(Diagnostic::error(span, "empty interpolation hole `{}`"));
                continue;
            }
            let expr = self.parse_embedded_expression(&hole, span);
            let hole_span = expr.span();
            parts.push(StrPart::Interp {
                span: hole_span,
                expr: Box::new(expr),
            });
        }
        if !text.is_empty() || parts.is_empty() {
            parts.push(StrPart::Text(text));
        }
        return Expr::String { span, parts };
    }

    /// Parses an interpolation hole by running a sub-parser on the hole's
    /// source. Spans are shifted by the string token's start, so diagnostics
    /// land near the hole (offsets are approximate when escapes are present,
    /// since escapes are resolved before this step).
    fn parse_embedded_expression(&mut self, source: &str, string_span: Span) -> Expr {
        let LexOutcome {
            mut tokens,
            mut diagnostics,
        } = lex(source);
        let base = string_span.start;
        for token in &mut tokens {
            token.span = Span::new(token.span.start + base, token.span.end + base);
        }
        for diagnostic in &mut diagnostics {
            diagnostic.span = Span::new(diagnostic.span.start + base, diagnostic.span.end + base);
        }
        self.diagnostics.append(&mut diagnostics);
        let mut embedded = Parser {
            tokens,
            pos: 0,
            diagnostics: Vec::new(),
        };
        let expr = embedded.parse_expression();
        if !matches!(embedded.peek(), TokenKind::Eof) {
            embedded.error("unexpected tokens after the expression in the interpolation hole");
        }
        self.diagnostics.append(&mut embedded.diagnostics);
        return expr;
    }

    /// Parses call arguments until `)` (not consumed): positional values or
    /// `name = value` pairs, comma-separated with an optional trailing
    /// comma.
    pub(crate) fn parse_call_args(&mut self) -> Vec<CallArg> {
        let mut args = Vec::new();
        loop {
            if matches!(
                self.peek(),
                TokenKind::Punct(Punct::RParen) | TokenKind::Eof
            ) {
                return args;
            }
            let start_index = self.pos;
            let name = if matches!(self.peek(), TokenKind::Ident(_))
                && matches!(self.second(), TokenKind::Punct(Punct::Assign))
            {
                let ident = self.expect_ident();
                self.bump(); // `=`
                ident
            } else {
                None
            };
            let value = self.parse_expression();
            args.push(CallArg {
                span: self.span_from(start_index),
                name,
                value,
            });
            if !self.eat_punct(Punct::Comma) {
                return args;
            }
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use crate::ast::{BinaryOp, Expr};
    use crate::parser::parse;

    fn parse_expr_of(source: &str) -> Expr {
        let wrapped = format!("component A {{ Window(title <- {source}) {{}} }}");
        let outcome = parse(&wrapped);
        assert!(
            outcome.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            outcome.diagnostics
        );
        let component = &outcome.document.components[0];
        let member = &component.members[0];
        let crate::ast::ComponentMember::Node(window) = member else {
            panic!("expected the Window node");
        };
        let crate::ast::NodeArg::Property(assignment) = &window.args[0] else {
            panic!("expected the title assignment");
        };
        return assignment.value.clone();
    }

    #[test]
    fn parses_operator_precedence() {
        let expr = parse_expr_of("1 + 2 * 3");
        let Expr::Binary { op, lhs, rhs, .. } = &expr else {
            panic!("expected a binary expression");
        };
        assert_eq!(*op, BinaryOp::Add);
        assert!(matches!(**lhs, Expr::Int { value: 1, .. }));
        let Expr::Binary { op: rhs_op, .. } = &**rhs else {
            panic!("expected a nested binary expression");
        };
        assert_eq!(*rhs_op, BinaryOp::Mul);
    }

    #[test]
    fn parses_ternary() {
        let expr = parse_expr_of("count > 3 ? \"big\" : \"small\"");
        assert!(matches!(expr, Expr::Ternary { .. }));
    }

    #[test]
    fn parses_unary_not() {
        let expr = parse_expr_of("!enabled");
        assert!(matches!(expr, Expr::Unary { .. }));
    }

    #[test]
    fn parses_call_with_named_args() {
        let expr = parse_expr_of("tween(width, duration = 200ms, easing = ease-out)");
        let Expr::Call { args, .. } = &expr else {
            panic!("expected a call expression");
        };
        assert_eq!(args.len(), 3);
        assert!(args[0].name.is_none());
        assert_eq!(args[1].name.as_ref().unwrap().name, "duration");
        assert_eq!(args[2].name.as_ref().unwrap().name, "easing");
    }

    #[test]
    fn parses_interpolation_hole_as_expression() {
        let expr = parse_expr_of("\"n: {count + 1}\"");
        let Expr::String { parts, .. } = &expr else {
            panic!("expected a string expression");
        };
        // "n: {count + 1}" = trailing text + one hole; no text after the hole.
        assert_eq!(parts.len(), 2);
        assert!(matches!(&parts[0], crate::ast::StrPart::Text(_)));
        let crate::ast::StrPart::Interp { expr: hole, .. } = &parts[1] else {
            panic!("expected an interpolation part");
        };
        assert!(matches!(**hole, Expr::Binary { .. }));
    }

    #[test]
    fn rejects_empty_interpolation_hole() {
        let outcome = parse("component A { Window(title <- \"{}\") {} }");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("empty interpolation hole"))
        );
    }

    #[test]
    fn parses_parenthesized_grouping() {
        let expr = parse_expr_of("(1 + 2) * 3");
        let Expr::Binary { op, lhs, .. } = &expr else {
            panic!("expected a binary expression");
        };
        assert_eq!(*op, BinaryOp::Mul);
        assert!(matches!(**lhs, Expr::Binary { .. }));
    }
}
