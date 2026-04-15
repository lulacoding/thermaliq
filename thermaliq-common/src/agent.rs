use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::collections::VecDeque;

/// All possible states of the AI thermal agent.
#[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
pub enum AgentState {
    Sampling,
    Analyzing,
    Proposing,
    Awaiting { intervention_id: String },
    Executing,
    Verifying,
    RollingBack,
    Error { reason: String },
    Degraded { reason: String },
    Suspended,
}

/// Events that drive state transitions.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum AgentTransition {
    StartSampling,
    AnalysisComplete,
    ProposeIntervention { id: String },
    UserApproved,
    UserRejected,
    ExecutionComplete,
    VerificationPassed,
    VerificationFailed,
    RollbackComplete,
    FaultDetected { reason: String },
    Degrade { reason: String },
    Suspend,
    Resume,
    Reset,
}

pub struct AgentStateMachine {
    pub state: AgentState,
}

impl AgentStateMachine {
    pub fn new() -> Self {
        Self { state: AgentState::Sampling }
    }

    /// Apply a transition. Returns Err if the transition is illegal in current state.
    pub fn transition(&mut self, t: AgentTransition) -> Result<()> {
        let next = match (&self.state, &t) {
            // From Sampling
            (AgentState::Sampling, AgentTransition::AnalysisComplete) => AgentState::Analyzing,
            (AgentState::Sampling, AgentTransition::FaultDetected { reason }) => {
                AgentState::Error { reason: reason.clone() }
            }
            (AgentState::Sampling, AgentTransition::Suspend) => AgentState::Suspended,

            // From Analyzing
            (AgentState::Analyzing, AgentTransition::ProposeIntervention { id: _ }) => {
                AgentState::Proposing
            }
            (AgentState::Analyzing, AgentTransition::StartSampling) => AgentState::Sampling,
            (AgentState::Analyzing, AgentTransition::FaultDetected { reason }) => {
                AgentState::Degraded { reason: reason.clone() }
            }

            // From Proposing
            (AgentState::Proposing, AgentTransition::ProposeIntervention { id }) => {
                AgentState::Awaiting { intervention_id: id.clone() }
            }
            (AgentState::Proposing, AgentTransition::StartSampling) => AgentState::Sampling,

            // From Awaiting
            (AgentState::Awaiting { .. }, AgentTransition::UserApproved) => AgentState::Executing,
            (AgentState::Awaiting { .. }, AgentTransition::UserRejected) => AgentState::Sampling,
            (AgentState::Awaiting { .. }, AgentTransition::FaultDetected { reason }) => {
                AgentState::Error { reason: reason.clone() }
            }

            // From Executing
            (AgentState::Executing, AgentTransition::ExecutionComplete) => AgentState::Verifying,
            (AgentState::Executing, AgentTransition::FaultDetected { reason }) => {
                AgentState::Error { reason: reason.clone() }
            }

            // From Verifying
            (AgentState::Verifying, AgentTransition::VerificationPassed) => AgentState::Sampling,
            (AgentState::Verifying, AgentTransition::VerificationFailed) => AgentState::RollingBack,
            (AgentState::Verifying, AgentTransition::FaultDetected { reason }) => {
                AgentState::Error { reason: reason.clone() }
            }

            // From RollingBack
            (AgentState::RollingBack, AgentTransition::RollbackComplete) => AgentState::Degraded {
                reason: "rolled back after failed verification".into(),
            },
            (AgentState::RollingBack, AgentTransition::FaultDetected { reason }) => {
                AgentState::Error { reason: reason.clone() }
            }

            // From Error — only Reset recovers
            (AgentState::Error { .. }, AgentTransition::Reset) => AgentState::Sampling,
            (AgentState::Error { .. }, AgentTransition::Suspend) => AgentState::Suspended,

            // From Degraded — Resume drops to Sampling, Suspend pauses
            (AgentState::Degraded { .. }, AgentTransition::Resume) => AgentState::Sampling,
            (AgentState::Degraded { .. }, AgentTransition::Suspend) => AgentState::Suspended,
            (AgentState::Degraded { .. }, AgentTransition::Reset) => AgentState::Sampling,

            // From Suspended — Resume returns to Sampling
            (AgentState::Suspended, AgentTransition::Resume) => AgentState::Sampling,

            // All other combinations are illegal
            _ => bail!(
                "illegal transition {:?} in state {:?}",
                t,
                self.state
            ),
        };
        self.state = next;
        Ok(())
    }
}

