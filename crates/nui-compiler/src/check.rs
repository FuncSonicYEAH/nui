//! The checker: walks the AST, resolves names, checks types, and lowers to
//! Document IR.

use std::collections::{HashMap, HashSet};

use nui_core::{Length, Value};
use nui_syntax::{
    AssignOp as AstAssignOp, BinaryOp, ComponentDecl, ComponentMember, Document, Expr, Handler,
    Ident, InitOp, NodeArg, NodeDecl, NodeMember, PropertyAssignment, PropertyDecl, Span,
    Statement, UnaryOp, WhenBlock,
};

use crate::bytecode::{AssignOp, Builtin, Effect, InterpPart, PropertyTarget, TypedExpr};
use crate::document::{
    AssignmentIr, ComponentIr, DocumentIr, ForIr, HandlerIr, InitKind, MachineIr, NodeIr,
    PropertyDefaultIr, PropertyIr, StateIr, TransitionIr, WhenIr, assign_op_name,
};
use crate::types::{Type, unify};

/// Supported hex-digit counts of color literals.
const COLOR_DIGIT_COUNTS: [usize; 4] = [3, 4, 6, 8];

/// Outcome of checking: the compiled document plus diagnostics.
pub struct CheckOutcome {
    /// Compiled document (best effort; erroneous parts become `Error`
    /// placeholders or are dropped).
    pub document: DocumentIr,
    /// Diagnostics collected while checking.
    pub diagnostics: Vec<nui_syntax::Diagnostic>,
}

/// Checks a parsed document and lowers it to Document IR.
pub fn check(document: &Document) -> CheckOutcome {
    return check_with(document, &[]);
}

/// Checks a parsed document with a set of host-registered function names
/// (plan §5 宿主互操作): calls to those names lower to [`TypedExpr::HostCall`]
/// instead of producing an unknown-function diagnostic.
pub fn check_with(document: &Document, extern_functions: &[String]) -> CheckOutcome {
    let mut components = Vec::new();
    let mut diagnostics = Vec::new();
    for decl in &document.components {
        let mut binder = ComponentBinder::new(decl, extern_functions);
        let component = binder.bind();
        diagnostics.append(&mut binder.diagnostics);
        components.push(component);
    }
    return CheckOutcome {
        document: DocumentIr { components },
        diagnostics,
    };
}

/// Per-component binding context.
struct ComponentBinder<'source> {
    diagnostics: Vec<nui_syntax::Diagnostic>,
    /// Property name -> type; inferred properties are inserted as their
    /// defaults are checked, so forward references see `Unknown`.
    property_types: HashMap<String, Type>,
    /// Declared signal names.
    signals: HashSet<String>,
    /// Machine name -> declared state names.
    machine_states: HashMap<String, HashSet<String>>,
    /// Ids declared by nodes (`id` -> node type name).
    ids: HashMap<String, String>,
    /// Ids seen so far, for duplicate detection.
    seen_ids: HashSet<String>,
    /// Lexical scopes of `let` locals and `For` variables; the last frame
    /// is innermost.
    scopes: Vec<Vec<(String, Type)>>,
    /// Total number of local slots ever allocated; slot indices stay stable
    /// across scope pops so the runtime can address them flatly.
    local_count: usize,
    /// Reactive dependency edges between component properties, collected
    /// from `<-` defaults and `<-` assignments while binding; consumed by
    /// the static cycle check at the end of `bind`.
    binding_edges: Vec<(String, String, Span)>,
    /// Host-registered function names; calls to them lower to `HostCall`.
    extern_functions: HashSet<String>,
    component: &'source ComponentDecl,
}

