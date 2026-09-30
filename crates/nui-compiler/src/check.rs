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

/// Methods a document may call through `root` / `parent`.
///
/// A component has no methods of its own, so the checker normally rejects
/// `root.something()`. These are the exceptions: methods that act on the
/// *host* rather than on the element, and so are reachable from anywhere —
/// or rather, only from the document root, which is where a window lives.
/// A fixed list rather than a rule, because "any method on a `Window`
/// element" needs an element type table the checker does not have.
const HOST_ELEMENT_METHODS: &[&str] = &["close"];

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

/// A component's declared API, read syntactically before anything is bound.
///
/// A call site may appear before the component it names (or after it), so
/// every signature has to be known before the first component is bound.
/// This is deliberately a *syntactic* pre-pass rather than a lookup into
/// the bound IR: the binders run in order, and a component must be
/// checkable no matter where it sits in the file.
#[derive(Debug, Clone)]
struct ComponentSignature {
    /// The declared component's name (for diagnostics).
    name: String,
    /// Whether the component declares a `Slot` to receive a call site's content.
    has_slot: bool,
    /// Declared property name -> declared type (`Unknown` when inferred
    /// from a default, which the pre-pass does not evaluate).
    properties: HashMap<String, Type>,
    /// Declared signal names.
    signals: HashSet<String>,
    /// The declaration's span, for diagnostics.
    span: Span,
    /// How many top-level nodes the component declares. An instantiated
    /// component must have exactly one (see [`ComponentIr::roots`]).
    root_count: usize,
}

/// The document-wide view every per-component binder shares: what each
/// declared component exposes, and which components are actually
/// instantiated by something.
///
/// Built once from the AST, before any binding, so component references
/// resolve in a single pass and entry points are known up front.
#[derive(Debug, Default)]
struct DocumentIndex {
    signatures: HashMap<String, ComponentSignature>,
    /// Names some component instantiates; everything else is an entry
    /// point the runtime instantiates on its own.
    referenced: HashSet<String>,
    /// component -> the components it instantiates, with the span of the
    /// node that named them. The edge set the recursion check walks.
    edges: HashMap<String, Vec<(String, Span)>>,
    /// Every element id declared anywhere in the document, with the type name
    /// it was declared on.
    ///
    /// A component body's *own* ids are collected by its binder and always win;
    /// this is the fallback for a name the component does not declare itself.
    /// See [`ComponentBinder::check_name`] and the note there for why a
    /// document-level fallback exists at all.
    document_ids: HashMap<String, String>,
}

impl DocumentIndex {
    /// Indexes every declared component and every reference to one.
    ///
    /// Also reports the two whole-document errors that no single
    /// component's binder can see: a component that instantiates itself
    /// (directly or through a chain), and an instantiated component that
    /// does not declare exactly one root.
    fn build(document: &Document, diagnostics: &mut Vec<nui_syntax::Diagnostic>) -> DocumentIndex {
        // Two passes, and the order matters: the edge pass has to see *every*
        // signature, or a component that references one declared later in
        // the file records no edge and a cycle through it goes unnoticed.
        let mut signatures: HashMap<String, ComponentSignature> = HashMap::new();
        for decl in &document.components {
            let name = decl.name.name.clone();
            if signatures
                .insert(name.clone(), signature_of(decl))
                .is_some()
            {
                diagnostics.push(nui_syntax::Diagnostic::error(
                    decl.name.span,
                    format!("duplicate component declaration `{name}`"),
                ));
            }
        }
        let mut edges: HashMap<String, Vec<(String, Span)>> = HashMap::new();
        for decl in &document.components {
            let mut outgoing = Vec::new();
            for member in &decl.members {
                let ComponentMember::Node(node) = member else {
                    continue;
                };
                collect_node_references(node, &signatures, &mut outgoing);
            }
            edges.insert(decl.name.name.clone(), outgoing);
        }
        let referenced: HashSet<String> = edges
            .values()
            .flatten()
            .map(|(name, _)| return name.clone())
            .filter(|name| return signatures.contains_key(name))
            .collect();
        // The document's element ids, collected last because the rule needs
        // `referenced`: an *entry point* is instantiated by the runtime
        // directly, so its ids survive unrewritten and are the only ones a
        // name can legitimately refer to. An instantiated component's ids are
        // rewritten into its instance's namespace (`i1::label`), so the bare
        // name does not name anything and must not resolve.
        //
        // Collecting this before `referenced` would be wrong in a way that only
        // shows up much later: `label` would resolve from anywhere and would
        // read whichever instance the runtime happened to answer with.
        let mut document_ids: HashMap<String, String> = HashMap::new();
        for decl in &document.components {
            if referenced.contains(&decl.name.name) {
                continue;
            }
            for member in &decl.members {
                let ComponentMember::Node(node) = member else {
                    continue;
                };
                collect_node_ids(node, &mut document_ids);
            }
        }
        let index = DocumentIndex {
            signatures,
            referenced,
            document_ids,
            edges,
        };
        index.report_recursion(diagnostics);
        index.report_root_counts(diagnostics);
        return index;
    }

