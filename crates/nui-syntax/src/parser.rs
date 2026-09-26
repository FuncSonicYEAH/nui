//! Parser: tokens to AST, with error recovery.

mod expr;

use crate::ast::{
    AssignOp, ComponentDecl, ComponentMember, Document, ForBinding, Handler, Ident, InitOp,
    MachineDecl, NodeArg, NodeDecl, NodeMember, PropertyAssignment, PropertyDecl, PropertyInit,
    PropertyPath, SignalDecl, StateDecl, Statement, TransitionDecl, WhenBlock,
};
use crate::diagnostics::Diagnostic;
use crate::lexer::{LexOutcome, lex};
use crate::span::Span;
use crate::token::{Keyword, Punct, Token, TokenKind};

/// Outcome of parsing: a best-effort document plus collected diagnostics.
///
/// Parsing always returns a document; erroneous parts are dropped and
/// reported through `diagnostics`.
pub struct ParseOutcome {
    /// Parsed document (possibly incomplete when errors occurred).
    pub document: Document,
    /// Diagnostics collected while parsing.
    pub diagnostics: Vec<Diagnostic>,
}

/// Parses nui-lang source into an AST.
pub fn parse(source: &str) -> ParseOutcome {
    let LexOutcome {
        tokens,
        diagnostics: lexer_diagnostics,
    } = lex(source);
    let mut parser = Parser {
        tokens,
        pos: 0,
        diagnostics: Vec::new(),
    };
    let mut components = Vec::new();
    while !parser.at_eof() {
        let before = parser.pos;
        if let Some(component) = parser.parse_component() {
            components.push(component);
        }
        parser.recover_top_level(before);
    }
    let mut diagnostics = lexer_diagnostics;
    diagnostics.append(&mut parser.diagnostics);
    return ParseOutcome {
        document: Document { components },
        diagnostics,
    };
}

pub(crate) struct Parser {
    pub(crate) tokens: Vec<Token>,
    pub(crate) pos: usize,
    pub(crate) diagnostics: Vec<Diagnostic>,
}

impl Parser {
    fn parse_component(&mut self) -> Option<ComponentDecl> {
        let start_index = self.pos;
        if !self.expect_keyword(Keyword::Component) {
            return None;
        }
        let name = self.expect_ident()?;
        if !self.expect_punct(Punct::LBrace) {
            return None;
        }
        let members = self.parse_component_members();
        self.close_body();
        return Some(ComponentDecl {
            span: self.span_from(start_index),
            name,
            members,
        });
    }

    fn parse_component_members(&mut self) -> Vec<ComponentMember> {
        let mut members = Vec::new();
        loop {
            match self.peek().clone() {
                // A following `component` means the current body was never
                // closed; stop so the top-level loop can parse it.
                TokenKind::Keyword(Keyword::Component) => {
                    self.error("expected `}` before the next `component`");
                    return members;
                }
                TokenKind::Punct(Punct::RBrace) => return members,
                TokenKind::Eof => {
                    self.error("expected `}` before end of file");
                    return members;
                }
                TokenKind::Keyword(Keyword::Property) => match self.parse_property_decl() {
                    Some(member) => members.push(ComponentMember::Property(member)),
                    None => self.recover_member(),
                },
                TokenKind::Keyword(Keyword::Signal) => {
                    let start_index = self.pos;
                    self.bump();
                    if let Some(name) = self.expect_ident() {
                        self.eat_punct(Punct::Semi);
                        members.push(ComponentMember::Signal(SignalDecl {
                            span: self.span_from(start_index),
                            name,
                        }));
                    } else {
                        self.recover_member();
                    }
                }
                TokenKind::Keyword(Keyword::Machine) => match self.parse_machine() {
                    Some(member) => members.push(ComponentMember::Machine(member)),
                    None => self.recover_member(),
                },
                TokenKind::Ident(_) => match self.parse_node() {
                    Some(node) => members.push(ComponentMember::Node(node)),
                    None => self.recover_member(),
                },
                other => {
                    self.error(format!(
                        "expected a property, signal, machine, or child node, found {other}"
                    ));
                    self.recover_member();
                }
            }
        }
    }

