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

use std::collections::HashMap;

use nui_compiler::{
    AssignmentIr, ComponentIr, DocumentIr, InitKind, MachineIr, NodeIr, PropertyDefaultIr,
    TypedExpr,
};
use nui_core::Value;

use crate::binding::{Binding, Engine, TwoWayEdge};
use crate::element::SLOT;
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
///
/// Only the document's *entry* components become tree roots. A component
/// something else instantiates contributes no root of its own — its
/// instances appear where they are written — which is what stops a library
/// of components from painting itself at the origin.
pub fn instantiate_with(document: &DocumentIr, registry: Registry) -> Instance {
    let mut tree = ElementTree::new();
    let mut engine = Engine::new();
    engine.registry = registry;
    let catalog = ComponentCatalog::new(&document.components);
    let mut instantiator = Instantiator::new(catalog);
    for component in &document.components {
        if component.referenced {
            continue;
        }
        // An entry component is an instance of itself, and needs the same
        // rewrite an instance gets -- but for a different reason. `mangle_node`
        // sends `PropertyTarget::Component(name)` to the instance element, and
        // without it an entry component's own properties resolve to *the
        // document's first root*: which is a different element as soon as the
        // document has more than one entry component, and is that element's
        // property, or nothing at all.
        //
        // It shows up as a component reading `0` where it declared `""`, with
        // no diagnostic -- the property is simply a different element's.
        let self_id = format!("entry{}::self", component.name);
        let own: crate::mangle::OwnIds = component
            .ids
            .iter()
            .map(|(id, _)| return id.clone())
            .collect();
        for node in &component.roots {
            let mut node = node.clone();
            crate::mangle::mangle_node(&mut node, "", &self_id, &own);
            let root = instantiate_scoped_node(
                &mut tree,
                &mut engine,
                &node,
                None,
                &mut instantiator,
                None,
            );
            tree.push_root(root);
            tree.register_id(&self_id, root);
            attach_machines(&mut tree, root, &component.machines);
            apply_component_properties(&mut tree, &mut engine, root, &component.properties);
        }
    }
    build_id_index(&mut tree);
    let two_way = link_two_way_pairs(&mut tree);
    engine.index_two_way_links(two_way);
    return Instance { tree, engine };
}

/// The instantiable components of one document, by name.
///
/// A component reference is resolved through this rather than through a
/// field on the engine, so the engine stays reactive state and never
/// carries the program. The consequence is that a `For` row — which the
/// engine instantiates on its own, with no catalog in hand — cannot expand
/// a component reference today; the checker rejects that shape instead of
/// letting it fail at runtime.
struct ComponentCatalog<'doc> {
    components: HashMap<&'doc str, &'doc ComponentIr>,
}

impl<'doc> ComponentCatalog<'doc> {
    fn new(components: &'doc [ComponentIr]) -> ComponentCatalog<'doc> {
        return ComponentCatalog {
            components: components
                .iter()
                .map(|component| return (component.name.as_str(), component))
                .collect(),
        };
    }

    fn get(&self, name: &str) -> Option<&'doc ComponentIr> {
        return self.components.get(name).copied();
    }
}

/// The state one instantiation run threads through the recursion: the
/// catalog to expand against and the counter that keeps every instance's id
/// namespace distinct.
pub(crate) struct Instantiator<'doc> {
    catalog: ComponentCatalog<'doc>,
    /// How many component instances have been expanded so far. Each one
    /// takes the next number as its id prefix.
    instances: u32,
}

