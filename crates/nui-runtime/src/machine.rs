//! State machine instances: runtime half of the compiler's `MachineIr`.
//!
//! Transition *taking* (guard evaluation, exit/enter effects) lives in the
//! engine (`binding.rs`) because it needs expression evaluation; this module
//! owns the instance state and the pure transition-table queries.

use nui_compiler::MachineIr;

/// One machine instance attached to an element: the compiled transition
/// table plus the mutable current state.
#[derive(Debug, Clone)]
pub struct MachineInstance {
    /// Compiled machine (states, transitions, guards, enter/exit effects).
    pub ir: MachineIr,
    /// Current state name; starts at the first declared state.
    pub current_state: String,
}

impl MachineInstance {
    /// Creates an instance for `machine`, starting in its first state.
    ///
    /// Returns `None` for machines with no states (rejected by the checker
    /// in practice, but the IR is best-effort).
    pub fn new(machine: &MachineIr) -> Option<MachineInstance> {
        let first = machine.states.first()?;
        return Some(MachineInstance {
            ir: machine.clone(),
            current_state: first.name.clone(),
        });
    }

    /// The machine name.
    pub fn name(&self) -> &str {
        return self.ir.name.as_str();
    }

    /// Whether `transition` can fire from the current state (event and
    /// source-state match; guards are engine-side).
    pub fn accepts(&self, transition: &nui_compiler::TransitionIr, signal: &str) -> bool {
        return transition.event == signal && transition.from.contains(&self.current_state);
    }

    /// Whether the machine is currently in `state` (`machine.playing` reads).
    pub fn is_in(&self, state: &str) -> bool {
        return self.current_state == state;
    }
}

#[cfg(test)]
#[allow(clippy::unwrap_used)]
mod tests {
    use super::*;
    use nui_compiler::{StateIr, TransitionIr};

    fn machine() -> MachineIr {
        return MachineIr {
            name: "playback".to_string(),
            states: vec![
                StateIr {
                    name: "stopped".to_string(),
                    enter: Vec::new(),
                    exit: Vec::new(),
                },
                StateIr {
                    name: "playing".to_string(),
                    enter: Vec::new(),
                    exit: Vec::new(),
                },
            ],
            transitions: vec![TransitionIr {
                event: "play".to_string(),
                from: vec!["stopped".to_string()],
                guard: None,
                to: "playing".to_string(),
            }],
        };
    }

    #[test]
    fn instance_starts_in_first_declared_state() {
        let instance = MachineInstance::new(&machine()).unwrap();
        assert_eq!(instance.current_state, "stopped");
        assert!(instance.is_in("stopped"));
        assert!(!instance.is_in("playing"));
    }

    #[test]
    fn accepts_matches_event_and_source_only() {
        let instance = MachineInstance::new(&machine()).unwrap();
        let transition = &machine().transitions[0];
        assert!(instance.accepts(transition, "play"));
        assert!(!instance.accepts(transition, "pause"));
        assert_eq!(instance.name(), "playback");
    }

    #[test]
    fn stateless_machine_yields_no_instance() {
        let empty = MachineIr {
            name: "m".to_string(),
            states: Vec::new(),
            transitions: Vec::new(),
        };
        assert!(MachineInstance::new(&empty).is_none());
    }
}