    fn parse_property_decl(&mut self) -> Option<PropertyDecl> {
        let start_index = self.pos;
        self.bump(); // `property`
        let name = self.expect_ident()?;
        let declared_type = if self.eat_punct(Punct::Colon) {
            self.expect_ident()
        } else {
            None
        };
        let default = if let Some(op) = self.parse_init_op() {
            let value = self.parse_expression();
            self.eat_punct(Punct::Semi);
            Some(PropertyInit {
                span: value.span(),
                op,
                value,
            })
        } else {
            self.eat_punct(Punct::Semi);
            None
        };
        return Some(PropertyDecl {
            span: self.span_from(start_index),
            name,
            declared_type,
            default,
        });
    }

    fn parse_machine(&mut self) -> Option<MachineDecl> {
        let start_index = self.pos;
        self.bump(); // `machine`
        let name = self.expect_ident()?;
        if !self.expect_punct(Punct::LBrace) {
            return None;
        }
        let mut states = Vec::new();
        let mut transitions = Vec::new();
        loop {
            match self.peek().clone() {
                TokenKind::Punct(Punct::RBrace) => break,
                TokenKind::Eof => {
                    self.error("expected `}` before end of file");
                    break;
                }
                TokenKind::Keyword(Keyword::State) => match self.parse_state() {
                    Some(state) => states.push(state),
                    None => self.recover_member(),
                },
                TokenKind::Keyword(Keyword::On) => match self.parse_transition() {
                    Some(transition) => transitions.push(transition),
                    None => self.recover_member(),
                },
                other => {
                    self.error(format!(
                        "expected `state` or `on` inside a machine, found {other}"
                    ));
                    self.recover_member();
                }
            }
        }
        self.close_body();
        return Some(MachineDecl {
            span: self.span_from(start_index),
            name,
            states,
            transitions,
        });
    }

    fn parse_state(&mut self) -> Option<StateDecl> {
        let start_index = self.pos;
        self.bump(); // `state`
        let name = self.expect_ident()?;
        let mut enter = None;
        let mut exit = None;
        if self.eat_punct(Punct::LBrace) {
            loop {
                match self.peek().clone() {
                    TokenKind::Punct(Punct::RBrace) => break,
                    TokenKind::Eof => {
                        self.error("expected `}` before end of file");
                        break;
                    }
                    TokenKind::Keyword(Keyword::Enter) => {
                        self.bump();
                        if self.expect_punct(Punct::Arrow) {
                            enter = Some(self.parse_effect());
                        }
                    }
                    TokenKind::Keyword(Keyword::Exit) => {
                        self.bump();
                        if self.expect_punct(Punct::Arrow) {
                            exit = Some(self.parse_effect());
                        }
                    }
                    other => {
                        self.error(format!(
                            "expected `enter` or `exit` inside a state, found {other}"
                        ));
                        self.recover_member();
                    }
                }
            }
            self.close_body();
        }
        return Some(StateDecl {
            span: self.span_from(start_index),
            name,
            enter,
            exit,
        });
    }

    /// `on event from states when guard => target`
    ///
    /// `from` is a contextual keyword (matched by text, not reserved):
    /// reserving it would block member paths like `gradient.from`.
    fn parse_transition(&mut self) -> Option<TransitionDecl> {
        let start_index = self.pos;
        self.bump(); // `on`
        let event = self.expect_ident()?;
        if !self.expect_word("from") {
            return None;
        }
        let mut from_states = Vec::new();
        while let Some(state) = self.expect_ident() {
            from_states.push(state);
            if !self.eat_punct(Punct::Comma) {
                break;
            }
        }
        if from_states.is_empty() {
            self.error("expected at least one source state after `from`");
            return None;
        }
        let guard = if self.eat_keyword(Keyword::When) {
            Some(self.parse_expression())
        } else {
            None
        };
        if !self.expect_punct(Punct::Arrow) {
            return None;
        }
        let to_state = self.expect_ident()?;
        return Some(TransitionDecl {
            span: self.span_from(start_index),
            event,
            from_states,
            guard,
            to_state,
        });
    }

