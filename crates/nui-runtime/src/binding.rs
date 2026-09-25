//! Binding engine: TypedExpr evaluation, dynamic dependency tracking, and
//! pull-based frame-start propagation.
//!
//! Plan §5 semantics:
//! - Evaluation runs under a thread-local "current evaluation stack"; every
//!   property read while a binding evaluates records a dependency edge
//!   `read property -> evaluating binding`.
//! - Reads pull: reading a property whose binding is dirty evaluates that
//!   binding first (topological order for free); writes mark dependent
//!   bindings dirty via reverse edges. Re-entrant evaluation of a binding
//!   inside its own evaluation is the runtime cycle check (plan §12) and
//!   reports an error instead of looping.
//! - Effect-block writes clear the target's `<-` binding (D10); `<=>` writes
//!   go through the value channel and sync the partner side.

use std::cell::RefCell;
use std::collections::HashMap;

use nui_compiler::{Builtin, PropertyTarget, TypedExpr};
use nui_core::{Length, Value};

use crate::element::{ElementId, ElementTree};
use crate::notify::{ChangeSource, ObserverHandle, Observers, PropertyChange, PropertyObserver};

/// Identifies one binding site: the element and property that carry it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct BindingIndex(pub usize);

/// A `<-` reactive binding attached to a property slot.
#[derive(Debug, Clone)]
pub struct Binding {
    /// Opaque registration index handed out by the engine.
    pub index: BindingIndex,
}

/// A `<=>` two-way link between two property slots.
#[derive(Debug, Clone)]
pub struct TwoWayLink {
    /// Partner element.
    pub partner: ElementId,
    /// Partner property name.
    pub property: String,
}

/// Evaluation error: cycle detected or unresolvable read.
#[derive(Debug, Clone, PartialEq)]
pub enum EvalError {
    /// A binding re-entered during its own evaluation (binding cycle).
    Cycle {
        /// Chain of element.property reads, in re-entry order.
        chain: Vec<String>,
    },
    /// A name or element could not be resolved (typically a downstream
    /// consequence of a compile-time error).
    Unresolved {
        /// What could not be resolved.
        what: String,
    },
    /// Evaluation depth cap hit (safety net for dynamic cycles the static
    /// checker cannot see, e.g. through `Dynamic` member reads).
    DepthExceeded {
        /// The depth limit.
        limit: usize,
    },
}

impl std::fmt::Display for EvalError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        return match self {
            EvalError::Cycle { chain } => {
                write!(formatter, "binding cycle: {}", chain.join(" <- "))
            }
            EvalError::Unresolved { what } => {
                write!(formatter, "unresolved: {what}")
            }
            EvalError::DepthExceeded { limit } => {
                write!(formatter, "evaluation depth exceeded ({limit})")
            }
        };
    }
}

impl std::error::Error for EvalError {}

/// One captured pre-override value of a `when` block property.
type WhenOverride = (ElementId, String, Option<Value>);

/// Active `when` overrides keyed by `(element, condition key)`.
type WhenOverrides = HashMap<(ElementId, String), Vec<WhenOverride>>;

/// Evaluation depth cap (plan §12 second line of defense).
const MAX_EVAL_DEPTH: usize = 64;

thread_local! {
    /// Chain of `element.property` keys currently being evaluated; property
    /// reads append dependency edges against the innermost entry.
    static EVAL_STACK: RefCell<Vec<(ElementId, String)>> = const { RefCell::new(Vec::new()) };
}

/// One registered binding for propagation bookkeeping.
#[derive(Debug, Clone)]
struct BindingRecord {
    /// Element carrying the bound property.
    element: ElementId,
    /// Bound property name.
    property: String,
    /// The expression to (re)evaluate.
    expr: TypedExpr,
    /// Properties this binding last read: `(element, property)` edges.
    dependencies: Vec<(ElementId, String)>,
    /// Whether the binding needs re-evaluation.
    dirty: bool,
    /// Whether the owning element was removed (For row rebuilds). Dead
    /// records are skipped everywhere and never evaluate; the record slot
    /// stays so `BindingIndex` handles remain stable.
    dead: bool,
}

/// The reactive engine: owns evaluation and propagation for one tree.
#[derive(Debug, Default)]
pub struct Engine {
    /// All registered bindings, keyed by their stable [`BindingIndex`].
    bindings: Vec<BindingRecord>,
    /// Reverse edges: `(element, property)` -> binding indices that read it.
    readers: HashMap<(ElementId, String), Vec<usize>>,
    /// Active `when` overrides per `(element, condition key)`: the
    /// properties the block currently overrides with their pre-override
    /// values, so a leaving condition restores what the block replaced.
    when_overrides: WhenOverrides,
    /// Property changes buffered since the last drain (see [`crate::notify`]).
    changes: Vec<PropertyChange>,
    /// Subscribed property observers, notified at drain time.
    observers: Observers,
    /// Registered models (`For` iterables), keyed by [`ModelId`] index.
    pub(crate) models: Vec<Option<Box<dyn crate::model::Model>>>,
    /// `For` element sites per model index, refreshed by each
    /// [`Engine::sync_for_nodes`](crate::model) pass.
    pub(crate) model_sites: HashMap<u32, Vec<ElementId>>,
    /// Whether model mutations await a row-sync pass.
    pub(crate) models_dirty: bool,
    /// `let` locals of the effect currently executing (innermost last).
    locals: Vec<(String, Value)>,
    /// Host extension registry (custom components + host functions).
    pub(crate) registry: crate::registry::Registry,
    /// The keyboard-focused element (M7 Tab cycling + text input).
    pub(crate) focused: Option<ElementId>,
    /// Virtualized `ListView` windows: element -> (first row, row count).
    pub(crate) list_windows: HashMap<ElementId, (usize, usize)>,
    /// The animation clock (D8): `tween`/`spring` binding writes start or
    /// retarget animations here instead of writing the displayed value.
    pub(crate) clock: crate::animation::AnimationClock,
}

/// Default duration of a `tween` without an explicit `duration` argument.
const DEFAULT_TWEEN_MILLIS: f64 = 200.0;
/// Default spring parameters (plan D8 examples).
const DEFAULT_STIFFNESS: f64 = 120.0;
const DEFAULT_DAMPING: f64 = 14.0;

impl Engine {
    /// Creates an empty engine.
    pub fn new() -> Engine {
        return Engine::default();
    }

    /// Registers a binding for `element.property = expr`; returns the stable
    /// index to store in the element's property slot.
    pub fn register_binding(
        &mut self,
        element: ElementId,
        property: &str,
        expr: TypedExpr,
    ) -> BindingIndex {
        let index = BindingIndex(self.bindings.len());
        self.bindings.push(BindingRecord {
            element,
            property: property.to_string(),
            expr,
            dependencies: Vec::new(),
            dirty: true,
            dead: false,
        });
        return index;
    }

    /// Marks every binding owned by the given elements dead (their elements
    /// were removed by a `For` row rebuild) and drops the reverse edges
    /// that point at them.
    pub(crate) fn retire_bindings(&mut self, removed: &[ElementId]) {
        let removed: std::collections::HashSet<ElementId> = removed.iter().copied().collect();
        for index in 0..self.bindings.len() {
            let record = &mut self.bindings[index];
            if record.dead || !removed.contains(&record.element) {
                continue;
            }
            record.dead = true;
            record.dirty = false;
            let dependencies = std::mem::take(&mut record.dependencies);
            for (dep_element, dep_property) in dependencies {
                let key = (dep_element, dep_property);
                if let Some(readers) = self.readers.get_mut(&key) {
                    readers.retain(|reader| return *reader != index);
                    if readers.is_empty() {
                        self.readers.remove(&key);
                    }
                }
            }
        }
    }

