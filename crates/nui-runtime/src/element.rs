//! Element tree: slotmap-arena storage for instantiated nodes.
//!
//! Every node of a compiled document becomes an [`Element`] here. Elements
//! own a compact property store plus reactive bookkeeping (bindings, two-way
//! links, dirty flags). Handles ([`ElementId`]) are generational, so stale
//! references after tree rebuilds are detected instead of silently wrong.

use std::collections::HashMap;

use nui_core::Value;
use slotmap::SlotMap;

use crate::binding::{Binding, BindingIndex, TwoWayLink};
use crate::machine::MachineInstance;

slotmap::new_key_type! {
    /// Generational handle of an element in the tree.
    pub struct ElementId;
}

/// Property slot on `For` elements that stores the iterable's current
/// [`Value::Model`](nui_core::Value::Model) (registered as a regular
/// binding so dependency invalidation works through the normal engine).
pub const FOR_VALUE_PROPERTY: &str = "@for";

/// Runtime description of a `For` node, kept on its element: the loop
/// variable, the iterable expression, and the prototype subtree cloned per
/// row by [`crate::binding::Engine::sync_for_nodes`].
#[derive(Debug, Clone)]
pub struct ForBinding {
    /// Loop variable name (`item`).
    pub variable: String,
    /// Iterable expression (evaluates to a `Value::Model`).
    pub iterable: nui_compiler::TypedExpr,
    /// Prototype children of the `For` node in the IR.
    pub prototype: Vec<nui_compiler::NodeIr>,
}

/// Scope carried by every element of one `For` row subtree: `item.label`
/// reads resolve through the nearest enclosing row scope.
#[derive(Debug, Clone)]
pub struct RowScope {
    /// Loop variable name this scope answers to.
    pub variable: String,
    /// Model being iterated.
    pub model: crate::model::ModelId,
    /// Row index this subtree renders.
    pub row: usize,
}

/// Reactive bookkeeping attached to one element property slot.
#[derive(Debug, Clone, Default)]
struct PropertySlot {
    /// Current value (the animation layer reads through this); `None` until
    /// the first write or typed seed.
    value: Option<Value>,
    /// `<-` reactive binding, if the property carries one.
    binding: Option<Binding>,
    /// `<=>` partner, if the property participates in a two-way link.
    two_way: Option<TwoWayLink>,
    /// Whether the value changed since the last propagation pass.
    dirty: bool,
}

/// An instantiated node.
#[derive(Debug)]
pub struct Element {
    /// Node type name (`Window`, `Button`, ...).
    pub ty: String,
    /// `id = root` name, if any.
    pub id: Option<String>,
    /// Whether this element is the component instance itself (`root` and
    /// bare component-property reads land here).
    pub is_component_instance: bool,
    /// `<=>` assignments awaiting partner resolution after the id table is
    /// built: `(property, partner expression)` — the partner address lives
    /// in the assignment's value expression (`text <=> root.userName`).
    pub pending_two_way: Vec<(String, nui_compiler::TypedExpr)>,
    /// Parent handle (`None` for roots).
    pub parent: Option<ElementId>,
    /// Children in document order.
    pub children: Vec<ElementId>,
    /// Property slots by name; created on first write.
    properties: HashMap<String, PropertySlot>,
    /// State machines declared on this element's component, by name.
    pub machines: Vec<MachineInstance>,
    /// Handlers declared on this node: `(signal, effect)`.
    pub handlers: Vec<HandlerEntry>,
    /// `when` blocks: evaluated each frame; assignments apply while true.
    pub when_blocks: Vec<WhenEntry>,
    /// Timer state (`Timer` nodes): pending interval; `None` = stopped.
    pub timer_interval: Option<nui_core::Duration>,
    /// `For` runtime binding (template + variable), present on `For`
    /// elements only; rows are instantiated by the engine's model sync.
    pub for_binding: Option<ForBinding>,
    /// Row scope when this element belongs to a `For` row subtree.
    pub for_scope: Option<RowScope>,
    /// Host behavior of a custom Rust component instance (plan §5 宿主
    /// 互操作), created from the registry at instantiation.
    pub behavior: Option<Box<dyn crate::registry::ElementBehavior>>,
    /// Shared command buffer when the element is a `Canvas` (FUTURE batch
    /// 3): the host behavior paints through [`crate::canvas::CanvasPainter`]
    /// and the scene builder interprets the ops. Clones share the buffer.
    pub canvas: Option<crate::canvas::CanvasPainter>,
    /// Whether the element participates in Tab focus cycling.
    pub focusable: bool,
    /// Editing state when this element is a `TextInput` (M7).
    pub text_input: Option<crate::text_input::TextInputState>,
}

