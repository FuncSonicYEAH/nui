//! Instantiation: builds an [`ElementTree`] from a compiled
//! [`DocumentIr`](nui_compiler::DocumentIr).
//!
//! Per plan §5, instantiation creates elements in document order, resolves
//! `id`s after construction, writes static (`=`) defaults, and registers
//! `<-` bindings as dirty so the first
//! [`propagate`](crate::binding::Engine::propagate) pass fills the reactive
//! values. `For` nodes store their prototype and a `@for` binding; the row
//! elements are instantiated per model row by
//! [`Engine::sync_for_nodes`](crate::binding::Engine::sync_for_nodes).

use nui_compiler::{
    AssignmentIr, DocumentIr, InitKind, MachineIr, NodeIr, PropertyDefaultIr, TypedExpr,
};
use nui_core::Value;

use crate::binding::{Binding, Engine};
use crate::element::{
    Element, ElementId, ElementTree, FOR_VALUE_PROPERTY, ForBinding, HandlerEntry, WhenEntry,
};
use crate::machine::MachineInstance;
use crate::registry::Registry;

/// The outcome of instantiation: the tree and its reactive engine.
pub struct Instance {
    /// The built element tree.
    pub tree: ElementTree,
    /// The reactive engine with all bindings registered (all dirty).
    pub engine: Engine,
}

/// Instantiates a compiled document with an empty registry.
pub fn instantiate(document: &DocumentIr) -> Instance {
    return instantiate_with(document, Registry::new());
}

/// Instantiates a compiled document with a host registry: custom component
/// types get their descriptor defaults and behaviors (plan §5 宿主互操作).
pub fn instantiate_with(document: &DocumentIr, registry: Registry) -> Instance {
    let mut tree = ElementTree::new();
    let mut engine = Engine::new();
    engine.registry = registry;
    for component in &document.components {
        for node in &component.roots {
            let root = instantiate_scoped_node(&mut tree, &mut engine, node, None);
            tree.push_root(root);
            attach_machines(&mut tree, root, &component.machines);
            apply_component_properties(&mut tree, &mut engine, root, component);
        }
    }
    build_id_index(&mut tree);
    link_two_way_pairs(&mut tree);
    return Instance { tree, engine };
}

/// Hot reload (plan §3.4, v1 semantics): compiles `source` fresh (host
/// function names are validated against the instance's current registry),
/// then rebuilds the tree and engine in place and re-runs the registry's
/// attach hook so models and behaviors re-register. Element state is lost —
/// a full rebuild, matching the hot-reload v1 contract. On a compile error
/// the instance is left untouched and the rendered diagnostics are
/// returned, so the caller keeps showing the last good UI.
pub fn reload_from_source(instance: &mut Instance, source: &str) -> Result<(), String> {
    let function_names = instance.engine.registry().function_names();
    let outcome = nui_compiler::compile_with_functions(source, &function_names);
    if !outcome.diagnostics.is_empty() {
        let rendered = outcome
            .diagnostics
            .iter()
            .map(|diagnostic| {
                return nui_syntax::render_diagnostic(source, "reload.nui", diagnostic);
            })
            .collect::<Vec<_>>()
            .join("\n");
        return Err(rendered);
    }
    // The registry (with its attach hook) survives the engine swap; the
    // old engine owned the models and dies with it.
    let registry = std::mem::take(&mut instance.engine.registry);
    let next = instantiate_with(&outcome.document, registry);
    instance.tree = next.tree;
    instance.engine = next.engine;
    instance.engine.run_registry_init(&mut instance.tree);
    return Ok(());
}