impl<'source> ComponentBinder<'source> {
    fn new(
        component: &'source ComponentDecl,
        extern_functions: &[String],
    ) -> ComponentBinder<'source> {
        return ComponentBinder {
            diagnostics: Vec::new(),
            property_types: HashMap::new(),
            signals: HashSet::new(),
            machine_states: HashMap::new(),
            ids: HashMap::new(),
            seen_ids: HashSet::new(),
            scopes: Vec::new(),
            local_count: 0,
            binding_edges: Vec::new(),
            extern_functions: extern_functions.iter().cloned().collect(),
            component,
        };
    }

    fn bind(&mut self) -> ComponentIr {
        self.collect_info();
        let mut component_ir = ComponentIr {
            name: self.component.name.name.clone(),
            ..ComponentIr::default()
        };
        for member in &self.component.members {
            match member {
                ComponentMember::Property(decl) => {
                    if let Some(property) = self.bind_property(decl) {
                        component_ir.properties.push(property);
                    }
                }
                ComponentMember::Signal(decl) => {
                    component_ir.signals.push(decl.name.name.clone());
                }
                ComponentMember::Machine(decl) => {
                    component_ir.machines.push(self.bind_machine(decl));
                }
                ComponentMember::Node(node) => {
                    component_ir.roots.push(self.bind_node(node));
                }
            }
        }
        self.check_binding_cycles();
        return component_ir;
    }

    /// First pass: collect property names, signals, machines, and ids so
    /// that expressions can resolve them regardless of declaration order.
    fn collect_info(&mut self) {
        for member in &self.component.members {
            match member {
                ComponentMember::Property(decl) => {
                    let ty = decl
                        .declared_type
                        .as_ref()
                        .and_then(|type_ident| return Type::from_name(&type_ident.name));
                    self.property_types
                        .insert(decl.name.name.clone(), ty.unwrap_or(Type::Unknown));
                }
                ComponentMember::Signal(decl) => {
                    self.signals.insert(decl.name.name.clone());
                }
                ComponentMember::Machine(decl) => {
                    let states: HashSet<String> = decl
                        .states
                        .iter()
                        .map(|state| return state.name.name.clone())
                        .collect();
                    self.machine_states.insert(decl.name.name.clone(), states);
                }
                ComponentMember::Node(node) => self.collect_ids(node),
            }
        }
    }

    fn collect_ids(&mut self, node: &NodeDecl) {
        for arg in &node.args {
            if let NodeArg::Id(id) = arg {
                if !self.seen_ids.insert(id.name.clone()) {
                    self.diagnostics.push(nui_syntax::Diagnostic::error(
                        id.span,
                        format!("duplicate id `{}`", id.name),
                    ));
                }
                self.ids.insert(id.name.clone(), node.ty.name.clone());
            }
        }
        for member in &node.body {
            if let NodeMember::Node(child) = member {
                self.collect_ids(child);
            }
        }
    }

    fn bind_property(&mut self, decl: &PropertyDecl) -> Option<PropertyIr> {
        let declared = decl
            .declared_type
            .as_ref()
            .map(|type_ident| return (type_ident, Type::from_name(&type_ident.name)));
        if let Some((type_ident, None)) = declared {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                type_ident.span,
                format!("unknown type `{}`", type_ident.name),
            ));
        }
        let declared_ty = declared.and_then(|(_, ty)| return ty);
        let Some(default) = &decl.default else {
            if declared_ty.is_none() {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    decl.name.span,
                    format!(
                        "property `{}` needs a declared type or a default value",
                        decl.name.name
                    ),
                ));
            }
            return Some(PropertyIr {
                name: decl.name.name.clone(),
                ty: declared_ty.unwrap_or(Type::Unknown),
                default: None,
            });
        };
        if default.op == InitOp::TwoWay {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                decl.span,
                format!(
                    "two-way binding is not allowed on the declaration of `{}`; use `=` or `<-`",
                    decl.name.name
                ),
            ));
            return None;
        }
        let value = self.check_expr(&default.value);
        let value_ty = value.type_of();
        let property_ty = match declared_ty {
            Some(declared_ty) => {
                if unify(declared_ty, value_ty).is_none() {
                    self.diagnostics.push(nui_syntax::Diagnostic::error(
                        default.span,
                        format!(
                            "type mismatch in the default of `{}`: expected {}, found {}",
                            decl.name.name, declared_ty, value_ty
                        ),
                    ));
                }
                declared_ty
            }
            None => {
                if value_ty == Type::Unknown {
                    self.diagnostics.push(nui_syntax::Diagnostic::error(
                        decl.name.span,
                        format!(
                            "cannot infer the type of property `{}`; add `: Type`",
                            decl.name.name
                        ),
                    ));
                    Type::Unknown
                } else {
                    self.property_types.insert(decl.name.name.clone(), value_ty);
                    value_ty
                }
            }
        };
        let default_ir = match default.op {
            InitOp::Static => PropertyDefaultIr::Static(value),
            InitOp::Bind => {
                self.record_binding_edge(&decl.name.name, &value, default.span);
                PropertyDefaultIr::Bind(value)
            }
            InitOp::TwoWay => unreachable!("two-way defaults are rejected above"),
        };
        return Some(PropertyIr {
            name: decl.name.name.clone(),
            ty: property_ty,
            default: Some(default_ir),
        });
    }

    fn bind_machine(&mut self, decl: &nui_syntax::MachineDecl) -> MachineIr {
        let states: Vec<StateIr> = decl
            .states
            .iter()
            .map(|state| {
                return StateIr {
                    name: state.name.name.clone(),
                    enter: self.bind_effect(state.enter.as_deref().unwrap_or(&[])),
                    exit: self.bind_effect(state.exit.as_deref().unwrap_or(&[])),
                };
            })
            .collect();
        let mut transitions = Vec::new();
        for transition in &decl.transitions {
            if !self.signals.contains(&transition.event.name) {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    transition.event.span,
                    format!(
                        "transition event `{}` is not a declared signal",
                        transition.event.name
                    ),
                ));
            }
            let declared = self.machine_states.get(&decl.name.name);
            let mut from = Vec::new();
            for state in &transition.from_states {
                if declared.is_some_and(|set| return !set.contains(&state.name)) {
                    self.diagnostics.push(nui_syntax::Diagnostic::error(
                        state.span,
                        format!(
                            "unknown state `{}` in machine `{}`",
                            state.name, decl.name.name
                        ),
                    ));
                }
                from.push(state.name.clone());
            }
            if declared.is_some_and(|set| return !set.contains(&transition.to_state.name)) {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    transition.to_state.span,
                    format!(
                        "unknown target state `{}` in machine `{}`",
                        transition.to_state.name, decl.name.name
                    ),
                ));
            }
            let guard = transition.guard.as_ref().map(|guard| {
                let checked = self.check_expr(guard);
                self.require_bool(&checked, "transition guard");
                return checked;
            });
            transitions.push(TransitionIr {
                event: transition.event.name.clone(),
                from,
                guard,
                to: transition.to_state.name.clone(),
            });
        }
        return MachineIr {
            name: decl.name.name.clone(),
            states,
            transitions,
        };
    }

    fn bind_node(&mut self, decl: &NodeDecl) -> NodeIr {
        let mut node = NodeIr {
            ty: decl.ty.name.clone(),
            ..NodeIr::default()
        };
        let mut assignments_from_args = Vec::new();
        // Argument handlers land in the same list as body handlers and are
        // applied in `bind_node_members` order further down.
        let mut handlers_from_args = Vec::new();
        for arg in &decl.args {
            match arg {
                NodeArg::Id(id) => {
                    if node.id.is_some() {
                        self.diagnostics.push(nui_syntax::Diagnostic::error(
                            id.span,
                            format!("duplicate `id` argument on `{}`", decl.ty.name),
                        ));
                    }
                    node.id = Some(id.name.clone());
                }
                NodeArg::Property(assignment) => {
                    assignments_from_args.push(self.bind_node_assignment(assignment, &node));
                }
                NodeArg::Handler(handler) => {
                    handlers_from_args.push(self.bind_handler(handler));
                }
            }
        }
        // Argument assignments run before body members, so a body `on click`
        // can still read a property set in the argument list.
        node.assignments = assignments_from_args;
        node.handlers = handlers_from_args;
        // `ListView` shares `For`'s binding path (M10): the runtime
        // virtualizes its rows against the visible window.
        if decl.ty.name == "For" || decl.ty.name == "ListView" {
            let Some(binding) = &decl.for_binding else {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    decl.ty.span,
                    "`For` requires an `(item in iterable)` binding".to_string(),
                ));
                return node;
            };
            let iterable = self.check_expr(&binding.iterable);
            node.for_binding = Some(ForIr {
                variable: binding.variable.name.clone(),
                iterable,
            });
            self.push_scope();
            self.declare_local(&binding.variable.name, Type::Unknown);
            self.bind_node_members(&mut node, &decl.body);
            self.pop_scope();
            return node;
        }
        self.bind_node_members(&mut node, &decl.body);
        return node;
    }

    /// Binds body members in source order; handlers and `when` blocks are
    /// appended to the node, child nodes recurse.
    fn bind_node_members(&mut self, node: &mut NodeIr, members: &[NodeMember]) {
        for member in members {
            match member {
                NodeMember::Assignment(assignment) => {
                    node.assignments
                        .push(self.bind_node_assignment(assignment, node));
                }
                NodeMember::Handler(handler) => {
                    node.handlers.push(self.bind_handler(handler));
                }
                NodeMember::When(when) => {
                    let bound = self.bind_when(when, node);
                    node.when_blocks.push(bound);
                }
                NodeMember::Node(child) => {
                    node.children.push(self.bind_node(child));
                }
            }
        }
    }

    /// Binds an assignment whose target is a node's own property
    /// (constructor args and node-body assignments): a bare path addresses
    /// the node itself, not the component.
    fn bind_node_assignment(
        &mut self,
        assignment: &PropertyAssignment,
        node: &NodeIr,
    ) -> AssignmentIr {
        if assignment.target.parts.len() == 1
            && !self
                .property_types
                .contains_key(&assignment.target.parts[0].name)
            && !self
                .lookup_local(&assignment.target.parts[0].name)
                .is_some()
        {
            // Own-node property: check the value, record no component edge.
            let value = self.check_expr(&assignment.value);
            if assignment.op == InitOp::TwoWay
                && !matches!(&assignment.value, Expr::Ident { .. } | Expr::Member { .. })
            {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    assignment.span,
                    "two-way binding `<=>` requires a property path on the right-hand side",
                ));
            }
            return AssignmentIr {
                path: assignment
                    .target
                    .parts
                    .iter()
                    .map(|part| return part.name.clone())
                    .collect(),
                target: PropertyTarget::Id(
                    node.id
                        .clone()
                        .unwrap_or_else(|| return "<self>".to_string()),
                    assignment.target.parts[0].name.clone(),
                ),
                kind: match assignment.op {
                    InitOp::Static => InitKind::Static,
                    InitOp::Bind => InitKind::Bind,
                    InitOp::TwoWay => InitKind::TwoWay,
                },
                value,
            };
        }
        return self.bind_assignment(assignment);
    }

    fn bind_assignment(&mut self, assignment: &PropertyAssignment) -> AssignmentIr {
        let value = self.check_expr(&assignment.value);
        if assignment.op == InitOp::TwoWay
            && !matches!(&assignment.value, Expr::Ident { .. } | Expr::Member { .. })
        {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                assignment.span,
                "two-way binding `<=>` requires a property path on the right-hand side",
            ));
        }
        if assignment.op == InitOp::Bind
            && assignment.target.parts.len() == 1
            && self
                .property_types
                .contains_key(&assignment.target.parts[0].name)
        {
            self.record_binding_edge(&assignment.target.parts[0].name, &value, assignment.span);
        }
        return AssignmentIr {
            path: assignment
                .target
                .parts
                .iter()
                .map(|part| return part.name.clone())
                .collect(),
            target: self.resolve_write_target(&assignment.target).unwrap_or(
                PropertyTarget::Component(assignment.target.parts[0].name.clone()),
            ),
            kind: match assignment.op {
                InitOp::Static => InitKind::Static,
                InitOp::Bind => InitKind::Bind,
                InitOp::TwoWay => InitKind::TwoWay,
            },
            value,
        };
    }

    fn bind_handler(&mut self, handler: &Handler) -> HandlerIr {
        let effect = self.bind_effect(&handler.effect);
        return HandlerIr {
            signal: handler.signal.name.clone(),
            effect,
        };
    }

    fn bind_when(&mut self, when: &WhenBlock, node: &NodeIr) -> WhenIr {
        let condition = self.check_expr(&when.condition);
        self.require_bool(&condition, "`when` condition");
        let assignments = when
            .assignments
            .iter()
            .map(|assignment| return self.bind_node_assignment(assignment, node))
            .collect();
        return WhenIr {
            condition,
            assignments,
        };
    }

    fn bind_effect(&mut self, statements: &[Statement]) -> Vec<Effect> {
        self.push_scope();
        let mut effects = Vec::new();
        for statement in statements {
            match statement {
                Statement::Let { name, value, .. } => {
                    let checked = self.check_expr(value);
                    let ty = checked.type_of();
                    self.declare_local(&name.name, ty);
                    effects.push(Effect::Let {
                        name: name.name.clone(),
                        value: checked,
                    });
                }
                Statement::If {
                    condition,
                    then_branch,
                    else_branch,
                    ..
                } => {
                    let checked_condition = self.check_expr(condition);
                    self.require_bool(&checked_condition, "`if` condition");
                    let then_effects = self.bind_effect(then_branch);
                    let else_effects = match else_branch {
                        Some(branch) => self.bind_effect(branch),
                        None => Vec::new(),
                    };
                    effects.push(Effect::If {
                        condition: checked_condition,
                        then_branch: then_effects,
                        else_branch: else_effects,
                    });
                }
                Statement::Assign {
                    target, op, value, ..
                } => {
                    if let Some(effect) = self.bind_assign(target, *op, value) {
                        effects.push(effect);
                    }
                }
                Statement::Emit { signal, .. } => {
                    if !self.signals.contains(&signal.name) {
                        self.diagnostics.push(nui_syntax::Diagnostic::error(
                            signal.span,
                            format!("`emit {}` names an undeclared signal", signal.name),
                        ));
                        continue;
                    }
                    effects.push(Effect::Emit {
                        signal: signal.name.clone(),
                    });
                }
                Statement::Call { callee, args, .. } => {
                    if let Some(effect) = self.bind_call(callee, args) {
                        effects.push(effect);
                    }
                }
            }
        }
        self.pop_scope();
        return effects;
    }

    fn bind_assign(
        &mut self,
        target: &nui_syntax::PropertyPath,
        op: AstAssignOp,
        value: &Expr,
    ) -> Option<Effect> {
        let checked_value = self.check_expr(value);
        let resolved = self.resolve_write_target(target)?;
        let op = match op {
            AstAssignOp::Set => AssignOp::Set,
            AstAssignOp::Add => AssignOp::Add,
            AstAssignOp::Sub => AssignOp::Sub,
        };
        if op != AssignOp::Set {
            let target_ty = self.target_type(&resolved);
            if !target_ty.is_numeric() {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    target.span,
                    format!(
                        "`{}` requires a numeric target, found {}",
                        assign_op_name(op),
                        target_ty
                    ),
                ));
            }
        }
        return Some(Effect::Assign {
            target: resolved,
            op,
            value: checked_value,
        });
    }

    fn bind_call(
        &mut self,
        callee: &nui_syntax::PropertyPath,
        args: &[nui_syntax::CallArg],
    ) -> Option<Effect> {
        if callee.parts.len() == 1 {
            // Single-name call: a host-registered function (plan §5 宿主
            // 互操作), resolved from the runtime registry at emit time.
            let name = &callee.parts[0].name;
            let mut checked_args = Vec::new();
            for arg in args {
                if arg.name.is_some() {
                    self.diagnostics.push(nui_syntax::Diagnostic::error(
                        arg.span,
                        "effect call arguments must be positional".to_string(),
                    ));
                }
                checked_args.push(self.check_expr(&arg.value));
            }
            return Some(Effect::Call {
                callee: vec![name.clone()],
                args: checked_args,
            });
        }
        if callee.parts.len() > 2 {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                callee.span,
                "effect call paths deeper than `id.method` are not supported".to_string(),
            ));
            return None;
        }
        let id = &callee.parts[0];
        if id.name == "root" || id.name == "parent" {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                id.span,
                format!(
                    "`{}.{}`: components have no methods",
                    id.name, callee.parts[1].name
                ),
            ));
            return None;
        }
        if !self.ids.contains_key(&id.name) {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                id.span,
                format!("unknown id `{}`", id.name),
            ));
            return None;
        }
        let mut checked_args = Vec::new();
        for arg in args {
            if arg.name.is_some() {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    arg.span,
                    "effect call arguments must be positional".to_string(),
                ));
            }
            checked_args.push(self.check_expr(&arg.value));
        }
        return Some(Effect::Call {
            callee: callee
                .parts
                .iter()
                .map(|part| return part.name.clone())
                .collect(),
            args: checked_args,
        });
    }

    fn resolve_write_target(
        &mut self,
        target: &nui_syntax::PropertyPath,
    ) -> Option<PropertyTarget> {
        let first = &target.parts[0];
        if target.parts.len() == 1 {
            if self.lookup_local(&first.name).is_some() {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    first.span,
                    format!(
                        "cannot assign to `let` variable `{}`; variables are immutable",
                        first.name
                    ),
                ));
                return None;
            }
            if self.property_types.contains_key(&first.name) {
                return Some(PropertyTarget::Component(first.name.clone()));
            }
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                first.span,
                format!("unknown property `{}`", first.name),
            ));
            return None;
        }
        if target.parts.len() > 2 {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                target.span,
                "property paths deeper than two segments are not supported yet".to_string(),
            ));
            return None;
        }
        let second = &target.parts[1];
        return match first.name.as_str() {
            "root" => Some(PropertyTarget::Root(second.name.clone())),
            "parent" => Some(PropertyTarget::Parent(second.name.clone())),
            "true" | "false" => {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    first.span,
                    format!("`{}` is not a valid assignment target", first.name),
                ));
                None
            }
            _ => {
                if self.machine_states.contains_key(&first.name) {
                    self.diagnostics.push(nui_syntax::Diagnostic::error(
                        first.span,
                        format!(
                            "machine states are read-only; `{}` cannot be assigned",
                            first.name
                        ),
                    ));
                    return None;
                }
                if !self.ids.contains_key(&first.name) {
                    // Not a known id: an attached property of the current
                    // node (`font.size`, `layout.margin`). The host type
                    // table (M3 component descriptors) validates the prefix;
                    // the path survives joined in `AssignmentIr::path`.
                    return Some(PropertyTarget::Id(
                        "<self>".to_string(),
                        target
                            .parts
                            .iter()
                            .map(|part| return part.name.clone())
                            .collect::<Vec<_>>()
                            .join("."),
                    ));
                }
                Some(PropertyTarget::Id(first.name.clone(), second.name.clone()))
            }
        };
    }

    fn target_type(&self, target: &PropertyTarget) -> Type {
        return match target {
            PropertyTarget::Component(name) | PropertyTarget::Root(name) => self
                .property_types
                .get(name)
                .copied()
                .unwrap_or(Type::Unknown),
            PropertyTarget::Parent(_) | PropertyTarget::Id(_, _) => Type::Unknown,
            PropertyTarget::MachineState(_, _) => Type::Bool,
        };
    }

    // -- expression checking ------------------------------------------------

    fn check_expr(&mut self, expr: &Expr) -> TypedExpr {
        return match expr {
            Expr::Int { value, .. } => TypedExpr::Const(Value::Int(*value)),
            Expr::Float { value, .. } => TypedExpr::Const(Value::Float(*value)),
            Expr::Bool { value, .. } => TypedExpr::Const(Value::Bool(*value)),
            Expr::Auto { .. } => TypedExpr::Const(Value::Length(Length::Auto)),
            Expr::Length { length, .. } => TypedExpr::Const(Value::Length(*length)),
            Expr::Duration { duration, .. } => TypedExpr::Const(Value::Duration(*duration)),
            Expr::Color { span, digits } => self.check_color(*span, digits),
            Expr::String { parts, .. } => {
                let mut checked_parts = Vec::new();
                for part in parts {
                    match part {
                        nui_syntax::StrPart::Text(text) => {
                            checked_parts.push(InterpPart::Text(text.clone()));
                        }
                        nui_syntax::StrPart::Interp { expr: inner, .. } => {
                            let checked = self.check_expr(inner);
                            checked_parts.push(InterpPart::Expr(Box::new(checked)));
                        }
                    }
                }
                TypedExpr::Interp {
                    parts: checked_parts,
                }
            }
            Expr::Ident { span, name } => self.check_ident(*span, name),
            Expr::Member { base, name, .. } => self.check_member(base, name),
            Expr::Unary { op, operand, .. } => self.check_unary(*op, operand),
            Expr::Binary { op, lhs, rhs, .. } => self.check_binary(*op, lhs, rhs),
            Expr::Ternary {
                condition,
                then_expr,
                else_expr,
                ..
            } => self.check_ternary(condition, then_expr, else_expr),
            Expr::Call { callee, args, .. } => self.check_call(callee, args),
            Expr::Error { .. } => TypedExpr::Error,
        };
    }

    fn check_color(&mut self, span: Span, digits: &str) -> TypedExpr {
        let valid = COLOR_DIGIT_COUNTS.contains(&digits.len())
            && digits.chars().all(|ch| return ch.is_ascii_hexdigit());
        if !valid {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                span,
                format!(
                    "invalid color literal `#{digits}`: expected {} hex digits",
                    COLOR_DIGIT_COUNTS
                        .iter()
                        .map(|count| return count.to_string())
                        .collect::<Vec<_>>()
                        .join(" or ")
                ),
            ));
            return TypedExpr::Error;
        }
        let Ok(color) = nui_core::Color::from_hex(&format!("#{digits}")) else {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                span,
                format!("invalid color literal `#{digits}`"),
            ));
            return TypedExpr::Error;
        };
        return TypedExpr::Const(Value::Color(color));
    }

    fn check_ident(&mut self, span: Span, name: &str) -> TypedExpr {
        if let Some((index, ty)) = self.lookup_local(name) {
            return TypedExpr::Local {
                name: name.to_string(),
                index,
                ty,
            };
        }
        if let Some(ty) = self.property_types.get(name).copied() {
            return TypedExpr::Property {
                target: PropertyTarget::Component(name.to_string()),
                ty,
            };
        }
        if name == "root" || name == "parent" {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                span,
                format!("`{name}` must be followed by a property access (`.name`)"),
            ));
            return TypedExpr::Error;
        }
        // A bare identifier that resolves to nothing is an enum variant
        // literal (`bold`, `ease-out`); the registry validates the variant
        // name per property at instantiation time.
        return TypedExpr::Const(Value::Enum(name.to_string()));
    }

    fn check_member(&mut self, base: &Expr, name: &Ident) -> TypedExpr {
        let Expr::Ident {
            name: base_name, ..
        } = base
        else {
            // Member access on arbitrary expressions (models in M4) is
            // unchecked; the base is still checked for its own diagnostics.
            let checked_base = self.check_expr(base);
            return TypedExpr::Dynamic {
                base: Box::new(checked_base),
                name: name.name.clone(),
            };
        };
        // Locals (`For` variables, `let` bindings) shadow outer names; the
        // member is unchecked (row fields resolve at runtime against the
        // model the `For` iterates).
        if let Some((index, ty)) = self.lookup_local(base_name) {
            return TypedExpr::Dynamic {
                base: Box::new(TypedExpr::Local {
                    name: base_name.to_string(),
                    index,
                    ty,
                }),
                name: name.name.clone(),
            };
        }
        if base_name == "root" {
            let Some(ty) = self.property_types.get(&name.name).copied() else {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    name.span,
                    format!("`root.{}`: unknown property `{}`", name.name, name.name),
                ));
                return TypedExpr::Error;
            };
            return TypedExpr::Property {
                target: PropertyTarget::Root(name.name.clone()),
                ty,
            };
        }
        if base_name == "parent" {
            return TypedExpr::Property {
                target: PropertyTarget::Parent(name.name.clone()),
                ty: Type::Unknown,
            };
        }
        if let Some(states) = self.machine_states.get(base_name.as_str()) {
            if !states.contains(&name.name) {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    name.span,
                    format!("machine `{base_name}` has no state `{}`", name.name),
                ));
                return TypedExpr::Error;
            }
            return TypedExpr::Property {
                target: PropertyTarget::MachineState(base_name.clone(), name.name.clone()),
                ty: Type::Bool,
            };
        }
        if self.ids.contains_key(base_name) {
            return TypedExpr::Property {
                target: PropertyTarget::Id(base_name.clone(), name.name.clone()),
                ty: Type::Unknown,
            };
        }
        self.diagnostics.push(nui_syntax::Diagnostic::error(
            name.span,
            format!("unknown name `{base_name}`"),
        ));
        return TypedExpr::Error;
    }

    fn check_unary(&mut self, op: UnaryOp, operand: &Expr) -> TypedExpr {
        let checked = self.check_expr(operand);
        let operand_ty = checked.type_of();
        return match op {
            UnaryOp::Neg => {
                if !operand_ty.is_numeric() {
                    self.diagnostics.push(nui_syntax::Diagnostic::error(
                        operand.span(),
                        format!("cannot negate a value of type {operand_ty}"),
                    ));
                    return TypedExpr::Error;
                }
                TypedExpr::Unary {
                    op,
                    operand: Box::new(checked),
                    ty: operand_ty,
                }
            }
            UnaryOp::Not => {
                if operand_ty != Type::Bool && operand_ty != Type::Unknown {
                    self.diagnostics.push(nui_syntax::Diagnostic::error(
                        operand.span(),
                        format!("`!` requires Bool, found {operand_ty}"),
                    ));
                    return TypedExpr::Error;
                }
                TypedExpr::Unary {
                    op,
                    operand: Box::new(checked),
                    ty: Type::Bool,
                }
            }
        };
    }

    fn check_binary(&mut self, op: BinaryOp, lhs: &Expr, rhs: &Expr) -> TypedExpr {
        let checked_lhs = self.check_expr(lhs);
        let checked_rhs = self.check_expr(rhs);
        let lhs_ty = checked_lhs.type_of();
        let rhs_ty = checked_rhs.type_of();
        let span = checked_lhs.span().merge(checked_rhs.span());
        let mismatch = |binder: &mut Self| -> TypedExpr {
            binder.diagnostics.push(nui_syntax::Diagnostic::error(
                span,
                format!(
                    "type mismatch: operator `{}` cannot apply to {} and {}",
                    binary_op_name(op),
                    lhs_ty,
                    rhs_ty
                ),
            ));
            return TypedExpr::Error;
        };
        return match op {
            BinaryOp::Add | BinaryOp::Sub | BinaryOp::Mul | BinaryOp::Div | BinaryOp::Rem => {
                let Some(ty) = arithmetic_result(op, lhs_ty, rhs_ty) else {
                    return mismatch(self);
                };
                TypedExpr::Binary {
                    op,
                    lhs: Box::new(checked_lhs),
                    rhs: Box::new(checked_rhs),
                    ty,
                }
            }
            BinaryOp::Eq | BinaryOp::NotEq => {
                if unify(lhs_ty, rhs_ty).is_none() {
                    return mismatch(self);
                }
                TypedExpr::Binary {
                    op,
                    lhs: Box::new(checked_lhs),
                    rhs: Box::new(checked_rhs),
                    ty: Type::Bool,
                }
            }
            BinaryOp::Lt | BinaryOp::Le | BinaryOp::Gt | BinaryOp::Ge => {
                let ordered = (lhs_ty.is_numeric() && rhs_ty.is_numeric())
                    || (matches!(lhs_ty, Type::Duration | Type::Unknown)
                        && matches!(rhs_ty, Type::Duration | Type::Unknown));
                if !ordered {
                    return mismatch(self);
                }
                TypedExpr::Binary {
                    op,
                    lhs: Box::new(checked_lhs),
                    rhs: Box::new(checked_rhs),
                    ty: Type::Bool,
                }
            }
            BinaryOp::And | BinaryOp::Or => {
                for operand in [&checked_lhs, &checked_rhs] {
                    let operand_ty = operand.type_of();
                    if operand_ty != Type::Bool && operand_ty != Type::Unknown {
                        self.diagnostics.push(nui_syntax::Diagnostic::error(
                            operand.span(),
                            format!(
                                "`{}` requires Bool operands, found {operand_ty}",
                                binary_op_name(op)
                            ),
                        ));
                        return TypedExpr::Error;
                    }
                }
                TypedExpr::Binary {
                    op,
                    lhs: Box::new(checked_lhs),
                    rhs: Box::new(checked_rhs),
                    ty: Type::Bool,
                }
            }
        };
    }

    fn check_ternary(&mut self, condition: &Expr, then_expr: &Expr, else_expr: &Expr) -> TypedExpr {
        let checked_condition = self.check_expr(condition);
        self.require_bool(&checked_condition, "ternary condition");
        let checked_then = self.check_expr(then_expr);
        let checked_else = self.check_expr(else_expr);
        let then_ty = checked_then.type_of();
        let else_ty = checked_else.type_of();
        let Some(ty) = unify(then_ty, else_ty) else {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                checked_then.span().merge(checked_else.span()),
                format!("ternary branches have incompatible types: {then_ty} and {else_ty}"),
            ));
            return TypedExpr::Error;
        };
        return TypedExpr::Ternary {
            condition: Box::new(checked_condition),
            then_expr: Box::new(checked_then),
            else_expr: Box::new(checked_else),
            ty,
        };
    }

    fn check_call(&mut self, callee: &Expr, args: &[nui_syntax::CallArg]) -> TypedExpr {
        let Expr::Ident { span, name } = callee else {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                callee.span(),
                "functions must be called by name".to_string(),
            ));
            let _ = self.check_expr(callee);
            return TypedExpr::Error;
        };
        let Some(func) = Builtin::from_name(name) else {
            if self.extern_functions.contains(name) {
                return self.check_host_call(name, args);
            }
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                *span,
                format!("unknown function `{name}`"),
            ));
            for arg in args {
                let _ = self.check_expr(&arg.value);
            }
            return TypedExpr::Error;
        };
        return match func {
            Builtin::Min | Builtin::Max => self.check_min_max(func, args),
            Builtin::Clamp => self.check_clamp(args),
            Builtin::Tween | Builtin::Spring => self.check_animation(func, args),
            _ if func.is_unary_math() => self.check_unary_math(func, args),
            _ => {
                // Exhaustive safety net: every Builtin is handled above.
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    *span,
                    format!("`{name}` cannot be called here"),
                ));
                TypedExpr::Error
            }
        };
    }

    /// Checks a single-argument math builtin (`sin`, `sqrt`, ..): one
    /// positional numeric argument, always yields `Float`.
    fn check_unary_math(&mut self, func: Builtin, args: &[nui_syntax::CallArg]) -> TypedExpr {
        if args.len() != 1 {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                args.first()
                    .map(|arg| return arg.span)
                    .unwrap_or(Span::new(0, 0)),
                format!(
                    "`{}` takes exactly 1 argument, found {}",
                    func_name(func),
                    args.len()
                ),
            ));
            return TypedExpr::Error;
        }
        let arg = &args[0];
        if arg.name.is_some() {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                arg.span,
                format!("`{}` takes positional arguments only", func_name(func)),
            ));
        }
        let checked = self.check_expr(&arg.value);
        if !checked.type_of().is_numeric() {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                arg.span,
                format!(
                    "`{}` requires a numeric argument, found {}",
                    func_name(func),
                    checked.type_of()
                ),
            ));
            return TypedExpr::Error;
        }
        return TypedExpr::Call {
            func,
            args: vec![checked],
            ty: Type::Float,
        };
    }

    fn check_min_max(&mut self, func: Builtin, args: &[nui_syntax::CallArg]) -> TypedExpr {
        if args.len() != 2 {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                args.first()
                    .map(|arg| return arg.span)
                    .unwrap_or(Span::new(0, 0)),
                format!(
                    "`{}` takes exactly 2 arguments, found {}",
                    func_name(func),
                    args.len()
                ),
            ));
            return TypedExpr::Error;
        }
        let mut checked = Vec::new();
        for arg in args {
            if arg.name.is_some() {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    arg.span,
                    format!("`{}` takes positional arguments only", func_name(func)),
                ));
            }
            checked.push(self.check_expr(&arg.value));
        }
        let Some(ty) = unify(checked[0].type_of(), checked[1].type_of()) else {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                args[0].span.merge(args[1].span),
                format!(
                    "`{}` requires compatible arguments, found {} and {}",
                    func_name(func),
                    checked[0].type_of(),
                    checked[1].type_of()
                ),
            ));
            return TypedExpr::Error;
        };
        return TypedExpr::Call {
            func,
            args: checked,
            ty,
        };
    }

    fn check_clamp(&mut self, args: &[nui_syntax::CallArg]) -> TypedExpr {
        if args.len() != 3 {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                args.first()
                    .map(|arg| return arg.span)
                    .unwrap_or(Span::new(0, 0)),
                format!("`clamp` takes exactly 3 arguments, found {}", args.len()),
            ));
            return TypedExpr::Error;
        }
        let mut checked = Vec::new();
        for arg in args {
            if arg.name.is_some() {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    arg.span,
                    "`clamp` takes positional arguments only".to_string(),
                ));
            }
            checked.push(self.check_expr(&arg.value));
        }
        let all_numeric = checked
            .iter()
            .all(|expr| return expr.type_of().is_numeric());
        if !all_numeric {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                args[0].span.merge(args[2].span),
                "`clamp` requires numeric arguments".to_string(),
            ));
            return TypedExpr::Error;
        }
        let value_ty = checked[0].type_of();
        return TypedExpr::Call {
            func: Builtin::Clamp,
            args: checked,
            ty: value_ty,
        };
    }

    /// Checks a call to a host-registered function: positional arguments
    /// only; the result type is `Unknown` (the host owns typing).
    fn check_host_call(&mut self, name: &str, args: &[nui_syntax::CallArg]) -> TypedExpr {
        let mut checked_args = Vec::new();
        for arg in args {
            if arg.name.is_some() {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    arg.span,
                    format!("`{name}` takes positional arguments only"),
                ));
            }
            checked_args.push(self.check_expr(&arg.value));
        }
        return TypedExpr::HostCall {
            name: name.to_string(),
            args: checked_args,
            ty: Type::Unknown,
        };
    }

    fn check_animation(&mut self, func: Builtin, args: &[nui_syntax::CallArg]) -> TypedExpr {
        let Some((first, rest)) = args.split_first() else {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                Span::new(0, 0),
                format!("`{}` takes a value argument", func_name(func)),
            ));
            return TypedExpr::Error;
        };
        if first.name.is_some() {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                first.span,
                format!(
                    "the first `{}` argument must be positional",
                    func_name(func)
                ),
            ));
        }
        let value = self.check_expr(&first.value);
        let value_ty = value.type_of();
        let mut checked_args = vec![value];
        for arg in rest {
            let Some(name) = &arg.name else {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    arg.span,
                    format!(
                        "extra arguments to `{}` must be named (`duration = ...`)",
                        func_name(func)
                    ),
                ));
                let _ = self.check_expr(&arg.value);
                continue;
            };
            let expected = match (func, name.name.as_str()) {
                (Builtin::Tween, "duration") => Some(Type::Duration),
                (Builtin::Tween, "easing") => Some(Type::String),
                (Builtin::Spring, "stiffness") | (Builtin::Spring, "damping") => Some(Type::Float),
                _ => None,
            };
            let Some(expected_ty) = expected else {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    name.span,
                    format!("unknown argument `{}` for `{}`", name.name, func_name(func)),
                ));
                let _ = self.check_expr(&arg.value);
                continue;
            };
            let checked = self.check_expr(&arg.value);
            let arg_ty = checked.type_of();
            if unify(expected_ty, arg_ty).is_none() {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    arg.span,
                    format!(
                        "`{} = ...` of `{}` must be {expected_ty}, found {arg_ty}",
                        name.name,
                        func_name(func)
                    ),
                ));
                continue;
            }
            checked_args.push(checked);
        }
        return TypedExpr::Call {
            func,
            args: checked_args,
            ty: value_ty,
        };
    }

    // -- helpers -------------------------------------------------------------

    /// Records a reactive edge `target <- reads…` when the value reads
    /// component properties, for the static cycle check. Edges through
    /// `root`/ids/machines are skipped (they are element-scoped, not
    /// component-property nodes of this graph).
    fn record_binding_edge(&mut self, target: &str, value: &TypedExpr, span: Span) {
        let mut sources = Vec::new();
        collect_property_reads(value, &mut sources);
        for source in sources {
            self.binding_edges.push((source, target.to_string(), span));
        }
    }

    /// Static cycle detection over component-property reactive edges
    /// (plan §3.4 first line of defense; the runtime evaluation-depth cap
    /// is the second). Reports one diagnostic per cycle with the chain
    /// (`a <- b <- a`).
    fn check_binding_cycles(&mut self) {
        let mut adjacency: HashMap<String, Vec<String>> = HashMap::new();
        for (source, target, _) in &self.binding_edges {
            adjacency
                .entry(source.clone())
                .or_default()
                .push(target.clone());
        }
        let mut visited: HashSet<String> = HashSet::new();
        for (start, _, span) in &self.binding_edges {
            if !visited.insert(start.clone()) {
                continue;
            }
            let mut path: Vec<String> = vec![start.clone()];
            let mut on_path: HashSet<String> = HashSet::from([start.clone()]);
            let mut stack: Vec<Vec<String>> =
                vec![adjacency.get(start).cloned().unwrap_or_default()];
            while let Some(successors) = stack.last_mut() {
                let Some(next) = successors.pop() else {
                    stack.pop();
                    if let Some(left) = path.pop() {
                        on_path.remove(&left);
                    }
                    continue;
                };
                if on_path.contains(&next) {
                    let position = path
                        .iter()
                        .position(|name| return *name == next)
                        .unwrap_or(0);
                    let mut chain: Vec<String> = path[position..].to_vec();
                    chain.push(next.clone());
                    self.diagnostics.push(
                        nui_syntax::Diagnostic::error(
                            *span,
                            format!("reactive binding cycle: {}", chain.join(" <- ")),
                        )
                        .with_note("break the cycle by making one of these bindings static `=`"),
                    );
                    continue;
                }
                if visited.contains(&next) {
                    continue;
                }
                visited.insert(next.clone());
                on_path.insert(next.clone());
                path.push(next.clone());
                stack.push(adjacency.get(&next).cloned().unwrap_or_default());
            }
        }
    }

    fn require_bool(&mut self, expr: &TypedExpr, what: &str) {
        let ty = expr.type_of();
        if ty != Type::Bool && ty != Type::Unknown {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                expr.span(),
                format!("{what} must be Bool, found {ty}"),
            ));
        }
    }

    fn push_scope(&mut self) {
        self.scopes.push(Vec::new());
    }

    fn pop_scope(&mut self) {
        self.scopes.pop();
    }

    fn declare_local(&mut self, name: &str, ty: Type) {
        if let Some(frame) = self.scopes.last_mut() {
            frame.push((name.to_string(), ty));
            self.local_count += 1;
        }
    }

    fn lookup_local(&self, name: &str) -> Option<(usize, Type)> {
        let mut slot = self.local_count;
        for frame in self.scopes.iter().rev() {
            for (local_name, ty) in frame.iter().rev() {
                slot -= 1;
                if local_name == name {
                    return Some((slot, *ty));
                }
            }
        }
        return None;
    }
}

