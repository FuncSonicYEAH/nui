//! Registry: host extensibility (plan §5 宿主互操作). The host registers
//! custom component types (a property descriptor plus a per-instance
//! behavior implemented in Rust) and functions callable from nui effects
//! and expressions. The compiler validates function names against the
//! registry's names via
//! [`compile_with_functions`](nui_compiler::compile_with_functions), so a
//! typo stays a compile-time error.
//!
//! # Two kinds of callable
//!
//! A [`HostFunction`] computes: it is called while an expression is being
//! evaluated, sees only its arguments, and returns a [`Value`]. A
//! [`HostCommand`] acts: it is called from an effect *statement*, and gets
//! a [`BehaviorContext`] — property writes, signals, models — the same
//! channel an element behavior uses.
//!
//! The split is forced by where the two run, not by taste. Expression
//! evaluation borrows the engine immutably and tracks the properties it
//! read to know what to recompute; a call that wrote a property there could
//! not be accounted for. Statement execution owns the engine mutably and
//! already expects to change things.
//!
//! Both are registered by name in one namespace, so a document's calls are
//! checked against a single list ([`Registry::vocabulary`]); a command is
//! additionally marked as one, so using it where a value is expected is a
//! compile error.

use std::collections::HashMap;

use nui_compiler::Type;
use nui_core::Value;

use crate::binding::{Engine, EvalError};
use crate::element::{Element, ElementId, ElementTree};

/// Error returned by a host function call.
#[derive(Debug, Clone, PartialEq)]
pub struct FunctionError {
    /// What went wrong.
    pub message: String,
}

impl FunctionError {
    /// Creates an error with a message.
    pub fn new(message: impl Into<String>) -> FunctionError {
        return FunctionError {
            message: message.into(),
        };
    }
}

impl std::fmt::Display for FunctionError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        return write!(formatter, "{}", self.message);
    }
}

impl std::error::Error for FunctionError {}

/// A command's own failing and the engine's are the same kind of news to
/// the host, so a command can use `?` on [`BehaviorContext`] calls
/// (`context.emit(...)?`) instead of converting by hand at every line.
impl From<EvalError> for FunctionError {
    fn from(error: EvalError) -> FunctionError {
        return FunctionError::new(error.to_string());
    }
}

/// A host function callable from nui: maps evaluated arguments to a result.
pub type HostFunction = Box<dyn Fn(&[Value]) -> Result<Value, FunctionError>>;

