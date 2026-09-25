//! Animation clock: `tween`/`spring` wrappers over the two-layer value
//! model (plan §5: properties store the binding target plus an animated
//! current value; rendering reads the animated layer).
//!
//! M2 scope: the engine writes the target as usual; when a property has an
//! active animation, the clock advances the current value toward the target
//! and the engine's read layer returns the animated value. Easing follows
//! the CSS `ease`-family curves (plan D8 keeps animations inline; easing
//! names come from the `easing` argument as `String`).

use nui_core::{Duration, Value};

/// Easing curve of a tween.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Easing {
    /// Linear (`x = t`).
    Linear,
    /// Quadratic ease-in-out (default; `ease-out` etc. map here in M2).
    EaseInOut,
    /// Cubic ease-out (fast start, slow end).
    EaseOut,
}

impl Easing {
    /// Maps an `easing = ...` string; unknown names fall back to the
    /// default (the compiler restricts the argument type, not the value).
    pub fn from_name(name: &str) -> Easing {
        return match name {
            "linear" => Easing::Linear,
            "ease-out" => Easing::EaseOut,
            _ => Easing::EaseInOut,
        };
    }

    /// Progresses `t` (0..=1) through the curve.
    pub fn apply(self, t: f64) -> f64 {
        let clamped = t.clamp(0.0, 1.0);
        return match self {
            Easing::Linear => clamped,
            Easing::EaseInOut => {
                if clamped < 0.5 {
                    2.0 * clamped * clamped
                } else {
                    let mirrored = 2.0 - 2.0 * clamped;
                    1.0 - mirrored * mirrored / 2.0
                }
            }
            Easing::EaseOut => {
                let inverted = 1.0 - clamped;
                1.0 - inverted * inverted
            }
        };
    }
}

/// One active property animation.
#[derive(Debug, Clone)]
pub struct ActiveAnimation {
    /// Animated element.
    pub element: crate::element::ElementId,
    /// Animated property.
    pub property: String,
    /// Animation kind.
    pub kind: AnimationKind,
    /// Value when the animation started.
    pub from: Value,
    /// Binding target value (what the property should reach).
    pub target: Value,
    /// Elapsed time since the animation started.
    pub elapsed: Duration,
}

/// Animation kind with its parameters.
#[derive(Debug, Clone)]
pub enum AnimationKind {
    /// `tween(target, duration, easing)`.
    Tween {
        /// Total duration.
        duration: Duration,
        /// Easing curve.
        easing: Easing,
    },
    /// `spring(target, stiffness, damping)` — critically damped approximation.
    Spring {
        /// Spring stiffness (higher = snappier).
        stiffness: f64,
        /// Damping (higher = less oscillation).
        damping: f64,
        /// Current velocity (internal state).
        velocity: f64,
        /// Continuous position (internal state). The displayed value is a
        /// projection of this (Int targets truncate per tick), so the
        /// integration is not poisoned by display quantization.
        position: f64,
    },
}

/// The animation clock: active animations and time advancement.
#[derive(Debug, Default)]
pub struct AnimationClock {
    /// Running animations.
    pub active: Vec<ActiveAnimation>,
}

impl AnimationClock {
    /// Creates an idle clock.
    pub fn new() -> AnimationClock {
        return AnimationClock::default();
    }

    /// Starts (or retargets) a tween on `element.property` from the current
    /// displayed value toward `target`.
    #[allow(clippy::too_many_arguments)]
    pub fn start_tween(
        &mut self,
        element: crate::element::ElementId,
        property: &str,
        target: Value,
        duration: Duration,
        easing: Easing,
        current: Value,
    ) {
        self.start(
            element,
            property,
            AnimationKind::Tween { duration, easing },
            target,
            current,
        );
    }

    /// Starts (or retargets) a spring on `element.property`.
    pub fn start_spring(
        &mut self,
        element: crate::element::ElementId,
        property: &str,
        target: Value,
        stiffness: f64,
        damping: f64,
        current: Value,
    ) {
        self.start(
            element,
            property,
            AnimationKind::Spring {
                stiffness,
                damping,
                velocity: 0.0,
                position: 0.0,
            },
            target,
            current,
        );
    }