/// Collects component-property reads from an expression (for the binding
/// cycle graph). `Root` reads resolve to the same property namespace of the
/// component, so they participate too; `Id`/`Parent`/machine reads do not.
fn collect_property_reads(expr: &TypedExpr, out: &mut Vec<String>) {
    match expr {
        TypedExpr::Property { target, .. } => match target {
            PropertyTarget::Component(name) | PropertyTarget::Root(name) => {
                out.push(name.clone());
            }
            PropertyTarget::Parent(_) | PropertyTarget::Id(_, _) => {}
            PropertyTarget::MachineState(_, _) => {}
        },
        TypedExpr::Unary { operand, .. } => collect_property_reads(operand, out),
        TypedExpr::Binary { lhs, rhs, .. } => {
            collect_property_reads(lhs, out);
            collect_property_reads(rhs, out);
        }
        TypedExpr::Ternary {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            collect_property_reads(condition, out);
            collect_property_reads(then_expr, out);
            collect_property_reads(else_expr, out);
        }
        TypedExpr::Call { args, .. } => {
            for arg in args {
                collect_property_reads(arg, out);
            }
        }
        TypedExpr::HostCall { args, .. } => {
            for arg in args {
                collect_property_reads(arg, out);
            }
        }
        TypedExpr::Interp { parts } => {
            for part in parts {
                if let InterpPart::Expr(inner) = part {
                    collect_property_reads(inner, out);
                }
            }
        }
        TypedExpr::Dynamic { base, .. } => collect_property_reads(base, out),
        TypedExpr::Const(_) | TypedExpr::Local { .. } | TypedExpr::Error => {}
    }
}

