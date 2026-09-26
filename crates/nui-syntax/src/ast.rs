//! Abstract syntax tree of nui-lang.

use nui_core::{Duration, Length};

use crate::span::Span;

/// A parsed document: one or more component declarations.
#[derive(Debug, Clone)]
pub struct Document {
    /// Component declarations in source order.
    pub components: Vec<ComponentDecl>,
}

/// A single component declaration (`component Name { ... }`).
#[derive(Debug, Clone)]
pub struct ComponentDecl {
    /// Span covering the whole declaration.
    pub span: Span,
    /// Component name.
    pub name: Ident,
    /// Body members.
    pub members: Vec<ComponentMember>,
}

/// Members allowed directly inside a component body.
#[derive(Debug, Clone)]
pub enum ComponentMember {
    /// Property declaration.
    Property(PropertyDecl),
    /// Signal declaration.
    Signal(SignalDecl),
    /// State machine declaration.
    Machine(MachineDecl),
    /// Child node.
    Node(NodeDecl),
}

/// An identifier with its span.
#[derive(Debug, Clone)]
pub struct Ident {
    /// Span of the identifier.
    pub span: Span,
    /// Identifier text (may be kebab-cased).
    pub name: String,
}

/// A property declaration (`property count: Int = 0`).
#[derive(Debug, Clone)]
pub struct PropertyDecl {
    /// Span covering the declaration.
    pub span: Span,
    /// Property name.
    pub name: Ident,
    /// Declared type; `None` means inferred from the default value.
    pub declared_type: Option<Ident>,
    /// Default value with its operator, if present.
    pub default: Option<PropertyInit>,
}

/// The default-value part of a property declaration.
#[derive(Debug, Clone)]
pub struct PropertyInit {
    /// Span of the value expression.
    pub span: Span,
    /// Which operator connects property and value.
    pub op: InitOp,
    /// Value expression.
    pub value: Expr,
}

/// The three data operators of nui-lang.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitOp {
    /// `=` static assignment: written once, never re-evaluated.
    Static,
    /// `<-` reactive binding: re-evaluated when dependencies change.
    Bind,
    /// `<=>` two-way binding: writes sync in both directions.
    TwoWay,
}

/// A signal declaration (`signal resetRequested`).
#[derive(Debug, Clone)]
pub struct SignalDecl {
    /// Span covering the declaration.
    pub span: Span,
    /// Signal name.
    pub name: Ident,
}

/// A child node: `Type(args) { body }`.
#[derive(Debug, Clone)]
pub struct NodeDecl {
    /// Span covering the whole node.
    pub span: Span,
    /// Node type name.
    pub ty: Ident,
    /// Constructor arguments (property assignments and the `id` argument).
    pub args: Vec<NodeArg>,
    /// `For(variable in iterable)` binding, present on `For` nodes only.
    pub for_binding: Option<ForBinding>,
    /// Body members.
    pub body: Vec<NodeMember>,
}

/// The `For(item in model)` clause.
#[derive(Debug, Clone)]
pub struct ForBinding {
    /// Span of the clause.
    pub span: Span,
    /// Loop variable name.
    pub variable: Ident,
    /// Iterable expression.
    pub iterable: Expr,
}

/// Constructor arguments of a node.
#[derive(Debug, Clone)]
pub enum NodeArg {
    /// `id = name` pseudo-argument.
    Id(Ident),
    /// Property assignment.
    Property(PropertyAssignment),
    /// Event handler (`on click => ...`), allowed among the arguments so
    /// compact widgets fit on one line.
    Handler(Handler),
}

/// Members allowed inside a node body.
#[derive(Debug, Clone)]
pub enum NodeMember {
    /// Property assignment (`title <- "..."`).
    Assignment(PropertyAssignment),
    /// Event handler (`on click => ...`).
    Handler(Handler),
    /// Child node.
    Node(NodeDecl),
    /// Conditional property block (`when cond { ... }`).
    When(WhenBlock),
}

/// A property assignment: `target op value`.
#[derive(Debug, Clone)]
pub struct PropertyAssignment {
    /// Span covering the assignment.
    pub span: Span,
    /// Target property path (`title`, `font.size`, `btn.enabled`).
    pub target: PropertyPath,
    /// Assignment operator.
    pub op: InitOp,
    /// Value expression.
    pub value: Expr,
}