    fn parse_node(&mut self) -> Option<NodeDecl> {
        let start_index = self.pos;
        let ty = self.expect_ident()?;
        let mut for_binding = None;
        let mut args = Vec::new();
        let parse_args: bool;
        if self.eat_punct(Punct::LParen) {
            // `For`/`ListView` take an `(item in iterable)` clause; the
            // clause is recognized only when `ident in` follows, so a plain
            // argument list (`ListView(id = list, ...)`) still parses.
            let clause_first = ty.name == "For"
                || (ty.name == "ListView"
                    && matches!(self.second(), TokenKind::Keyword(Keyword::In)));
            if clause_first {
                let clause_start = self.pos;
                let variable = self.expect_ident()?;
                if !self.expect_keyword(Keyword::In) {
                    return None;
                }
                let iterable = self.parse_expression();
                for_binding = Some(ForBinding {
                    span: self.span_from(clause_start),
                    variable,
                    iterable,
                });
                // `ListView` may mix the clause with property arguments
                // (`ListView(item in rows, height = 50dp)`).
                parse_args = self.eat_punct(Punct::Comma);
            } else {
                parse_args = true;
            }
            if parse_args {
                loop {
                    match self.peek().clone() {
                        TokenKind::Punct(Punct::RParen) | TokenKind::Eof => break,
                        // Handlers are allowed among the arguments so a
                        // compact widget stays on one line
                        // (`Button(label = "x", on click => run())`).
                        TokenKind::Keyword(Keyword::On)
                            if matches!(self.second(), TokenKind::Ident(_)) =>
                        {
                            match self.parse_handler() {
                                Some(handler) => args.push(NodeArg::Handler(handler)),
                                None => self.recover_arg(),
                            }
                        }
                        TokenKind::Ident(text)
                            if text == "id"
                                && matches!(self.second(), TokenKind::Punct(Punct::Assign)) =>
                        {
                            self.bump(); // `id`
                            self.bump(); // `=`
                            match self.expect_ident() {
                                Some(id) => args.push(NodeArg::Id(id)),
                                None => self.recover_arg(),
                            }
                        }
                        _ => match self.parse_assignment() {
                            Some(assignment) => args.push(NodeArg::Property(assignment)),
                            None => self.recover_arg(),
                        },
                    }
                    if !self.eat_punct(Punct::Comma) {
                        break;
                    }
                }
            }
            self.expect_punct(Punct::RParen);
        }
        let mut body = Vec::new();
        if self.eat_punct(Punct::LBrace) {
            body = self.parse_node_members();
            self.close_body();
        }
        return Some(NodeDecl {
            span: self.span_from(start_index),
            ty,
            args,
            for_binding,
            body,
        });
    }

    fn parse_node_members(&mut self) -> Vec<NodeMember> {
        let mut members = Vec::new();
        loop {
            match self.peek().clone() {
                TokenKind::Keyword(Keyword::Component) => {
                    self.error("expected `}` before the next `component`");
                    return members;
                }
                TokenKind::Punct(Punct::RBrace) => return members,
                TokenKind::Eof => {
                    self.error("expected `}` before end of file");
                    return members;
                }
                TokenKind::Keyword(Keyword::When) => match self.parse_when() {
                    Some(when) => members.push(NodeMember::When(when)),
                    None => self.recover_member(),
                },
                TokenKind::Keyword(Keyword::On) => match self.parse_handler() {
                    Some(handler) => members.push(NodeMember::Handler(handler)),
                    None => self.recover_member(),
                },
                TokenKind::Ident(_) if self.starts_assignment() => match self.parse_assignment() {
                    Some(assignment) => members.push(NodeMember::Assignment(assignment)),
                    None => self.recover_member(),
                },
                TokenKind::Ident(_) => match self.parse_node() {
                    Some(node) => members.push(NodeMember::Node(node)),
                    None => self.recover_member(),
                },
                other => {
                    self.error(format!(
                        "expected a property assignment, handler, `when` block, or child node, found {other}"
                    ));
                    self.recover_member();
                }
            }
        }
    }