/// Result type of an arithmetic operator, including Length/Duration rules.
fn arithmetic_result(op: BinaryOp, lhs: Type, rhs: Type) -> Option<Type> {
    match op {
        BinaryOp::Add => {
            // String concatenation: `+` joins String/Enum values (runtime
            // `add_values` implements this); Unknown defers to the other side.
            let stringy = |ty: Type| return matches!(ty, Type::String | Type::Enum);
            if (stringy(lhs) && stringy(rhs))
                || (stringy(lhs) && rhs == Type::Unknown)
                || (lhs == Type::Unknown && stringy(rhs))
            {
                return Some(Type::String);
            }
            if matches!((lhs, rhs), (Type::Length, Type::Length)) {
                return Some(Type::Length);
            }
            if matches!((lhs, rhs), (Type::Duration, Type::Duration)) {
                return Some(Type::Duration);
            }
        }
        BinaryOp::Sub => {
            if matches!((lhs, rhs), (Type::Length, Type::Length)) {
                return Some(Type::Length);
            }
            if matches!((lhs, rhs), (Type::Duration, Type::Duration)) {
                return Some(Type::Duration);
            }
        }
        BinaryOp::Mul => {
            if lhs == Type::Length && rhs.is_numeric() {
                return Some(Type::Length);
            }
            if lhs.is_numeric() && rhs == Type::Length {
                return Some(Type::Length);
            }
            if lhs == Type::Duration && rhs.is_numeric() {
                return Some(Type::Duration);
            }
        }
        BinaryOp::Div => {
            if lhs == Type::Length && rhs.is_numeric() {
                return Some(Type::Length);
            }
            if lhs == Type::Duration && rhs.is_numeric() {
                return Some(Type::Duration);
            }
        }
        BinaryOp::Rem => {}
        _ => return None,
    }
    let result = unify(lhs, rhs)?;
    if !result.is_numeric() {
        return None;
    }
    return Some(result);
}