    /// Buffers a property change (see [`crate::notify`]).
    pub(crate) fn record_change(
        &mut self,
        element: ElementId,
        property: &str,
        new_value: Value,
        source: ChangeSource,
    ) {
        self.changes.push(PropertyChange {
            element,
            property: property.to_string(),
            new_value,
            source,
        });
    }

    /// Drains the buffered property changes, notifying subscribers. Returns
    /// the changes in write order; call once per frame after propagation.
    pub fn take_changes(&mut self) -> Vec<PropertyChange> {
        let changes = std::mem::take(&mut self.changes);
        if !changes.is_empty() {
            self.observers.for_each(&mut |observer| {
                for change in &changes {
                    observer.on_property_change(change);
                }
            });
        }
        return changes;
    }

    /// Subscribes a property observer; notified on every
    /// [`Engine::take_changes`] drain.
    pub fn subscribe(&mut self, observer: Box<dyn PropertyObserver>) -> ObserverHandle {
        return self.observers.subscribe(observer);
    }

    /// Removes a previously subscribed observer.
    pub fn unsubscribe(&mut self, handle: ObserverHandle) -> bool {
        return self.observers.unsubscribe(handle);
    }

    /// Syncs a `<=>` partner through the value channel after
    /// `element.property` changed.
    pub(crate) fn sync_two_way_partner(
        &mut self,
        tree: &mut ElementTree,
        element: ElementId,
        property: &str,
    ) {
        let Some(link) = tree.arena[element]
            .two_way_links()
            .into_iter()
            .find(|(name, _)| return *name == property)
            .map(|(_, link)| return link)
        else {
            return;
        };
        let Some(value) = tree.arena[element].get(property).cloned() else {
            return;
        };
        if tree.arena[link.partner].set(&link.property, value.clone()) {
            self.invalidate(link.partner, &link.property);
            self.record_change(link.partner, &link.property, value, ChangeSource::TwoWay);
        }
    }

    /// Starts or retargets the animation a `tween`/`spring` binding drives:
    /// parameters are re-evaluated per binding evaluation so reactive
    /// `duration`/`stiffness` arguments track their sources.
    fn start_bound_animation(
        &mut self,
        tree: &mut ElementTree,
        element: ElementId,
        property: &str,
        expr: &TypedExpr,
        target: Value,
    ) -> Result<(), EvalError> {
        let current = tree.arena[element]
            .get(property)
            .cloned()
            .unwrap_or(target.clone());
        if current == target {
            // Nothing to animate (e.g. the initial evaluation targeting the
            // seed value); retire any in-flight animation.
            self.clock.cancel(element, property);
            return Ok(());
        }
        let TypedExpr::Call { func, args, .. } = expr else {
            return Ok(());
        };
        return match func {
            Builtin::Tween => {
                let duration = match args.get(1) {
                    Some(arg) => self
                        .evaluate_free(tree, element, arg)?
                        .as_duration()
                        .unwrap_or(nui_core::Duration::from_millis(DEFAULT_TWEEN_MILLIS)),
                    None => nui_core::Duration::from_millis(DEFAULT_TWEEN_MILLIS),
                };
                let easing = match args.get(2) {
                    Some(arg) => {
                        let name = self.evaluate_free(tree, element, arg)?.to_string();
                        crate::animation::Easing::from_name(&name)
                    }
                    None => crate::animation::Easing::EaseInOut,
                };
                self.clock
                    .start_tween(element, property, target, duration, easing, current);
                Ok(())
            }
            Builtin::Spring => {
                let mut numeric =
                    |engine: &mut Self, index: usize, default: f64| -> Result<f64, EvalError> {
                        return match args.get(index) {
                            Some(arg) => Ok(engine
                                .evaluate_free(tree, element, arg)?
                                .as_f64()
                                .unwrap_or(default)),
                            None => Ok(default),
                        };
                    };
                let stiffness = numeric(self, 1, DEFAULT_STIFFNESS)?;
                let damping = numeric(self, 2, DEFAULT_DAMPING)?;
                self.clock
                    .start_spring(element, property, target, stiffness, damping, current);
                Ok(())
            }
            // Math builtins are plain value functions: nothing to animate.
            Builtin::Min
            | Builtin::Max
            | Builtin::Clamp
            | Builtin::Sin
            | Builtin::Cos
            | Builtin::Tan
            | Builtin::Sqrt
            | Builtin::Abs
            | Builtin::Floor
            | Builtin::Ceil => Ok(()),
        };
    }

    /// Advances the animation clock (frame pipeline input). Returns the
    /// animated `(element, property)` pairs whose displayed value changed.
    pub fn tick_animations(
        &mut self,
        tree: &mut ElementTree,
        delta: nui_core::Duration,
    ) -> Vec<(ElementId, String)> {
        let mut clock = std::mem::take(&mut self.clock);
        let changed = clock.tick_with_notify(tree, delta, self);
        self.clock = clock;
        return changed;
    }

    /// Whether any animation is running (frame scheduling input: no dirty
    /// data and no active animation means no redraw, plan §2).
    pub fn has_active_animations(&self) -> bool {
        return self.clock.is_running();
    }

    /// Direct host write (plan §5 宿主互操作; M4 models land on this): sets
    /// the property, invalidates dependents, buffers a
    /// [`ChangeSource::Host`] change. Returns whether the value changed.
    pub fn set_direct(
        &mut self,
        tree: &mut ElementTree,
        element: ElementId,
        property: &str,
        value: Value,
    ) -> bool {
        let changed = tree.arena[element].set(property, value.clone());
        if changed {
            self.invalidate(element, property);
            self.record_change(element, property, value, ChangeSource::Host);
        }
        return changed;
    }

    /// Marks bindings reading `element.property` dirty (dependency edge
    /// invalidation).
    pub fn invalidate(&mut self, element: ElementId, property: &str) {
        let key = (element, property.to_string());
        if let Some(readers) = self.readers.get(&key) {
            for &binding_index in readers {
                self.bindings[binding_index].dirty = true;
            }
        }
    }

    /// Returns the binding registered on `element.property`, if any.
    fn binding_at(&self, element: ElementId, property: &str) -> Option<BindingIndex> {
        return self
            .bindings
            .iter()
            .position(|record| {
                return !record.dead && record.element == element && record.property == property;
            })
            .map(BindingIndex);
    }

    /// Whether any binding needs evaluation.
    pub fn has_dirty_bindings(&self) -> bool {
        return self.bindings.iter().any(|record| return record.dirty);
    }

    /// Evaluates every dirty binding to a fixed point (two-phase frame
    /// start). Returns evaluation errors; each erroring binding is marked
    /// clean so one failure does not wedge the frame.
    pub fn propagate(&mut self, tree: &mut ElementTree) -> Vec<(BindingIndex, EvalError)> {
        let mut errors = Vec::new();
        // Iterate: evaluating bindings may dirty further bindings (chains).
        loop {
            let dirty: Vec<usize> = self
                .bindings
                .iter()
                .enumerate()
                .filter(|(_, record)| return record.dirty)
                .map(|(index, _)| return index)
                .collect();
            if dirty.is_empty() {
                break;
            }
            let mut any_evaluated = false;
            for binding_index in dirty {
                let outcome = self.evaluate_binding(binding_index, tree);
                match outcome {
                    Ok(()) => {
                        any_evaluated = true;
                    }
                    Err(error) => {
                        self.bindings[binding_index].dirty = false;
                        errors.push((BindingIndex(binding_index), error));
                    }
                }
            }
            if !any_evaluated && errors.len() < usize::MAX {
                // All remaining dirty bindings errored; they were marked
                // clean above, so the loop terminates.
                break;
            }
        }
        return errors;
    }