/// A dotted property path.
#[derive(Debug, Clone)]
pub struct PropertyPath {
    /// Span covering the path.
    pub span: Span,
    /// Path segments.
    pub parts: Vec<Ident>,
}

/// An event handler: `on signal => effect`.
#[derive(Debug, Clone)]
pub struct Handler {
    /// Span covering the handler.
    pub span: Span,
    /// Signal name.
    pub signal: Ident,
    /// Handler effect statements.
    pub effect: Vec<Statement>,
}

/// A conditional property block: `when condition { assignments }`.
#[derive(Debug, Clone)]
pub struct WhenBlock {
    /// Span covering the block.
    pub span: Span,
    /// Condition expression (must be Bool).
    pub condition: Expr,
    /// Assignments applied while the condition holds.
    pub assignments: Vec<PropertyAssignment>,
}

/// A state machine declaration (`machine name { ... }`).
#[derive(Debug, Clone)]
pub struct MachineDecl {
    /// Span covering the machine.
    pub span: Span,
    /// Machine name.
    pub name: Ident,
    /// Declared states.
    pub states: Vec<StateDecl>,
    /// Transitions between states.
    pub transitions: Vec<TransitionDecl>,
}

/// A state inside a machine.
#[derive(Debug, Clone)]
pub struct StateDecl {
    /// Span covering the state.
    pub span: Span,
    /// State name.
    pub name: Ident,
    /// Effect run when entering the state.
    pub enter: Option<Vec<Statement>>,
    /// Effect run when leaving the state.
    pub exit: Option<Vec<Statement>>,
}

/// A transition: `on event from states when guard => target`.
#[derive(Debug, Clone)]
pub struct TransitionDecl {
    /// Span covering the transition.
    pub span: Span,
    /// Event (signal) that triggers the transition.
    pub event: Ident,
    /// Source states (at least one).
    pub from_states: Vec<Ident>,
    /// Optional guard expression (must be Bool).
    pub guard: Option<Expr>,
    /// Target state.
    pub to_state: Ident,
}

/// Statements allowed inside effect blocks: `let`, `if`/`else`, property
/// assignment, `emit`, and method calls. No loops, no closures.
#[derive(Debug, Clone)]
pub enum Statement {
    /// `let name = value`.
    Let {
        /// Span covering the statement.
        span: Span,
        /// Variable name.
        name: Ident,
        /// Value expression.
        value: Expr,
    },
    /// `if condition { ... } else { ... }`.
    If {
        /// Span covering the statement.
        span: Span,
        /// Condition expression.
        condition: Expr,
        /// Branch taken when the condition holds.
        then_branch: Vec<Statement>,
        /// Branch taken otherwise (`else if` is a nested `If`).
        else_branch: Option<Vec<Statement>>,
    },
    /// `target op value` (`=`, `+=`, `-=`).
    Assign {
        /// Span covering the statement.
        span: Span,
        /// Assignment target.
        target: PropertyPath,
        /// Assignment operator.
        op: AssignOp,
        /// Value expression.
        value: Expr,
    },
    /// `emit signal`.
    Emit {
        /// Span covering the statement.
        span: Span,
        /// Signal name.
        signal: Ident,
    },
    /// `element.method(args)` (e.g. `timer.start()`).
    Call {
        /// Span covering the statement.
        span: Span,
        /// Callee path.
        callee: PropertyPath,
        /// Call arguments.
        args: Vec<CallArg>,
    },
}

/// Compound assignment operators in effect blocks.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    /// `=`
    Set,
    /// `+=`
    Add,
    /// `-=`
    Sub,
}

