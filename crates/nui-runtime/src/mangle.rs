//! Per-instance rewriting of a component's compiled IR.
//!
//! A component is compiled once and instantiated many times, but the
//! runtime's name resolution is global: [`ElementTree::ids`] is one flat
//! `name -> element` table, and a bare property read resolves through
//! [`ElementTree::resolve_target`]. Two instances of a component that both
//! declare `id = label` would therefore fight over one slot.
//!
//! Rather than teach the evaluator a lexical scope chain — which would put
//! a tree walk on the hot path of every property read — each instance gets
//! its own **name prefix** and the component's IR is rewritten to use it.
//! `Chip`'s `id = label` becomes `i3::label` for the fourth instance, so
//! the flat table stays flat and correct.
//!
//! Two things are rewritten, and both are "make the implicit explicit":
//!
//! - **ids.** Every `PropertyTarget::Id(name, ..)` naming a component-local
//!   id gains the prefix.
//! - **the component's own state.** A bare `count` (or `root.dark`) inside
//!   a component means "this component's property", which the evaluator
//!   resolves to the document's *first* root — right for a single-instance
//!   document, wrong the moment there are two. Inside an instance these
//!   become explicit references to the instance element, registered under
//!   `self_id`.
//!
//! The rewrite is destructive and per-instance, so the runtime clones the
//! component's IR before mangling it. That is the price of keeping
//! resolution global, and it is paid once per instantiation.

use nui_compiler::{
    AssignmentIr, Effect, MachineIr, NodeIr, PropertyTarget, StateIr, TransitionIr, TypedExpr,
    WhenIr,
};

/// The ids one component *owns* — the set the id rewrite is allowed to touch.
///
/// A component's ids belong to each instance and are rewritten into its
/// namespace. A *document-level* id — one an entry component declares, which
/// another component reads (D20) — belongs to the document and must survive
/// the rewrite untouched, or a reference to it becomes `i29::shell` and every
/// binding through it fails to resolve at run time.
pub type OwnIds = std::collections::HashSet<String>;

/// Rewrites one instance of a component's subtree.
///
/// `prefix` namespaces the ids the component declares (`format!("{prefix}label")`),
/// and `self_id` is the name the instance element is registered under — the
/// address every implicit property read inside the component resolves to.
///
/// The instance element does not exist yet when this runs (the subtree is
/// what creates it), so `self_id` is chosen by the caller and registered
/// afterwards. Keeping the name and the registration together is the
/// caller's job precisely so this function stays pure.
pub(crate) fn mangle_node(node: &mut NodeIr, prefix: &str, self_id: &str, own: &OwnIds) {
    // The element's own `id` is what `build_id_index` registers, so it is
    // the reference that actually has to be namespaced — the
    // `PropertyTarget::Id` entries below are the ones that have to *find*
    // it.
    if let Some(id) = &node.id {
        node.id = Some(format!("{prefix}{id}"));
    }
    for assignment in &mut node.assignments {
        mangle_assignment(assignment, prefix, self_id, own);
    }
    for when in &mut node.when_blocks {
        mangle_when(when, prefix, self_id, own);
    }
    for handler in &mut node.handlers {
        mangle_effects(&mut handler.effect, prefix, self_id, own);
    }
    if let Some(binding) = &mut node.for_binding {
        mangle_expr(&mut binding.iterable, prefix, self_id, own);
    }
    for child in &mut node.children {
        mangle_node(child, prefix, self_id, own);
    }
}

/// Rewrites one component's state machines for an instance.
///
/// A machine's guards and enter/exit effects read and write exactly the
/// same names as any other effect, so they need the same rewrite; leaving
/// them out would make a state machine inside a component drive the
/// *document's* first root.
pub(crate) fn mangle_machine(machine: &mut MachineIr, prefix: &str, self_id: &str, own: &OwnIds) {
    for state in &mut machine.states {
        mangle_state(state, prefix, self_id, own);
    }
    for transition in &mut machine.transitions {
        mangle_transition(transition, prefix, self_id, own);
    }
}