    /// Evaluates one binding and writes the resulting value back.
    fn evaluate_binding(
        &mut self,
        binding_index: usize,
        tree: &mut ElementTree,
    ) -> Result<(), EvalError> {
        let record = &mut self.bindings[binding_index];
        record.dirty = false;
        let element = record.element;
        let property = record.property.clone();
        let expr = record.expr.clone();

        // Fresh dependency set for this evaluation.
        let mut dependencies = Vec::new();
        let value =
            evaluate_under_tracking(tree, self, element, &property, &expr, &mut dependencies)?;

        // Rebuild reverse edges (drop stale ones of this binding first).
        let previous = std::mem::take(&mut self.bindings[binding_index].dependencies);
        for (dep_element, dep_property) in &previous {
            let key = (*dep_element, dep_property.clone());
            if let Some(readers) = self.readers.get_mut(&key) {
                readers.retain(|&reader| return reader != binding_index);
                if readers.is_empty() {
                    self.readers.remove(&key);
                }
            }
        }
        for (dep_element, dep_property) in &dependencies {
            self.readers
                .entry((*dep_element, dep_property.clone()))
                .or_default()
                .push(binding_index);
        }
        self.bindings[binding_index].dependencies = dependencies;

        // D8: `p <- tween(..)` / `p <- spring(..)` routes the computed value
        // into the animation clock as the new target instead of writing the
        // displayed value; the displayed value advances on clock ticks.
        if matches!(
            expr,
            TypedExpr::Call {
                func: Builtin::Tween | Builtin::Spring,
                ..
            }
        ) {
            return self.start_bound_animation(tree, element, &property, &expr, value);
        }

        // Write-back without clobbering the binding itself.
        let changed = tree.arena[element].set(&property, value.clone());
        if changed {
            self.record_change(element, &property, value, ChangeSource::Binding);
            // Invalidate readers of this property, but skip self (a binding
            // writing its own property must not re-trigger itself).
            let key = (element, property.clone());
            if let Some(readers) = self.readers.get(&key) {
                for &reader in readers {
                    if reader != binding_index {
                        self.bindings[reader].dirty = true;
                    }
                }
            }
            // Two-way partner sync (value channel).
            self.sync_two_way_partner(tree, element, &property);
        }
        return Ok(());
    }

    /// Evaluates an expression outside any binding (defaults, conditions).
    pub fn evaluate_free(
        &mut self,
        tree: &mut ElementTree,
        element: ElementId,
        expr: &TypedExpr,
    ) -> Result<Value, EvalError> {
        let mut dependencies = Vec::new();
        return evaluate_tracked(tree, self, element, expr, &mut dependencies, 0);
    }

    /// Evaluates every `when` block in the tree and applies/un-applies its
    /// assignments (plan §3.3: `when` blocks compile to conditional
    /// bindings). The pre-override values are captured when a block first
    /// covers a property; when the condition leaves, bound properties
    /// re-evaluate their bindings and static properties are restored to the
    /// captured values.
    pub fn apply_when_blocks(
        &mut self,
        tree: &mut ElementTree,
    ) -> Result<Vec<(ElementId, String)>, EvalError> {
        let mut written = Vec::new();
        let mut ids: Vec<ElementId> = Vec::new();
        tree.visit_pre_order(|id, _| ids.push(id));
        for id in ids {
            let blocks = tree.arena[id].when_blocks.clone();
            for block in &blocks {
                let holds = self
                    .evaluate_free(tree, id, &block.condition)?
                    .as_bool()
                    .unwrap_or(false);
                let key = (id, block_key(&block.condition));
                if holds {
                    for assignment in &block.assignments {
                        // The write target decides the property name: an id
                        // reference (`panel.opacity`) writes `opacity` on the
                        // panel; attached properties keep their dotted name
                        // (`font.size`).
                        let property = target_property_name(&assignment.target);
                        let resolved = tree.resolve_target(id, &assignment.target).unwrap_or(id);
                        if let Some(value) = evaluate_when_value(self, tree, id, &assignment.value)
                        {
                            let overrides = self.when_overrides.entry(key.clone()).or_default();
                            if !overrides.iter().any(|(element, name, _)| {
                                return *element == resolved && name == &property;
                            }) {
                                // First frame of the override: capture what
                                // the block is replacing (for the restore).
                                let previous = tree.arena[resolved].get(&property).cloned();
                                overrides.push((resolved, property.clone(), previous));
                            }
                            if tree.arena[resolved].set(&property, value.clone()) {
                                self.invalidate(resolved, &property);
                                self.record_change(
                                    resolved,
                                    &property,
                                    value,
                                    ChangeSource::WhenBlock,
                                );
                                written.push((resolved, property.clone()));
                            }
                        }
                    }
                } else if let Some(previous) = self.when_overrides.remove(&key) {
                    // Condition stopped holding: bound properties re-evaluate
                    // their bindings; statics restore the captured values.
                    for (resolved, property, previous_value) in previous {
                        if let Some(index) = self.binding_at(resolved, &property) {
                            self.bindings[index.0].dirty = true;
                            continue;
                        }
                        let Some(value) = previous_value else {
                            continue;
                        };
                        if tree.arena[resolved].set(&property, value.clone()) {
                            self.invalidate(resolved, &property);
                            self.record_change(resolved, &property, value, ChangeSource::WhenBlock);
                            written.push((resolved, property.clone()));
                        }
                    }
                }
            }
        }
        return Ok(written);
    }

    /// Advances timer nodes: fires each running timer whose accumulated
    /// interval elapsed, emitting the `timer` signal on the timer element.
    /// Returns the fired timer element ids.
    pub fn tick_timers(
        &mut self,
        tree: &mut ElementTree,
        delta: nui_core::Duration,
        elapsed: &mut HashMap<ElementId, f64>,
    ) -> Result<Vec<ElementId>, EvalError> {
        let mut fired = Vec::new();
        let mut ids: Vec<ElementId> = Vec::new();
        tree.visit_pre_order(|id, element| {
            if element.timer_interval.is_some() {
                ids.push(id);
            }
        });
        for id in ids {
            let Some(interval) = tree.arena[id].timer_interval else {
                continue;
            };
            let interval_ms = interval.as_millis_f64();
            if interval_ms <= 0.0 {
                continue;
            }
            let total = elapsed.entry(id).or_insert(0.0);
            *total += delta.as_millis_f64();
            while *total >= interval_ms {
                *total -= interval_ms;
                fired.push(id);
                self.emit_signal(tree, id, "timer")?;
            }
        }
        return Ok(fired);
    }

    /// Runs an effect (handler/statements) against the tree, collecting the
    /// written properties into `written`. Implements D10: property writes
    /// clear `<-` bindings. The return collection form lets machine
    /// transitions and signal fan-out accumulate into one list.
    pub fn run_effect(
        &mut self,
        tree: &mut ElementTree,
        element: ElementId,
        effects: &[nui_compiler::Effect],
        written: &mut Vec<(ElementId, String)>,
    ) -> Result<(), EvalError> {
        // `let` locals are scoped to their statement list: pop everything
        // the list pushed when it finishes.
        let scope_base = self.locals.len();
        let outcome = (|| {
            for effect in effects {
                self.run_effect_one(tree, element, effect, written)?;
            }
            return Ok(());
        })();
        self.locals.truncate(scope_base);
        return outcome;
    }