/// Pure expressions of nui-lang.
#[derive(Debug, Clone)]
pub enum Expr {
    /// Integer literal.
    Int {
        /// Span of the literal.
        span: Span,
        /// Value.
        value: i64,
    },
    /// Float literal.
    Float {
        /// Span of the literal.
        span: Span,
        /// Value.
        value: f64,
    },
    /// Boolean literal.
    Bool {
        /// Span of the literal.
        span: Span,
        /// Value.
        value: bool,
    },
    /// `auto` literal (a member of the Length type).
    Auto {
        /// Span of the literal.
        span: Span,
    },
    /// Color literal; `digits` excludes the `#` prefix.
    Color {
        /// Span of the literal.
        span: Span,
        /// Raw hex digits, validated by the compiler.
        digits: String,
    },
    /// Length literal (`420dp`, `50%`).
    Length {
        /// Span of the literal.
        span: Span,
        /// Value.
        length: Length,
    },
    /// Duration literal (`200ms`).
    Duration {
        /// Span of the literal.
        span: Span,
        /// Value.
        duration: Duration,
    },
    /// String literal with interpolation holes.
    String {
        /// Span of the literal.
        span: Span,
        /// Interleaved text and expression parts.
        parts: Vec<StrPart>,
    },
    /// Identifier reference.
    Ident {
        /// Span of the identifier.
        span: Span,
        /// Identifier name.
        name: String,
    },
    /// Member access (`base.name`).
    Member {
        /// Span covering base and name.
        span: Span,
        /// Base expression.
        base: Box<Expr>,
        /// Accessed member.
        name: Ident,
    },
    /// Unary operation.
    Unary {
        /// Span covering operator and operand.
        span: Span,
        /// Operator.
        op: UnaryOp,
        /// Operand.
        operand: Box<Expr>,
    },
    /// Binary operation.
    Binary {
        /// Span covering both sides.
        span: Span,
        /// Operator.
        op: BinaryOp,
        /// Left operand.
        lhs: Box<Expr>,
        /// Right operand.
        rhs: Box<Expr>,
    },
    /// Ternary selection (`cond ? then : else`).
    Ternary {
        /// Span covering all three parts.
        span: Span,
        /// Condition.
        condition: Box<Expr>,
        /// Value when the condition holds.
        then_expr: Box<Expr>,
        /// Value otherwise.
        else_expr: Box<Expr>,
    },
    /// Function call (`min(a, b)`, `tween(x, duration = 200ms)`).
    Call {
        /// Span covering callee and arguments.
        span: Span,
        /// Callee (an identifier for builtin functions).
        callee: Box<Expr>,
        /// Arguments, positional or named.
        args: Vec<CallArg>,
    },
    /// Synthetic node produced by error recovery; never type-checks.
    Error {
        /// Span of the unexpected input.
        span: Span,
    },
}

impl Expr {
    /// Source span of the expression.
    pub fn span(&self) -> Span {
        return match self {
            Expr::Int { span, .. }
            | Expr::Float { span, .. }
            | Expr::Bool { span, .. }
            | Expr::Auto { span }
            | Expr::Color { span, .. }
            | Expr::Length { span, .. }
            | Expr::Duration { span, .. }
            | Expr::String { span, .. }
            | Expr::Ident { span, .. }
            | Expr::Member { span, .. }
            | Expr::Unary { span, .. }
            | Expr::Binary { span, .. }
            | Expr::Ternary { span, .. }
            | Expr::Call { span, .. }
            | Expr::Error { span } => *span,
        };
    }
}

/// Parts of a string literal.
#[derive(Debug, Clone)]
pub enum StrPart {
    /// Literal text.
    Text(String),
    /// Interpolation hole `{expr}`.
    Interp {
        /// Span of the hole (approximate: escapes shift offsets).
        span: Span,
        /// Parsed sub-expression.
        expr: Box<Expr>,
    },
}

/// Unary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UnaryOp {
    /// `-`
    Neg,
    /// `!`
    Not,
}

/// Binary operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BinaryOp {
    /// `+`
    Add,
    /// `-`
    Sub,
    /// `*`
    Mul,
    /// `/`
    Div,
    /// `%`
    Rem,
    /// `==`
    Eq,
    /// `!=`
    NotEq,
    /// `<`
    Lt,
    /// `<=`
    Le,
    /// `>`
    Gt,
    /// `>=`
    Ge,
    /// `&&` (eager: expressions are pure, both sides are evaluated)
    And,
    /// `||` (eager, same rationale)
    Or,
}

/// A call argument: positional, or named (`duration = 200ms`).
#[derive(Debug, Clone)]
pub struct CallArg {
    /// Span covering the argument.
    pub span: Span,
    /// Argument name for named arguments.
    pub name: Option<Ident>,
    /// Argument value.
    pub value: Expr,
}