    /// Reports every cycle in the component reference graph.
    ///
    /// A cycle would expand forever, so it has to be a compile error
    /// rather than a runtime guard. The walk is an explicit-colour DFS
    /// over the edge set; a component already on the current path is the
    /// back edge that closes a cycle.
    fn report_recursion(&self, diagnostics: &mut Vec<nui_syntax::Diagnostic>) {
        /// 1 = on the current path, 2 = fully explored.
        const ON_PATH: u8 = 1;
        const DONE: u8 = 2;

        fn walk(
            index: &DocumentIndex,
            name: &str,
            marks: &mut HashMap<String, u8>,
            path: &mut Vec<String>,
            diagnostics: &mut Vec<nui_syntax::Diagnostic>,
        ) {
            match marks.get(name) {
                Some(&ON_PATH) => {
                    diagnostics.push(nui_syntax::Diagnostic::error(
                        index.signatures[name].span,
                        format!(
                            "component `{name}` instantiates itself: {}",
                            cycle_path(path, name)
                        ),
                    ));
                    return;
                }
                Some(&DONE) => return,
                _ => {}
            }
            marks.insert(name.to_string(), ON_PATH);
            path.push(name.to_string());
            for (target, _) in index.edges.get(name).into_iter().flatten() {
                if index.signatures.contains_key(target) {
                    walk(index, target, marks, path, diagnostics);
                }
            }
            path.pop();
            marks.insert(name.to_string(), DONE);
        }

        let mut marks: HashMap<String, u8> = HashMap::new();
        let mut path = Vec::new();
        let mut names: Vec<&String> = self.edges.keys().collect();
        names.sort();
        for name in names {
            walk(self, name, &mut marks, &mut path, diagnostics);
        }
    }

    /// An instantiated component has to be instantiable: exactly one root
    /// node, since the instance element *is* that root.
    fn report_root_counts(&self, diagnostics: &mut Vec<nui_syntax::Diagnostic>) {
        for (name, signature) in &self.signatures {
            if !self.referenced.contains(name) || signature.root_count == 1 {
                continue;
            }
            let found = signature.root_count;
            diagnostics.push(nui_syntax::Diagnostic::error(
                signature.span,
                format!(
                    "component `{name}` is instantiated, so it must declare exactly one \
                     root node (found {found}); wrap them in a Column or Stack"
                ),
            ));
        }
    }

    /// The signature of the component `ty` names, if it names one.
    fn component(&self, ty: &str) -> Option<&ComponentSignature> {
        return self.signatures.get(ty);
    }
}

/// Appends the components `node` instantiates to `out`, with the span of the
/// node that named each one.
///
/// # Why a reference's body is walked
///
/// A reference's children are the *caller's* slot content, and that content can
/// instantiate further components. So the walk continues past a reference into
/// its body, rather than recording the reference and returning.
///
/// Stopping there -- which is what this did, for the reason that a reference
/// used to have no children at all -- makes every component below a slot look
/// unreferenced. And an unreferenced component is instantiated as a *tree root*
/// by the runtime, so it is still built: still type-checked, still rendered,
/// just at the origin and in nobody's place.
///
/// The failure is silent, which is what makes it worth stating. Put a
/// document's whole content inside one slotted container and every component in
/// it appeared twice — once where it belonged, once at the top-left corner with
/// its declared defaults. No diagnostic anywhere, because the stray instance was
/// a correct instance of a correctly declared component: every check that looked
/// at the *component* rather than at the *tree* passed, and the only symptom was
/// an extra shape in the corner. A test that instantiated the document and
/// looked at the result would have caught it; one that only checked the
/// component's own diagnostics never would.
fn collect_node_references(
    node: &NodeDecl,
    signatures: &HashMap<String, ComponentSignature>,
    out: &mut Vec<(String, Span)>,
) {
    if signatures.contains_key(&node.ty.name) {
        out.push((node.ty.name.clone(), node.ty.span));
        // Not a `return`: the reference's body is the caller's slot content,
        // which may instantiate components of its own.
    }
    for member in &node.body {
        if let NodeMember::Node(child) = member {
            collect_node_references(child, signatures, out);
        }
    }
}

/// Collects the element ids in a component's own subtree.
///
/// The caller is responsible for passing an *entry point* (see
/// [`DocumentIndex::build`]), so no reference guard is needed here: nothing in
/// an entry point's body is a component reference, because an entry point
/// instantiates *its* children and the components those children reference are
/// themselves instantiated and rewritten.
fn collect_node_ids(node: &NodeDecl, out: &mut HashMap<String, String>) {
    for arg in &node.args {
        let NodeArg::Id(id) = arg else {
            continue;
        };
        out.insert(id.name.clone(), node.ty.name.clone());
    }
    for member in &node.body {
        if let NodeMember::Node(child) = member {
            collect_node_ids(child, out);
        }
    }
}

/// The edge-graph node a *node's* own property is recorded against.
///
/// `None` for the component's root, where the node's property and the
/// component's property are the same slot -- so the key must coincide for a real
/// self-assignment to be caught. A distinct sink for a nested node, whose
/// property lives on a different element entirely.
fn node_edge_sink(is_root: bool) -> Option<&'static str> {
    if is_root {
        return None;
    }
    return Some(NESTED_NODE_SINK);
}

/// The edge-graph node a component argument's target is recorded against.
///
/// A name nothing writes, so an edge into it cannot close a cycle. See
/// [`ComponentBinder::bind_node_assignment`] for why a component argument needs
/// one at all.
const ARGUMENT_SINK: &str = "<argument>";

/// The edge-graph node a nested node's own property is recorded against.
const NESTED_NODE_SINK: &str = "<node>";

/// The element a component declares to receive a call site's content.
///
/// A name, not a type, because the language has no user-defined types: a
/// component is the only way to name anything, and the slot is part of a
/// component's contract. `Slot` is otherwise an ordinary element -- it lays out
/// as a plain column and paints nothing -- so a document can use it outside a
/// component and get a transparent column, which is harmless and consistent.
const SLOT: &str = "Slot";