    fn run_effect_one(
        &mut self,
        tree: &mut ElementTree,
        element: ElementId,
        effect: &nui_compiler::Effect,
        written: &mut Vec<(ElementId, String)>,
    ) -> Result<(), EvalError> {
        return match effect {
            nui_compiler::Effect::Let { name, value } => {
                let evaluated = self.evaluate_free(tree, element, value)?;
                self.locals.push((name.clone(), evaluated));
                Ok(())
            }
            nui_compiler::Effect::If {
                condition,
                then_branch,
                else_branch,
            } => {
                let value = self.evaluate_free(tree, element, condition)?;
                if value.as_bool().unwrap_or(false) {
                    for statement in then_branch {
                        self.run_effect_one(tree, element, statement, written)?;
                    }
                } else {
                    for statement in else_branch {
                        self.run_effect_one(tree, element, statement, written)?;
                    }
                }
                Ok(())
            }
            nui_compiler::Effect::Assign { target, op, value } => {
                let resolved = self.resolve_write_target(tree, element, target)?;
                let current = tree.arena[resolved]
                    .get(&target_property_name(target))
                    .cloned()
                    .unwrap_or(Value::Int(0));
                let new_value = match op {
                    nui_compiler::AssignOp::Set => self.evaluate_free(tree, element, value)?,
                    nui_compiler::AssignOp::Add => {
                        let operand = self.evaluate_free(tree, element, value)?;
                        add_values(&current, &operand)
                    }
                    nui_compiler::AssignOp::Sub => {
                        let operand = self.evaluate_free(tree, element, value)?;
                        sub_values(&current, &operand)
                    }
                };
                let property = target_property_name(target);
                // D10: an effect write clears the `<-` binding on the target.
                tree.arena[resolved].clear_binding(&property);
                if tree.arena[resolved].set(&property, new_value.clone()) {
                    self.invalidate(resolved, &property);
                    self.record_change(resolved, &property, new_value, ChangeSource::Effect);
                    written.push((resolved, property));
                }
                Ok(())
            }
            nui_compiler::Effect::Emit { signal } => {
                // Signal delivery is engine-level; the host connects signals
                // to handlers via `Engine::emit_signal`.
                self.emit_signal(tree, element, signal)?;
                Ok(())
            }
            nui_compiler::Effect::Call { callee, args } => {
                self.run_method_call(tree, element, callee, args)?;
                Ok(())
            }
        };
    }

    /// Resolves an effect write target to an element handle.
    fn resolve_write_target(
        &mut self,
        tree: &mut ElementTree,
        element: ElementId,
        target: &PropertyTarget,
    ) -> Result<ElementId, EvalError> {
        return tree.resolve_target(element, target).ok_or_else(|| {
            return EvalError::Unresolved {
                what: format!("{target:?}"),
            };
        });
    }

    /// Method calls: `id.method(...)` element methods (`timer.start()` /
    /// `timer.stop()`) and single-name host function calls.
    fn run_method_call(
        &mut self,
        tree: &mut ElementTree,
        element: ElementId,
        callee: &[String],
        args: &[TypedExpr],
    ) -> Result<(), EvalError> {
        let mut evaluated_args = Vec::new();
        for arg in args {
            evaluated_args.push(self.evaluate_free(tree, element, arg)?);
        }
        return match callee {
            [name] => {
                self.call_host_function(name, &evaluated_args)?;
                Ok(())
            }
            [id_name, method, ..] => {
                let Some(target) = tree.lookup_id(id_name) else {
                    return Err(EvalError::Unresolved {
                        what: format!("unknown id `{id_name}`"),
                    });
                };
                match method.as_str() {
                    "start" => {
                        let interval = evaluated_args
                            .first()
                            .and_then(|value| return value.as_duration().ok())
                            .unwrap_or(nui_core::Duration::from_millis(0.0));
                        tree.arena[target].timer_interval = Some(interval);
                        return Ok(());
                    }
                    "stop" => {
                        tree.arena[target].timer_interval = None;
                        return Ok(());
                    }
                    other => {
                        return Err(EvalError::Unresolved {
                            what: format!("unknown method `{other}` on `{id_name}`"),
                        });
                    }
                }
            }
            [] => Err(EvalError::Unresolved {
                what: "empty call target".to_string(),
            }),
        };
    }

    /// Delivers a signal: runs every handler for `signal` reachable from the
    /// element's tree, plus machine transitions listening to it.
    pub fn emit_signal(
        &mut self,
        tree: &mut ElementTree,
        element: ElementId,
        signal: &str,
    ) -> Result<Vec<(ElementId, String)>, EvalError> {
        let mut written = Vec::new();
        // Handlers on the emitting element's subtree (self included).
        let mut targets = vec![element];
        let mut index = 0;
        while index < targets.len() {
            let current = targets[index];
            for child in tree.arena[current].children.clone() {
                targets.push(child);
            }
            index += 1;
        }
        for target in targets {
            let handlers = tree.arena[target].handlers.clone();
            for handler in &handlers {
                if handler.signal == signal {
                    self.run_effect(tree, target, &handler.effect, &mut written)?;
                }
            }
            // Machine transitions triggered by this signal.
            self.fire_machine_signal(tree, target, signal, &mut written)?;
            // Custom Rust component behavior (plan §5 宿主互操作), after
            // the element's own handlers and machines.
            self.run_behavior(tree, target, signal)?;
        }
        // Root-registered handlers see component-level signals too.
        if let Some(root) = tree.roots.first().copied()
            && root != element
        {
            let handlers = tree.arena[root].handlers.clone();
            for handler in &handlers {
                if handler.signal == signal {
                    self.run_effect(tree, root, &handler.effect, &mut written)?;
                }
            }
            self.fire_machine_signal(tree, root, signal, &mut written)?;
        }
        return Ok(written);
    }

    /// Bubbles a signal from `element` up through its ancestors: each
    /// element's own handlers, machines, and behaviors run for `signal`
    /// (M7/M10 click dispatch — a click on a button's label must reach the
    /// button). Returns the written properties in fire order.
    pub fn emit_bubble(
        &mut self,
        tree: &mut ElementTree,
        element: ElementId,
        signal: &str,
    ) -> Result<Vec<(ElementId, String)>, EvalError> {
        let mut written = Vec::new();
        let mut current = Some(element);
        while let Some(target) = current {
            let handlers = tree.arena[target].handlers.clone();
            for handler in &handlers {
                if handler.signal == signal {
                    self.run_effect(tree, target, &handler.effect, &mut written)?;
                }
            }
            self.fire_machine_signal(tree, target, signal, &mut written)?;
            self.run_behavior(tree, target, signal)?;
            current = tree.arena[target].parent;
        }
        return Ok(written);
    }

    /// Fires machine transitions listening to `signal` on `element`.
    fn fire_machine_signal(
        &mut self,
        tree: &mut ElementTree,
        element: ElementId,
        signal: &str,
        written: &mut Vec<(ElementId, String)>,
    ) -> Result<(), EvalError> {
        let Some(machine_index) = tree.arena[element].machines.len().checked_sub(1) else {
            return Ok(());
        };
        let instance = tree.arena[element].machines[machine_index].clone();
        let current_state = instance.current_state.clone();
        let mut taken_transition: Option<String> = None;
        for transition in &instance.ir.transitions {
            if transition.event != signal || !transition.from.contains(&current_state) {
                continue;
            }
            let guard_ok = match &transition.guard {
                None => true,
                Some(guard) => {
                    let value = self.evaluate_free(tree, element, guard)?;
                    value.as_bool().unwrap_or(false)
                }
            };
            if guard_ok {
                taken_transition = Some(transition.to.clone());
                break;
            }
        }
        let Some(to_state) = taken_transition else {
            return Ok(());
        };
        // Run exit effects of the old state, then enter effects of the new.
        if let Some(old_state) = instance
            .ir
            .states
            .iter()
            .find(|state| return state.name == current_state)
        {
            self.run_effect(tree, element, &old_state.exit, written)?;
        }
        if let Some(new_state) = instance
            .ir
            .states
            .iter()
            .find(|state| return state.name == to_state)
        {
            self.run_effect(tree, element, &new_state.enter, written)?;
        }
        tree.arena[element].machines[machine_index].current_state = to_state;
        return Ok(());
    }