fn mangle_state(state: &mut StateIr, prefix: &str, self_id: &str, own: &OwnIds) {
    mangle_effects(&mut state.enter, prefix, self_id, own);
    mangle_effects(&mut state.exit, prefix, self_id, own);
}

fn mangle_transition(transition: &mut TransitionIr, prefix: &str, self_id: &str, own: &OwnIds) {
    if let Some(guard) = &mut transition.guard {
        mangle_expr(guard, prefix, self_id, own);
    }
}

fn mangle_assignment(assignment: &mut AssignmentIr, prefix: &str, self_id: &str, own: &OwnIds) {
    mangle_target(&mut assignment.target, prefix, self_id, own);
    mangle_expr(&mut assignment.value, prefix, self_id, own);
}

fn mangle_when(when: &mut WhenIr, prefix: &str, self_id: &str, own: &OwnIds) {
    mangle_expr(&mut when.condition, prefix, self_id, own);
    for assignment in &mut when.assignments {
        mangle_assignment(assignment, prefix, self_id, own);
    }
}

fn mangle_effects(effects: &mut [Effect], prefix: &str, self_id: &str, own: &OwnIds) {
    for effect in effects {
        match effect {
            Effect::Let { value, .. } => mangle_expr(value, prefix, self_id, own),
            Effect::If {
                condition,
                then_branch,
                else_branch,
            } => {
                mangle_expr(condition, prefix, self_id, own);
                mangle_effects(then_branch, prefix, self_id, own);
                mangle_effects(else_branch, prefix, self_id, own);
            }
            Effect::Assign { target, value, .. } => {
                mangle_target(target, prefix, self_id, own);
                mangle_expr(value, prefix, self_id, own);
            }
            // The one rewrite with no name in it: a component's signal
            // belongs to its *call site*, which is a different element from
            // whatever handler happens to be running when it fires. Inside
            // the instance it is addressed explicitly.
            Effect::Emit { on, .. } => {
                if on.is_none() {
                    *on = Some(self_id.to_string());
                }
            }
            Effect::Call { callee, args } => {
                mangle_callee(callee, prefix, self_id);
                for arg in args {
                    mangle_expr(arg, prefix, self_id, own);
                }
            }
        }
    }
}

/// Points a call's target at this instance.
///
/// An `id.method()` callee is resolved through the same flat id table as
/// everything else, so its id needs the prefix for the same reason.
/// `parent` is left alone: it is structural, and the method dispatcher
/// resolves it that way already.
fn mangle_callee(callee: &mut [String], prefix: &str, self_id: &str) {
    // A one-segment callee is a host function, not an element address, and
    // has nothing to namespace. Matched on the length rather than with a
    // `[first, ..]` slice, which would happily match a single segment too.
    if callee.len() < 2 {
        return;
    }
    let target = &mut callee[0];
    match target.as_str() {
        "root" => *target = self_id.to_string(),
        "parent" => {}
        name => *target = format!("{prefix}{name}"),
    }
}

/// Points a target at this instance.
///
/// `Component`/`Root` are the component's own state and become an explicit
/// reference to the instance element. `Parent` is left alone: a parent is
/// reached structurally, so it is already the right element at every
/// nesting depth. `MachineState` and `Id` are resolved by the caller, and
/// an `Id` that is a component-local name gains the prefix.
fn mangle_target(target: &mut PropertyTarget, prefix: &str, self_id: &str, own: &OwnIds) {
    match target {
        PropertyTarget::Component(name) | PropertyTarget::Root(name) => {
            *target = PropertyTarget::Id(self_id.to_string(), name.clone());
        }
        PropertyTarget::Id(name, _) => {
            // `<self>` addresses the node the assignment sits on, which is
            // already unique: it is never a name in the id table.
            //
            // And a document-level id is left alone: the checker's D20
            // fallback lets a component body read an element an *entry*
            // component declares, and that element is the same one in every
            // instance. Prefixing it would make the reference point at nothing.
            if name != "<self>" && own.contains(name) {
                *name = format!("{prefix}{name}");
            }
        }
        PropertyTarget::Parent(_) | PropertyTarget::MachineState(_, _) => {}
    }
}

