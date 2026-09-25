//! Typed expression bytecode: the checked form of nui-lang expressions,
//! ready for the M2 runtime to evaluate.

use nui_core::Value;
use nui_syntax::{BinaryOp, Span, UnaryOp};

use crate::types::Type;

/// Where a property read resolves to.
#[derive(Debug, Clone, PartialEq)]
pub enum PropertyTarget {
    /// A property of the enclosing component (`count`).
    Component(String),
    /// A property through the `root` keyword (`root.count`).
    Root(String),
    /// A property through the `parent` keyword (`parent.width`).
    Parent(String),
    /// A property of an element declared with `id` (`btn.enabled`).
    Id(String, String),
    /// The current state of a machine (`playback.playing`), typed Bool.
    MachineState(String, String),
}

/// Builtin functions callable from expressions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Builtin {
    /// `min(a, b)` numeric.
    Min,
    /// `max(a, b)` numeric.
    Max,
    /// `clamp(v, lo, hi)` numeric.
    Clamp,
    /// `tween(value, duration, easing)` animated binding.
    Tween,
    /// `spring(value, stiffness, damping)` animated binding.
    Spring,
}

impl Builtin {
    /// Maps a callee identifier to a builtin; `None` for unknown names.
    pub fn from_name(name: &str) -> Option<Builtin> {
        return match name {
            "min" => Some(Builtin::Min),
            "max" => Some(Builtin::Max),
            "clamp" => Some(Builtin::Clamp),
            "tween" => Some(Builtin::Tween),
            "spring" => Some(Builtin::Spring),
            _ => None,
        };
    }
}

/// A part of an interpolated string.
#[derive(Debug, Clone)]
pub enum InterpPart {
    /// Literal text.
    Text(String),
    /// An expression rendered with [`std::fmt::Display`] semantics.
    Expr(Box<TypedExpr>),
}

/// A type-checked expression.
///
/// Every variant carries its result type except [`TypedExpr::Error`], which
/// marks input that already produced a diagnostic; `type_of` returns
/// `Unknown` for it.
#[derive(Debug, Clone)]
pub enum TypedExpr {
    /// A literal value.
    Const(Value),
    /// A `let`-bound local from an enclosing effect or `For` clause.
    Local {
        /// Variable name (runtime resolution is name-based so `For` row
        /// subtrees can resolve their loop variable through the scope
        /// chain of ancestor elements).
        name: String,
        /// Scope slot index (compile-time ordering; shadowing already
        /// resolved by nearest match in the checker).
        index: usize,
        /// Local's type.
        ty: Type,
    },
    /// A property read.
    Property {
        /// Resolution of the read.
        target: PropertyTarget,
        /// Read type.
        ty: Type,
    },
    /// Unary operation.
    Unary {
        /// Operator.
        op: UnaryOp,
        /// Operand.
        operand: Box<TypedExpr>,
        /// Result type.
        ty: Type,
    },
    /// Binary operation.
    Binary {
        /// Operator.
        op: BinaryOp,
        /// Left operand.
        lhs: Box<TypedExpr>,
        /// Right operand.
        rhs: Box<TypedExpr>,
        /// Result type.
        ty: Type,
    },
    /// Ternary selection.
    Ternary {
        /// Condition (Bool).
        condition: Box<TypedExpr>,
        /// Value when the condition holds.
        then_expr: Box<TypedExpr>,
        /// Value otherwise.
        else_expr: Box<TypedExpr>,
        /// Unified result type.
        ty: Type,
    },
    /// Builtin function call.
    Call {
        /// Which builtin.
        func: Builtin,
        /// Checked arguments.
        args: Vec<TypedExpr>,
        /// Result type.
        ty: Type,
    },
    /// Host-registered function call (plan §5 宿主互操作): the name is
    /// validated against the host's registered function names at compile
    /// time; the implementation is resolved from the runtime registry.
    HostCall {
        /// Registered function name.
        name: String,
        /// Checked arguments.
        args: Vec<TypedExpr>,
        /// Result type (unknown for host functions; the host owns typing).
        ty: Type,
    },
    /// Interpolated string.
    Interp {
        /// Text and expression parts.
        parts: Vec<InterpPart>,
    },
    /// Member access on an expression (models in M4); unchecked, typed
    /// `Unknown`.
    Dynamic {
        /// Base expression.
        base: Box<TypedExpr>,
        /// Accessed member name.
        name: String,
    },
    /// Placeholder for input that already produced a diagnostic.
    Error,
}