    fn start(
        &mut self,
        element: crate::element::ElementId,
        property: &str,
        kind: AnimationKind,
        target: Value,
        current: Value,
    ) {
        // Retargeting an in-flight animation preserves motion state (plan
        // §5: interpolate from the current displayed value).
        if let Some(existing) = self
            .active
            .iter_mut()
            .find(|animation| return animation.element == element && animation.property == property)
        {
            existing.target = target;
            match (&mut existing.kind, kind) {
                (
                    AnimationKind::Spring {
                        stiffness, damping, ..
                    },
                    AnimationKind::Spring {
                        stiffness: new_stiffness,
                        damping: new_damping,
                        ..
                    },
                ) => {
                    // Keep position/velocity: mid-flight retarget continues
                    // the motion, only the parameters update.
                    *stiffness = new_stiffness;
                    *damping = new_damping;
                }
                (_, new_kind) => {
                    // Tweens restart their interpolation from the displayed
                    // value; kinds that switch start fresh.
                    existing.kind = new_kind;
                    existing.elapsed = Duration::ZERO;
                    existing.from = current;
                }
            }
            return;
        }
        let mut kind = kind;
        if let AnimationKind::Spring { position, .. } = &mut kind {
            *position = numeric_of(&current).unwrap_or_else(|| {
                return numeric_of(&target).unwrap_or(0.0);
            });
        }
        self.active.push(ActiveAnimation {
            element,
            property: property.to_string(),
            kind,
            from: current,
            target,
            elapsed: Duration::ZERO,
        });
    }

    /// Removes any active animation on `element.property` (used when a new
    /// target equals the displayed value — nothing to animate).
    pub fn cancel(&mut self, element: crate::element::ElementId, property: &str) {
        self.active.retain(|animation| {
            return !(animation.element == element && animation.property == property);
        });
    }

    /// Whether any animation is running (frame scheduling input: no dirty
    /// data and no active animation means no redraw, plan §2).
    pub fn is_running(&self) -> bool {
        return !self.active.is_empty();
    }

    /// Advances time by `delta` and writes the interpolated values into the
    /// element tree. Returns the animated `(element, property)` pairs whose
    /// displayed value changed this tick.
    ///
    /// Convenience for tests/hosts that do not track changes; the frame loop
    /// should call [`AnimationClock::tick_with_notify`].
    pub fn tick(
        &mut self,
        tree: &mut crate::element::ElementTree,
        delta: Duration,
    ) -> Vec<(crate::element::ElementId, String)> {
        let mut idle_engine = crate::binding::Engine::new();
        return self.tick_with_notify(tree, delta, &mut idle_engine);
    }

