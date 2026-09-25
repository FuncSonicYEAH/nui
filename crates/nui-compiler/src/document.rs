//! Document IR: the resolved, checked form of a `.nui` document, ready for
//! runtime instantiation.

use crate::bytecode::{AssignOp, Effect, PropertyTarget, TypedExpr};
use crate::types::Type;

/// A compiled document: all components, in source order.
#[derive(Debug, Clone, Default)]
pub struct DocumentIr {
    /// Compiled components.
    pub components: Vec<ComponentIr>,
}

/// A compiled component.
#[derive(Debug, Clone, Default)]
pub struct ComponentIr {
    /// Component name.
    pub name: String,
    /// Declared properties.
    pub properties: Vec<PropertyIr>,
    /// Declared signals.
    pub signals: Vec<String>,
    /// Declared state machines.
    pub machines: Vec<MachineIr>,
    /// Ids declared inside the component (`id` -> node type name).
    pub ids: Vec<(String, String)>,
    /// Root child nodes (components usually have exactly one).
    pub roots: Vec<NodeIr>,
}

/// A compiled property declaration.
#[derive(Debug, Clone)]
pub struct PropertyIr {
    /// Property name.
    pub name: String,
    /// Property type (declared or inferred).
    pub ty: Type,
    /// Default value with its operator.
    pub default: Option<PropertyDefaultIr>,
}

/// The default value of a property.
#[derive(Debug, Clone)]
pub enum PropertyDefaultIr {
    /// `= expr`: evaluated once at instantiation.
    Static(TypedExpr),
    /// `<- expr`: a reactive binding, re-evaluated on dependency changes.
    Bind(TypedExpr),
}

/// A compiled state machine.
#[derive(Debug, Clone)]
pub struct MachineIr {
    /// Machine name.
    pub name: String,
    /// States in declaration order.
    pub states: Vec<StateIr>,
    /// Transitions.
    pub transitions: Vec<TransitionIr>,
}

/// A compiled state.
#[derive(Debug, Clone)]
pub struct StateIr {
    /// State name.
    pub name: String,
    /// Effect run when the state is entered.
    pub enter: Vec<Effect>,
    /// Effect run when the state is left.
    pub exit: Vec<Effect>,
}

/// A compiled transition.
#[derive(Debug, Clone)]
pub struct TransitionIr {
    /// Triggering signal name.
    pub event: String,
    /// Source state names.
    pub from: Vec<String>,
    /// Optional guard (Bool).
    pub guard: Option<TypedExpr>,
    /// Target state name.
    pub to: String,
}

/// A compiled node (element instance).
#[derive(Debug, Clone, Default)]
pub struct NodeIr {
    /// Node type name (`Window`, `Button`, `For`, ...).
    pub ty: String,
    /// Declared `id`, if any.
    pub id: Option<String>,
    /// `For` binding, present on `For` nodes only.
    pub for_binding: Option<ForIr>,
    /// Property assignments (arguments and body assignments merged).
    pub assignments: Vec<AssignmentIr>,
    /// Event handlers.
    pub handlers: Vec<HandlerIr>,
    /// Conditional property blocks.
    pub when_blocks: Vec<WhenIr>,
    /// Child nodes.
    pub children: Vec<NodeIr>,
}

/// A `For(variable in iterable)` binding.
#[derive(Debug, Clone)]
pub struct ForIr {
    /// Loop variable name.
    pub variable: String,
    /// Iterable expression (typed M4 with models).
    pub iterable: TypedExpr,
}

/// A property assignment on a node.
#[derive(Debug, Clone)]
pub struct AssignmentIr {
    /// Dotted property path (`width`, `font.size`).
    pub path: Vec<String>,
    /// Resolved write target (which element the path addresses).
    pub target: PropertyTarget,
    /// Assignment operator.
    pub kind: InitKind,
    /// Value expression.
    pub value: TypedExpr,
}

/// The three data operators, as recorded in the IR.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InitKind {
    /// `=` static assignment.
    Static,
    /// `<-` reactive binding.
    Bind,
    /// `<=>` two-way binding.
    TwoWay,
}

/// A compiled event handler.
#[derive(Debug, Clone)]
pub struct HandlerIr {
    /// Signal name (`click`, component signals, ...).
    pub signal: String,
    /// Effect statements.
    pub effect: Vec<Effect>,
}

/// A compiled `when` block.
#[derive(Debug, Clone)]
pub struct WhenIr {
    /// Condition (Bool).
    pub condition: TypedExpr,
    /// Assignments applied while the condition holds.
    pub assignments: Vec<AssignmentIr>,
}

/// Convenience accessor for compound assignment ops in IR consumers.
pub fn assign_op_name(op: AssignOp) -> &'static str {
    return match op {
        AssignOp::Set => "=",
        AssignOp::Add => "+=",
        AssignOp::Sub => "-=",
    };
}