impl<'doc> Instantiator<'doc> {
    fn new(catalog: ComponentCatalog<'doc>) -> Instantiator<'doc> {
        return Instantiator {
            catalog,
            instances: 0,
        };
    }

    /// The next instance's id prefix.
    ///
    /// A monotonic counter rather than a path built from the call site,
    /// because the same call site can be reached from inside a `For` row
    /// and the prefix only has to be *unique*, not pretty. Debug-asserting
    /// the wrap keeps a u32 rollover from silently merging two instances'
    /// namespaces.
    fn next_prefix(&mut self) -> String {
        self.instances = self.instances.checked_add(1).unwrap_or_else(|| {
            panic!("component instance counter overflowed; the document is too large")
        });
        return format!("i{}::", self.instances);
    }
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

/// The property values a component instance's root element must carry *before*
/// its children are instantiated.
///
/// # Why they cannot wait
///
/// A component's subtree is built before the instance's declared properties and
/// the caller's arguments are applied, because applying them needs the element
/// handle. So a nested reference that *statically* initialises from a parent
/// property -- `Wrapper(kind = kind)`, which is what a component wrapping
/// another component almost always does -- reads the property's default instead
/// of the value the caller passed.
///
/// A reactive binding self-corrects on the first propagate, which is before
/// anything is laid out or drawn. A static initialiser does not: it is written
/// once, at build time, and never again. The symptom is a component that shows
/// the right shape in the source and the wrong one on screen, with no
/// diagnostic anywhere -- `name` was `""`, which is a perfectly good string.
///
/// So the values are seeded here, at insertion, and the reactive ones are
/// registered afterwards as usual.
pub(crate) struct InstanceSeed<'a> {
    /// The component's declared property defaults.
    pub properties: &'a [nui_compiler::PropertyIr],
    /// The caller's arguments for this reference.
    pub reference: &'a NodeIr,
}

/// Instantiates one node (recursively) and returns its handle. `scope` is
/// the enclosing `For` row scope for elements instantiated inside a row.
pub(crate) fn instantiate_scoped_node(
    tree: &mut ElementTree,
    engine: &mut Engine,
    node: &NodeIr,
    scope: Option<crate::element::RowScope>,
    instantiator: &mut Instantiator<'_>,
    seed: Option<&InstanceSeed<'_>>,
) -> ElementId {
    // A component reference expands into the referenced component's own
    // root, so the instance element is a real element in the tree with the
    // call site's arguments, handlers and API on it.
    if let Some(name) = &node.component {
        return instantiate_component_instance(tree, engine, node, name, scope, instantiator);
    }
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
    // Before the children: a child's static initialiser may read this node's
    // properties, and this element only has them because the caller passed
    // them. See `InstanceSeed`.
    if let Some(seed) = seed {
        seed_instance(tree, engine, id, seed);
    }
    // The keyboard focus order: text fields plus every control a user can
    // *operate* from the keyboard (see `widget::is_focusable_type` — a
    // Dialog is interactive but is not a tab stop).
    if node.ty == "TextInput" || crate::widget::is_focusable_type(&node.ty) {
        tree.arena[id].focusable = true;
    }
    if node.ty == "TextInput" {
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
        // The interaction half of the same registration. The focus walk
        // reads this flag rather than `Engine::interaction`, so a registered
        // type that declares itself focusable has to set it here or the two
        // disagree — a bare `TodoCheckbox` would report a control with hover
        // states and still never be reachable from the keyboard.
        if desc.interaction.focusable {
            tree.arena[id].focusable = true;
        }
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
            let child_id =
                instantiate_scoped_node(tree, engine, child, scope.clone(), instantiator, None);
            tree.append_child(id, child_id);
        }
    }
    return id;
}