    /// Same as [`AnimationClock::tick`], recording each displayed-value
    /// write as a [`ChangeSource::Animation`] change in `engine`'s change
    /// buffer (the M4 notification seam).
    pub fn tick_with_notify(
        &mut self,
        tree: &mut crate::element::ElementTree,
        delta: Duration,
        engine: &mut crate::binding::Engine,
    ) -> Vec<(crate::element::ElementId, String)> {
        let mut changed = Vec::new();
        let mut finished_indices = Vec::new();
        for (index, animation) in self.active.iter_mut().enumerate() {
            animation.elapsed =
                Duration::from_millis(animation.elapsed.as_millis_f64() + delta.as_millis_f64());
            match &mut animation.kind {
                AnimationKind::Tween { duration, easing } => {
                    let total = duration.as_millis_f64();
                    let progress = if total <= 0.0 {
                        1.0
                    } else {
                        easing.apply(animation.elapsed.as_millis_f64() / total)
                    };
                    let interpolated = interpolate(&animation.from, &animation.target, progress);
                    if tree.arena[animation.element].set(&animation.property, interpolated.clone())
                    {
                        engine.record_change(
                            animation.element,
                            &animation.property,
                            interpolated,
                            crate::notify::ChangeSource::Animation,
                        );
                        changed.push((animation.element, animation.property.clone()));
                    }
                    if animation.elapsed.as_millis_f64() >= total {
                        finished_indices.push(index);
                    }
                }
                AnimationKind::Spring {
                    stiffness,
                    damping,
                    velocity,
                    position,
                } => {
                    let target = numeric_of(&animation.target);
                    let Some(target) = target else {
                        // Non-numeric spring target: snap.
                        tree.arena[animation.element]
                            .set(&animation.property, animation.target.clone());
                        engine.record_change(
                            animation.element,
                            &animation.property,
                            animation.target.clone(),
                            crate::notify::ChangeSource::Animation,
                        );
                        changed.push((animation.element, animation.property.clone()));
                        finished_indices.push(index);
                        continue;
                    };
                    let dt = delta.as_millis_f64() / 1000.0;
                    if dt <= 0.0 {
                        continue;
                    }
                    // Integrate the continuous internal state; the displayed
                    // value is only a projection (Int targets truncate).
                    let displacement = *position - target;
                    let acceleration = -(*stiffness) * displacement - (*damping) * (*velocity);
                    *velocity += acceleration * dt;
                    *position += *velocity * dt;
                    let epsilon = match &animation.target {
                        Value::Int(_) => SETTLE_EPSILON.max(0.5),
                        _ => SETTLE_EPSILON,
                    };
                    let settled = displacement.abs() < epsilon && velocity.abs() < epsilon;
                    let value = numeric_value(*position, &animation.target);
                    if tree.arena[animation.element].set(&animation.property, value.clone()) {
                        engine.record_change(
                            animation.element,
                            &animation.property,
                            value,
                            crate::notify::ChangeSource::Animation,
                        );
                        changed.push((animation.element, animation.property.clone()));
                    }
                    if settled {
                        tree.arena[animation.element]
                            .set(&animation.property, animation.target.clone());
                        engine.record_change(
                            animation.element,
                            &animation.property,
                            animation.target.clone(),
                            crate::notify::ChangeSource::Animation,
                        );
                        finished_indices.push(index);
                    }
                }
            }
        }
        for index in finished_indices.into_iter().rev() {
            self.active.remove(index);
        }
        return changed;
    }
}

/// Spring settle threshold (values below this count as arrived).
const SETTLE_EPSILON: f64 = 0.001;

/// Extracts the numeric value of a Value for spring physics.
fn numeric_of(value: &Value) -> Option<f64> {
    return match value {
        Value::Int(inner) => Some(*inner as f64),
        Value::Float(inner) => Some(*inner),
        Value::Length(nui_core::Length::Dp(inner)) => Some(*inner as f64),
        _ => None,
    };
}

/// Wraps a raw number back into the target's value type.
fn numeric_value(raw: f64, like: &Value) -> Value {
    return match like {
        Value::Int(_) => Value::Int(raw as i64),
        Value::Float(_) => Value::Float(raw),
        Value::Length(nui_core::Length::Dp(_)) => Value::Length(nui_core::Length::Dp(raw as f32)),
        _ => like.clone(),
    };
}