    /// Whether the upcoming tokens look like `path op ...` rather than a
    /// child node (`Type(` / `Type {`).
    fn starts_assignment(&self) -> bool {
        if !matches!(self.peek(), TokenKind::Ident(_)) {
            return false;
        }
        return matches!(
            self.second(),
            TokenKind::Punct(Punct::Assign | Punct::Bind | Punct::TwoWay | Punct::Dot)
        );
    }

    fn parse_when(&mut self) -> Option<WhenBlock> {
        let start_index = self.pos;
        self.bump(); // `when`
        let condition = self.parse_expression();
        if !self.expect_punct(Punct::LBrace) {
            return None;
        }
        let mut assignments = Vec::new();
        loop {
            match self.peek().clone() {
                TokenKind::Punct(Punct::RBrace) => break,
                TokenKind::Eof => {
                    self.error("expected `}` before end of file");
                    break;
                }
                _ => match self.parse_assignment() {
                    Some(assignment) => assignments.push(assignment),
                    None => self.recover_member(),
                },
            }
        }
        self.close_body();
        return Some(WhenBlock {
            span: self.span_from(start_index),
            condition,
            assignments,
        });
    }

    fn parse_handler(&mut self) -> Option<Handler> {
        let start_index = self.pos;
        self.bump(); // `on`
        let signal = self.expect_ident()?;
        if !self.expect_punct(Punct::Arrow) {
            return None;
        }
        let effect = self.parse_effect();
        return Some(Handler {
            span: self.span_from(start_index),
            signal,
            effect,
        });
    }

    /// An effect: a single statement, or a `{ ... }` block of statements.
    fn parse_effect(&mut self) -> Vec<Statement> {
        if self.eat_punct(Punct::LBrace) {
            let mut statements = Vec::new();
            loop {
                match self.peek().clone() {
                    TokenKind::Punct(Punct::RBrace) => break,
                    TokenKind::Eof => {
                        self.error("expected `}` before end of file");
                        break;
                    }
                    _ => match self.parse_statement() {
                        Some(statement) => statements.push(statement),
                        None => self.recover_member(),
                    },
                }
            }
            self.close_body();
            return statements;
        }
        return match self.parse_statement() {
            Some(statement) => vec![statement],
            None => Vec::new(),
        };
    }