/// Expands one component reference into a real subtree.
///
/// The instance element **is** the component's single root: that is what
/// makes the call site's `id`, the component's declared properties, its
/// machines and the call site's signal handlers all land on one address
/// without a wrapper node that layout would have to account for.
///
/// Order matters here. The component's own root is instantiated first (so
/// its internal `<-` bindings exist), then the declared properties get
/// their defaults, then the caller's arguments overwrite them — an argument
/// is a later, more specific statement about the same slot. Each argument
/// that *replaces* a property the component bound itself clears that
/// binding, which is the same precedence rule an effect-block assignment
/// gets (plan D10): the call site wins outright, it does not fight the
/// component's binding every frame.
fn instantiate_component_instance(
    tree: &mut ElementTree,
    engine: &mut Engine,
    reference: &NodeIr,
    name: &str,
    scope: Option<crate::element::RowScope>,
    instantiator: &mut Instantiator<'_>,
) -> ElementId {
    // The checker rejects a referenced component without exactly one root,
    // and rejects references to components that do not exist (an unknown
    // name is an element type, not a reference, so this cannot be reached
    // from a checked document).
    let component = instantiator
        .catalog
        .get(name)
        .unwrap_or_else(|| panic!("component `{name}` is referenced but not in the catalog"));
    let [root_node] = component.roots.as_slice() else {
        panic!("component `{name}` is instantiated without exactly one root");
    };
    // The root node is cloned per instance anyway (each needs its own
    // rewritten copy), and the machines are a handful of small tables.
    // Taking them here keeps the catalog borrow from being live across the
    // recursive instantiation below.
    let root_node = root_node.clone();
    let properties = component.properties.clone();
    let machines = component.machines.clone();
    let prefix = instantiator.next_prefix();
    // The instance's own address. Chosen here and registered below, once
    // the element the rewrite points at actually exists.
    let self_id = format!("{prefix}self");
    // The ids this component declares, which are the only ones the rewrite is
    // allowed to namespace. A document-level id — one an entry component
    // declares and this component reads (D20) — must survive verbatim, or the
    // reference becomes `i29::shell` and every binding through it is
    // unresolved at run time with no diagnostic, because it compiled.
    let own: crate::mangle::OwnIds = component
        .ids
        .iter()
        .map(|(id, _)| return id.clone())
        .collect();
    let mut root_node = root_node;
    crate::mangle::mangle_node(&mut root_node, &prefix, &self_id, &own);

    let seed = InstanceSeed {
        properties: &properties,
        reference,
    };
    let element =
        instantiate_scoped_node(tree, engine, &root_node, scope, instantiator, Some(&seed));
    // The slot: the call site's content, dropped into the `Slot` element the
    // component declares. Done after the subtree exists because the slot is
    // found by looking at what was built, not by re-deriving from the
    // component's IR -- the slot can sit anywhere in the subtree, and the
    // instantiator is what knows where.
    //
    // The content is the *reference's* nodes, so it is not namespaced with the
    // instance prefix: an `id` inside a slot belongs to the call site, exactly
    // as it would outside a component.
    for slot in tree.descendants_of_type(element, SLOT) {
        let slot_scope = tree.arena[slot].for_scope.clone();
        for child in &reference.children {
            let child_id = instantiate_scoped_node(
                tree,
                engine,
                child,
                slot_scope.clone(),
                instantiator,
                None,
            );
            tree.append_child(slot, child_id);
        }
    }
    if let Some(call_site_id) = &reference.id {
        tree.register_id(call_site_id, element);
    }
    tree.arena[element].component = Some(name.to_string());
    tree.arena[element].instance_id = Some(self_id.clone());
    tree.register_id(&self_id, element);
    // A component that declares itself focusable joins the Tab order, the
    // same as a built-in whose type name is in the focusable table. Set
    // here rather than per frame because the focus walk reads the flag.
    if let Some(descriptor) = engine.registry().component(name)
        && descriptor.interaction.focusable
    {
        tree.arena[element].focusable = true;
    }

    apply_component_properties(tree, engine, element, &properties);
    for assignment in &reference.assignments {
        apply_instance_argument(tree, engine, element, assignment);
    }
    for handler in &reference.handlers {
        tree.arena[element].handlers.push(HandlerEntry {
            signal: handler.signal.clone(),
            effect: handler.effect.clone(),
        });
    }
    for mut machine in machines {
        crate::mangle::mangle_machine(&mut machine, &prefix, &self_id, &own);
        attach_machine(tree, element, machine);
    }
    return element;
}

/// Instantiates one node of a `For` / `ListView` row prototype.
///
/// Same walk as [`instantiate_scoped_node`], minus the component catalog:
/// the engine instantiates rows on its own and does not carry the
/// document, so it cannot expand a component reference. The checker
/// rejects that combination, which is what keeps this from being a silent
/// no-op — a row that quietly rendered as an empty element would be far
/// harder to diagnose than a compile error.
pub(crate) fn instantiate_prototype_node(
    tree: &mut ElementTree,
    engine: &mut Engine,
    node: &NodeIr,
    scope: Option<crate::element::RowScope>,
) -> ElementId {
    if let Some(name) = &node.component {
        panic!(
            "component `{name}` cannot be instantiated inside a For row: \
             the engine has no document to expand it from"
        );
    }
    return instantiate_scoped_node(
        tree,
        engine,
        node,
        scope,
        &mut Instantiator::new(ComponentCatalog::new(&[])),
        // A `For` row cannot contain a component reference (it would have no
        // document to expand), so there is never an instance to seed here.
        None,
    );
}