fn binary_op_name(op: BinaryOp) -> &'static str {
    return match op {
        BinaryOp::Add => "+",
        BinaryOp::Sub => "-",
        BinaryOp::Mul => "*",
        BinaryOp::Div => "/",
        BinaryOp::Rem => "%",
        BinaryOp::Eq => "==",
        BinaryOp::NotEq => "!=",
        BinaryOp::Lt => "<",
        BinaryOp::Le => "<=",
        BinaryOp::Gt => ">",
        BinaryOp::Ge => ">=",
        BinaryOp::And => "&&",
        BinaryOp::Or => "||",
    };
}

fn func_name(func: Builtin) -> &'static str {
    return match func {
        Builtin::Min => "min",
        Builtin::Max => "max",
        Builtin::Clamp => "clamp",
        Builtin::Sin => "sin",
        Builtin::Cos => "cos",
        Builtin::Tan => "tan",
        Builtin::Sqrt => "sqrt",
        Builtin::Abs => "abs",
        Builtin::Floor => "floor",
        Builtin::Ceil => "ceil",
        Builtin::Tween => "tween",
        Builtin::Spring => "spring",
    };
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::bytecode::PropertyTarget;
    use crate::compile;

    const COUNTER: &str = r#"
        component Counter {
            property count: Int = 0
            signal resetRequested

            Window(id = root, width = 420dp, height = 300dp) {
                title <- "count: {count}"

                Text(content <- "n: {count}", font.size = 20dp)

                Button(label = "+1", enabled <- count < 10) {
                    on click => count += 1
                }

                Button(label = "reset") {
                    on click => {
                        emit resetRequested
                    }
                }

                when count >= 10 {
                    opacity = 0.6
                }
            }
        }
    "#;

    #[test]
    fn compiles_counter_example_cleanly() {
        let outcome = compile(COUNTER);
        assert!(
            outcome.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            outcome.diagnostics
        );
        let component = &outcome.document.components[0];
        assert_eq!(component.name, "Counter");
        assert_eq!(component.signals, vec!["resetRequested".to_string()]);
        let property = &component.properties[0];
        assert_eq!(property.name, "count");
        assert_eq!(property.ty, Type::Int);
    }

    #[test]
    fn compiles_machine_and_state_access() {
        let source = r#"
            component Player {
                property canStop: Bool = true
                signal play
                signal pause

                machine playback {
                    state stopped
                    state playing
                    on play from stopped => playing
                    on pause from playing when canStop => stopped
                }

                Text(content <- playback.playing ? "playing" : "stopped")
            }
        "#;
        let outcome = compile(source);
        assert!(
            outcome.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            outcome.diagnostics
        );
        let component = &outcome.document.components[0];
        assert_eq!(component.machines.len(), 1);
        assert_eq!(component.machines[0].transitions.len(), 2);
        assert!(component.machines[0].transitions[1].guard.is_some());
    }

    #[test]
    fn reports_default_type_mismatch() {
        let outcome = compile("component A { property flag: Bool = 3 }");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("expected Bool, found Int"))
        );
    }

    #[test]
    fn accepts_model_type_and_rejects_model_literal() {
        let outcome = compile("component A { property items: Model For(item in root.items) {} }");
        assert!(
            outcome.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            outcome.diagnostics
        );
        let outcome = compile("component A { property items: Model = 1 }");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("expected Model, found Int")),
            "expected a Model type mismatch, got {:?}",
            outcome.diagnostics
        );
    }

    #[test]
    fn reports_uninferrable_property() {
        let outcome = compile("component A { property x = parent.thing }");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("cannot infer the type"))
        );
    }

    #[test]
    fn infers_type_from_default() {
        let outcome = compile("component A { property x = 1 + 2 }");
        assert!(outcome.diagnostics.is_empty());
        assert_eq!(outcome.document.components[0].properties[0].ty, Type::Int);
    }

    #[test]
    fn reports_invalid_color_literal() {
        let outcome = compile("component A { Text(fill = #12345) {} }");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("invalid color literal"))
        );
    }

    #[test]
    fn rejects_two_way_property_declaration() {
        let outcome = compile("component A { property x: Int <=> 3 }");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("two-way binding is not allowed"))
        );
    }

    #[test]
    fn rejects_two_way_binding_to_expression() {
        let outcome = compile("component A { Text(text <=> count + 1) {} }");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("requires a property path"))
        );
    }

    #[test]
    fn reports_arithmetic_type_mismatch() {
        let outcome =
            compile("component A { property count: Int = 0 Text(x <- count + \"x\") {} }");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("operator `+` cannot apply"))
        );
    }

    #[test]
    fn reports_ternary_branch_mismatch() {
        let outcome =
            compile("component A { property flag: Bool = true Text(x <- flag ? 1 : \"s\") {} }");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("ternary branches"))
        );
    }

    #[test]
    fn reports_unknown_transition_state_and_event() {
        let outcome =
            compile("component A { machine m { state a on tick from a => b on go from a => a } }");
        let messages: Vec<&str> = outcome
            .diagnostics
            .iter()
            .map(|d| return d.message.as_str())
            .collect();
        assert!(
            messages
                .iter()
                .any(|m| return m.contains("not a declared signal"))
        );
        assert!(
            messages
                .iter()
                .any(|m| return m.contains("unknown target state `b`"))
        );
    }

    #[test]
    fn reports_emit_of_undeclared_signal() {
        let outcome = compile("component A { Button { on click => { emit boom } } }");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("undeclared signal"))
        );
    }

    #[test]
    fn checks_animation_argument_types() {
        let good = compile(
            "component A { property count: Int = 0 Text(x <- tween(count, duration = 200ms, easing = ease-out)) {} }",
        );
        assert!(good.diagnostics.is_empty());

        let bad = compile(
            "component A { property count: Int = 0 Text(x <- tween(count, duration = 1)) {} }",
        );
        assert!(
            bad.diagnostics
                .iter()
                .any(|d| return d.message.contains("must be Duration"))
        );
    }

    #[test]
    fn validates_effect_call_ids() {
        let bad = compile("component A { Button { on click => timer.start() } }");
        assert!(
            bad.diagnostics
                .iter()
                .any(|d| return d.message.contains("unknown id `timer`"))
        );

        let good =
            compile("component A { Timer(id = timer) { Button { on click => timer.start() } } }");
        assert!(good.diagnostics.is_empty());
    }

    #[test]
    fn rejects_assignment_to_let_variable() {
        let outcome = compile("component A { Button { on click => { let n = 1 n = 2 } } }");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("cannot assign to `let` variable"))
        );
    }

    #[test]
    fn rejects_compound_assignment_on_non_numeric() {
        let outcome = compile(
            "component A { property label: String = \"x\" Button { on click => label += 1 } }",
        );
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("requires a numeric target"))
        );
    }

    #[test]
    fn scopes_for_variables_to_their_subtree() {
        let outcome = compile(
            "component A { property items: Int = 0 For(item in root.items) { Row(key = item.id) {} } }",
        );
        assert!(
            outcome.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            outcome.diagnostics
        );
    }

    #[test]
    fn resolves_root_and_machine_members() {
        let outcome = compile(COUNTER);
        let component = &outcome.document.components[0];
        // Assignments from constructor args (`width`, `height`) come first;
        // the `title` body binding follows them.
        let title = &component.roots[0].assignments[2];
        let TypedExpr::Interp { parts } = &title.value else {
            panic!("expected an interpolated string");
        };
        let crate::bytecode::InterpPart::Expr(expr) = &parts[1] else {
            panic!("expected an interpolation expression");
        };
        let TypedExpr::Property { target, ty } = &**expr else {
            panic!("expected a property read");
        };
        assert_eq!(*target, PropertyTarget::Component("count".to_string()));
        assert_eq!(*ty, Type::Int);
    }

    #[test]
    fn reports_duplicate_ids() {
        let outcome = compile("component A { Button(id = b) {} Button(id = b) {} }");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("duplicate id `b`"))
        );
    }

    #[test]
    fn supports_length_and_duration_arithmetic() {
        let good = compile(
            "component A { property w: Length = 420dp Text(x <- w * 0.5, y <- w + 10dp) {} }",
        );
        assert!(
            good.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            good.diagnostics
        );
    }

    #[test]
    fn detects_reactive_binding_cycle() {
        let outcome = compile("component A { property a: Int <- b property b: Int <- a }");
        let cycle = outcome
            .diagnostics
            .iter()
            .find(|d| return d.message.contains("reactive binding cycle"));
        assert!(cycle.is_some(), "expected a cycle diagnostic");
        let cycle = cycle.unwrap();
        assert!(
            cycle.message.contains("a <- b <- a") || cycle.message.contains("b <- a <- b"),
            "chain should name the cycle, got: {}",
            cycle.message
        );
    }

    #[test]
    fn detects_self_binding_cycle() {
        let outcome = compile("component A { property n: Int <- n + 1 }");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("reactive binding cycle"))
        );
    }

    #[test]
    fn detects_three_node_cycle_via_root() {
        let outcome = compile("component A { property a: Int <- b property b: Int <- root.a }");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("reactive binding cycle"))
        );
    }

    #[test]
    fn acyclic_reactive_chain_is_accepted() {
        let outcome = compile(
            "component A { property a: Int = 1 property b: Int <- a + 1 property c: Int <- b + 1 }",
        );
        assert!(
            outcome.diagnostics.is_empty(),
            "unexpected diagnostics: {:?}",
            outcome.diagnostics
        );
    }

    #[test]
    fn enum_variant_literals_type_check() {
        let good = compile("component A { Text(font.weight = bold) {} }");
        assert!(good.diagnostics.is_empty(), "{:?}", good.diagnostics);

        let tween = compile(
            "component A { property count: Int = 0 Text(x <- tween(count, duration = 200ms, easing = ease-out)) {} }",
        );
        assert!(tween.diagnostics.is_empty(), "{:?}", tween.diagnostics);
    }

    #[test]
    fn renders_rustc_style_diagnostic_end_to_end() {
        let outcome = compile("component A { property flag: Bool = 3 }");
        assert_eq!(outcome.diagnostics.len(), 1);
        let rendered = nui_syntax::render_diagnostic(
            "component A { property flag: Bool = 3 }",
            "main.nui",
            &outcome.diagnostics[0],
        );
        assert!(rendered.contains("error: type mismatch"), "{rendered}");
        assert!(rendered.contains("main.nui:1:"), "{rendered}");
        assert!(rendered.contains("|"), "{rendered}");
        assert!(rendered.contains("^"), "{rendered}");
    }

    #[test]
    fn list_view_takes_for_binding() {
        let outcome = compile(
            "component A { property rows: Model For(item in root.rows) {} ListView(it in root.rows) { Row(key = it.id) {} } }",
        );
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
        let component = &outcome.document.components[0];
        // The second root is a ListView with the binding recorded.
        let list = component
            .roots
            .iter()
            .find(|node| return node.ty == "ListView")
            .expect("ListView node");
        assert!(list.for_binding.is_some());
        assert_eq!(list.for_binding.as_ref().unwrap().variable, "it");
    }

    #[test]
    fn list_view_scope_variants() {
        for (label, source) in [
            (
                "clause-only-for",
                "component B { property rows: Model Window(id = root) { For(item in root.rows) { Rectangle(height = 10dp) { Text(label <- item.label) {} } } } }",
            ),
            (
                "clause-only-direct",
                "component B { property rows: Model Window(id = root) { For(item in root.rows) { Text(label <- item.label) {} } } }",
            ),
            (
                "id-arg",
                "component B { property rows: Model Window(id = root) { ListView(item in root.rows, id = list) { Rectangle(height = 10dp) { Text(label <- item.label) {} } } } }",
            ),
            (
                "prop-arg",
                "component B { property rows: Model Window(id = root) { ListView(item in root.rows, height = 50dp) { Rectangle(height = 10dp) { Text(label <- item.label) {} } } } }",
            ),
        ] {
            let outcome = compile(source);
            assert!(
                outcome.diagnostics.is_empty(),
                "{label}: {:?}",
                outcome.diagnostics
            );
        }
    }

    #[test]
    fn list_view_with_mixed_args_resolves_item() {
        let outcome = compile(
            "component Big { property rows: Model Window(id = root) { ListView(item in root.rows, id = list, height = 50dp, row_height = 10dp) { Rectangle(height = 10dp) { Text(label <- item.label) {} } } } }",
        );
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    }

    #[test]
    fn never_panics_on_garbage() {
        let outcome = compile("component A { ??? } component B { property = = = }");
        assert!(!outcome.diagnostics.is_empty());
    }

    #[test]
    fn math_builtins_accept_numeric_arguments() {
        let outcome = compile(
            "component A { property p: Float = 4.0 Window(id = root, width = 200dp, height = 100dp) { Rectangle(width <- sqrt(p), height <- abs(-2.5), x <- sin(3.14), y <- floor(1.9)) {} } }",
        );
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    }

    #[test]
    fn math_builtins_reject_argument_count_and_type() {
        let too_many = compile(
            "component B { Window(id = root, width = 100dp, height = 100dp) { Rectangle(width <- sin(1, 2)) {} } }",
        );
        assert!(
            !too_many.diagnostics.is_empty(),
            "arity error expected: {:?}",
            too_many.diagnostics
        );
        let wrong_type = compile(
            "component C { Window(id = root, width = 100dp, height = 100dp) { Rectangle(width <- sqrt(\"x\")) {} } }",
        );
        assert!(
            !wrong_type.diagnostics.is_empty(),
            "type error expected: {:?}",
            wrong_type.diagnostics
        );
    }

    #[test]
    fn temporary_demo_source_check() {
        let demo = r#"
component RotationGradient {
    property spin: Float = 45

    Window(id = root) {
        Column(id = content, spacing = 16dp, padding = 24dp) {
            Text(content = "rotation + gradient + math builtins", font.size = 14dp)

            Rectangle(id = diamond, width = 80dp, height = 80dp, radius = 14dp,
                      fill = #e05555, rotation <- spin) {
                on click => spin += 15
            }

            Rectangle(id = bar, height = 40dp,
                      gradient.from = #ff5544, gradient.to = #4466ff,
                      gradient.angle = 0)

            Rectangle(id = pill, height = 40dp, radius = 20dp,
                      gradient.from = #ffb347, gradient.to = #7a4c9e)

            Text(content <- "floor(sin(1.57) * 100) / 100 = {floor(sin(1.57) * 100.0) / 100.0}")
        }
    }
}
"#;
        let outcome = compile(demo);
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    }

    #[test]
    fn temporary_strokes_demo_source_check() {
        let demo = r#"
component Strokes {
    Window(id = root) {
        Column(id = content, spacing = 20dp, padding = 24dp) {
            Text(content = "polyline + arc stroking", font.size = 14dp)

            Polyline(id = chart, width = 292dp, height = 110dp,
                     points = "0,95 48,60 96,74 144,28 192,46 240,14 292,30",
                     stroke.width = 3dp, color = #55aaee)

            Polyline(id = divider, width = 292dp, height = 2dp,
                     points = "0,0 292,0",
                     stroke.width = 2dp, stroke.cap = "butt", color = #444a55)

            Arc(id = ring, width = 150dp, height = 150dp,
                cx = 75, cy = 75, radius = 60,
                start = -90, end = 180,
                stroke.width = 8dp, stroke.cap = "round", color = #e05555)
        }
    }
}
"#;
        let outcome = compile(demo);
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    }

    #[test]
    fn temporary_waveline_demo_source_check() {
        let demo = r#"
component WavelineDemo {
    property tick: Float = 0

    Window(id = root) {
        Column(id = content, spacing = 16dp, padding = 24dp) {
            Text(content = "waveline — an infinite timer drives the phase", font.size = 14dp)

            Timer(id = ticker, interval = 33ms, running = true) {
                on timer => tick += 11
            }

            Waveline(id = wave, width = 292dp, height = 80dp,
                     amplitude <- 16 + 7 * sin(tick * 0.017),
                     frequency = 3, phase <- tick,
                     stroke.width = 3dp, color = #55aaee)

            Waveline(id = twin, width = 292dp, height = 120dp,
                     amplitude = 26, frequency = 2.5, phase <- tick,
                     mirror = true, stroke.width = 2dp, color = #e05555)

            Waveline(id = samples, width = 292dp, height = 70dp,
                     levels = "0.2 0.45 0.85 0.5 0.75 0.3 0.6 0.25 0.55 0.4 0.7 0.3 0.5 0.65 0.35 0.55",
                     amplitude = 26, stroke.width = 2dp, color = #7bc47f)
        }
    }
}
"#;
        let outcome = compile(demo);
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    }

    #[test]
    fn temporary_paths_demo_source_check() {
        let demo = r#"
component Paths {
    Window(id = root) {
        Column(id = content, spacing = 20dp, padding = 24dp) {
            Text(content = "path fill + stroke", font.size = 14dp)

            Path(id = triangle, width = 292dp, height = 140dp,
                 d = "M 10 10 L 282 10 L 146 130 Z",
                 fill = #ff5544, stroke.width = 3dp, stroke.color = #ffffff)

            Path(id = star, width = 292dp, height = 120dp,
                 d = "M 146 10 L 157.8 43.8 L 193.6 44.5 L 165 66.2 L 175.4 100.5 L 146 80 L 116.6 100.5 L 127 66.2 L 98.4 44.5 L 134.2 43.8 Z",
                 fill = #7bc47f)

            Path(id = curve, width = 292dp, height = 110dp,
                 d = "M 40 70 C 40 25 100 25 146 45 S 240 100 252 60 Q 260 20 200 18 T 60 40 Z",
                 fill = #55aaee, stroke.width = 2dp, stroke.color = #ddeeff)
        }
    }
}
"#;
        let outcome = compile(demo);
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    }

    #[test]
    fn temporary_canvas_demo_source_check() {
        let demo = r#"
component CanvasDemo {
    Window(id = root) {
        Column(id = content, spacing = 16dp, padding = 24dp) {
            Text(content = "canvas — the host paints through a command buffer", font.size = 14dp)

            Canvas(id = chart, width = 240dp, height = 230dp, clip = true)
        }
    }
}
"#;
        let outcome = compile(demo);
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    }

    #[test]
    fn handlers_are_allowed_in_node_arguments() {
        // `on signal => ...` used to be a body-only member: an argument
        // list rejected it with "expected an identifier, found keyword
        // `on`". Compact widgets need it next to their properties.
        let outcome = compile(
            r#"component A { property c: Int = 0 Window(id = root) { Button(id = b, label = "x", variant = "primary", on click => c += 1) } }"#,
        );
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
        let node = &outcome.document.components[0].roots[0];
        let button = &node.children[0];
        assert_eq!(button.ty, "Button");
        assert_eq!(button.handlers.len(), 1);
        assert_eq!(button.handlers[0].signal, "click");
        // The argument assignments still bind (`variant` among them).
        assert!(button.assignments.iter().any(|assignment| {
            return assignment.path == vec!["variant".to_string()];
        }));
    }

    #[test]
    fn argument_and_body_handlers_coexist() {
        let outcome = compile(
            r#"component A { property c: Int = 0 Window(id = root) { Button(on click => c += 1, label = "x") { on press => c -= 1 } } }"#,
        );
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
        let button = &outcome.document.components[0].roots[0].children[0];
        let signals: Vec<&str> = button
            .handlers
            .iter()
            .map(|handler| return handler.signal.as_str())
            .collect();
        // Argument handlers precede body handlers.
        assert_eq!(signals, vec!["click", "press"]);
        assert_eq!(
            button
                .assignments
                .iter()
                .map(|assignment| return assignment.path.join("."))
                .collect::<Vec<_>>(),
            vec!["label"]
        );
    }

    #[test]
    fn temporary_widget_demo_source_check() {
        let demo = r#"
component Widgets {
    property listOpen: Bool = true

    Window(id = root) {
        Column(id = content, spacing = 16dp, padding = 24dp) {
            Text(content = "widget foundation — states, capture, keyboard")

            Button(id = primary, label = "Primary", variant = "primary",
                   on click => listOpen = !listOpen)

            Button(id = danger, label = "Danger", variant = "danger",
                   icon = "!", enabled <- listOpen)

            CheckBox(id = agree, label = "Enable the dialog", checked = true)
            Switch(id = toggle, label = "Switch", checked <- agree.checked)
            Slider(id = volume, min = 0, max = 100, value = 40, step = 5)
            RadioButton(id = optionA, label = "Fahrenheit", group = "units", selected = true)

            Dialog(id = sheet, open <- !listOpen, title = "Are you sure?")
        }
    }
}
"#;
        let outcome = compile(demo);
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    }

    #[test]
    fn the_widgets_gallery_page_compiles_clean() {
        // The gallery's `widgets` page (see
        // `crates/nui/examples/gallery/pages/widgets.nui`) is the living
        // copy of the widget set; if it stops compiling, the shipped demo
        // is broken and this test says so without needing a display. The
        // fragment is a single top-level element binding only its own
        // root's properties, so the smallest hosting window is enough.
        let demo = format!(
            "component WidgetsPage {{\n    Window(id = root) {{\n{}\n    }}\n}}\n",
            include_str!("../../nui/examples/gallery/pages/widgets.nui")
        );
        let outcome = compile(&demo);
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    }
}