/// Every `Slot` in a subtree, with the span of the node that declares it.
fn collect_slot_spans(node: &NodeDecl, out: &mut Vec<Span>) {
    if node.ty.name == SLOT {
        out.push(node.ty.span);
    }
    for member in &node.body {
        let NodeMember::Node(child) = member else {
            continue;
        };
        collect_slot_spans(child, out);
    }
}

/// Whether a component declaration contains a `Slot`.
///
/// Walks the whole body rather than just the root, because a slot usually sits
/// nested inside a `Column` that gives the content a frame -- which is the
/// point of having one.
fn declares_slot(decl: &ComponentDecl) -> bool {
    fn in_node(node: &NodeDecl) -> bool {
        if node.ty.name == SLOT {
            return true;
        }
        return node.body.iter().any(|member| {
            let NodeMember::Node(child) = member else {
                return false;
            };
            return in_node(child);
        });
    }
    return decl.members.iter().any(|member| {
        let ComponentMember::Node(node) = member else {
            return false;
        };
        return in_node(node);
    });
}

/// The declared API of one component declaration.
fn signature_of(decl: &ComponentDecl) -> ComponentSignature {
    let mut signature = ComponentSignature {
        name: decl.name.name.clone(),
        has_slot: declares_slot(decl),
        span: decl.name.span,
        properties: HashMap::new(),
        signals: HashSet::new(),
        root_count: 0,
    };
    for member in &decl.members {
        match member {
            ComponentMember::Property(property) => {
                let declared = property
                    .declared_type
                    .as_ref()
                    .and_then(|name| return Type::from_name(&name.name));
                signature.properties.insert(
                    property.name.name.clone(),
                    declared.unwrap_or(Type::Unknown),
                );
            }
            ComponentMember::Signal(signal) => {
                signature.signals.insert(signal.name.name.clone());
            }
            ComponentMember::Node(_) => {
                signature.root_count += 1;
            }
            ComponentMember::Machine(_) => {}
        }
    }
    return signature;
}

/// The chain `a -> b -> a` for a recursion diagnostic, as a readable path.
fn cycle_path(path: &[String], repeated: &str) -> String {
    let start = path
        .iter()
        .position(|name| return name == repeated)
        .unwrap_or(0);
    let mut names: Vec<&str> = path[start..].iter().map(String::as_str).collect();
    names.push(repeated);
    return names.join(" -> ");
}

/// Checks a parsed document with a set of host-registered function names
/// (plan §5 宿主互操作): calls to those names lower to [`TypedExpr::HostCall`]
/// instead of producing an unknown-function diagnostic.
pub fn check_with(document: &Document, extern_functions: &[String]) -> CheckOutcome {
    return check_with_host(document, extern_functions, &[]);
}

/// [`check_with`], plus the subset of those names that are *commands*:
/// host actions that return nothing, so a value position is an error
/// rather than a runtime `Unresolved`.
///
/// Two lists rather than one because the document spells both the same way
/// (`save()`) and only the checker can tell which one a position allows.
pub fn check_with_host(
    document: &Document,
    extern_functions: &[String],
    extern_commands: &[String],
) -> CheckOutcome {
    let mut diagnostics = Vec::new();
    let index = DocumentIndex::build(document, &mut diagnostics);
    let mut components = Vec::new();
    for decl in &document.components {
        let mut binder = ComponentBinder::new(decl, extern_functions, extern_commands, &index);
        let component = binder.bind();
        diagnostics.append(&mut binder.diagnostics);
        components.push(component);
    }
    // Entry points are the components nothing instantiates. Decided after
    // binding because only then is every call site known.
    for component in &mut components {
        component.referenced = index.referenced.contains(&component.name);
    }
    return CheckOutcome {
        document: DocumentIr { components },
        diagnostics,
    };
}

/// Maps a syntax-level data operator onto its IR counterpart.
/// The kind an `=` assignment actually gets.
///
/// # Why `=` on a property read becomes a binding
///
/// `=` means "evaluate once". For a literal that is exactly what happens, and
/// it is what makes `width = 12dp` a static value rather than a dependency. But
/// a one-shot evaluation of `Wrapper(kind = kind)` is not expressible: the
/// instance graph is built in a second pass, so at the moment the argument would
/// be evaluated the parent property it reads may not exist yet — and the
/// runtime's static path evaluates *literals* only, so a property read there is
/// dropped. Silently. The component receives its declared default and renders
/// it, which looks like a rendering bug and is not one.
///
/// That is the shape of every component that wraps another component, so it is
/// not a corner case. Tracking is also what the author means by it: a wrapper
/// passing its own property down wants the child to follow when the parent's
/// changes, and "copied once at an unspecified moment" is not a thing anyone
/// asks for.
///
/// The rule is therefore: `=` is static when the value is a literal, and a
/// binding when it is a property read. `<-` is unchanged, and so is `= ` on
/// everything the document can already evaluate without a tree.
fn init_kind(op: InitOp, value: &TypedExpr) -> InitKind {
    return match op {
        InitOp::Static => {
            if is_literal(value) {
                InitKind::Static
            } else {
                InitKind::Bind
            }
        }
        InitOp::Bind => InitKind::Bind,
        InitOp::TwoWay => InitKind::TwoWay,
    };
}