/// Interpolates between two values of the same type at `progress` (0..=1).
fn interpolate(from: &Value, to: &Value, progress: f64) -> Value {
    let mix = |a: f64, b: f64| -> f64 {
        return a + (b - a) * progress;
    };
    return match (from, to) {
        (Value::Int(a), Value::Int(b)) => Value::Int(mix(*a as f64, *b as f64) as i64),
        (Value::Float(a), Value::Float(b)) => Value::Float(mix(*a, *b)),
        (Value::Length(nui_core::Length::Dp(a)), Value::Length(nui_core::Length::Dp(b))) => {
            Value::Length(nui_core::Length::Dp(mix(*a as f64, *b as f64) as f32))
        }
        (Value::Duration(a), Value::Duration(b)) => Value::Duration(Duration::from_millis(mix(
            a.as_millis_f64(),
            b.as_millis_f64(),
        ))),
        // Non-interpolable values hold the target for the whole run.
        _ => to.clone(),
    };
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use crate::element::{Element, ElementTree};

    fn tree_with_opacity() -> (ElementTree, crate::element::ElementId) {
        let mut tree = ElementTree::new();
        let root = tree.insert(Element::new("Window", None));
        tree.push_root(root);
        tree.arena[root].set("opacity", Value::Float(0.0));
        return (tree, root);
    }

    #[test]
    fn easing_curves_hit_endpoints() {
        for easing in [Easing::Linear, Easing::EaseInOut, Easing::EaseOut] {
            assert!((easing.apply(0.0) - 0.0).abs() < 1e-9);
            assert!((easing.apply(1.0) - 1.0).abs() < 1e-9);
        }
        assert!(Easing::EaseOut.apply(0.5) > 0.5, "ease-out front-loads");
    }

    #[test]
    fn tween_completes_over_duration() {
        let (mut tree, root) = tree_with_opacity();
        let mut clock = AnimationClock::new();
        clock.start_tween(
            root,
            "opacity",
            Value::Float(1.0),
            Duration::from_millis(100.0),
            Easing::Linear,
            Value::Float(0.0),
        );
        assert!(clock.is_running());
        let changed = clock.tick(&mut tree, Duration::from_millis(50.0));
        assert_eq!(changed, vec![(root, "opacity".to_string())]);
        let mid = tree.arena[root].get("opacity").cloned().unwrap();
        assert!((mid.as_f64().unwrap() - 0.5).abs() < 1e-9, "got {mid:?}");
        clock.tick(&mut tree, Duration::from_millis(50.0));
        assert_eq!(tree.arena[root].get("opacity"), Some(&Value::Float(1.0)));
        assert!(!clock.is_running(), "finished tweens are removed");
    }

    #[test]
    fn retarget_restarts_from_current_value() {
        let (mut tree, root) = tree_with_opacity();
        let mut clock = AnimationClock::new();
        clock.start_tween(
            root,
            "opacity",
            Value::Float(1.0),
            Duration::from_millis(100.0),
            Easing::Linear,
            Value::Float(0.0),
        );
        clock.tick(&mut tree, Duration::from_millis(50.0));
        // Retarget mid-flight: keeps interpolating from the displayed value.
        let current = tree.arena[root].get("opacity").cloned().unwrap();
        clock.start_tween(
            root,
            "opacity",
            Value::Float(2.0),
            Duration::from_millis(100.0),
            Easing::Linear,
            current,
        );
        assert_eq!(clock.active.len(), 1, "retarget must not duplicate");
        assert_eq!(clock.active[0].target, Value::Float(2.0));
    }

    #[test]
    fn spring_settles_at_target() {
        let (mut tree, root) = tree_with_opacity();
        let mut clock = AnimationClock::new();
        clock.start_spring(
            root,
            "opacity",
            Value::Float(1.0),
            120.0,
            14.0,
            Value::Float(0.0),
        );
        for _ in 0..200 {
            clock.tick(&mut tree, Duration::from_millis(16.0));
            if !clock.is_running() {
                break;
            }
        }
        let final_value = tree.arena[root].get("opacity").cloned().unwrap();
        let distance = (final_value.as_f64().unwrap() - 1.0).abs();
        assert!(
            distance < 0.01,
            "spring should settle near 1.0, got {final_value:?}"
        );
        assert!(!clock.is_running());
    }

    #[test]
    fn interpolates_length_values() {
        let mut tree = ElementTree::new();
        let root = tree.insert(Element::new("Window", None));
        tree.push_root(root);
        tree.arena[root].set("x", Value::Length(nui_core::Length::Dp(0.0)));
        let mut clock = AnimationClock::new();
        clock.start_tween(
            root,
            "x",
            Value::Length(nui_core::Length::Dp(100.0)),
            Duration::from_millis(100.0),
            Easing::Linear,
            Value::Length(nui_core::Length::Dp(0.0)),
        );
        clock.tick(&mut tree, Duration::from_millis(25.0));
        let value = tree.arena[root].get("x").cloned().unwrap();
        assert_eq!(value, Value::Length(nui_core::Length::Dp(25.0)));
    }
}
