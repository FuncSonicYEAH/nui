//! Registry: host extensibility (plan §5 宿主互操作). The host registers
//! custom component types (a property descriptor plus a per-instance
//! behavior implemented in Rust) and functions callable from nui effects
//! and expressions. The compiler validates function names against the
//! registry's names via
//! [`compile_with_functions`](nui_compiler::compile_with_functions), so a
//! typo stays a compile-time error.

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

/// A host function callable from nui: maps evaluated arguments to a result.
pub type HostFunction = Box<dyn Fn(&[Value]) -> Result<Value, FunctionError>>;

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
}

/// The host extension registry: custom components and host functions.
#[derive(Default)]
pub struct Registry {
    functions: HashMap<String, HostFunction>,
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
        self.functions.insert(name.to_string(), function);
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

    /// The registered function with `name`, if any.
    pub fn function(&self, name: &str) -> Option<&HostFunction> {
        return self.functions.get(name);
    }

    /// The registered component descriptor with `name`, if any.
    pub fn component(&self, name: &str) -> Option<&ComponentDesc> {
        return self.components.get(name);
    }

    /// The registered behavior factory for the node type, if any.
    pub fn behavior(&self, type_name: &str) -> Option<&BehaviorFactory> {
        return self.behaviors.get(type_name);
    }

    /// All registered function names (input for
    /// [`compile_with_functions`](nui_compiler::compile_with_functions)).
    pub fn function_names(&self) -> Vec<String> {
        return self.functions.keys().cloned().collect();
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
                what: format!("unknown function `{name}`"),
            });
        };
        return function(args).map_err(|error| {
            return EvalError::Unresolved {
                what: format!("function `{name}`: {error}"),
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