/// A host *command*: an action callable from an effect statement, which may
/// touch the engine and the tree.
///
/// The difference from [`HostFunction`] is what the two are allowed to do,
/// not how a document spells them. A value function is called while an
/// expression is being evaluated — the engine is borrowed immutably there,
/// and a dependency-tracking walk has no way to notice a property the call
/// changed — so it can only compute. A command is called from a statement,
/// where the engine is `&mut`, and it gets the same [`BehaviorContext`] an
/// element behavior gets: property writes, signals, models.
///
/// It receives the element whose effect called it, which is the element a
/// document would have written the call on.
pub type HostCommand =
    Box<dyn Fn(&mut BehaviorContext<'_>, ElementId, &[Value]) -> Result<(), FunctionError>>;

/// A registered command, as stored: a shared handle rather than a `Box`.
///
/// The engine has to hold the command *and* hand the command a mutable
/// borrow of itself, which one `Box` cannot do — and unlike an element
/// behavior, which is taken off the element for the call, a command stays
/// registered while it runs. Sharing is what lets both be true at once.
pub type HostCommandRef =
    std::rc::Rc<dyn Fn(&mut BehaviorContext<'_>, ElementId, &[Value]) -> Result<(), FunctionError>>;

/// One entry in the host callable table: a name is either a value or a
/// command, and the registry keeps them in one namespace so that a document
/// can be checked against a single list of names.
enum HostCallable {
    /// Returns a value; callable from an expression.
    Value(HostFunction),
    /// Acts; callable from a statement.
    Command(HostCommandRef),
}

/// Factory creating one behavior per instantiated element.
pub type BehaviorFactory = Box<dyn Fn() -> Box<dyn ElementBehavior>>;

/// Engine-attach hook (register models, seed state); re-run on hot reload.
pub type AttachHook = Box<dyn Fn(&mut Engine, &mut ElementTree)>;

/// Descriptor of a host-registered component type: its property surface
/// with typed defaults, applied to instances at instantiation.
#[derive(Debug, Clone, Default)]
pub struct ComponentDesc {
    /// Component type name as written in `.nui` nodes.
    pub name: String,
    /// Property descriptors (name, type, optional default).
    pub properties: Vec<PropertyDescriptor>,
    /// How instances of this component behave under the pointer and the
    /// keyboard.
    ///
    /// The default (`kind: None`) makes the component inert, which is the
    /// right answer for a component that is only a layout wrapper. Setting
    /// a kind is what turns it into a control: the widget tracker then
    /// writes `hovered` / `pressed` / `armed` / `focused` / `disabled` on
    /// every instance, Space and Enter activate it, and a click runs
    /// [`ElementBehavior::on_signal`] — the same contract the built-in
    /// `Button` gets from its type name.
    ///
    /// This is per *instance* state on purpose. A component *property* is
    /// persistent (declared once, own storage, survives every frame), so
    /// modelling hover as one would cost a property slot per control and
    /// still have no way to notice a pointer that left mid-gesture.
    pub interaction: crate::widget::Interaction,
}

impl ComponentDesc {
    /// A descriptor for a component that is not a control: no hover, no
    /// press, no keyboard, no Tab stop. The right answer for a component
    /// that only arranges other things.
    pub fn new(name: impl Into<String>) -> ComponentDesc {
        return ComponentDesc {
            name: name.into(),
            properties: Vec::new(),
            interaction: crate::widget::Interaction::default(),
        };
    }

    /// Builder: declares this component a control.
    pub fn with_interaction(mut self, interaction: crate::widget::Interaction) -> ComponentDesc {
        self.interaction = interaction;
        return self;
    }

    /// Builder: adds a property descriptor.
    pub fn with_property(mut self, property: PropertyDescriptor) -> ComponentDesc {
        self.properties.push(property);
        return self;
    }
}

/// One property of a [`ComponentDesc`].
#[derive(Debug, Clone)]
pub struct PropertyDescriptor {
    /// Property name.
    pub name: String,
    /// Property type (used to seed reads before the first write).
    pub ty: Type,
    /// Default value; `None` seeds the type's zero value.
    pub default: Option<Value>,
}

/// The engine-facing channel a behavior uses to touch the tree.
pub struct BehaviorContext<'a> {
    engine: &'a mut Engine,
    tree: &'a mut ElementTree,
}

impl BehaviorContext<'_> {
    /// Writes a property through the engine (invalidates dependents and
    /// buffers a [`ChangeSource::Host`](crate::notify::ChangeSource::Host)
    /// change). Returns whether the value changed.
    pub fn set_property(&mut self, element: ElementId, property: &str, value: Value) -> bool {
        return self.engine.set_direct(self.tree, element, property, value);
    }

    /// Reads a property value.
    pub fn property(&self, element: ElementId, property: &str) -> Option<Value> {
        return self.tree.arena[element].get(property).cloned();
    }

    /// Resolves an element `id` — the name a document writes — to its
    /// element, so a command can address one it was not called on.
    pub fn lookup_id(&self, id: &str) -> Option<ElementId> {
        return self.tree.lookup_id(id);
    }

    /// The element type name of `element` (`"Button"`, `"Text"`, …).
    pub fn element_type(&self, element: ElementId) -> &str {
        return &self.tree.arena[element].ty;
    }

    /// Emits a signal from an element (fan-out to handlers, machines, and
    /// behaviors).
    pub fn emit(&mut self, element: ElementId, signal: &str) -> Result<(), EvalError> {
        let _ = self.engine.emit_signal(self.tree, element, signal)?;
        return Ok(());
    }

    /// The `For` row scope enclosing `element`, if it lives inside a row
    /// subtree (custom components inside `For` prototypes use this to
    /// address their model row).
    pub fn row_scope(&self, element: ElementId) -> Option<crate::element::RowScope> {
        let mut current = element;
        loop {
            let node = &self.tree.arena[current];
            if let Some(scope) = &node.for_scope {
                return Some(scope.clone());
            }
            current = node.parent?;
        }
    }

    /// Reads one field of the model row at `scope`.
    pub fn model_field(&self, scope: &crate::element::RowScope, field: &str) -> Option<Value> {
        return self.engine.model_field(scope.model, scope.row, field);
    }

    /// Writes one field of the model row at `scope` (invalidates the row's
    /// bindings).
    pub fn set_model_field(
        &mut self,
        scope: &crate::element::RowScope,
        field: &str,
        value: Value,
    ) -> bool {
        return self
            .engine
            .model_set_field(self.tree, scope.model, scope.row, field, value);
    }

    /// Removes the model row at `scope` (the next frame rebuilds the `For`
    /// rows).
    pub fn remove_model_row(&mut self, scope: &crate::element::RowScope) -> bool {
        return self.engine.model_remove(scope.model, scope.row);
    }
}