impl Default for AgentStateMachine {
    fn default() -> Self {
        Self::new()
    }
}

/// dT/dt thermal runaway guard. Returns true if temperature is rising faster
/// than `threshold_celsius_per_sec` over the sliding window.
pub struct DtDtGuard {
    pub threshold_celsius_per_sec: f32,
    pub window_secs: u64,
    readings: VecDeque<(u64, f32)>, // (unix_timestamp, temp_celsius)
}

impl DtDtGuard {
    pub fn new(threshold_celsius_per_sec: f32, window_secs: u64) -> Self {
        Self {
            threshold_celsius_per_sec,
            window_secs,
            readings: VecDeque::new(),
        }
    }

    /// Push a new reading. Returns true if thermal runaway is detected.
    pub fn update(&mut self, timestamp: u64, temp: f32) -> bool {
        self.readings.push_back((timestamp, temp));
        // Evict readings outside the window
        while let Some(&(oldest_ts, _)) = self.readings.front() {
            if timestamp.saturating_sub(oldest_ts) > self.window_secs {
                self.readings.pop_front();
            } else {
                break;
            }
        }
        if self.readings.len() < 2 {
            return false;
        }
        let (oldest_ts, oldest_temp) = self.readings.front().copied().unwrap();
        let elapsed = timestamp.saturating_sub(oldest_ts);
        if elapsed == 0 {
            return false;
        }
        let rate = (temp - oldest_temp) / elapsed as f32;
        rate > self.threshold_celsius_per_sec
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn legal_transition_sampling_to_analyzing() {
        let mut sm = AgentStateMachine::new();
        assert!(sm.transition(AgentTransition::AnalysisComplete).is_ok());
        assert_eq!(sm.state, AgentState::Analyzing);
    }

    #[test]
    fn illegal_transition_returns_err() {
        let mut sm = AgentStateMachine::new();
        // Cannot go from Sampling directly to Executing
        assert!(sm.transition(AgentTransition::ExecutionComplete).is_err());
        // State must not have changed
        assert_eq!(sm.state, AgentState::Sampling);
    }

    #[test]
    fn full_happy_path() {
        let mut sm = AgentStateMachine::new();
        sm.transition(AgentTransition::AnalysisComplete).unwrap();
        sm.transition(AgentTransition::ProposeIntervention { id: "p1".into() }).unwrap();
        sm.transition(AgentTransition::ProposeIntervention { id: "i1".into() }).unwrap();
        sm.transition(AgentTransition::UserApproved).unwrap();
        sm.transition(AgentTransition::ExecutionComplete).unwrap();
        sm.transition(AgentTransition::VerificationPassed).unwrap();
        assert_eq!(sm.state, AgentState::Sampling);
    }

    #[test]
    fn error_state_recovers_via_reset() {
        let mut sm = AgentStateMachine::new();
        sm.transition(AgentTransition::FaultDetected { reason: "test".into() }).unwrap();
        assert!(matches!(sm.state, AgentState::Error { .. }));
        sm.transition(AgentTransition::Reset).unwrap();
        assert_eq!(sm.state, AgentState::Sampling);
    }

    #[test]
    fn dtdt_guard_triggers_on_fast_rise() {
        let mut guard = DtDtGuard::new(5.0, 30);
        assert!(!guard.update(1000, 40.0));
        assert!(guard.update(1005, 70.0)); // 30°C in 5s = 6°C/s > threshold
    }

    #[test]
    fn dtdt_guard_silent_on_slow_rise() {
        let mut guard = DtDtGuard::new(5.0, 30);
        guard.update(1000, 40.0);
        assert!(!guard.update(1010, 42.0)); // 2°C in 10s = 0.2°C/s
    }
}