    /// Reads the current state of `element`'s machine (for `machine.state`
    /// property reads).
    pub fn machine_state_of(
        &self,
        tree: &mut ElementTree,
        element: ElementId,
        machine: &str,
        state: &str,
    ) -> Value {
        return match tree.arena[element]
            .machines
            .iter()
            .find(|instance| return instance.name() == machine)
        {
            Some(instance) => Value::Bool(instance.current_state == state),
            None => Value::Bool(false),
        };
    }

    /// Evaluates `variable.field` against the nearest enclosing `For` row
    /// scope. All bindings of a row share the dependency key
    /// `(row root, "@field.<name>")`, so [`Engine::model_set_field`]
    /// invalidates the whole row with one key.
    fn eval_row_field(
        &mut self,
        tree: &mut ElementTree,
        element: ElementId,
        variable: &str,
        field: &str,
        dependencies: &mut Vec<(ElementId, String)>,
    ) -> Result<Value, EvalError> {
        let Some((scope_element, scope)) = find_row_scope(tree, element, variable) else {
            return Err(EvalError::Unresolved {
                what: format!("for-variable `{variable}` not in scope"),
            });
        };
        dependencies.push((scope_element, format!("@field.{field}")));
        return self
            .model_field(scope.model, scope.row, field)
            .ok_or_else(|| {
                return EvalError::Unresolved {
                    what: format!("model field `{field}` of row {}", scope.row),
                };
            });
    }
}

/// Walks `element` and its ancestors for the nearest row scope answering to
/// `variable`, then hoists to the row root: every element of the row
/// subtree carries the scope, but the dependency key and
/// [`Engine::model_set_field`] invalidation must agree on one element (the
/// `For` element's child at the row index).
fn find_row_scope(
    tree: &ElementTree,
    element: ElementId,
    variable: &str,
) -> Option<(ElementId, crate::element::RowScope)> {
    let mut current = element;
    let scope = loop {
        let node = &tree.arena[current];
        if let Some(found) = &node.for_scope
            && found.variable == variable
        {
            break found.clone();
        }
        current = node.parent?;
    };
    let mut row_root = current;
    while let Some(parent) = tree.arena[row_root].parent {
        let node = &tree.arena[parent];
        let same_row = node.for_scope.as_ref().is_some_and(|parent_scope| {
            return parent_scope.variable == scope.variable
                && parent_scope.model == scope.model
                && parent_scope.row == scope.row;
        });
        if !same_row {
            break;
        }
        row_root = parent;
    }
    return Some((row_root, scope));
}

/// Stable per-condition key for `when` override bookkeeping (hash of the
/// condition expression text; equal conditions share restore state).
fn block_key(condition: &TypedExpr) -> String {
    return format!("{condition:?}");
}

/// Evaluates a `when` assignment value; falls back to the expression's
/// literal form when the free evaluator cannot resolve it.
fn evaluate_when_value(
    engine: &mut Engine,
    tree: &mut ElementTree,
    element: ElementId,
    expr: &TypedExpr,
) -> Option<Value> {
    return engine.evaluate_free(tree, element, expr).ok();
}

/// The property name a write target addresses (`x`, or the last segment of
/// an attached property path — M2 supports single-segment paths).
pub(crate) fn target_property_name(target: &PropertyTarget) -> String {
    return match target {
        PropertyTarget::Component(name)
        | PropertyTarget::Root(name)
        | PropertyTarget::Parent(name)
        | PropertyTarget::MachineState(_, name) => name.clone(),
        PropertyTarget::Id(_, name) => name.clone(),
    };
}

/// Evaluates `expr` while recording reads of `property` on `element` into
/// `dependencies`; used for binding evaluation with the EVAL_STACK active.
fn evaluate_under_tracking(
    tree: &mut ElementTree,
    engine: &mut Engine,
    element: ElementId,
    property: &str,
    expr: &TypedExpr,
    dependencies: &mut Vec<(ElementId, String)>,
) -> Result<Value, EvalError> {
    dependencies.push((element, property.to_string()));
    EVAL_STACK.with(|stack| stack.borrow_mut().push((element, property.to_string())));
    let outcome = evaluate_tracked(tree, engine, element, expr, dependencies, 0);
    EVAL_STACK.with(|stack| {
        stack.borrow_mut().pop();
    });
    return outcome;
}

/// Evaluates an expression, resolving property reads against the tree and
/// recording dependency edges into `dependencies` when the stack is active.
fn evaluate_tracked(
    tree: &mut ElementTree,
    engine: &mut Engine,
    element: ElementId,
    expr: &TypedExpr,
    dependencies: &mut Vec<(ElementId, String)>,
    depth: usize,
) -> Result<Value, EvalError> {
    if depth > MAX_EVAL_DEPTH {
        return Err(EvalError::DepthExceeded {
            limit: MAX_EVAL_DEPTH,
        });
    }
    // Cycle check: reading a property that is currently being evaluated
    // (anywhere up the stack) means the dependency graph has a loop.
    let read_key = match expr {
        TypedExpr::Property { target, .. } => tree
            .resolve_target(element, target)
            .map(|resolved| return (resolved, target_property_name(target))),
        _ => None,
    };
    if let Some(key) = read_key.clone() {
        let reentrant = EVAL_STACK.with(|stack| {
            return stack.borrow().iter().any(|entry| return *entry == key);
        });
        if reentrant {
            let mut chain: Vec<String> = EVAL_STACK.with(|stack| {
                return stack
                    .borrow()
                    .iter()
                    .map(|(element_id, property)| {
                        return format!("{}.{}", tree.arena[*element_id].ty, property);
                    })
                    .collect();
            });
            chain.push(format!("{}.{}", tree.arena[key.0].ty, key.1));
            return Err(EvalError::Cycle { chain });
        }
        dependencies.push(key);
    }
    return evaluate_expr(tree, engine, element, expr, dependencies, depth);
}