    fn parse_statement(&mut self) -> Option<Statement> {
        match self.peek().clone() {
            TokenKind::Keyword(Keyword::Let) => {
                let start_index = self.pos;
                self.bump();
                let name = self.expect_ident()?;
                if !self.expect_punct(Punct::Assign) {
                    return None;
                }
                let value = self.parse_expression();
                self.eat_punct(Punct::Semi);
                return Some(Statement::Let {
                    span: self.span_from(start_index),
                    name,
                    value,
                });
            }
            TokenKind::Keyword(Keyword::If) => {
                let start_index = self.pos;
                self.bump();
                let condition = self.parse_expression();
                if !self.expect_punct(Punct::LBrace) {
                    return None;
                }
                let then_branch = self.parse_statement_block();
                self.close_body();
                let mut else_branch = None;
                if self.eat_keyword(Keyword::Else) {
                    if matches!(self.peek(), TokenKind::Keyword(Keyword::If)) {
                        // `else if` desugars to a nested If inside the else branch.
                        let nested = self.parse_statement()?;
                        else_branch = Some(vec![nested]);
                    } else {
                        if !self.expect_punct(Punct::LBrace) {
                            return None;
                        }
                        else_branch = Some(self.parse_statement_block());
                        self.close_body();
                    }
                }
                return Some(Statement::If {
                    span: self.span_from(start_index),
                    condition,
                    then_branch,
                    else_branch,
                });
            }
            TokenKind::Keyword(Keyword::Emit) => {
                let start_index = self.pos;
                self.bump();
                let signal = self.expect_ident()?;
                self.eat_punct(Punct::Semi);
                return Some(Statement::Emit {
                    span: self.span_from(start_index),
                    signal,
                });
            }
            TokenKind::Ident(_) => {
                let start_index = self.pos;
                let callee = self.parse_property_path()?;
                if self.eat_punct(Punct::LParen) {
                    let args = self.parse_call_args();
                    self.expect_punct(Punct::RParen);
                    self.eat_punct(Punct::Semi);
                    return Some(Statement::Call {
                        span: self.span_from(start_index),
                        callee,
                        args,
                    });
                }
                let op = if self.eat_punct(Punct::Assign) {
                    AssignOp::Set
                } else if self.eat_punct(Punct::PlusEq) {
                    AssignOp::Add
                } else if self.eat_punct(Punct::MinusEq) {
                    AssignOp::Sub
                } else {
                    self.error("expected `=`, `+=`, `-=`, or a method call");
                    return None;
                };
                let value = self.parse_expression();
                self.eat_punct(Punct::Semi);
                return Some(Statement::Assign {
                    span: self.span_from(start_index),
                    target: callee,
                    op,
                    value,
                });
            }
            other => {
                self.error(format!("expected a statement, found {other}"));
                return None;
            }
        }
    }

    fn parse_statement_block(&mut self) -> Vec<Statement> {
        let mut statements = Vec::new();
        loop {
            match self.peek().clone() {
                TokenKind::Punct(Punct::RBrace) | TokenKind::Eof => return statements,
                _ => match self.parse_statement() {
                    Some(statement) => statements.push(statement),
                    None => self.recover_member(),
                },
            }
        }
    }

    pub(crate) fn parse_assignment(&mut self) -> Option<PropertyAssignment> {
        let start_index = self.pos;
        let target = self.parse_property_path()?;
        let Some(op) = self.parse_init_op() else {
            self.error("expected `=`, `<-`, or `<=>` after the property path");
            return None;
        };
        let value = self.parse_expression();
        return Some(PropertyAssignment {
            span: self.span_from(start_index),
            target,
            op,
            value,
        });
    }

    fn parse_property_path(&mut self) -> Option<PropertyPath> {
        let start_index = self.pos;
        let first = self.expect_ident()?;
        let mut parts = vec![first];
        while self.eat_punct(Punct::Dot) {
            let part = self.expect_ident()?;
            parts.push(part);
        }
        return Some(PropertyPath {
            span: self.span_from(start_index),
            parts,
        });
    }

    fn parse_init_op(&mut self) -> Option<InitOp> {
        if self.eat_punct(Punct::Assign) {
            return Some(InitOp::Static);
        }
        if self.eat_punct(Punct::Bind) {
            return Some(InitOp::Bind);
        }
        if self.eat_punct(Punct::TwoWay) {
            return Some(InitOp::TwoWay);
        }
        return None;
    }

    // -- cursor helpers ----------------------------------------------------

    pub(crate) fn peek(&self) -> &TokenKind {
        return &self.tokens[self.pos].kind;
    }

    pub(crate) fn second(&self) -> &TokenKind {
        let index = (self.pos + 1).min(self.tokens.len() - 1);
        return &self.tokens[index].kind;
    }

    pub(crate) fn prev_span(&self) -> Span {
        let index = self.pos.saturating_sub(1);
        return self.tokens[index].span;
    }

    pub(crate) fn at_eof(&self) -> bool {
        return matches!(self.peek(), TokenKind::Eof);
    }

