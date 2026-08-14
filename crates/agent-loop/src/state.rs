use agent_core::LoopPhase;

use crate::{LoopTransitionError, ReflectDecision};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct LoopState {
    current_iteration: Option<u32>,
    completed_iterations: u32,
    position: LoopPosition,
}

impl Default for LoopState {
    fn default() -> Self {
        Self {
            current_iteration: None,
            completed_iterations: 0,
            position: LoopPosition::Ready,
        }
    }
}

impl LoopState {
    #[must_use]
    pub const fn new() -> Self {
        Self {
            current_iteration: None,
            completed_iterations: 0,
            position: LoopPosition::Ready,
        }
    }

    #[must_use]
    pub const fn current_iteration(&self) -> Option<u32> {
        self.current_iteration
    }

    #[must_use]
    pub const fn completed_iterations(&self) -> u32 {
        self.completed_iterations
    }

    #[must_use]
    pub const fn position(&self) -> LoopPosition {
        self.position
    }

    pub(crate) fn begin_iteration(&mut self, iteration: u32) -> Result<(), LoopTransitionError> {
        match self.position {
            LoopPosition::Ready => {
                self.current_iteration = Some(iteration);
                self.position = LoopPosition::PhaseReady(LoopPhase::Observe);
                Ok(())
            }
            LoopPosition::PhaseReady(_)
            | LoopPosition::PhaseRunning(_)
            | LoopPosition::Finished(_) => Err(LoopTransitionError::BeginRequiresReady),
        }
    }

    pub(crate) fn enter_phase(&mut self, phase: LoopPhase) -> Result<(), LoopTransitionError> {
        match self.position {
            LoopPosition::PhaseReady(expected) if expected == phase => {
                self.position = LoopPosition::PhaseRunning(phase);
                Ok(())
            }
            LoopPosition::Ready | LoopPosition::PhaseReady(_) | LoopPosition::PhaseRunning(_) => {
                Err(LoopTransitionError::IllegalPhaseEntry { phase })
            }
            LoopPosition::Finished(_) => Err(LoopTransitionError::AlreadyTerminal),
        }
    }

    pub(crate) fn complete_phase(&mut self, phase: LoopPhase) -> Result<(), LoopTransitionError> {
        match (self.position, next_phase(phase)) {
            (LoopPosition::PhaseRunning(current), Some(next)) if current == phase => {
                self.position = LoopPosition::PhaseReady(next);
                Ok(())
            }
            (LoopPosition::Finished(_), _) => Err(LoopTransitionError::AlreadyTerminal),
            _ => Err(LoopTransitionError::IllegalPhaseCompletion { phase }),
        }
    }

    pub(crate) fn accept_reflect(
        &mut self,
        decision: ReflectDecision,
    ) -> Result<(), LoopTransitionError> {
        match self.position {
            LoopPosition::PhaseRunning(LoopPhase::Reflect) => {
                self.completed_iterations += 1;
                match decision {
                    ReflectDecision::Continue => {
                        self.current_iteration = None;
                        self.position = LoopPosition::Ready;
                        Ok(())
                    }
                    ReflectDecision::Complete => {
                        let terminal = LoopTerminal::Complete;
                        self.position = LoopPosition::Finished(terminal);
                        Ok(())
                    }
                    ReflectDecision::Fail { kind } => {
                        let terminal = LoopTerminal::Fail { kind };
                        self.position = LoopPosition::Finished(terminal);
                        Ok(())
                    }
                }
            }
            LoopPosition::Finished(_) => Err(LoopTransitionError::AlreadyTerminal),
            _ => Err(LoopTransitionError::IllegalPhaseCompletion {
                phase: LoopPhase::Reflect,
            }),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopPosition {
    Ready,
    PhaseReady(LoopPhase),
    PhaseRunning(LoopPhase),
    Finished(LoopTerminal),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum LoopTerminal {
    Complete,
    Fail { kind: agent_core::LoopFailureKind },
}

const fn next_phase(phase: LoopPhase) -> Option<LoopPhase> {
    match phase {
        LoopPhase::Observe => Some(LoopPhase::Retrieve),
        LoopPhase::Retrieve => Some(LoopPhase::Plan),
        LoopPhase::Plan => Some(LoopPhase::Act),
        LoopPhase::Act => Some(LoopPhase::Verify),
        LoopPhase::Verify => Some(LoopPhase::Reflect),
        LoopPhase::Reflect => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use agent_core::LoopFailureKind;

    #[test]
    fn legal_order_is_exact() {
        let mut state = LoopState::new();

        state.begin_iteration(1).expect("iteration must begin");
        for phase in [
            LoopPhase::Observe,
            LoopPhase::Retrieve,
            LoopPhase::Plan,
            LoopPhase::Act,
            LoopPhase::Verify,
        ] {
            state.enter_phase(phase).expect("phase must enter");
            state.complete_phase(phase).expect("phase must complete");
        }
        state
            .enter_phase(LoopPhase::Reflect)
            .expect("reflect must enter");
        assert_eq!(state.accept_reflect(ReflectDecision::Complete), Ok(()));
        assert_eq!(state.completed_iterations(), 1);
        assert!(matches!(
            state.position(),
            LoopPosition::Finished(LoopTerminal::Complete)
        ));
    }

    #[test]
    fn illegal_transition_is_rejected() {
        let mut state = LoopState::new();

        assert_eq!(
            state.enter_phase(LoopPhase::Plan),
            Err(LoopTransitionError::IllegalPhaseEntry {
                phase: LoopPhase::Plan,
            })
        );
        state.begin_iteration(1).expect("iteration must begin");
        assert_eq!(
            state.complete_phase(LoopPhase::Observe),
            Err(LoopTransitionError::IllegalPhaseCompletion {
                phase: LoopPhase::Observe,
            })
        );
    }

    #[test]
    fn continue_returns_to_ready_without_persistence_contract() {
        let mut state = LoopState::new();
        state.begin_iteration(1).expect("iteration must begin");
        for phase in [
            LoopPhase::Observe,
            LoopPhase::Retrieve,
            LoopPhase::Plan,
            LoopPhase::Act,
            LoopPhase::Verify,
        ] {
            state.enter_phase(phase).expect("phase must enter");
            state.complete_phase(phase).expect("phase must complete");
        }
        state
            .enter_phase(LoopPhase::Reflect)
            .expect("reflect must enter");

        assert_eq!(state.accept_reflect(ReflectDecision::Continue), Ok(()));
        assert_eq!(state.position(), LoopPosition::Ready);
        assert_eq!(state.current_iteration(), None);
        assert_eq!(state.completed_iterations(), 1);
    }

    #[test]
    fn fail_terminalizes_control_state() {
        let mut state = LoopState::new();
        state.begin_iteration(1).expect("iteration must begin");
        for phase in [
            LoopPhase::Observe,
            LoopPhase::Retrieve,
            LoopPhase::Plan,
            LoopPhase::Act,
            LoopPhase::Verify,
        ] {
            state.enter_phase(phase).expect("phase must enter");
            state.complete_phase(phase).expect("phase must complete");
        }
        state
            .enter_phase(LoopPhase::Reflect)
            .expect("reflect must enter");

        let kind = LoopFailureKind::VerificationFailed;
        assert_eq!(state.accept_reflect(ReflectDecision::Fail { kind }), Ok(()));
        assert!(matches!(
            state.position(),
            LoopPosition::Finished(LoopTerminal::Fail { kind: returned }) if returned == kind
        ));
    }
}