/// The recursive evaluator proper (post cycle check).
fn evaluate_expr(
    tree: &mut ElementTree,
    engine: &mut Engine,
    element: ElementId,
    expr: &TypedExpr,
    dependencies: &mut Vec<(ElementId, String)>,
    depth: usize,
) -> Result<Value, EvalError> {
    return match expr {
        TypedExpr::Const(value) => Ok(value.clone()),
        TypedExpr::Local { name, .. } => {
            // Effect `let` locals resolve from the running effect's stack.
            if let Some((_, value)) = engine
                .locals
                .iter()
                .rev()
                .find(|(local, _)| return local == name)
            {
                return Ok(value.clone());
            }
            return Err(EvalError::Unresolved {
                what: format!("`{name}` is a for-variable; read its fields with `.field`"),
            });
        }
        TypedExpr::Property { target, .. } => {
            let Some(resolved) = tree.resolve_target(element, target) else {
                return Err(EvalError::Unresolved {
                    what: format!("{target:?}"),
                });
            };
            if let PropertyTarget::MachineState(machine, state) = target {
                return Ok(engine.machine_state_of(tree, resolved, machine, state));
            }
            let name = target_property_name(target);
            // Pull model (plan §5: frame-start evaluation in topological
            // order): reading a property whose binding is dirty evaluates
            // that binding first, so a chain resolves in one pass. The
            // EVAL_STACK reentrancy check above catches cycles.
            if let Some(binding_index) = engine.binding_at(resolved, &name) {
                let is_dirty = engine.bindings[binding_index.0].dirty;
                if is_dirty {
                    engine.evaluate_binding(binding_index.0, tree)?;
                }
            }
            match tree.arena[resolved].get(&name) {
                Some(value) => Ok(value.clone()),
                None => Ok(Value::Int(0)),
            }
        }
        TypedExpr::Unary { op, operand, .. } => {
            let value = evaluate_tracked(tree, engine, element, operand, dependencies, depth + 1)?;
            return match op {
                nui_syntax::UnaryOp::Neg => Ok(negate(&value)),
                nui_syntax::UnaryOp::Not => Ok(Value::Bool(!value.as_bool().unwrap_or(false))),
            };
        }
        TypedExpr::Binary { op, lhs, rhs, .. } => {
            let left = evaluate_tracked(tree, engine, element, lhs, dependencies, depth + 1)?;
            let right = evaluate_tracked(tree, engine, element, rhs, dependencies, depth + 1)?;
            return Ok(apply_binary(*op, &left, &right));
        }
        TypedExpr::Ternary {
            condition,
            then_expr,
            else_expr,
            ..
        } => {
            let flag = evaluate_tracked(tree, engine, element, condition, dependencies, depth + 1)?;
            // Eager branch evaluation keeps dependency tracking uniform
            // (both branches are dependencies either way).
            let then_value =
                evaluate_tracked(tree, engine, element, then_expr, dependencies, depth + 1)?;
            let else_value =
                evaluate_tracked(tree, engine, element, else_expr, dependencies, depth + 1)?;
            if flag.as_bool().unwrap_or(false) {
                return Ok(then_value);
            }
            return Ok(else_value);
        }
        TypedExpr::Call { func, args, .. } => {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                values.push(evaluate_tracked(
                    tree,
                    engine,
                    element,
                    arg,
                    dependencies,
                    depth + 1,
                )?);
            }
            return Ok(apply_builtin(*func, &values));
        }
        TypedExpr::HostCall { name, args, .. } => {
            let mut values = Vec::with_capacity(args.len());
            for arg in args {
                values.push(evaluate_tracked(
                    tree,
                    engine,
                    element,
                    arg,
                    dependencies,
                    depth + 1,
                )?);
            }
            return engine.call_host_function(name, &values);
        }
        TypedExpr::Interp { parts } => {
            let mut text = String::new();
            for part in parts {
                match part {
                    nui_compiler::InterpPart::Text(fragment) => text.push_str(fragment),
                    nui_compiler::InterpPart::Expr(inner) => {
                        let value = evaluate_tracked(
                            tree,
                            engine,
                            element,
                            inner,
                            dependencies,
                            depth + 1,
                        )?;
                        text.push_str(&value.to_string());
                    }
                }
            }
            return Ok(Value::String(text));
        }
        TypedExpr::Dynamic { base, name } => {
            // `item.field`: resolve the loop variable through the row scope
            // chain and read the field from the model (dependency-tracked
            // per row so `model_set_field` invalidates exactly that row).
            if let TypedExpr::Local { name: variable, .. } = &**base {
                return engine.eval_row_field(tree, element, variable, name, dependencies);
            }
            let base_value =
                evaluate_tracked(tree, engine, element, base, dependencies, depth + 1)?;
            return Err(EvalError::Unresolved {
                what: format!("member `{name}` on value {base_value:?}"),
            });
        }
        TypedExpr::Error => Err(EvalError::Unresolved {
            what: "error expression (compile-time diagnostic already reported)".to_string(),
        }),
    };
}

/// Unary negate over Int/Float.
fn negate(value: &Value) -> Value {
    return match value {
        Value::Int(inner) => Value::Int(-inner),
        Value::Float(inner) => Value::Float(-inner),
        _ => value.clone(),
    };
}

/// Numeric/length/duration `+` and string concatenation.
fn add_values(left: &Value, right: &Value) -> Value {
    return match (left, right) {
        (Value::Int(a), Value::Int(b)) => Value::Int(a + b),
        (Value::Float(a), Value::Float(b)) => Value::Float(a + b),
        (Value::Int(a), Value::Float(b)) => Value::Float(*a as f64 + b),
        (Value::Float(a), Value::Int(b)) => Value::Float(a + *b as f64),
        (Value::String(a), Value::String(b)) => Value::String(format!("{a}{b}")),
        (Value::Length(Length::Dp(a)), Value::Length(Length::Dp(b))) => {
            Value::Length(Length::Dp(a + b))
        }
        (Value::Duration(a), Value::Duration(b)) => Value::Duration(
            nui_core::Duration::from_millis(a.as_millis_f64() + b.as_millis_f64()),
        ),
        _ => left.clone(),
    };
}

/// Numeric/length/duration `-`.
fn sub_values(left: &Value, right: &Value) -> Value {
    return match (left, right) {
        (Value::Int(a), Value::Int(b)) => Value::Int(a - b),
        (Value::Float(a), Value::Float(b)) => Value::Float(a - b),
        (Value::Int(a), Value::Float(b)) => Value::Float(*a as f64 - b),
        (Value::Float(a), Value::Int(b)) => Value::Float(a - *b as f64),
        (Value::Length(Length::Dp(a)), Value::Length(Length::Dp(b))) => {
            Value::Length(Length::Dp(a - b))
        }
        (Value::Duration(a), Value::Duration(b)) => Value::Duration(
            nui_core::Duration::from_millis(a.as_millis_f64() - b.as_millis_f64()),
        ),
        _ => left.clone(),
    };
}

/// Applies a binary operator with the language's promotion rules.
fn apply_binary(op: nui_syntax::BinaryOp, left: &Value, right: &Value) -> Value {
    return match op {
        nui_syntax::BinaryOp::Add => add_values(left, right),
        nui_syntax::BinaryOp::Sub => sub_values(left, right),
        nui_syntax::BinaryOp::Mul => match (left, right) {
            (Value::Int(a), Value::Int(b)) => Value::Int(a * b),
            (Value::Float(a), Value::Float(b)) => Value::Float(a * b),
            (Value::Length(Length::Dp(a)), Value::Float(b)) => {
                Value::Length(Length::Dp((*a) * (*b) as f32))
            }
            (Value::Float(a), Value::Length(Length::Dp(b))) => {
                Value::Length(Length::Dp((*a) as f32 * (*b)))
            }
            (Value::Duration(a), Value::Float(b)) => {
                Value::Duration(nui_core::Duration::from_millis(a.as_millis_f64() * b))
            }
            _ => left.clone(),
        },
        nui_syntax::BinaryOp::Div => match (left, right) {
            (Value::Int(a), Value::Int(b)) => {
                if *b == 0 {
                    Value::Int(0)
                } else {
                    Value::Int(a / b)
                }
            }
            (Value::Float(a), Value::Float(b)) => Value::Float(a / b),
            (Value::Length(Length::Dp(a)), Value::Float(b)) if *b != 0.0 => {
                Value::Length(Length::Dp((*a) / (*b) as f32))
            }
            (Value::Duration(a), Value::Float(b)) if *b != 0.0 => {
                Value::Duration(nui_core::Duration::from_millis(a.as_millis_f64() / b))
            }
            _ => left.clone(),
        },
        nui_syntax::BinaryOp::Rem => match (left, right) {
            (Value::Int(a), Value::Int(b)) if *b != 0 => Value::Int(a % b),
            (Value::Float(a), Value::Float(b)) if *b != 0.0 => Value::Float(a % b),
            _ => left.clone(),
        },
        nui_syntax::BinaryOp::Eq => Value::Bool(values_equal(left, right)),
        nui_syntax::BinaryOp::NotEq => Value::Bool(!values_equal(left, right)),
        nui_syntax::BinaryOp::Lt
        | nui_syntax::BinaryOp::Le
        | nui_syntax::BinaryOp::Gt
        | nui_syntax::BinaryOp::Ge => {
            let ordering = compare_values(left, right);
            let holds = match op {
                nui_syntax::BinaryOp::Lt => ordering == std::cmp::Ordering::Less,
                nui_syntax::BinaryOp::Le => ordering != std::cmp::Ordering::Greater,
                nui_syntax::BinaryOp::Gt => ordering == std::cmp::Ordering::Greater,
                nui_syntax::BinaryOp::Ge => ordering != std::cmp::Ordering::Less,
                _ => false,
            };
            Value::Bool(holds)
        }
        nui_syntax::BinaryOp::And => {
            Value::Bool(left.as_bool().unwrap_or(false) && right.as_bool().unwrap_or(false))
        }
        nui_syntax::BinaryOp::Or => {
            Value::Bool(left.as_bool().unwrap_or(false) || right.as_bool().unwrap_or(false))
        }
    };
}