/// Instantiates one node (recursively) and returns its handle. `scope` is
/// the enclosing `For` row scope for elements instantiated inside a row.
pub(crate) fn instantiate_scoped_node(
    tree: &mut ElementTree,
    engine: &mut Engine,
    node: &NodeIr,
    scope: Option<crate::element::RowScope>,
) -> ElementId {
    let mut element = Element::new(node.ty.clone(), node.id.clone());
    element.for_scope = scope.clone();
    // Static (`=`) assignments evaluate before insertion; reactive and
    // two-way assignments are applied after (they need the element handle).
    let mut deferred: Vec<&AssignmentIr> = Vec::new();
    for assignment in &node.assignments {
        if assignment.kind == InitKind::Static {
            apply_static(&mut element, assignment);
        } else {
            deferred.push(assignment);
        }
    }
    for handler in &node.handlers {
        element.handlers.push(HandlerEntry {
            signal: handler.signal.clone(),
            effect: handler.effect.clone(),
        });
    }
    for when in &node.when_blocks {
        element.when_blocks.push(WhenEntry {
            condition: when.condition.clone(),
            assignments: when.assignments.clone(),
        });
    }
    if node.id.as_deref() == Some("root") {
        element.is_component_instance = true;
    }
    if let Some(for_ir) = &node.for_binding {
        element.for_binding = Some(ForBinding {
            variable: for_ir.variable.clone(),
            iterable: for_ir.iterable.clone(),
            prototype: node.children.clone(),
        });
    }
    let id = tree.insert(element);
    if node.ty == "TextInput" {
        tree.arena[id].focusable = true;
        let seed = match tree.arena[id].get("text") {
            Some(Value::String(text)) => text.clone(),
            _ => String::new(),
        };
        tree.arena[id].text_input = Some(crate::text_input::TextInputState::from_text(seed));
    }
    // Registry hook: custom component types get their descriptor defaults
    // and a fresh behavior instance (plan §5 宿主互操作).
    if let Some(desc) = engine.registry().component(&node.ty).cloned() {
        crate::registry::apply_descriptor_defaults(&mut tree.arena[id], &desc);
    }
    if let Some(factory) = engine.registry().behavior(&node.ty) {
        let behavior = factory();
        // Canvas behaviors expose their painter so the scene builder can
        // interpret the command buffer (FUTURE batch 3).
        tree.arena[id].canvas = behavior.canvas();
        tree.arena[id].behavior = Some(behavior);
    }
    for assignment in deferred {
        apply_reactive_assignment(tree, engine, id, assignment);
    }
    if let Some(for_ir) = &node.for_binding {
        // The iterable evaluates into the `@for` slot as a `Value::Model`;
        // row sync reads it after propagation.
        tree.arena[id].set(FOR_VALUE_PROPERTY, Value::Model(Value::UNSET_MODEL));
        let index = engine.register_binding(id, FOR_VALUE_PROPERTY, for_ir.iterable.clone());
        tree.arena[id].set_binding(FOR_VALUE_PROPERTY, Binding { index });
    } else {
        for child in &node.children {
            let child_id = instantiate_scoped_node(tree, engine, child, scope.clone());
            tree.append_child(id, child_id);
        }
    }
    return id;
}

/// Writes a static (`=`) assignment; attached properties (`font.size`) store
/// under their dotted name until the M3 component descriptors split them.
fn apply_static(element: &mut Element, assignment: &AssignmentIr) {
    let property = assignment.path.join(".");
    if let Some(value) = eval_literal(&assignment.value) {
        element.set(&property, value);
    }
}

/// Applies a `<-` or `<=>` assignment to the inserted element.
fn apply_reactive_assignment(
    tree: &mut ElementTree,
    engine: &mut Engine,
    id: ElementId,
    assignment: &AssignmentIr,
) {
    let property = assignment.path.join(".");
    match assignment.kind {
        InitKind::Bind => {
            tree.arena[id].set(&property, Value::Int(0));
            let index = engine.register_binding(id, &property, assignment.value.clone());
            tree.arena[id].set_binding(&property, Binding { index });
        }
        InitKind::TwoWay => {
            tree.arena[id].set(&property, Value::Int(0));
            tree.arena[id]
                .pending_two_way
                .push((property, assignment.value.clone()));
        }
        InitKind::Static => {}
    }
}

