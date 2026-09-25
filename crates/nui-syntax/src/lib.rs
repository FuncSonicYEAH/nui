//! nui-syntax: lexer, parser, AST, and span-based diagnostics for nui-lang,
//! the declarative UI language of the nui framework.
//!
//! The pipeline: source text -> [`lex`] (tokens + lexer diagnostics) ->
//! [`parse`] (AST + parser diagnostics). Both stages always return a full
//! result and collect diagnostics along the way, so tooling can report all
//! problems in one pass.
//!
//! Syntax summary (call style):
//!
//! ```text
//! component Counter {
//!     property count: Int = 0
//!     Window(id = root, width = 420dp) {
//!         title <- "count: {count}"        // <- reactive, = static
//!         Button(label = "+1") {
//!             on click => count += 1
//!         }
//!     }
//! }
//! ```

pub mod ast;
pub mod diagnostics;
pub mod lexer;
pub mod parser;
pub mod span;
pub mod token;

pub use ast::{
    AssignOp, BinaryOp, CallArg, ComponentDecl, ComponentMember, Document, Expr, ForBinding,
    Handler, Ident, InitOp, MachineDecl, NodeArg, NodeDecl, NodeMember, PropertyAssignment,
    PropertyDecl, PropertyInit, PropertyPath, SignalDecl, StateDecl, Statement, StrPart,
    TransitionDecl, UnaryOp, WhenBlock,
};
pub use diagnostics::{Diagnostic, Severity, render_diagnostic};
pub use lexer::{LexOutcome, lex};
pub use parser::{ParseOutcome, parse};
pub use span::Span;
pub use token::{Keyword, NumberUnit, Punct, Token, TokenKind};