    pub(crate) fn bump(&mut self) -> Token {
        let token = self.tokens[self.pos].clone();
        if self.pos + 1 < self.tokens.len() {
            self.pos += 1;
        }
        return token;
    }

    pub(crate) fn at_punct(&self, punct: Punct) -> bool {
        return matches!(self.peek(), TokenKind::Punct(p) if *p == punct);
    }

    pub(crate) fn eat_punct(&mut self, punct: Punct) -> bool {
        if self.at_punct(punct) {
            self.bump();
            return true;
        }
        return false;
    }

    pub(crate) fn expect_punct(&mut self, punct: Punct) -> bool {
        if self.eat_punct(punct) {
            return true;
        }
        self.error(format!("expected `{punct}`, found {}", self.peek()));
        return false;
    }

    fn at_keyword(&self, keyword: Keyword) -> bool {
        return matches!(self.peek(), TokenKind::Keyword(k) if *k == keyword);
    }

    fn eat_keyword(&mut self, keyword: Keyword) -> bool {
        if self.at_keyword(keyword) {
            self.bump();
            return true;
        }
        return false;
    }

    fn expect_keyword(&mut self, keyword: Keyword) -> bool {
        if self.eat_keyword(keyword) {
            return true;
        }
        self.error(format!(
            "expected keyword `{keyword}`, found {}",
            self.peek()
        ));
        return false;
    }

    pub(crate) fn expect_ident(&mut self) -> Option<Ident> {
        if let TokenKind::Ident(name) = self.peek().clone() {
            let span = self.tokens[self.pos].span;
            self.bump();
            return Some(Ident { span, name });
        }
        self.error(format!("expected an identifier, found {}", self.peek()));
        return None;
    }

    /// Matches an identifier by exact text without reserving it (`from` in
    /// transitions); reports a diagnostic on mismatch.
    fn expect_word(&mut self, word: &str) -> bool {
        let matches_word = matches!(self.peek(), TokenKind::Ident(name) if name == word);
        if matches_word {
            self.bump();
            return true;
        }
        self.error(format!("expected `{word}`, found {}", self.peek()));
        return false;
    }

    pub(crate) fn span_from(&self, start_index: usize) -> Span {
        let first = self.tokens[start_index].span;
        let last_index = if self.pos > start_index {
            self.pos - 1
        } else {
            start_index
        };
        return first.merge(self.tokens[last_index].span);
    }

    pub(crate) fn error(&mut self, message: impl Into<String>) {
        let span = self.tokens[self.pos].span;
        self.diagnostics.push(Diagnostic::error(span, message));
    }

    /// Consumes a closing `}` if present; reports at EOF only when the body
    /// itself did not already diagnose (missing-`}` errors are emitted by
    /// the member loops, so this stays quiet at EOF).
    fn close_body(&mut self) {
        if self.at_punct(Punct::RBrace) {
            self.bump();
        }
    }

    // -- recovery ----------------------------------------------------------

    /// Skips past the offending token, then to the next plausible member
    /// start, `}`, or EOF. Always advances at least one token.
    fn recover_member(&mut self) {
        self.bump();
        while !matches!(
            self.peek(),
            TokenKind::Eof | TokenKind::Punct(Punct::RBrace)
        ) && !self.at_member_start()
        {
            self.bump();
        }
    }

    fn at_member_start(&self) -> bool {
        let is_keyword = matches!(
            self.peek(),
            TokenKind::Keyword(
                Keyword::Property
                    | Keyword::Signal
                    | Keyword::Machine
                    | Keyword::State
                    | Keyword::On
                    | Keyword::When
                    | Keyword::Let
                    | Keyword::If
                    | Keyword::Emit
            )
        );
        return matches!(self.peek(), TokenKind::Ident(_)) || is_keyword;
    }