/// Evaluates a literal-only expression (M2 static defaults).
fn eval_literal(expr: &TypedExpr) -> Option<Value> {
    return match expr {
        TypedExpr::Const(value) => Some(value.clone()),
        TypedExpr::Interp { parts } => {
            let mut text = String::new();
            for part in parts {
                match part {
                    nui_compiler::InterpPart::Text(fragment) => text.push_str(fragment),
                    nui_compiler::InterpPart::Expr(inner) => {
                        let value = eval_literal(inner)?;
                        text.push_str(&value.to_string());
                    }
                }
            }
            return Some(Value::String(text));
        }
        _ => None,
    };
}

/// Attaches component machines to the component instance element.
fn attach_machines(tree: &mut ElementTree, element: ElementId, machines: &[MachineIr]) {
    for machine in machines {
        if let Some(instance) = MachineInstance::new(machine) {
            tree.arena[element].machines.push(instance);
        }
    }
}

/// Applies component-level property declarations onto the component instance
/// element: static defaults write now; reactive defaults register bindings.
fn apply_component_properties(
    tree: &mut ElementTree,
    engine: &mut Engine,
    root: ElementId,
    component: &nui_compiler::ComponentIr,
) {
    for property in &component.properties {
        match &property.default {
            Some(PropertyDefaultIr::Static(expr)) => {
                if let Some(value) = eval_literal(expr) {
                    tree.arena[root].set(&property.name, value);
                }
            }
            Some(PropertyDefaultIr::Bind(expr)) => {
                tree.arena[root].set(&property.name, Value::Int(0));
                let index = engine.register_binding(root, &property.name, expr.clone());
                tree.arena[root].set_binding(&property.name, Binding { index });
            }
            None => {
                // Declared without default: typed zero seed so reads succeed.
                tree.arena[root].set(&property.name, zero_value_for(property.ty));
            }
        }
    }
}

/// Wires `<=>` pairs once the id table exists. The partner address is the
/// assignment's value expression (`text <=> root.userName`, `a.text <=>
/// other.text`): resolve it like any property read, then link the two
/// property slots.
fn link_two_way_pairs(tree: &mut ElementTree) {
    let mut links = Vec::new();
    tree.visit_pre_order(|id, element| {
        for (property, expr) in &element.pending_two_way {
            let nui_compiler::TypedExpr::Property { target, .. } = expr else {
                continue;
            };
            let Some(partner_element) = tree.resolve_target(id, target) else {
                continue;
            };
            links.push((
                id,
                property.clone(),
                partner_element,
                crate::binding::target_property_name(target),
            ));
        }
    });
    for (element, property, partner, partner_property) in links {
        tree.arena[element].set_two_way(
            &property,
            crate::binding::TwoWayLink {
                partner,
                property: partner_property,
            },
        );
    }
}

/// The zero value of a compile-time type (reads before first write).
pub(crate) fn zero_value_for(ty: nui_compiler::Type) -> Value {
    return match ty {
        nui_compiler::Type::Bool => Value::Bool(false),
        nui_compiler::Type::Int => Value::Int(0),
        nui_compiler::Type::Float => Value::Float(0.0),
        nui_compiler::Type::String => Value::String(String::new()),
        nui_compiler::Type::Color => Value::Color(nui_core::Color::BLACK),
        nui_compiler::Type::Length => Value::Length(nui_core::Length::ZERO),
        nui_compiler::Type::Duration => Value::Duration(nui_core::Duration::ZERO),
        nui_compiler::Type::Enum => Value::Enum(String::new()),
        // Host-assigned: reads before the host registers a model see the
        // unset sentinel (For sync skips it).
        nui_compiler::Type::Model => Value::Model(Value::UNSET_MODEL),
        nui_compiler::Type::Unknown => Value::Int(0),
    };
}

/// Builds the `id -> element` index after the whole tree exists (plan §5:
/// ids resolve after construction, before binding evaluation).
fn build_id_index(tree: &mut ElementTree) {
    let mut entries = Vec::new();
    tree.visit_pre_order(|id, element| {
        if let Some(name) = &element.id {
            entries.push((name.clone(), id));
        }
    });
    for (name, id) in entries {
        tree.register_id(&name, id);
    }
}