/// A handler entry: signal name plus compiled effect bytecode.
#[derive(Debug, Clone)]
pub struct HandlerEntry {
    /// Signal that triggers the handler.
    pub signal: String,
    /// Effect statements.
    pub effect: Vec<nui_compiler::Effect>,
}

/// A `when` block entry: condition plus assignments applied while it holds.
#[derive(Debug, Clone)]
pub struct WhenEntry {
    /// Condition expression (Bool).
    pub condition: nui_compiler::TypedExpr,
    /// Assignments applied while the condition holds.
    pub assignments: Vec<nui_compiler::AssignmentIr>,
}

impl Element {
    /// Creates an empty element of the given type.
    pub fn new(ty: impl Into<String>, id: Option<String>) -> Element {
        return Element {
            ty: ty.into(),
            id,
            is_component_instance: false,
            pending_two_way: Vec::new(),
            parent: None,
            children: Vec::new(),
            properties: HashMap::new(),
            machines: Vec::new(),
            handlers: Vec::new(),
            when_blocks: Vec::new(),
            timer_interval: None,
            for_binding: None,
            for_scope: None,
            behavior: None,
            canvas: None,
            focusable: false,
            text_input: None,
        };
    }

    /// Reads a property value; `None` when the name was never written.
    pub fn get(&self, name: &str) -> Option<&Value> {
        return self
            .properties
            .get(name)
            .and_then(|slot| return slot.value.as_ref());
    }

    /// Reads a property value with a fallback default.
    pub fn get_or(&self, name: &str, default: Value) -> Value {
        return self.get(name).cloned().unwrap_or(default);
    }

    /// Writes the value and returns whether it changed (dirty marking).
    pub fn set(&mut self, name: &str, value: Value) -> bool {
        let slot = self.properties.entry(name.to_string()).or_default();
        if slot.value.as_ref() == Some(&value) {
            return false;
        }
        slot.value = Some(value);
        slot.dirty = true;
        return true;
    }

    /// Attaches a `<-` binding to the property (replacing any previous one,
    /// per the single-binding-per-property rule).
    pub fn set_binding(&mut self, name: &str, binding: Binding) {
        let slot = self.properties.entry(name.to_string()).or_default();
        slot.binding = Some(binding);
    }

    /// Removes the `<-` binding from the property (D10: effect-block writes
    /// clear reactive bindings).
    pub fn clear_binding(&mut self, name: &str) {
        if let Some(slot) = self.properties.get_mut(name) {
            slot.binding = None;
        }
    }

    /// Attaches a `<=>` two-way link.
    pub fn set_two_way(&mut self, name: &str, link: TwoWayLink) {
        let slot = self.properties.entry(name.to_string()).or_default();
        slot.two_way = Some(link);
    }

    /// Whether the property currently has a `<-` binding.
    pub fn has_binding(&self, name: &str) -> bool {
        return self
            .properties
            .get(name)
            .is_some_and(|slot| return slot.binding.is_some());
    }

    /// Whether this element can take keyboard focus.
    pub fn is_focusable(&self) -> bool {
        return self.focusable;
    }

    /// Marks the property dirty.
    pub fn mark_dirty(&mut self, name: &str) {
        if let Some(slot) = self.properties.get_mut(name) {
            slot.dirty = true;
        }
    }