/// Runtime behavior of one instance of a custom Rust component.
pub trait ElementBehavior: std::fmt::Debug {
    /// Called when a signal reaches the element, after the element's DSL
    /// handlers and machines have run.
    fn on_signal(&mut self, context: &mut BehaviorContext<'_>, element: ElementId, signal: &str);

    /// The painter this behavior exposes for `Canvas` elements (FUTURE
    /// batch 3): the engine stores it on the element so the scene builder
    /// can interpret the command buffer. `None` (the default) for
    /// behaviors without a canvas.
    fn canvas(&self) -> Option<crate::canvas::CanvasPainter> {
        return None;
    }
}

/// The host extension registry: custom components and host functions.
#[derive(Default)]
pub struct Registry {
    /// Every callable name, both values and commands: one namespace, so a
    /// document's names can be checked against one list.
    functions: HashMap<String, HostCallable>,
    components: HashMap<String, ComponentDesc>,
    behaviors: HashMap<String, BehaviorFactory>,
    /// Hook run by the host every time an engine attaches the registry —
    /// on construction and again on each hot reload (models must be
    /// re-registered because the engine owning them is rebuilt).
    init: Option<AttachHook>,
}

impl std::fmt::Debug for Registry {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        // Closures are not Debug; report the registered names.
        let mut names = self.functions.keys().cloned().collect::<Vec<_>>();
        names.sort();
        let mut behaviors = self.behaviors.keys().cloned().collect::<Vec<_>>();
        behaviors.sort();
        return formatter
            .debug_struct("Registry")
            .field("functions", &names)
            .field("components", &self.components)
            .field("behaviors", &behaviors)
            .finish();
    }
}

impl Registry {
    /// Creates an empty registry.
    pub fn new() -> Registry {
        return Registry::default();
    }

    /// Registers a function callable from nui effects and expressions.
    pub fn register_function(&mut self, name: &str, function: HostFunction) {
        self.functions
            .insert(name.to_string(), HostCallable::Value(function));
    }

    /// Registers a *command*: an action callable from an effect statement,
    /// which may touch the engine and the tree (see [`HostCommand`]).
    ///
    /// It counts as a callable name for the compiler like any other, so a
    /// document needs no declaration for it — but the compiler is told
    /// which names are commands, so using one where a value is expected is
    /// a compile error rather than a silently-dropped evaluation failure.
    pub fn register_command(&mut self, name: &str, command: HostCommand) {
        self.functions.insert(
            name.to_string(),
            HostCallable::Command(HostCommandRef::from(command)),
        );
    }

    /// Registers a custom component type: instances get the descriptor's
    /// property defaults and, when a factory is given, a behavior.
    pub fn register_component(&mut self, desc: ComponentDesc, behavior: Option<BehaviorFactory>) {
        if let Some(factory) = behavior {
            self.behaviors.insert(desc.name.clone(), factory);
        }
        self.components.insert(desc.name.clone(), desc);
    }

    /// Registers a one-shot hook the host runs after engine + tree exist —
    /// the seam for registering models and seeding initial state.
    pub fn on_attach(mut self, hook: AttachHook) -> Registry {
        self.init = Some(hook);
        return self;
    }

    /// The registered *value* function with `name`, if any.
    ///
    /// A command is deliberately not answered here: the caller is an
    /// expression, and a command has no value to give it.
    pub fn function(&self, name: &str) -> Option<&HostFunction> {
        return match self.functions.get(name) {
            Some(HostCallable::Value(function)) => Some(function),
            _ => None,
        };
    }

    /// The registered command with `name`, if any. The handle is shared
    /// rather than borrowed, so a caller can run the command while holding
    /// a mutable borrow of the engine (see [`HostCommandRef`]).
    pub fn command(&self, name: &str) -> Option<HostCommandRef> {
        return match self.functions.get(name) {
            Some(HostCallable::Command(command)) => Some(command.clone()),
            _ => None,
        };
    }

    /// The registered component descriptor with `name`, if any.
    pub fn component(&self, name: &str) -> Option<&ComponentDesc> {
        return self.components.get(name);
    }

    /// The registered behavior factory for the node type, if any.
    pub fn behavior(&self, type_name: &str) -> Option<&BehaviorFactory> {
        return self.behaviors.get(type_name);
    }

    /// All registered callable names — values and commands — which is what
    /// the compiler needs to accept a document's calls at all
    /// ([`compile_with_host`](nui_compiler::compile_with_host)).
    pub fn function_names(&self) -> Vec<String> {
        return self.functions.keys().cloned().collect();
    }

    /// The subset of [`Self::function_names`] that are commands, so the
    /// compiler can reject a command used where a value is expected.
    pub fn command_names(&self) -> Vec<String> {
        return self
            .functions
            .iter()
            .filter(|(_, callable)| return matches!(callable, HostCallable::Command(_)))
            .map(|(name, _)| return name.clone())
            .collect();
    }

