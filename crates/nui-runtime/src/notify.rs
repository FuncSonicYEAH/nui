//! Property change notification: the observer seam between the engine and
//! hosts (plan §5 "宿主互操作" groundwork for M4's `Model` protocol).
//!
//! Every engine-mediated property write funnels through
//! [`Engine::record_change`], which accumulates [`PropertyChange`] events in
//! a frame buffer. Hosts either drain the buffer per frame
//! ([`Engine::take_changes`]) or subscribe a [`PropertyObserver`] and receive
//! the same events at drain time. Observers are keyed by handle so they can
//! unsubscribe; the observer list stays tiny (host-side concern), so
//! subscription is a `Vec` scan, not a map.

use nui_core::Value;

use crate::element::ElementId;

/// One observed property write.
#[derive(Debug, Clone, PartialEq)]
pub struct PropertyChange {
    /// Element whose property changed.
    pub element: ElementId,
    /// Property name.
    pub property: String,
    /// Value after the write.
    pub new_value: Value,
    /// What kind of engine path produced the write.
    pub source: ChangeSource,
}

/// Origin of a property change (host routing/debugging aid).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ChangeSource {
    /// A `<-` binding re-evaluated and wrote its property.
    Binding,
    /// An effect-block assignment (`on click => count += 1`).
    Effect,
    /// A `when` block applying or restoring an override.
    WhenBlock,
    /// A two-way `<=>` partner sync through the value channel.
    TwoWay,
    /// The animation clock advancing a displayed value.
    Animation,
    /// A direct host write ([`ElementTree::set_direct`]) — M4 models.
    Host,
    /// Keyboard / IME input into a focused `TextInput` (M7).
    Input,
}

/// Observer of property changes, called when the engine's change buffer is
/// drained.
pub trait PropertyObserver: std::fmt::Debug {
    /// Called once per buffered change, in write order.
    fn on_property_change(&mut self, change: &PropertyChange);
}

/// Handle for unsubscribing an observer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ObserverHandle(pub usize);

/// Subscription storage: boxed observers with stable handles.
#[derive(Default)]
pub(crate) struct Observers {
    /// Subscribed observers; `None` marks an unsubscribed slot.
    entries: Vec<Option<(ObserverHandle, Box<dyn PropertyObserver>)>>,
    /// Next handle value.
    next_handle: usize,
}

impl Observers {
    /// Subscribes an observer and returns its unsubscribe handle.
    pub fn subscribe(&mut self, observer: Box<dyn PropertyObserver>) -> ObserverHandle {
        let handle = ObserverHandle(self.next_handle);
        self.next_handle += 1;
        self.entries.push(Some((handle, observer)));
        return handle;
    }

    /// Removes the observer with `handle`; returns whether it was found.
    pub fn unsubscribe(&mut self, handle: ObserverHandle) -> bool {
        for entry in &mut self.entries {
            if let Some((entry_handle, _)) = entry
                && *entry_handle == handle
            {
                *entry = None;
                return true;
            }
        }
        return false;
    }

    /// Runs `visit` for every live observer (deduped by handle).
    pub fn for_each(&mut self, visit: &mut impl FnMut(&mut dyn PropertyObserver)) {
        for (_, observer) in self.entries.iter_mut().flatten() {
            visit(observer.as_mut());
        }
    }
}

impl std::fmt::Debug for Observers {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let live = self
            .entries
            .iter()
            .filter(|entry| return entry.is_some())
            .count();
        return write!(formatter, "Observers {{ live: {live} }}");
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use std::cell::RefCell;
    use std::rc::Rc;

    /// Observer that appends `property=value` into a shared log.
    #[derive(Debug, Clone)]
    struct Recording {
        log: Rc<RefCell<Vec<String>>>,
    }

    impl PropertyObserver for Recording {
        fn on_property_change(&mut self, change: &PropertyChange) {
            self.log
                .borrow_mut()
                .push(format!("{}={}", change.property, change.new_value));
        }
    }

    #[test]
    fn subscribe_delivers_and_unsubscribe_stops() {
        let log = Rc::new(RefCell::new(Vec::new()));
        let mut observers = Observers::default();
        let handle = observers.subscribe(Box::new(Recording {
            log: Rc::clone(&log),
        }));
        let change = PropertyChange {
            element: crate::element::ElementId::default(),
            property: "count".to_string(),
            new_value: Value::Int(1),
            source: ChangeSource::Effect,
        };
        observers.for_each(&mut |observer| observer.on_property_change(&change));
        assert_eq!(*log.borrow(), vec!["count=1".to_string()]);

        assert!(observers.unsubscribe(handle));
        observers.for_each(&mut |observer| observer.on_property_change(&change));
        assert_eq!(log.borrow().len(), 1, "unsubscribed observer must be quiet");
    }

    #[test]
    fn unsubscribe_of_unknown_handle_returns_false() {
        let mut observers = Observers::default();
        assert!(!observers.unsubscribe(ObserverHandle(99)));
    }
}