    /// Takes the dirty flag (read-and-clear).
    pub fn take_dirty(&mut self, name: &str) -> bool {
        return self
            .properties
            .get_mut(name)
            .map(|slot| return std::mem::take(&mut slot.dirty))
            .unwrap_or(false);
    }

    /// Iterates property names with dirty values (for change notification).
    pub fn dirty_property_names(&self) -> Vec<String> {
        return self
            .properties
            .iter()
            .filter(|(_, slot)| return slot.dirty)
            .map(|(name, _)| return name.clone())
            .collect();
    }

    /// Iterates `(name, binding)` pairs of bound properties.
    pub fn bindings(&self) -> Vec<(String, BindingIndex)> {
        let mut out = Vec::new();
        for (name, slot) in &self.properties {
            if let Some(binding) = &slot.binding {
                out.push((name.clone(), binding.index));
            }
        }
        return out;
    }

    /// Iterates `(name, link)` pairs of two-way properties.
    pub fn two_way_links(&self) -> Vec<(String, TwoWayLink)> {
        let mut out = Vec::new();
        for (name, slot) in &self.properties {
            if let Some(link) = &slot.two_way {
                out.push((name.clone(), link.clone()));
            }
        }
        return out;
    }

    /// Property names carrying a `<-` binding or `<=>` link (re-evaluation
    /// seeds after a two-way write).
    pub fn reactive_property_names(&self) -> Vec<String> {
        let mut out = Vec::new();
        for (name, slot) in &self.properties {
            if slot.binding.is_some() || slot.two_way.is_some() {
                out.push(name.clone());
            }
        }
        return out;
    }
}

/// The element arena: flat storage plus root list and id resolution.
#[derive(Debug, Default)]
pub struct ElementTree {
    /// Slot arena of all elements.
    pub arena: SlotMap<ElementId, Element>,
    /// Root elements in document order.
    pub roots: Vec<ElementId>,
    /// `id` name -> element handle (built after construction, per plan §5:
    /// ids resolve after the tree is built and before bindings evaluate).
    pub ids: HashMap<String, ElementId>,
}

impl ElementTree {
    /// Creates an empty tree.
    pub fn new() -> ElementTree {
        return ElementTree::default();
    }

    /// Inserts an element and returns its handle.
    pub fn insert(&mut self, element: Element) -> ElementId {
        return self.arena.insert(element);
    }

    /// Appends `child` to `parent`'s children and sets the back-pointer.
    pub fn append_child(&mut self, parent: ElementId, child: ElementId) {
        self.arena[child].parent = Some(parent);
        self.arena[parent].children.push(child);
    }

    /// Appends a root element.
    pub fn push_root(&mut self, root: ElementId) {
        self.roots.push(root);
    }

    /// Registers an id name for an element (later declaration wins for
    /// duplicate names; the compiler already reports duplicates).
    pub fn register_id(&mut self, name: &str, id: ElementId) {
        self.ids.insert(name.to_string(), id);
    }

    /// Resolves an `id` name to a handle.
    pub fn lookup_id(&self, name: &str) -> Option<ElementId> {
        return self.ids.get(name).copied();
    }

    /// Resolves a property read target to an element handle.
    ///
    /// `Component`/`Root` reads land on the component instance, which in M2
    /// is the tree's first root element (one document = one component
    /// instance; nested component instances arrive with M4 models). `MachineState`
    /// is intercepted by the engine before property lookup.
    pub fn resolve_target(
        &self,
        element: ElementId,
        target: &nui_compiler::PropertyTarget,
    ) -> Option<ElementId> {
        return match target {
            nui_compiler::PropertyTarget::Component(_) | nui_compiler::PropertyTarget::Root(_) => {
                self.roots.first().copied()
            }
            nui_compiler::PropertyTarget::Parent(_) => {
                return self.arena[element].parent;
            }
            nui_compiler::PropertyTarget::Id(name, _) => return self.lookup_id(name),
            nui_compiler::PropertyTarget::MachineState(_, _) => Some(element),
        };
    }