    /// Recovers at the top level: skips to the next `component` keyword or
    /// EOF, guaranteeing progress relative to `before`.
    fn recover_top_level(&mut self, before: usize) {
        if self.pos == before && !self.at_eof() {
            self.bump();
        }
        while !matches!(
            self.peek(),
            TokenKind::Eof | TokenKind::Keyword(Keyword::Component)
        ) {
            self.bump();
        }
    }

    /// Recovers inside a constructor argument list: skips to `,` or `)`.
    fn recover_arg(&mut self) {
        self.bump();
        while !matches!(
            self.peek(),
            TokenKind::Eof | TokenKind::Punct(Punct::RParen) | TokenKind::Punct(Punct::Comma)
        ) {
            self.bump();
        }
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    fn parse_ok(source: &str) -> Document {
        let outcome = parse(source);
        assert!(
            outcome.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            outcome.diagnostics
        );
        return outcome.document;
    }

    const COUNTER: &str = r#"
        component Counter {
            property count: Int = 0
            signal resetRequested

            Window(id = root, width = 420dp, height = 300dp) {
                title <- "count: {count}"

                Column(spacing = 8dp, padding = 16dp) {
                    Text(content <- "n: {count}", font.size = 20dp, font.weight = bold)

                    Button(label = "+1", enabled <- count < 10) {
                        on click => count += 1
                    }
                }

                when count >= 10 {
                    opacity = 0.6
                }
            }
        }
    "#;

    #[test]
    fn parses_counter_example() {
        let document = parse_ok(COUNTER);
        assert_eq!(document.components.len(), 1);
        let component = &document.components[0];
        assert_eq!(component.name.name, "Counter");
        assert_eq!(component.members.len(), 3); // property, signal, Window node
        match &component.members[0] {
            ComponentMember::Property(decl) => {
                assert_eq!(decl.name.name, "count");
                assert_eq!(decl.declared_type.as_ref().unwrap().name, "Int");
            }
            other => panic!("expected a property declaration, got {other:?}"),
        }
        assert!(matches!(component.members[1], ComponentMember::Signal(_)));
    }

    #[test]
    fn parses_node_arguments_and_operators() {
        let document = parse_ok(COUNTER);
        let ComponentMember::Node(window) = &document.components[0].members[2] else {
            panic!("expected the Window node");
        };
        assert_eq!(window.ty.name, "Window");
        assert_eq!(window.args.len(), 3);
        assert!(matches!(window.args[0], NodeArg::Id(_)));
        let assignments: Vec<&PropertyAssignment> = window
            .args
            .iter()
            .filter_map(|arg| match arg {
                NodeArg::Property(assignment) => return Some(assignment),
                NodeArg::Id(_) | NodeArg::Handler(_) => return None,
            })
            .collect();
        assert_eq!(assignments.len(), 2);
        assert_eq!(assignments[0].target.parts[0].name, "width");
        assert_eq!(assignments[0].op, InitOp::Static);
        assert_eq!(assignments[1].op, InitOp::Static);

        // Node-body binding on `title`.
        assert_eq!(window.body.len(), 3);
        match &window.body[0] {
            NodeMember::Assignment(assignment) => {
                assert_eq!(assignment.target.parts[0].name, "title");
                assert_eq!(assignment.op, InitOp::Bind);
            }
            other => panic!("expected the title assignment, got {other:?}"),
        }
    }

    #[test]
    fn parses_handler_with_compound_assignment() {
        let document = parse_ok(COUNTER);
        let ComponentMember::Node(window) = &document.components[0].members[2] else {
            panic!("expected the Window node");
        };
        let NodeMember::Node(column) = &window.body[1] else {
            panic!("expected the Column node");
        };
        let NodeMember::Node(button) = &column.body[1] else {
            panic!("expected the Button node");
        };
        assert_eq!(button.body.len(), 1);
        match &button.body[0] {
            NodeMember::Handler(handler) => {
                assert_eq!(handler.signal.name, "click");
                assert_eq!(handler.effect.len(), 1);
                assert!(matches!(
                    handler.effect[0],
                    Statement::Assign {
                        op: AssignOp::Add,
                        ..
                    }
                ));
            }
            other => panic!("expected the click handler, got {other:?}"),
        }
    }

    #[test]
    fn parses_when_block() {
        let document = parse_ok(COUNTER);
        let ComponentMember::Node(window) = &document.components[0].members[2] else {
            panic!("expected the Window node");
        };
        match &window.body[2] {
            NodeMember::When(when) => {
                assert!(matches!(when.condition, crate::ast::Expr::Binary { .. }));
                assert_eq!(when.assignments.len(), 1);
                assert_eq!(when.assignments[0].target.parts[0].name, "opacity");
            }
            other => panic!("expected the when block, got {other:?}"),
        }
    }

    use crate::ast::Expr;

    #[test]
    fn parses_expressions_with_precedence() {
        let document = parse_ok(COUNTER);
        let ComponentMember::Node(window) = &document.components[0].members[2] else {
            panic!("expected the Window node");
        };
        let NodeMember::Assignment(assignment) = &window.body[0] else {
            panic!("expected the title assignment");
        };
        match &assignment.value {
            Expr::String { parts, .. } => {
                assert_eq!(parts.len(), 2);
                assert!(matches!(&parts[0], crate::ast::StrPart::Text(_)));
                assert!(matches!(&parts[1], crate::ast::StrPart::Interp { .. }));
            }
            other => panic!("expected a string with holes, got {other:?}"),
        }
    }

    #[test]
    fn parses_machine() {
        let source = r#"
            component Player {
                machine playback {
                    state stopped
                    state playing {
                        enter => timer.start()
                        exit => { timer.stop() }
                    }
                    on pause from playing when canStop => stopped
                    on play from stopped, paused => playing
                }
            }
        "#;
        let document = parse_ok(source);
        let ComponentMember::Machine(machine) = &document.components[0].members[0] else {
            panic!("expected a machine");
        };
        assert_eq!(machine.name.name, "playback");
        assert_eq!(machine.states.len(), 2);
        assert!(machine.states[0].enter.is_none());
        assert!(machine.states[1].enter.is_some());
        assert_eq!(machine.transitions.len(), 2);
        assert_eq!(machine.transitions[0].from_states.len(), 1);
        assert!(machine.transitions[0].guard.is_some());
        assert_eq!(machine.transitions[1].from_states.len(), 2);
        assert_eq!(machine.transitions[1].to_state.name, "playing");
    }

    #[test]
    fn parses_for_binding() {
        let source = r#"
            component List {
                For(item in root.items) {
                    Row(key = item.id) {}
                }
            }
        "#;
        let document = parse_ok(source);
        let ComponentMember::Node(for_node) = &document.components[0].members[0] else {
            panic!("expected the For node");
        };
        assert_eq!(for_node.ty.name, "For");
        let Some(binding) = &for_node.for_binding else {
            panic!("expected a for binding");
        };
        assert_eq!(binding.variable.name, "item");
    }

    #[test]
    fn reports_missing_component_name() {
        let outcome = parse("component { }");
        assert!(!outcome.diagnostics.is_empty());
        assert!(outcome.document.components.is_empty());
    }

    #[test]
    fn reports_unterminated_body_and_recovers() {
        let outcome = parse("component A { property x: Int = 1 component B { }");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("expected `}`"))
        );
        // The second component still parses after recovery.
        assert_eq!(outcome.document.components.len(), 2);
    }

    #[test]
    fn number_literals_carry_units() {
        let source = "component A { Window(width = 420dp, height = 50%) {} }";
        let document = parse_ok(source);
        let ComponentMember::Node(window) = &document.components[0].members[0] else {
            panic!("expected the Window node");
        };
        let NodeArg::Property(assignment) = &window.args[0] else {
            panic!("expected a property argument");
        };
        assert!(matches!(
            &assignment.value,
            Expr::Length { length, .. } if matches!(length, nui_core::Length::Dp(420.0))
        ));
    }
}