    /// The vocabulary to compile a document against: every callable name,
    /// which of them act rather than return, and the type names the host
    /// registered.
    ///
    /// The component names matter for the same reason the function names
    /// do, and their absence was a real gap: the compiler only knows the
    /// *built-in* element types, so a document instantiating a
    /// host-registered component looked identical to one instantiating a
    /// type that does not exist. Reporting the names here is what lets the
    /// checker keep the typo diagnostic (`Buton(...)`) without rejecting
    /// the host's own types.
    ///
    /// Built per document load (and again per hot reload) rather than
    /// cached, so a host that registers a component mid-session gets the
    /// checker's answer on the next compile like any other registration.
    pub fn vocabulary(&self) -> nui_compiler::HostVocabulary {
        // `with_commands` counts the commands as callable names too, so the
        // separate list carries the *values* only — otherwise a command
        // would appear in the vocabulary twice.
        let values = self
            .functions
            .iter()
            .filter(|(_, callable)| return matches!(callable, HostCallable::Value(_)))
            .map(|(name, _)| return name.clone());
        return nui_compiler::HostVocabulary::new()
            .with_functions(values)
            .with_commands(self.command_names())
            .with_components(self.components.keys().cloned());
    }
}

impl Engine {
    /// The engine's registry (host functions and custom components).
    pub fn registry(&self) -> &Registry {
        return &self.registry;
    }

    /// The engine's registry, mutably (register more hosts mid-session).
    pub fn registry_mut(&mut self) -> &mut Registry {
        return &mut self.registry;
    }

    /// Calls a registered host function; `None`-mapped to an eval error so
    /// a failed lookup marks the calling binding clean like any other
    /// evaluation failure.
    pub(crate) fn call_host_function(
        &self,
        name: &str,
        args: &[Value],
    ) -> Result<Value, EvalError> {
        let Some(function) = self.registry.function(name) else {
            return Err(EvalError::Unresolved {
                what: match self.registry.command(name) {
                    // The name exists and does something; it just has
                    // nothing to give an expression. Reachable only from a
                    // document compiled without the host's command list.
                    Some(_) => format!("`{name}` is a command and has no value"),
                    None => format!("unknown function `{name}`"),
                },
            });
        };
        return function(args).map_err(|error| {
            return EvalError::Unresolved {
                what: format!("function `{name}`: {error}"),
            };
        });
    }

    /// Runs a registered host command from an effect statement, handing it
    /// the same [`BehaviorContext`] an element behavior gets.
    ///
    /// `element` is the element whose effect called it. A name that is not
    /// a command but *is* a value function falls through to
    /// [`Self::call_host_function`] and its result is dropped: `log("x")`
    /// as a statement is reasonable, and the reverse (a command read as a
    /// value) is the one that cannot be made to work.
    pub(crate) fn call_host_command(
        &mut self,
        tree: &mut ElementTree,
        element: ElementId,
        name: &str,
        args: &[Value],
    ) -> Result<(), EvalError> {
        let Some(command) = self.registry.command(name) else {
            self.call_host_function(name, args)?;
            return Ok(());
        };
        let mut context = BehaviorContext { engine: self, tree };
        return command(&mut context, element, args).map_err(|error| {
            return EvalError::Unresolved {
                what: format!("command `{name}`: {error}"),
            };
        });
    }

    /// Runs the registry's attach hook, if any (host seam for registering
    /// models and seeding state). Runs on every engine attach — including
    /// each hot reload, because the engine owning the models is rebuilt.
    pub fn run_registry_init(&mut self, tree: &mut ElementTree) {
        if self.registry.init.is_none() {
            return;
        }
        let hook = self.registry.init.take();
        if let Some(init) = &hook {
            init(self, tree);
        }
        self.registry.init = hook;
    }

    /// Runs the behavior of `element`, if it carries one. The behavior is
    /// taken out for the call so the context can borrow the tree.
    pub(crate) fn run_behavior(
        &mut self,
        tree: &mut ElementTree,
        element: ElementId,
        signal: &str,
    ) -> Result<(), EvalError> {
        if tree.arena[element].behavior.is_none() {
            return Ok(());
        }
        let mut behavior = tree.arena[element].behavior.take();
        if let Some(behavior) = &mut behavior {
            let mut context = BehaviorContext { engine: self, tree };
            behavior.on_signal(&mut context, element, signal);
        }
        tree.arena[element].behavior = behavior;
        return Ok(());
    }
}

/// Applies a component descriptor's property defaults to a fresh element
/// (properties not assigned at instantiation get their typed default).
pub(crate) fn apply_descriptor_defaults(element: &mut Element, desc: &ComponentDesc) {
    for property in &desc.properties {
        if element.get(&property.name).is_some() {
            continue;
        }
        let value = property
            .default
            .clone()
            .unwrap_or_else(|| return crate::instantiate::zero_value_for(property.ty));
        element.set(&property.name, value);
    }
}