    /// Depth-first pre-order visit of the whole tree.
    pub fn visit_pre_order(&self, mut visit: impl FnMut(ElementId, &Element)) {
        fn walk(tree: &ElementTree, id: ElementId, visit: &mut impl FnMut(ElementId, &Element)) {
            let element = &tree.arena[id];
            visit(id, element);
            for child in &element.children {
                walk(tree, *child, visit);
            }
        }
        for root in &self.roots {
            walk(self, *root, &mut visit);
        }
    }

    /// Removes an element and its whole subtree from the arena: detaches it
    /// from its parent (or the root list), drops id registrations that point
    /// into the subtree, and frees the slots. Callers must retire any
    /// engine state referencing the removed handles first.
    pub fn remove_subtree(&mut self, id: ElementId) {
        let mut removed = Vec::new();
        collect_subtree(self, id, &mut removed);
        if let Some(parent) = self.arena[id].parent {
            self.arena[parent]
                .children
                .retain(|child| return *child != id);
        } else {
            self.roots.retain(|root| return *root != id);
        }
        for handle in &removed {
            let element = &self.arena[*handle];
            if let Some(name) = &element.id
                && self.ids.get(name) == Some(handle)
            {
                self.ids.remove(name);
            }
        }
        for handle in removed {
            self.arena.remove(handle);
        }
    }
}

/// Collects a subtree's handles (root included), pre-order.
fn collect_subtree(tree: &ElementTree, id: ElementId, out: &mut Vec<ElementId>) {
    out.push(id);
    for child in &tree.arena[id].children {
        collect_subtree(tree, *child, out);
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;

    #[test]
    fn set_reports_change_and_marks_dirty_once() {
        let mut element = Element::new("Text", None);
        assert!(element.set("opacity", Value::Float(1.0)));
        assert!(!element.set("opacity", Value::Float(1.0)));
        assert!(element.take_dirty("opacity"));
        assert!(!element.take_dirty("opacity"));
    }

    #[test]
    fn get_returns_written_value_and_none_before_write() {
        let mut element = Element::new("Text", None);
        assert!(element.get("content").is_none());
        element.set("content", Value::String("hi".to_string()));
        assert_eq!(
            element.get("content"),
            Some(&Value::String("hi".to_string()))
        );
    }

    #[test]
    fn binding_flags_roundtrip() {
        let mut element = Element::new("Text", None);
        assert!(!element.has_binding("title"));
        element.set_binding(
            "title",
            Binding {
                index: BindingIndex(3),
            },
        );
        assert!(element.has_binding("title"));
        element.clear_binding("title");
        assert!(!element.has_binding("title"));
    }

    #[test]
    fn tree_ids_resolve_after_registration() {
        let mut tree = ElementTree::new();
        let button = tree.insert(Element::new("Button", Some("btn".to_string())));
        tree.register_id("btn", button);
        assert_eq!(tree.lookup_id("btn"), Some(button));
        assert_eq!(tree.lookup_id("nope"), None);
    }

    #[test]
    fn append_child_sets_parent_and_order() {
        let mut tree = ElementTree::new();
        let parent = tree.insert(Element::new("Column", None));
        let first = tree.insert(Element::new("Text", None));
        let second = tree.insert(Element::new("Text", None));
        tree.append_child(parent, first);
        tree.append_child(parent, second);
        assert_eq!(tree.arena[first].parent, Some(parent));
        assert_eq!(tree.arena[parent].children, vec![first, second]);
    }

    #[test]
    fn pre_order_walks_parents_before_children() {
        let mut tree = ElementTree::new();
        let root = tree.insert(Element::new("Window", None));
        let a = tree.insert(Element::new("Text", None));
        let b = tree.insert(Element::new("Button", None));
        tree.append_child(root, a);
        tree.append_child(a, b);
        tree.push_root(root);
        let mut seen = Vec::new();
        tree.visit_pre_order(|_, element| seen.push(element.ty.clone()));
        assert_eq!(seen, vec!["Window", "Text", "Button"]);
    }
}