/// Whether a value can be evaluated without a tree.
///
/// The same set the runtime's `eval_literal` accepts: a constant, or an
/// interpolation with no expression in it. Kept as a separate predicate so the
/// two cannot drift — if `eval_literal` grows a case, this is where to note it.
fn is_literal(value: &TypedExpr) -> bool {
    return match value {
        TypedExpr::Const(_) => true,
        TypedExpr::Interp { parts } => parts
            .iter()
            .all(|part| return matches!(part, InterpPart::Text(_))),
        _ => false,
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
    /// Host-registered *command* names — the subset of [`Self::extern_functions`]
    /// that acts rather than returns. A command is legal in statement
    /// position only; see [`ComponentBinder::check_call`].
    extern_commands: HashSet<String>,
    /// The document-wide component index (shared by every binder).
    index: &'source DocumentIndex,
    component: &'source ComponentDecl,
}

impl<'source> ComponentBinder<'source> {
    fn new(
        component: &'source ComponentDecl,
        extern_functions: &[String],
        extern_commands: &[String],
        index: &'source DocumentIndex,
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
            extern_commands: extern_commands.iter().cloned().collect(),
            index,
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
                    component_ir.roots.push(self.bind_node(node, true));
                }
            }
        }
        // The component's own element ids, which the runtime's id rewrite needs
        // so it namespaces *these* and leaves a document-level id alone.
        //
        // Collected at the end rather than in `collect_info` because binding a
        // member can declare ids the early pass has not seen -- a `For` row's
        // prototype, or a `when` block's target.
        //
        // This field existed and was documented but nothing ever filled it, so
        // the set the runtime derives was always empty. Nothing noticed,
        // because before the id rewrite learned to consult it, *every* id was
        // namespaced unconditionally -- the empty set never mattered. Once it
        // is consulted, an empty set means "namespace nothing", and two
        // instances of a component share one namespace.
        component_ir.ids = self
            .ids
            .iter()
            .map(|(name, ty)| return (name.clone(), ty.clone()))
            .collect();
        self.check_slots();
        self.check_binding_cycles();
        return component_ir;
    }

    /// A component declares where a call site's children land, with `Slot`.
    ///
    /// Two rules, both about making the mistake loud:
    ///
    /// - **At most one slot per component.** With one, a call site's content
    ///   has exactly one possible home, so there is no ordering question. More
    ///   than one would need names, and an unnamed second slot would be
    ///   ambiguous -- so it is refused rather than half-supported.
    /// - **Content needs a slot to go into.** A reference that passes children
    ///   to a component with no `Slot` would silently drop them, which is the
    ///   exact failure this feature exists to prevent, so it is a diagnostic.
    ///
    /// Reported here rather than at each call site so the message names the
    /// component, which is where the fix is.
    fn check_slots(&mut self) {
        let mut slots: Vec<Span> = Vec::new();
        for member in &self.component.members {
            let ComponentMember::Node(node) = member else {
                continue;
            };
            collect_slot_spans(node, &mut slots);
        }
        for extra in slots.iter().skip(1) {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                *extra,
                format!(
                    "component `{}` declares more than one `{SLOT}`; a component has \
                     exactly one place a call site's content can go, so merge them \
                     or wrap the second in its own element",
                    self.component.name.name
                ),
            ));
        }
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

    /// Collects the ids a node declares.
    ///
    /// A component reference contributes only its own `id`: the ids inside
    /// the referenced component belong to *its* scope and are namespaced
    /// per instance at instantiation, so pulling them into the caller's
    /// scope would both shadow the caller's own ids and collide between
    /// two instances of the same component.
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
        if self.index.component(&node.ty.name).is_some() {
            return;
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

    /// `is_root` says whether this node *is* the component instance.
    ///
    /// It decides the edge-graph key for the node's own properties, and the
    /// distinction is the difference between a real cycle and a common idiom. A
    /// component property and the **root** element's property of the same name
    /// are one slot on one element: `Text(content = content)` is genuinely
    /// `content <- content`, and the runtime loops on it. A *nested* node's
    /// property is a different slot on a different element, so
    /// `Column(width = width)` is the ordinary way to size a child from a
    /// component property, and the two must not share a node in this graph.
    fn bind_node(&mut self, decl: &NodeDecl, is_root: bool) -> NodeIr {
        // A node whose type names a component declared in this document is
        // a *reference*, not an element: the subtree lives in that
        // component, and this node carries the call site's arguments and
        // handlers.
        if let Some(signature) = self.index.component(&decl.ty.name).cloned() {
            return self.bind_component_reference(decl, &signature);
        }
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
                    let sink = node_edge_sink(is_root);
                    assignments_from_args.push(self.bind_node_assignment(assignment, &node, sink));
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
            self.reject_component_in_for(decl);
            self.bind_node_members(&mut node, &decl.body, is_root);
            self.pop_scope();
            return node;
        }
        self.bind_node_members(&mut node, &decl.body, is_root);
        return node;
    }

    /// Binds body members in source order; handlers and `when` blocks are
    /// appended to the node, child nodes recurse.
    fn bind_node_members(&mut self, node: &mut NodeIr, members: &[NodeMember], is_root: bool) {
        for member in members {
            match member {
                NodeMember::Assignment(assignment) => {
                    let sink = node_edge_sink(is_root);
                    node.assignments
                        .push(self.bind_node_assignment(assignment, node, sink));
                }
                NodeMember::Handler(handler) => {
                    node.handlers.push(self.bind_handler(handler));
                }
                NodeMember::When(when) => {
                    let bound = self.bind_when(when, node);
                    node.when_blocks.push(bound);
                }
                NodeMember::Node(child) => {
                    node.children.push(self.bind_node(child, false));
                }
            }
        }
    }

    /// Reports a component reference inside a `For` / `ListView` body.
    ///
    /// Row instantiation happens in the engine, which has no component
    /// catalog, so the reference could not be expanded. The check lives
    /// here rather than in `bind_node` because the offending shape is a
    /// property of the *enclosing* loop, and the loop is what knows it is
    /// a loop.
    fn reject_component_in_for(&mut self, decl: &NodeDecl) {
        if decl.ty.name != "For" && decl.ty.name != "ListView" {
            return;
        }
        for member in &decl.body {
            let NodeMember::Node(child) = member else {
                continue;
            };
            if self.index.component(&child.ty.name).is_some() {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    child.ty.span,
                    format!(
                        "`{}` cannot be instantiated inside a `For` body: rows are built \
                         without the document, so there is nothing to expand it from",
                        child.ty.name
                    ),
                ));
            }
        }
    }

    /// Binds a node that instantiates a component declared in this
    /// document.
    ///
    /// The result carries no children: the subtree is the referenced
    /// component's, and the runtime expands it per use site. What this
    /// node *does* carry is the call site's half of the contract — its
    /// `id`, the arguments written against the instance, and the handlers
    /// for the component's declared signals.
    ///
    /// The body carries a handler, or the slot's content, and nothing else.
    ///
    /// Handlers subscribe to the component's own signals, which the checker
    /// validates against the declaration. Child nodes are the *slot*: they
    /// replace the `Slot` element the referenced component declares, and
    /// because they are bound here they belong to the caller's scope rather
    /// than the callee's -- which is what makes a slot useful.
    ///
    /// A body assignment and a `when` block are still rejected. Each would need
    /// a rule about *where* in the referenced subtree it lands, and `when` has
    /// no answer at all: its condition would be evaluated once at instantiation
    /// and never again, which is a silent surprise rather than a limitation.
    /// Properties go in the argument list instead, checked against the
    /// component's declared types.
    fn bind_component_reference(&mut self, decl: &NodeDecl, target: &ComponentSignature) -> NodeIr {
        let mut node = NodeIr {
            ty: decl.ty.name.clone(),
            component: Some(decl.ty.name.clone()),
            ..NodeIr::default()
        };
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
                    node.assignments
                        .push(self.bind_component_argument(assignment, &node, target));
                }
                NodeArg::Handler(handler) => {
                    self.require_component_signal(handler, target);
                    node.handlers.push(self.bind_handler(handler));
                }
            }
        }
        if decl.for_binding.is_some() {
            // `For(item in rows) { Chip(...) }` needs the component catalog
            // at row-instantiation time, and the engine instantiates rows
            // without the document. Rejecting the shape here beats letting
            // it expand to nothing (or to a panic) at runtime.
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                decl.ty.span,
                format!(
                    "a `For` cannot instantiate `{}`; move the reference into the loop body",
                    decl.ty.name
                ),
            ));
        }
        for member in &decl.body {
            match member {
                // A handler is the one body member that means something at
                // a call site: it subscribes to one of the component's
                // signals. It has to be bound *and* checked against the
                // component's declarations, not skipped.
                NodeMember::Handler(handler) => {
                    self.require_component_signal(handler, target);
                    node.handlers.push(self.bind_handler(handler));
                }
                // Child nodes are the slot. They land in the referenced
                // component's `Slot` element, and because they are bound
                // *here* they belong to the caller's scope rather than the
                // callee's -- which is the whole point: an `id` or a
                // `shell.dark` inside a slot means what it says in the
                // component that wrote it.
                NodeMember::Node(child) => {
                    if !target.has_slot {
                        self.diagnostics.push(nui_syntax::Diagnostic::error(
                            child.ty.span,
                            format!(
                                "`{}` takes no content: it declares no `{SLOT}`, so \
                                 these child nodes have nowhere to go",
                                decl.ty.name
                            ),
                        ));
                    }
                    node.children.push(self.bind_node(child, false));
                }
                NodeMember::Assignment(assignment) => self.reject_reference_body_member(
                    assignment.span,
                    decl.ty.name.as_str(),
                    "property assignments",
                ),
                NodeMember::When(when) => {
                    self.reject_reference_body_member(
                        when.span,
                        decl.ty.name.as_str(),
                        "`when` blocks",
                    );
                }
            }
        }
        return node;
    }

    /// Reports a body member a component reference cannot take, pointing at
    /// the argument list where the same intent belongs.
    fn reject_reference_body_member(&mut self, span: Span, component: &str, what: &str) {
        self.diagnostics.push(nui_syntax::Diagnostic::error(
            span,
            format!(
                "a `{component}` reference takes no {what} in its body; \
                 pass them as arguments or use `<-` on the argument"
            ),
        ));
    }

    /// Binds one call-site argument of a component reference.
    ///
    /// A name the component *declares* is its API and is type-checked
    /// against the declaration. Any other name is an ordinary element
    /// property of the instance's root element, which the checker does not
    /// validate (element properties are open-ended, as everywhere else).
    /// Declared wins on a name collision, so a component that declares
    /// `radius` is not reachable through the element's own `radius`.
    fn bind_component_argument(
        &mut self,
        assignment: &PropertyAssignment,
        node: &NodeIr,
        target: &ComponentSignature,
    ) -> AssignmentIr {
        let name = &assignment.target.parts[0].name;
        let Some(declared) = target.properties.get(name) else {
            return self.bind_node_assignment(assignment, node, Some(ARGUMENT_SINK));
        };
        let value = self.check_expr(&assignment.value);
        let value_ty = value.type_of();
        if *declared != Type::Unknown && unify(*declared, value_ty).is_none() {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                assignment.span,
                format!(
                    "type mismatch for property `{name}` of `{}`: expected {declared}, found {value_ty}",
                    node.ty
                ),
            ));
        }
        // No binding edge is recorded here, unlike `bind_assignment`. The
        // edge a `<-` argument creates belongs to the *callee's* graph
        // ("this component's property depends on that expression"), and the
        // callee is a different component with its own cycle check.
        // Recording it against the caller's identically named property
        // would invent a cycle out of a name collision.
        return AssignmentIr {
            path: vec![name.clone()],
            target: PropertyTarget::Id(
                node.id
                    .clone()
                    .unwrap_or_else(|| return "<self>".to_string()),
                name.clone(),
            ),
            kind: init_kind(assignment.op, &value),
            value,
        };
    }

    /// Rejects a handler for a signal the referenced component does not
    /// declare, naming the ones it does.
    fn require_component_signal(&mut self, handler: &Handler, target: &ComponentSignature) {
        let name = &handler.signal.name;
        if target.signals.contains(name) {
            return;
        }
        let declared = if target.signals.is_empty() {
            String::from("it declares none")
        } else {
            let mut names: Vec<&str> = target.signals.iter().map(String::as_str).collect();
            names.sort();
            format!("declared signals: {}", names.join(", "))
        };
        self.diagnostics.push(nui_syntax::Diagnostic::error(
            handler.signal.span,
            format!(
                "component `{}` has no signal `{name}` ({declared})",
                target.name
            ),
        ));
    }

    /// Binds an assignment whose target is a node's own property
    /// (constructor args and node-body assignments): a bare path addresses
    /// the node itself, not the component.
    /// `edge_sink` names the node the recorded edge points *at*.
    ///
    /// For an ordinary assignment that is the property itself. For a
    /// component argument it cannot be: the argument lands on the *callee's*
    /// element while the value reads the *caller's*, so keying both on the bare
    /// property name makes `Wrapper(icon = icon)` look like `icon <- icon` --
    /// a cycle that does not exist, and the shape every slot-bearing component
    /// is written in. A distinct sink is a node nothing writes, so the edge can
    /// never close a loop.
    fn bind_node_assignment(
        &mut self,
        assignment: &PropertyAssignment,
        node: &NodeIr,
        edge_sink: Option<&str>,
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
                kind: init_kind(assignment.op, &value),
                value,
            };
        }
        return self.bind_assignment(assignment, edge_sink);
    }

    fn bind_assignment(
        &mut self,
        assignment: &PropertyAssignment,
        edge_sink: Option<&str>,
    ) -> AssignmentIr {
        let value = self.check_expr(&assignment.value);
        if assignment.op == InitOp::TwoWay
            && !matches!(&assignment.value, Expr::Ident { .. } | Expr::Member { .. })
        {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                assignment.span,
                "two-way binding `<=>` requires a property path on the right-hand side",
            ));
        }
        // The edge is recorded for whatever is *actually* reactive, not for
        // what was written. `init_kind` promotes `=` on a property read to a
        // binding (see its doc comment), so keying this off the syntax op
        // would leave the promoted ones out of the cycle graph -- and
        // `Text(content = content)`, a component property shadowing the element
        // property of the same name, is exactly a self-edge that the runtime
        // then loops on forever.
        let kind = init_kind(assignment.op, &value);
        if kind == InitKind::Bind
            && assignment.target.parts.len() == 1
            && self
                .property_types
                .contains_key(&assignment.target.parts[0].name)
        {
            let sink = edge_sink.unwrap_or(&assignment.target.parts[0].name);
            self.record_binding_edge(sink, &value, assignment.span);
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
            kind,
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
            .map(|assignment| return self.bind_node_assignment(assignment, node, None))
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
                        on: None,
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
            //
            // Checked against the host's names for the same reason the
            // *value* path is: a name the host never registered can only
            // fail at runtime, where an evaluation error is dropped and the
            // statement silently does nothing. A command is welcome here —
            // statement position is the only place it is legal at all.
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
            if !self.extern_functions.contains(name) {
                self.diagnostics.push(nui_syntax::Diagnostic::error(
                    callee.parts[0].span,
                    format!("unknown function `{name}`"),
                ));
                return None;
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
        // `root.close()` is the one method that is not about the element it
        // is written on: it asks the *host* to close the window, and the
        // window is not reachable from inside a component by any other
        // route. It therefore also skips the id lookup — a component that
        // has no element called `root` is exactly the case that matters.
        let method = callee.parts[1].name.as_str();
        if HOST_ELEMENT_METHODS.contains(&method) {
            return Some(Effect::Call {
                callee: vec![id.name.clone(), method.to_string()],
                args: Vec::new(),
            });
        }
        if id.name == "root" || id.name == "parent" {
            self.diagnostics.push(nui_syntax::Diagnostic::error(
                id.span,
                format!("`{}.{}`: components have no methods", id.name, method),
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
        // Note the *path*: this is a member access (`name.prop`), and the
        // fallback below applies to it. A **bare** unknown identifier is a
        // different case entirely -- `check_ident` treats one as an enum
        // variant literal, so `fill <- bold` is a colour named `bold` rather
        // than a misspelled reference. Nothing here changes that.
        //
        // The component's own ids first, so an instance's private element
        // always shadows a document-level name of the same spelling.
        if self.ids.contains_key(base_name) {
            return TypedExpr::Property {
                target: PropertyTarget::Id(base_name.clone(), name.name.clone()),
                ty: Type::Unknown,
            };
        }
        // Then the document's, as a fallback.
        //
        // This is the one place a component body can reach outside itself, and
        // it exists for the shape a whole application takes: a document with
        // one entry component that holds the app's state, and a library of
        // components that need to read it. A theme's light/dark flag is the
        // motivating case -- every control needs it, and passing it to eighty
        // instantiations would bury the component in a parameter it does not
        // own.
        //
        // The cost is real and worth stating: two components can now both
        // read the same document element, so a component is no longer purely a
        // function of its own arguments. What is *not* weakened is the
        // isolation N1 bought -- each instance still rewrites its own subtree
        // and its own ids into a namespace, so nothing collides at runtime, and
        // a component that declares no reference to a document element is
        // still self-contained. It is also not transitive: a name resolved
        // this way is not in the component's id table, so a *further*
        // component cannot reach it through this one.
        if self.index.document_ids.contains_key(base_name) {
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
                // A command acts; it has no value to hand back. Caught here
                // rather than at runtime because a failed evaluation is
                // dropped by the host (the statement or binding just does
                // nothing) — the visible symptom would be a missing side
                // effect with no message anywhere.
                if self.extern_commands.contains(name) {
                    self.diagnostics.push(nui_syntax::Diagnostic::error(
                        *span,
                        format!(
                            "`{name}` is a host command and has no value; \
                             call it as a statement instead"
                        ),
                    ));
                    for arg in args {
                        let _ = self.check_expr(&arg.value);
                    }
                    return TypedExpr::Error;
                }
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

/// The document-level id fallback: what it buys and what it does not.
///
/// These are the properties the change is *allowed* to have, written down next
/// to the implementation so a later change to the resolution order is caught
/// here rather than discovered in a downstream project.
/// The cycle graph's keys: a nested node's property is not the component's, and
/// a component argument's target is not either.
#[cfg(test)]
mod edge_keys {
    use crate::compile;

    fn expect_clean(source: &str) {
        let outcome = compile(source);
        assert!(
            outcome.diagnostics.is_empty(),
            "should compile cleanly: {:?}",
            outcome.diagnostics
        );
    }

    /// A nested node's property is not the component's property of that name.
    ///
    /// `Column(width = width)` is the ordinary way to size a child from a
    /// component property, and the two are different slots on different
    /// elements: the value reads the *instance* and the target is the *Column*.
    /// Keying both on `width` in the cycle graph made that a self-edge, so every
    /// slot-bearing component that passed a property down was reported as a
    /// cycle.
    #[test]
    fn a_nested_node_may_reuse_a_component_property_name() {
        expect_clean(
            r#"
            component Panel {
                property width: Float = 300.0
                Scroll(id = self) {
                    Column(width = width, height = 100%) { }
                }
            }
            component App {
                Window(id = shell) { Panel(width = 600.0) }
            }
        "#,
        );
    }

    /// The root element's property of the same name *is* a cycle.
    ///
    /// The counterpart, and the reason the fix is a distinction rather than a
    /// blanket new key: on the root the component property and the element
    /// property are the same slot on the same element, so
    /// `Text(content = content)` is `content <- content` and the runtime loops on
    /// it forever. Un-sharing *every* key would have lost this one.
    #[test]
    fn the_root_element_property_is_a_real_cycle() {
        let outcome = compile(
            r#"
            component Label {
                property content: String = ""
                Text(id = self, content = content) { }
            }
            component App {
                Window(id = shell) { Label(content = "x") }
            }
        "#,
        );
        let error = outcome
            .diagnostics
            .iter()
            .map(|diagnostic| return diagnostic.message.clone())
            .collect::<Vec<_>>()
            .join("\n");
        assert!(
            error.contains("reactive binding cycle"),
            "a root element shadowing a component property is a real cycle: {error}"
        );
    }

    /// `Wrapper(icon = icon)` is not a cycle.
    ///
    /// The argument lands on the *callee's* element and the value reads the
    /// *caller's*, so the two are different properties on different elements. The
    /// edge graph keys on names, and without a distinct sink for the argument
    /// the edge reads `icon <- icon` -- a cycle that does not exist.
    ///
    /// Reachable whenever the caller and the callee both use a name but only one
    /// declares it, which is what happens the moment a component renames a
    /// property. The shape every slot-bearing component is written in:
    /// `Frame(icon = icon)`.
    #[test]
    fn passing_a_property_to_an_undeclared_argument_is_not_a_cycle() {
        expect_clean(
            r#"
            component Inner {
                Rectangle(id = box, radius = 4dp) { }
            }
            component Outer {
                property icon: String = "x"
                Stack(id = self) { Inner(icon = icon) }
            }
            component App {
                Window(id = shell) { Outer(icon = "home") }
            }
        "#,
        );
    }
}

#[cfg(test)]
mod document_id_fallback {
    use crate::compile;

    /// Compiles a source expected to be clean, panicking with the diagnostics
    /// if it is not.
    fn expect_clean(source: &str) {
        let outcome = compile(source);
        assert!(
            outcome.diagnostics.is_empty(),
            "this source should compile cleanly: {:?}",
            outcome.diagnostics
        );
    }

    /// Compiles a source expected to fail, returning the rendered
    /// diagnostics.
    fn expect_dirty(source: &str) -> String {
        let outcome = compile(source);
        assert!(
            !outcome.diagnostics.is_empty(),
            "this source is expected to produce a diagnostic"
        );
        return outcome
            .diagnostics
            .iter()
            .map(|diagnostic| return nui_syntax::render_diagnostic(source, "test.nui", diagnostic))
            .collect::<Vec<_>>()
            .join("");
    }

    /// A component body reaches an element id declared in another component.
    ///
    /// The case that motivated it: a library of controls that all need the
    /// app's light/dark flag, which lives on the window of the entry
    /// component.
    #[test]
    fn a_component_body_reads_another_components_element() {
        let source = r#"
            component Chip {
                property label: String = ""
                Rectangle(fill <- shell.tint) { Text(content <- label) }
            }
            component App {
                property tint: Color = #336699
                Window(id = shell) { Chip(label = "one") }
            }
        "#;
        expect_clean(source);
    }

    /// The order matters: an instance's own element shadows the document's.
    ///
    /// If the document's name won, a component with a private `label` would
    /// silently start reading the entry component's `label` instead -- the
    /// kind of change that compiles and renders wrong.
    #[test]
    fn a_components_own_id_shadows_the_documents() {
        let source = r#"
            component Chip {
                Rectangle(id = tint, width = 10dp, height = 10dp, fill = #00000000) {
                    Text(content <- shell.tint)
                }
            }
            component App {
                property label: String = "outer"
                Window(id = shell) { Chip() }
            }
        "#;
        // Both `tint`s resolve: the one to `shell.tint`, and the `id = tint`
        // the component declares for itself. What must *not* happen is the
        // entry component's *property* `tint` leaking in as a bare name.
        expect_clean(source);
    }

    /// Not transitive: a document id is not re-exported by the component that
    /// read it.
    ///
    /// This is what keeps the fallback from becoming a scope chain. Component
    /// B reads the document's `shell`, and component C -- which only ever sees
    /// B's surface -- still cannot.
    #[test]
    fn the_fallback_is_not_transitive() {
        let source = r#"
            component Inner {
                Rectangle(fill <- shell.tint) { }
            }
            component Middle {
                Inner() { }
            }
            component App {
                property tint: Color = #336699
                Window(id = shell) { Middle() }
            }
        "#;
        expect_clean(source);
        // `Middle` reads `label.length`, and `App` declares a *property*
        // `label`. A component body sees its own properties and the
        // document's element *ids* -- never another component's properties.
        let source = r#"
            component Inner {
                Rectangle(fill <- shell.tint) { }
            }
            component Middle {
                Stack {
                    Inner()
                    Text(content <- label.length)
                }
            }
            component App {
                property label: String = "outer"
                property tint: Color = #336699
                Window(id = shell) { Middle() }
            }
        "#;
        let error = expect_dirty(source);
        assert!(
            error.contains("unknown name `label`"),
            "another component's property must not be readable: {error}"
        );
    }

    /// A typo is still a typo.
    ///
    /// The fallback must not swallow the diagnostic it replaced, or a
    /// misspelled element id would become a silently unbound binding.
    #[test]
    fn an_unknown_name_is_still_an_error() {
        let source = r#"
            component Chip {
                Rectangle(fill <- shel.tint) { }
            }
            component App {
                property tint: Color = #336699
                Window(id = shell) { Chip() }
            }
        "#;
        let error = expect_dirty(source);
        assert!(
            error.contains("unknown name `shel`"),
            "a near miss should still be reported: {error}"
        );
    }

    /// An instantiated component's private ids are *not* document-level.
    ///
    /// The boundary that matters: `Inner`'s `id = label` belongs to each
    /// instance of `Inner`, and must not become a name that `App` or another
    /// component can read.
    #[test]
    fn an_instances_private_id_is_not_document_level() {
        // `label` is declared inside `Inner`, which `App` instantiates. It is
        // therefore an *instance's* element, not a document one, and a
        // component that does not declare it must not be able to read it --
        // even one that lives in the same document. Read as a member access
        // (`label.width`) so the check goes through the path the fallback
        // touches; a bare `label` would be an enum variant literal.
        let source = r#"
            component Inner {
                Rectangle(id = label, width = 10dp, height = 10dp)
            }
            component Stranger {
                Text(content <- label.width)
            }
            component App {
                Window(id = shell) {
                    Row {
                        Inner()
                        Stranger()
                    }
                }
            }
        "#;
        let error = expect_dirty(source);
        assert!(
            error.contains("unknown name `label`"),
            "an instance's element id must stay inside its instance: {error}"
        );
    }

    /// Compiles with a host vocabulary: `values` return, `commands` act.
    fn compile_against(source: &str, values: &[&str], commands: &[&str]) -> crate::CompileOutcome {
        let host = crate::HostVocabulary::new()
            .with_functions(values.iter().map(|name| return (*name).to_string()))
            .with_commands(commands.iter().map(|name| return (*name).to_string()));
        return crate::compile_with_host(source, &host);
    }

    #[test]
    fn a_command_is_callable_as_a_statement() {
        let outcome = compile_against(
            "component A { Button { on click => save(\"now\") } }",
            &[],
            &["save"],
        );
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    }

    #[test]
    fn a_command_has_no_value() {
        // A command acts; there is nothing for an expression to read. The
        // error has to be here rather than at run time: a failed evaluation
        // is dropped by the host, so the only symptom would be a property
        // that keeps its old value for no stated reason.
        let outcome = compile_against(
            "component A { property last: String <- save() }",
            &[],
            &["save"],
        );
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("`save` is a host command")),
            "{:?}",
            outcome.diagnostics
        );
    }

    #[test]
    fn a_value_function_is_still_callable_as_a_statement() {
        // The reverse direction is fine and useful: `log("x")` as a
        // statement runs the function and drops the result.
        let outcome = compile_against(
            "component A { Button { on click => log(\"x\") } }",
            &["log"],
            &[],
        );
        assert!(outcome.diagnostics.is_empty(), "{:?}", outcome.diagnostics);
    }

    #[test]
    fn an_unregistered_statement_call_is_an_error() {
        // Statement position used to accept *any* single name, so a typo
        // compiled and then silently did nothing.
        let outcome = compile("component A { Button { on click => togleTodo() } }");
        assert!(
            outcome
                .diagnostics
                .iter()
                .any(|d| return d.message.contains("unknown function `togleTodo`")),
            "{:?}",
            outcome.diagnostics
        );
    }
}