impl TypedExpr {
    /// The expression's result type; `Unknown` for error placeholders.
    pub fn type_of(&self) -> Type {
        return match self {
            TypedExpr::Const(value) => match value {
                Value::Bool(_) => Type::Bool,
                Value::Int(_) => Type::Int,
                Value::Float(_) => Type::Float,
                Value::String(_) => Type::String,
                Value::Color(_) => Type::Color,
                Value::Length(_) => Type::Length,
                Value::Duration(_) => Type::Duration,
                Value::Enum(_) => Type::Enum,
                Value::Model(_) => Type::Model,
            },
            TypedExpr::Local { ty, .. }
            | TypedExpr::Property { ty, .. }
            | TypedExpr::Unary { ty, .. }
            | TypedExpr::Binary { ty, .. }
            | TypedExpr::Ternary { ty, .. }
            | TypedExpr::Call { ty, .. }
            | TypedExpr::HostCall { ty, .. } => *ty,
            TypedExpr::Interp { .. } => Type::String,
            TypedExpr::Dynamic { .. } => Type::Unknown,
            TypedExpr::Error => Type::Unknown,
        };
    }

    /// Source span of the expression (approximate for interpolated strings,
    /// which keep the string literal's span).
    pub fn span(&self) -> Span {
        return match self {
            TypedExpr::Const(_) => Span::new(0, 0),
            TypedExpr::Local { .. } => Span::new(0, 0),
            TypedExpr::Property { .. } => Span::new(0, 0),
            TypedExpr::Unary { operand, .. } => operand.span(),
            TypedExpr::Dynamic { base, .. } => base.span(),
            TypedExpr::Binary { lhs, rhs, .. } => lhs.span().merge(rhs.span()),
            TypedExpr::Ternary {
                condition,
                else_expr,
                ..
            } => condition.span().merge(else_expr.span()),
            TypedExpr::Call { args, .. } => args
                .first()
                .map(|arg| return arg.span())
                .unwrap_or(Span::new(0, 0)),
            TypedExpr::HostCall { args, .. } => args
                .first()
                .map(|arg| return arg.span())
                .unwrap_or(Span::new(0, 0)),
            TypedExpr::Interp { .. } => Span::new(0, 0),
            TypedExpr::Error => Span::new(0, 0),
        };
    }
}

/// Effect-statement bytecode: the checked form of handler and transition
/// bodies. No loops, no closures.
#[derive(Debug, Clone)]
pub enum Effect {
    /// `let name = value` (immutable binding).
    Let {
        /// Variable name.
        name: String,
        /// Value expression.
        value: TypedExpr,
    },
    /// `if condition { .. } else { .. }`.
    If {
        /// Condition (Bool).
        condition: TypedExpr,
        /// Branch taken when the condition holds.
        then_branch: Vec<Effect>,
        /// Branch taken otherwise.
        else_branch: Vec<Effect>,
    },
    /// Property assignment (`=`, `+=`, `-=`).
    Assign {
        /// Assignment target.
        target: PropertyTarget,
        /// Compound operator.
        op: AssignOp,
        /// Value expression.
        value: TypedExpr,
    },
    /// `emit signal`.
    Emit {
        /// Declared signal name.
        signal: String,
    },
    /// Method call on an element id (`timer.start()`).
    Call {
        /// Callee path (at least `[id, method]`).
        callee: Vec<String>,
        /// Checked arguments.
        args: Vec<TypedExpr>,
    },
}

/// Compound assignment operators.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssignOp {
    /// `=`
    Set,
    /// `+=`
    Add,
    /// `-=`
    Sub,
}