/// Applies one call-site argument to a component instance.
///
/// The target already names the instance element (the checker resolved it
/// against the caller's `id`, or `<self>` when it declared none), so this
/// only has to decide whether the slot keeps a binding the component's own
/// root installed.
fn apply_instance_argument(
    tree: &mut ElementTree,
    engine: &mut Engine,
    element: ElementId,
    assignment: &AssignmentIr,
) {
    let property = assignment.path.join(".");
    // A static argument is a final value: the component's own `<-` on this
    // property would overwrite it on the next propagation pass, so the
    // binding has to be retired on *both* sides — the engine's table (what
    // `propagate` walks) and the element's slot (what readers consult).
    if assignment.kind == InitKind::Static {
        engine.retire_binding(element, &property);
        tree.arena[element].clear_binding(&property);
    }
    match assignment.kind {
        InitKind::Static => apply_static(&mut tree.arena[element], assignment),
        InitKind::Bind => apply_reactive_assignment(tree, engine, element, assignment),
        InitKind::TwoWay => {
            // `<=>` against a component property: the value expression is
            // the partner address, and it was already rewritten in the
            // *caller's* scope when the caller was instantiated.
            engine.retire_binding(element, &property);
            tree.arena[element].clear_binding(&property);
            tree.arena[element].set(&property, Value::Int(0));
            tree.arena[element]
                .pending_two_way
                .push((property, assignment.value.clone()));
        }
    }
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
        attach_machine(tree, element, machine.clone());
    }
}

/// Attaches one machine to an element, if it declares a state to start in.
fn attach_machine(tree: &mut ElementTree, element: ElementId, machine: MachineIr) {
    if let Some(instance) = MachineInstance::new(&machine) {
        tree.arena[element].machines.push(instance);
    }
}

/// Applies component-level property declarations onto the component instance
/// element: static defaults write now; reactive defaults register bindings.
/// Applies an instance's declared defaults and the caller's *static* arguments
/// to its root element, before the subtree is built.
///
/// Only the static half. A reactive or two-way argument is registered as a
/// binding and re-evaluated on the first propagate, which is early enough; a
/// static one is written once and would otherwise be written with the wrong
/// value.
fn seed_instance(
    tree: &mut ElementTree,
    engine: &mut Engine,
    root: ElementId,
    seed: &InstanceSeed<'_>,
) {
    for property in seed.properties {
        let Some(PropertyDefaultIr::Static(expr)) = &property.default else {
            continue;
        };
        if let Some(value) = eval_literal(expr) {
            tree.arena[root].set(&property.name, value);
        }
    }
    for assignment in &seed.reference.assignments {
        if assignment.kind == InitKind::Static {
            apply_static(&mut tree.arena[root], assignment);
        }
    }
    // The engine is not used yet: nothing here registers a binding. Kept in the
    // signature so the two halves of seeding cannot drift apart, and so a
    // future reactive default has somewhere obvious to go.
    let _ = engine;
}

fn apply_component_properties(
    tree: &mut ElementTree,
    engine: &mut Engine,
    root: ElementId,
    properties: &[nui_compiler::PropertyIr],
) {
    for property in properties {
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
///
/// Returns the resolved pairs so the engine can build the *reverse* index
/// ([`Engine::index_two_way_links`]) — the declaring slot alone only carries
/// the forward direction, and half a pair is not a pair.
fn link_two_way_pairs(tree: &mut ElementTree) -> Vec<TwoWayEdge> {
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
    let mut edges = Vec::with_capacity(links.len());
    for (element, property, partner, partner_property) in links {
        // A pair starts *in sync*. The slot holds the `Int(0)` placeholder a
        // reactive assignment left behind (it exists so reads succeed before
        // the first write), and that placeholder is nobody's value: a
        // `Float` model would arrive in the field as an integer, so a
        // stepper or slider on it could only ever move in whole units. The
        // partner — the model side of `<=>` — seeds the pair.
        if let Some(value) = tree.arena[partner].get(&partner_property).cloned() {
            tree.arena[element].set(&property, value);
        }
        tree.arena[element].set_two_way(
            &property,
            crate::binding::TwoWayLink {
                partner,
                property: partner_property.clone(),
            },
        );
        edges.push(TwoWayEdge {
            element,
            property,
            partner,
            partner_property,
        });
    }
    return edges;
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
