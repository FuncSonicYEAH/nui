//! nui-runtime: the nui engine.
//!
//! M2 contents: the slotmap element tree ([`element`]), the binding engine
//! with dynamic dependency tracking and two-phase propagation
//! ([`binding`]), first-class state machines ([`machine`]), the animation
//! clock ([`animation`]), and Document-IR instantiation ([`instantiate`]).
//! Property change notification ([`notify`]) is the M4 `Model` seam: the
//! engine buffers every mediated write and hosts drain it per frame.
//! Layout lives in nui-layout; rendering in nui-render (M3).
//!
//! # Examples
//! ```
//! let outcome = nui_compiler::compile(
//!     "component Counter { property count: Int = 0 Text(content <- \"n: {count}\") {} }",
//! );
//! let mut instance = nui_runtime::instantiate(&outcome.document);
//! ```

pub mod animation;
pub mod binding;
pub mod canvas;
pub mod element;
pub mod instantiate;
pub mod machine;
pub mod model;
pub mod notify;
pub mod registry;
pub mod text_input;
pub mod widget;

pub use animation::{AnimationClock, AnimationKind, Easing};
pub use binding::{Binding, BindingIndex, Engine, EvalError, TwoWayLink};
pub use canvas::{CanvasCap, CanvasOp, CanvasPainter};
pub use element::{Element, ElementId, ElementTree, ForBinding, HandlerEntry, RowScope, WhenEntry};
pub use instantiate::{Instance, instantiate, instantiate_with, reload_from_source};
pub use machine::MachineInstance;
pub use model::{Model, ModelId, ModelRow, VecModel};
pub use notify::{ChangeSource, ObserverHandle, PropertyChange, PropertyObserver};
pub use registry::{
    AttachHook, BehaviorContext, BehaviorFactory, ComponentDesc, ElementBehavior, FunctionError,
    HostFunction, PropertyDescriptor, Registry,
};
pub use text_input::TextInputState;
pub use widget::{PointerInput, WidgetKind, WidgetState, WidgetStates, is_widget_type};