/// Equality with Int/Float promotion and exact match elsewhere.
fn values_equal(left: &Value, right: &Value) -> bool {
    return match (left, right) {
        (Value::Int(a), Value::Float(b)) => (*a as f64) == *b,
        (Value::Float(a), Value::Int(b)) => *a == (*b as f64),
        _ => left == right,
    };
}

/// Ordering for numeric/length/duration comparisons; incomparable types
/// order as equal (the compiler rejects them anyway).
fn compare_values(left: &Value, right: &Value) -> std::cmp::Ordering {
    let as_f64 = |value: &Value| -> Option<f64> {
        return match value {
            Value::Int(inner) => Some(*inner as f64),
            Value::Float(inner) => Some(*inner),
            Value::Length(Length::Dp(inner)) => Some(*inner as f64),
            Value::Duration(inner) => Some(inner.as_millis_f64()),
            _ => None,
        };
    };
    let Some(a) = as_f64(left) else {
        return std::cmp::Ordering::Equal;
    };
    let Some(b) = as_f64(right) else {
        return std::cmp::Ordering::Equal;
    };
    return a.partial_cmp(&b).unwrap_or(std::cmp::Ordering::Equal);
}

/// Builtin functions over evaluated arguments.
fn apply_builtin(func: Builtin, args: &[Value]) -> Value {
    let numeric = |value: &Value| -> Option<f64> {
        return match value {
            Value::Int(inner) => Some(*inner as f64),
            Value::Float(inner) => Some(*inner),
            _ => None,
        };
    };
    let wrap = |raw: f64, like: &Value| -> Value {
        return match like {
            Value::Int(_) => Value::Int(raw as i64),
            _ => Value::Float(raw),
        };
    };
    return match func {
        Builtin::Min => match (args.first(), args.get(1)) {
            (Some(a), Some(b)) => {
                if compare_values(a, b) == std::cmp::Ordering::Greater {
                    b.clone()
                } else {
                    a.clone()
                }
            }
            _ => Value::Int(0),
        },
        Builtin::Max => match (args.first(), args.get(1)) {
            (Some(a), Some(b)) => {
                if compare_values(a, b) == std::cmp::Ordering::Less {
                    b.clone()
                } else {
                    a.clone()
                }
            }
            _ => Value::Int(0),
        },
        Builtin::Clamp => match (args.first(), args.get(1), args.get(2)) {
            (Some(value), Some(lo), Some(hi)) => {
                let mut raw = numeric(value).unwrap_or(0.0);
                let low = numeric(lo).unwrap_or(f64::NEG_INFINITY);
                let high = numeric(hi).unwrap_or(f64::INFINITY);
                raw = raw.max(low).min(high);
                wrap(raw, value)
            }
            _ => Value::Int(0),
        },
        // Math builtins: numeric in, Float out. `floor`/`ceil` also return
        // Float for a uniform math surface (Int coercion stays explicit).
        Builtin::Sin
        | Builtin::Cos
        | Builtin::Tan
        | Builtin::Sqrt
        | Builtin::Abs
        | Builtin::Floor
        | Builtin::Ceil => {
            let raw = args.first().and_then(numeric).unwrap_or(0.0);
            let out = match func {
                Builtin::Sin => raw.sin(),
                Builtin::Cos => raw.cos(),
                Builtin::Tan => raw.tan(),
                Builtin::Sqrt => raw.sqrt(),
                Builtin::Abs => raw.abs(),
                Builtin::Floor => raw.floor(),
                Builtin::Ceil => raw.ceil(),
                // Non-math builtins never enter this branch; the value is
                // irrelevant and matches the enclosing arm's type.
                Builtin::Min | Builtin::Max | Builtin::Clamp | Builtin::Tween | Builtin::Spring => {
                    raw
                }
            };
            Value::Float(out)
        }
        // Animation wrappers pass the target value through in M2; the
        // animation clock (animation.rs) intercepts the property writes.
        Builtin::Tween | Builtin::Spring => args.first().cloned().unwrap_or(Value::Int(0)),
    };
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::element::Element;
    use nui_compiler::TypedExpr;

    fn const_int(value: i64) -> TypedExpr {
        return TypedExpr::Const(Value::Int(value));
    }

    fn property(target: PropertyTarget) -> TypedExpr {
        return TypedExpr::Property {
            target,
            ty: nui_compiler::Type::Int,
        };
    }

    #[test]
    fn evaluates_property_reads_and_arithmetic() {
        let mut tree = ElementTree::new();
        let root = tree.insert(Element::new("Window", None));
        tree.push_root(root);
        tree.arena[root].set("count", Value::Int(2));
        let mut engine = Engine::new();
        let expr = TypedExpr::Binary {
            op: nui_syntax::BinaryOp::Add,
            lhs: Box::new(property(PropertyTarget::Component("count".to_string()))),
            rhs: Box::new(const_int(3)),
            ty: nui_compiler::Type::Int,
        };
        let value = engine.evaluate_free(&mut tree, root, &expr).unwrap();
        assert_eq!(value, Value::Int(5));
    }

    #[test]
    fn propagate_evaluates_dirty_bindings_once_and_chains() {
        let mut tree = ElementTree::new();
        let root = tree.insert(Element::new("Window", None));
        tree.push_root(root);
        tree.arena[root].set("count", Value::Int(1));
        let mut engine = Engine::new();
        // doubled <- count * 2
        let doubled_expr = TypedExpr::Binary {
            op: nui_syntax::BinaryOp::Mul,
            lhs: Box::new(property(PropertyTarget::Component("count".to_string()))),
            rhs: Box::new(const_int(2)),
            ty: nui_compiler::Type::Int,
        };
        let doubled_index = engine.register_binding(root, "doubled", doubled_expr.clone());
        tree.arena[root].set_binding(
            "doubled",
            Binding {
                index: doubled_index,
            },
        );
        // quadrupled <- doubled * 2
        let quad_expr = TypedExpr::Binary {
            op: nui_syntax::BinaryOp::Mul,
            lhs: Box::new(property(PropertyTarget::Component("doubled".to_string()))),
            rhs: Box::new(const_int(2)),
            ty: nui_compiler::Type::Int,
        };
        let quad_index = engine.register_binding(root, "quadrupled", quad_expr);
        tree.arena[root].set_binding("quadrupled", Binding { index: quad_index });

        engine.propagate(&mut tree);
        assert_eq!(tree.arena[root].get("doubled"), Some(&Value::Int(2)));
        assert_eq!(tree.arena[root].get("quadrupled"), Some(&Value::Int(4)));

        // Writing count dirties only the dependent chain.
        tree.arena[root].set("count", Value::Int(5));
        engine.invalidate(root, "count");
        engine.propagate(&mut tree);
        assert_eq!(tree.arena[root].get("doubled"), Some(&Value::Int(10)));
        assert_eq!(tree.arena[root].get("quadrupled"), Some(&Value::Int(20)));
    }

    #[test]
    fn cycle_detection_reports_chain() {
        let mut tree = ElementTree::new();
        let root = tree.insert(Element::new("Window", None));
        tree.push_root(root);
        let mut engine = Engine::new();
        // a <- b, b <- a (a runtime cycle; the static checker catches the
        // direct form, this exercises the runtime second line of defense).
        let a_expr = property(PropertyTarget::Component("b".to_string()));
        let b_expr = property(PropertyTarget::Component("a".to_string()));
        let a_index = engine.register_binding(root, "a", a_expr);
        let b_index = engine.register_binding(root, "b", b_expr);
        tree.arena[root].set_binding("a", Binding { index: a_index });
        tree.arena[root].set_binding("b", Binding { index: b_index });
        let errors = engine.propagate(&mut tree);
        assert!(
            errors
                .iter()
                .any(|(_, error)| matches!(error, EvalError::Cycle { .. })),
            "expected a cycle error, got {errors:?}"
        );
    }

    #[test]
    fn effect_write_clears_binding_d10() {
        let mut tree = ElementTree::new();
        let root = tree.insert(Element::new("Window", None));
        tree.push_root(root);
        tree.arena[root].set("count", Value::Int(0));
        let mut engine = Engine::new();
        let count_expr = TypedExpr::Binary {
            op: nui_syntax::BinaryOp::Add,
            lhs: Box::new(property(PropertyTarget::Component("count".to_string()))),
            rhs: Box::new(const_int(1)),
            ty: nui_compiler::Type::Int,
        };
        let index = engine.register_binding(root, "next", count_expr);
        tree.arena[root].set_binding("next", Binding { index });
        engine.propagate(&mut tree);
        assert_eq!(tree.arena[root].get("next"), Some(&Value::Int(1)));
        assert!(tree.arena[root].has_binding("next"));

        // An effect write `next = 100` clears the binding (D10).
        let effects = vec![nui_compiler::Effect::Assign {
            target: PropertyTarget::Component("next".to_string()),
            op: nui_compiler::AssignOp::Set,
            value: const_int(100),
        }];
        let mut written = Vec::new();
        engine
            .run_effect(&mut tree, root, &effects, &mut written)
            .unwrap();
        assert_eq!(written, vec![(root, "next".to_string())]);
        assert_eq!(tree.arena[root].get("next"), Some(&Value::Int(100)));
        assert!(!tree.arena[root].has_binding("next"));
    }

    #[test]
    fn two_way_link_syncs_partner() {
        let mut tree = ElementTree::new();
        let root = tree.insert(Element::new("Window", None));
        let field = tree.insert(Element::new("TextField", None));
        tree.push_root(root);
        let _engine = Engine::new();
        tree.arena[field].set("text", Value::String("".to_string()));
        tree.arena[field].set_two_way(
            "text",
            TwoWayLink {
                partner: root,
                property: "userName".to_string(),
            },
        );
        // Write the field side; the partner syncs.
        tree.arena[field].set("text", Value::String("Ada".to_string()));
        let link = tree.arena[field]
            .two_way_links()
            .into_iter()
            .find(|(name, _)| return name == "text")
            .map(|(_, link)| return link)
            .unwrap();
        tree.arena[link.partner].set(&link.property, Value::String("Ada".to_string()));
        assert_eq!(
            tree.arena[root].get("userName"),
            Some(&Value::String("Ada".to_string()))
        );
    }

    #[test]
    fn interpolation_renders_display_semantics() {
        let mut tree = ElementTree::new();
        let root = tree.insert(Element::new("Window", None));
        tree.push_root(root);
        tree.arena[root].set("count", Value::Int(7));
        let mut engine = Engine::new();
        let expr = TypedExpr::Interp {
            parts: vec![
                nui_compiler::InterpPart::Text("count: ".to_string()),
                nui_compiler::InterpPart::Expr(Box::new(property(PropertyTarget::Component(
                    "count".to_string(),
                )))),
            ],
        };
        let value = engine.evaluate_free(&mut tree, root, &expr).unwrap();
        assert_eq!(value, Value::String("count: 7".to_string()));
    }

    #[test]
    fn builtin_min_max_clamp_work() {
        assert_eq!(
            apply_builtin(Builtin::Min, &[Value::Int(2), Value::Int(5)]),
            Value::Int(2)
        );
        assert_eq!(
            apply_builtin(Builtin::Max, &[Value::Int(2), Value::Int(5)]),
            Value::Int(5)
        );
        assert_eq!(
            apply_builtin(
                Builtin::Clamp,
                &[Value::Int(9), Value::Int(0), Value::Int(5)]
            ),
            Value::Int(5)
        );
    }

    #[test]
    fn math_builtins_evaluate_to_float() {
        let half_pi = Value::Float(std::f64::consts::FRAC_PI_2);
        let sin_half_pi = apply_builtin(Builtin::Sin, &[half_pi]);
        assert!((sin_half_pi.as_f64().unwrap() - 1.0).abs() < 1e-12);
        let cos_zero = apply_builtin(Builtin::Cos, &[Value::Float(0.0)]);
        assert!((cos_zero.as_f64().unwrap() - 1.0).abs() < 1e-12);
        let tan_zero = apply_builtin(Builtin::Tan, &[Value::Float(0.0)]);
        assert!(tan_zero.as_f64().unwrap().abs() < 1e-12);
        assert_eq!(
            apply_builtin(Builtin::Sqrt, &[Value::Int(9)]),
            Value::Float(3.0)
        );
        assert_eq!(
            apply_builtin(Builtin::Abs, &[Value::Float(-2.5)]),
            Value::Float(2.5)
        );
        assert_eq!(
            apply_builtin(Builtin::Floor, &[Value::Float(3.7)]),
            Value::Float(3.0)
        );
        assert_eq!(
            apply_builtin(Builtin::Ceil, &[Value::Float(3.2)]),
            Value::Float(4.0)
        );
    }

    #[test]
    fn evaluates_math_builtin_calls() {
        let mut tree = ElementTree::new();
        let root = tree.insert(Element::new("Window", None));
        tree.push_root(root);
        tree.arena[root].set("angle", Value::Float(std::f64::consts::FRAC_PI_2));
        let mut engine = Engine::new();
        let expr = TypedExpr::Call {
            func: Builtin::Sin,
            args: vec![TypedExpr::Property {
                target: PropertyTarget::Component("angle".to_string()),
                ty: nui_compiler::Type::Float,
            }],
            ty: nui_compiler::Type::Float,
        };
        let value = engine.evaluate_free(&mut tree, root, &expr).unwrap();
        assert!((value.as_f64().unwrap() - 1.0).abs() < 1e-12);
    }
}