fn mangle_expr(expr: &mut TypedExpr, prefix: &str, self_id: &str, own: &OwnIds) {
    match expr {
        TypedExpr::Property { target, .. } => mangle_target(target, prefix, self_id, own),
        TypedExpr::Unary { operand, .. } => mangle_expr(operand, prefix, self_id, own),
        TypedExpr::Binary { lhs, rhs, .. } => {
            mangle_expr(lhs, prefix, self_id, own);
            mangle_expr(rhs, prefix, self_id, own);
        }
        TypedExpr::Ternary {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            mangle_expr(condition, prefix, self_id, own);
            mangle_expr(then_expr, prefix, self_id, own);
            mangle_expr(else_expr, prefix, self_id, own);
        }
        TypedExpr::Call { args, .. } | TypedExpr::HostCall { args, .. } => {
            for arg in args {
                mangle_expr(arg, prefix, self_id, own);
            }
        }
        TypedExpr::Interp { parts } => {
            for part in parts {
                if let nui_compiler::InterpPart::Expr(inner) = part {
                    mangle_expr(inner, prefix, self_id, own);
                }
            }
        }
        // `Dynamic` reads a model row field, resolved through the row scope
        // at evaluation time; `Local` is a `let`; `Const` is a literal.
        TypedExpr::Dynamic { base, .. } => mangle_expr(base, prefix, self_id, own),
        TypedExpr::Const(_) | TypedExpr::Local { .. } | TypedExpr::Error => {}
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use nui_compiler::ComponentIr;
    use nui_compiler::document::{PropertyDefaultIr, PropertyIr};
    use nui_compiler::types::Type;
    use nui_core::Value;

    /// The set of ids a test's bare `NodeIr` is pretending to own.
    ///
    /// A real caller derives this from the component's IR; a test that builds a
    /// node by hand has no IR, so it says which names are its own.
    fn owned(names: &[&str]) -> OwnIds {
        return names
            .iter()
            .map(|name| return (*name).to_string())
            .collect();
    }

    /// The set a real caller derives: the ids the component's IR declares.
    fn owned_by(ir: &ComponentIr) -> OwnIds {
        return ir.ids.iter().map(|(id, _)| return id.clone()).collect();
    }

    /// The IR of a component with one declared property, one id, a machine
    /// and a signal — the four things a rewrite has to reach.
    fn component_ir() -> ComponentIr {
        return ComponentIr {
            name: "Chip".to_string(),
            // The ids this component declares. The checker fills this from the
            // declaration; the fixture spells it out because the rewrite is
            // driven by it, and an empty set would silently namespace nothing.
            ids: vec![
                ("box".to_string(), "Rectangle".to_string()),
                ("caption".to_string(), "Text".to_string()),
            ],
            properties: vec![PropertyIr {
                name: "label".to_string(),
                ty: Type::String,
                default: Some(PropertyDefaultIr::Static(TypedExpr::Const(Value::String(
                    String::new(),
                )))),
            }],
            signals: vec!["picked".to_string()],
            roots: vec![NodeIr {
                ty: "Rectangle".to_string(),
                id: Some("box".to_string()),
                assignments: vec![AssignmentIr {
                    path: vec!["fill".to_string()],
                    target: PropertyTarget::Id("box".to_string(), "fill".to_string()),
                    kind: nui_compiler::InitKind::Bind,
                    value: TypedExpr::Property {
                        target: PropertyTarget::Component("label".to_string()),
                        ty: Type::String,
                    },
                }],
                when_blocks: vec![WhenIr {
                    condition: TypedExpr::Property {
                        target: PropertyTarget::Root("dark".to_string()),
                        ty: Type::Bool,
                    },
                    assignments: vec![AssignmentIr {
                        path: vec!["radius".to_string()],
                        target: PropertyTarget::Id("box".to_string(), "radius".to_string()),
                        kind: nui_compiler::InitKind::Static,
                        value: TypedExpr::Const(Value::Int(4)),
                    }],
                }],
                handlers: vec![nui_compiler::HandlerIr {
                    signal: "click".to_string(),
                    effect: vec![
                        Effect::Assign {
                            target: PropertyTarget::Component("label".to_string()),
                            op: nui_compiler::AssignOp::Set,
                            value: TypedExpr::Const(Value::String("x".to_string())),
                        },
                        Effect::Emit {
                            signal: "picked".to_string(),
                            on: None,
                        },
                    ],
                }],
                children: vec![NodeIr {
                    ty: "Text".to_string(),
                    id: Some("caption".to_string()),
                    assignments: vec![AssignmentIr {
                        path: vec!["content".to_string()],
                        target: PropertyTarget::Id("caption".to_string(), "content".to_string()),
                        kind: nui_compiler::InitKind::Bind,
                        value: TypedExpr::Interp {
                            parts: vec![nui_compiler::InterpPart::Expr(Box::new(
                                TypedExpr::Property {
                                    target: PropertyTarget::Parent("width".to_string()),
                                    ty: Type::Length,
                                },
                            ))],
                        },
                    }],
                    ..NodeIr::default()
                }],
                ..NodeIr::default()
            }],
            machines: vec![MachineIr {
                name: "m".to_string(),
                states: vec![StateIr {
                    name: "idle".to_string(),
                    enter: vec![Effect::Assign {
                        target: PropertyTarget::Root("dark".to_string()),
                        op: nui_compiler::AssignOp::Set,
                        value: TypedExpr::Const(Value::Bool(false)),
                    }],
                    exit: Vec::new(),
                }],
                transitions: vec![TransitionIr {
                    event: "picked".to_string(),
                    from: vec!["idle".to_string()],
                    guard: Some(TypedExpr::Property {
                        target: PropertyTarget::Component("label".to_string()),
                        ty: Type::String,
                    }),
                    to: "idle".to_string(),
                }],
            }],
            ..ComponentIr::default()
        };
    }

    fn mangled() -> ComponentIr {
        let mut ir = component_ir();
        let own = owned_by(&ir);
        mangle_node(&mut ir.roots[0], "i0::", "i0::self", &own);
        let mut machines = ir.machines.clone();
        for machine in &mut machines {
            mangle_machine(machine, "i0::", "i0::self", &own);
        }
        ir.machines = machines;
        return ir;
    }

    #[test]
    fn local_ids_gain_the_instance_prefix() {
        let ir = mangled();
        let box_id = match &ir.roots[0].assignments[0].target {
            PropertyTarget::Id(name, _) => name.clone(),
            other => panic!("expected an Id target, got {other:?}"),
        };
        assert_eq!(box_id, "i0::box");
        // A nested id is prefixed too, and only once.
        let caption = &ir.roots[0].children[0];
        assert_eq!(caption.id.as_deref(), Some("i0::caption"));
    }

    #[test]
    fn the_components_own_state_points_at_the_instance() {
        let ir = mangled();
        // The `fill <- label` binding now reads the instance element.
        let target = &ir.roots[0].assignments[0].value;
        let TypedExpr::Property { target, .. } = target else {
            panic!("expected a property read");
        };
        assert_eq!(
            *target,
            PropertyTarget::Id("i0::self".to_string(), "label".to_string())
        );
        // A `when` condition that read `root.dark` does too.
        let TypedExpr::Property { target, .. } = &ir.roots[0].when_blocks[0].condition else {
            panic!("expected a property read");
        };
        assert_eq!(
            *target,
            PropertyTarget::Id("i0::self".to_string(), "dark".to_string())
        );
    }

    #[test]
    fn own_node_property_paths_keep_their_self_sentinel() {
        // `bind_node_assignment` addresses the node with the literal
        // `<self>` when the node has no `id`. Prefixing that would invent
        // an id that nothing registered.
        let mut node = NodeIr {
            assignments: vec![AssignmentIr {
                path: vec!["fill".to_string()],
                target: PropertyTarget::Id("<self>".to_string(), "fill".to_string()),
                kind: nui_compiler::InitKind::Static,
                value: TypedExpr::Const(Value::Int(1)),
            }],
            ..NodeIr::default()
        };
        mangle_node(&mut node, "i0::", "i0::self", &owned(&[]));
        let PropertyTarget::Id(name, _) = &node.assignments[0].target else {
            panic!("expected an Id target");
        };
        assert_eq!(name, "<self>");
    }

    #[test]
    fn an_emit_gains_the_instance_address() {
        let ir = mangled();
        let effect = &ir.roots[0].handlers[0].effect[1];
        let Effect::Emit { on, .. } = effect else {
            panic!("expected an emit");
        };
        assert_eq!(on.as_deref(), Some("i0::self"));
    }

    #[test]
    fn a_parent_read_survives_untouched() {
        // `parent` is structural, so it already means the right element at
        // any depth — rewriting it would break it.
        let ir = mangled();
        let value = &ir.roots[0].children[0].assignments[0].value;
        let TypedExpr::Interp { parts } = value else {
            panic!("expected an interpolated string");
        };
        let nui_compiler::InterpPart::Expr(inner) = &parts[0] else {
            panic!("expected an interpolated expression");
        };
        let TypedExpr::Property { target, .. } = inner.as_ref() else {
            panic!("expected a property read");
        };
        assert_eq!(*target, PropertyTarget::Parent("width".to_string()));
    }

    #[test]
    fn a_call_target_is_rewritten_like_any_other_address() {
        // `id.method()` resolves through the same flat id table, so a
        // component-local id needs the prefix; `root` becomes the instance's
        // own address, and `parent` is structural and left alone.
        let mut node = NodeIr {
            handlers: vec![nui_compiler::HandlerIr {
                signal: "click".to_string(),
                effect: vec![
                    Effect::Call {
                        callee: vec!["timer".to_string(), "start".to_string()],
                        args: Vec::new(),
                    },
                    Effect::Call {
                        callee: vec!["root".to_string(), "close".to_string()],
                        args: Vec::new(),
                    },
                    Effect::Call {
                        callee: vec!["parent".to_string(), "focus".to_string()],
                        args: Vec::new(),
                    },
                ],
            }],
            ..NodeIr::default()
        };
        mangle_node(&mut node, "i0::", "i0::self", &owned(&["timer"]));
        let Effect::Call { callee, .. } = &node.handlers[0].effect[0] else {
            panic!("expected a call");
        };
        assert_eq!(callee[0], "i0::timer", "a local id is namespaced");
        assert_eq!(callee[1], "start", "the method name is untouched");
        let Effect::Call { callee, .. } = &node.handlers[0].effect[1] else {
            panic!("expected a call");
        };
        assert_eq!(callee[0], "i0::self", "`root` becomes the instance");
        let Effect::Call { callee, .. } = &node.handlers[0].effect[2] else {
            panic!("expected a call");
        };
        assert_eq!(callee[0], "parent", "`parent` stays structural");
    }

    #[test]
    fn a_single_name_call_is_a_host_function_and_is_not_namespaced() {
        // One segment is not an element address, so prefixing it would
        // invent an id nothing registered.
        let mut node = NodeIr {
            handlers: vec![nui_compiler::HandlerIr {
                signal: "click".to_string(),
                effect: vec![Effect::Call {
                    callee: vec!["refresh".to_string()],
                    args: Vec::new(),
                }],
            }],
            ..NodeIr::default()
        };
        mangle_node(&mut node, "i0::", "i0::self", &owned(&["timer"]));
        let Effect::Call { callee, .. } = &node.handlers[0].effect[0] else {
            panic!("expected a call");
        };
        assert_eq!(callee[0], "refresh");
    }

    #[test]
    fn machine_guards_and_effects_are_rewritten_too() {
        let ir = mangled();
        let machine = &ir.machines[0];
        let Effect::Assign { target, .. } = &machine.states[0].enter[0] else {
            panic!("expected an assignment");
        };
        assert_eq!(
            *target,
            PropertyTarget::Id("i0::self".to_string(), "dark".to_string())
        );
        let Some(TypedExpr::Property { target, .. }) = &machine.transitions[0].guard else {
            panic!("expected a guard");
        };
        assert_eq!(
            *target,
            PropertyTarget::Id("i0::self".to_string(), "label".to_string())
        );
    }

    #[test]
    fn two_instances_of_one_component_never_collide() {
        let mut first = component_ir();
        let first_ids = owned_by(&first);
        mangle_node(&mut first.roots[0], "i0::", "i0::self", &first_ids);
        let mut second = component_ir();
        let second_ids = owned_by(&second);
        mangle_node(&mut second.roots[0], "i1::", "i1::self", &second_ids);
        let id_of = |ir: &ComponentIr| {
            return match &ir.roots[0].assignments[0].target {
                PropertyTarget::Id(name, _) => name.clone(),
                other => panic!("expected an Id target, got {other:?}"),
            };
        };
        assert_ne!(id_of(&first), id_of(&second));
    }

    /// The document-level id rule: namespaced or not.
    ///
    /// The bug this pins is one that compiles. D20 lets a component body read an
    /// element an entry component declares; the rewrite then prefixed *every*
    /// `PropertyTarget::Id`, so `shell.dark` became `i29::shell.dark` in every
    /// instance, and every binding through it failed at run time with
    /// `unresolved: Id("i29::shell", "dark")` — dozens of them, all from one
    /// rule, and nothing in the document to point at.
    mod document_ids {
        use super::*;
        use crate::instantiate;

        /// The set a real caller derives, and the whole of the fix.
        fn mangle(ir: &mut ComponentIr, prefix: &str) {
            let own: OwnIds = ir.ids.iter().map(|(id, _)| return id.clone()).collect();
            mangle_node(&mut ir.roots[0], prefix, &format!("{prefix}self"), &own);
        }

        /// A document-level reference is left exactly as written.
        #[test]
        fn a_document_level_id_is_not_namespaced() {
            let mut ir = component_ir();
            // A reference to `shell`, which this component does not declare.
            ir.roots[0].assignments.push(AssignmentIr {
                path: vec!["fill".to_string()],
                target: PropertyTarget::Id("shell".to_string(), "dark".to_string()),
                kind: nui_compiler::InitKind::Bind,
                value: TypedExpr::Const(Value::Int(1)),
            });
            mangle(&mut ir, "i7::");
            let PropertyTarget::Id(name, prop) = &ir.roots[0].assignments[1].target else {
                panic!("expected an Id target");
            };
            assert_eq!(name, "shell", "a document id must survive verbatim");
            assert_eq!(prop, "dark");
        }

        /// The component's *own* ids are still namespaced, in the same subtree.
        ///
        /// The two rules have to coexist: fixing the first by not prefixing
        /// anything would break the second, and two instances of one component
        /// would then share an id.
        #[test]
        fn the_components_own_ids_are_still_namespaced() {
            let mut ir = component_ir();
            mangle(&mut ir, "i7::");
            let PropertyTarget::Id(name, _) = &ir.roots[0].assignments[0].target else {
                panic!("expected an Id target");
            };
            assert_eq!(name, "i7::box", "an owned id gains the prefix");
        }

        /// And end to end: a document with a library component that reads the
        /// window resolves, at run time, with no propagate errors.
        ///
        /// The unit tests above are about the rewrite; this is about the
        /// consequence, which is the only thing a user would ever see.
        #[test]
        fn a_library_component_reaches_the_document_at_run_time() {
            let source = r#"
                component Chip {
                    property label: String = ""
                    Rectangle(fill <- shell.dark) {
                        Text(content <- label)
                    }
                }
                component App {
                    property dark: Bool = true
                    Window(id = shell, width = 200dp, height = 100dp) {
                        Chip(label = "one")
                    }
                }
            "#;
            let outcome = nui_compiler::compile(source);
            assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
            let instance = instantiate(&outcome.document);
            let (mut tree, mut engine) = (instance.tree, instance.engine);
            let errors = engine.propagate(&mut tree);
            assert!(
                errors.is_empty(),
                "every binding through a document-level id must resolve: {errors:?}"
            );
        }
    }
}
