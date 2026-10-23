//! The Dock state machine. Pure: no I/O, fully unit-tested. Drivers feed it probe results,
//! the UI reads the resulting `DockState`.

use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case", tag = "phase")]
pub enum Phase {
    Idle,
    Calling { seat: String },
    InCall { seat: String, elapsed_secs: u64 },
    /// Hanging up `from` and about to call `to`; cancellable until the deadline.
    Switching { from: String, to: String, undo_secs_left: u64 },
    /// The driver hit a captcha / unknown dialog and needs the user to look.
    NeedsHuman { seat: String, reason: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DockState {
    pub phase: Phase,
    pub muted: bool,
    pub expanded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    Call(String),
    Hangup,
    ToggleMute,
    SetMuted(bool),
    ToggleExpanded,
    SetExpanded(bool),
    Undo,
    /// From a driver probe: the seat reports it is (not) in a call.
    ProbeInCall { seat: String, in_call: bool },
    ProbeNeedsHuman { seat: String, reason: String },
    Tick,
}

/// Side effects the host must perform after an event. Keeps the machine pure.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Effect {
    DriverCall(String),
    DriverHangup(String),
    SystemMute(bool),
}

pub const UNDO_WINDOW: Duration = Duration::from_secs(3);
/// Long enough for an app woken from background throttling to put the call through.
pub const CALL_TIMEOUT: Duration = Duration::from_secs(25);
/// The Dock shows this reason in the user's language (src/i18n.ts `dock.callTimeout`).
pub const REASON_CALL_TIMEOUT: &str = "call_timeout";

#[derive(Debug)]
pub struct Machine {
    pub state: DockState,
    call_started: Option<Instant>,
    switch_deadline: Option<Instant>,
    calling_since: Option<Instant>,
}

impl Default for Machine {
    fn default() -> Self {
        Self::new()
    }
}

impl Machine {
    pub fn new() -> Self {
        Machine {
            state: DockState { phase: Phase::Idle, muted: false, expanded: false },
            call_started: None,
            switch_deadline: None,
            calling_since: None,
        }
    }

    pub fn handle(&mut self, ev: Event) -> Vec<Effect> {
        self.handle_at(ev, Instant::now())
    }

    pub fn handle_at(&mut self, ev: Event, now: Instant) -> Vec<Effect> {
        use Phase::*;
        let mut fx = Vec::new();
        match (self.state.phase.clone(), ev) {
            (_, Event::ToggleExpanded) => self.state.expanded = !self.state.expanded,
            (_, Event::SetExpanded(v)) => self.state.expanded = v,
            // No call to mute: the switch is the whole Mac's microphone, and turning it off here
            // would only silence dictation and every other app.
            (Idle, Event::ToggleMute | Event::SetMuted(_)) => {}
            (_, Event::ToggleMute) => {
                self.state.muted = !self.state.muted;
                fx.push(Effect::SystemMute(self.state.muted));
            }
            (_, Event::SetMuted(v)) => {
                if self.state.muted != v {
                    self.state.muted = v;
                    fx.push(Effect::SystemMute(v));
                }
            }

            (Idle, Event::Call(seat)) | (NeedsHuman { .. }, Event::Call(seat)) => {
                // The seat that needed a look may still hold a live call (a timeout the probe
                // missed), and most apps keep one call at a time: hang it up before dialling.
                if let NeedsHuman { seat: cur, .. } = &self.state.phase {
                    if *cur != seat {
                        fx.push(Effect::DriverHangup(cur.clone()));
                    }
                }
                self.state.phase = Calling { seat: seat.clone() };
                self.state.expanded = true;
                self.calling_since = Some(now);
                fx.push(Effect::DriverCall(seat));
            }
            (Calling { seat: cur }, Event::Call(seat)) if cur == seat => {}
            (Calling { seat: cur }, Event::Call(seat)) | (InCall { seat: cur, .. }, Event::Call(seat)) => {
                if cur == seat {
                    return fx;
                }
                // Switching keeps the mute state; the hangup is deferred until the undo window closes.
                self.state.phase = Switching { from: cur, to: seat, undo_secs_left: UNDO_WINDOW.as_secs() };
                self.switch_deadline = Some(now + UNDO_WINDOW);
            }
            (Switching { from, to, .. }, Event::Call(seat)) => {
                if seat == from {
                    // Tapping the original seat again cancels the switch.
                    self.restore_in_call(from, now);
                } else {
                    self.state.phase = Switching { from, to: seat, undo_secs_left: UNDO_WINDOW.as_secs() };
                    self.switch_deadline = Some(now + UNDO_WINDOW);
                    let _ = to;
                }
            }
            (Switching { from, .. }, Event::Undo) => self.restore_in_call(from, now),

            (Calling { seat }, Event::Hangup) | (InCall { seat, .. }, Event::Hangup) | (NeedsHuman { seat, .. }, Event::Hangup) => {
                fx.push(Effect::DriverHangup(seat));
                self.to_idle(&mut fx);
            }
            (Switching { from, .. }, Event::Hangup) => {
                fx.push(Effect::DriverHangup(from));
                self.to_idle(&mut fx);
            }

            // Also out of "needs you": a call the probe only confirmed after the timeout is live after all.
            (Calling { seat: cur } | NeedsHuman { seat: cur, .. }, Event::ProbeInCall { seat, in_call: true }) if cur == seat => {
                self.call_started = Some(now);
                self.calling_since = None;
                self.state.phase = InCall { seat, elapsed_secs: 0 };
            }
            (InCall { seat: cur, .. }, Event::ProbeInCall { seat, in_call: false }) if cur == seat => {
                // The far side hung up (or the page lost the call).
                self.to_idle(&mut fx);
            }
            (_, Event::ProbeNeedsHuman { seat, reason }) => {
                self.state.phase = NeedsHuman { seat, reason };
                self.state.expanded = true;
            }

            (InCall { seat, .. }, Event::Tick) => {
                let elapsed = self.call_started.map(|t| now.duration_since(t).as_secs()).unwrap_or(0);
                self.state.phase = InCall { seat, elapsed_secs: elapsed };
            }
            (Calling { seat }, Event::Tick) => {
                if let Some(since) = self.calling_since {
                    if now.duration_since(since) >= CALL_TIMEOUT {
                        self.state.phase = NeedsHuman { seat, reason: REASON_CALL_TIMEOUT.into() };
                    }
                }
            }
            (Switching { from, to, .. }, Event::Tick) => {
                let deadline = self.switch_deadline.unwrap_or(now);
                if now >= deadline {
                    self.state.phase = Calling { seat: to.clone() };
                    self.calling_since = Some(now);
                    self.switch_deadline = None;
                    fx.push(Effect::DriverHangup(from));
                    fx.push(Effect::DriverCall(to));
                } else {
                    let left = deadline.duration_since(now);
                    self.state.phase = Switching { from, to, undo_secs_left: ((left.as_millis() + 999) / 1000) as u64 };
                }
            }
            _ => {}
        }
        fx
    }

    /// The microphone comes back on with the call gone: a mute left on in idle is the whole Mac's.
    fn to_idle(&mut self, fx: &mut Vec<Effect>) {
        self.state.phase = Phase::Idle;
        self.call_started = None;
        self.switch_deadline = None;
        self.calling_since = None;
        if std::mem::take(&mut self.state.muted) {
            fx.push(Effect::SystemMute(false));
        }
    }

    fn restore_in_call(&mut self, seat: String, now: Instant) {
        let elapsed = self.call_started.map(|t| now.duration_since(t).as_secs()).unwrap_or(0);
        self.state.phase = Phase::InCall { seat, elapsed_secs: elapsed };
        self.switch_deadline = None;
    }

    pub fn current_seat(&self) -> Option<&str> {
        match &self.state.phase {
            Phase::Idle => None,
            Phase::Calling { seat } | Phase::InCall { seat, .. } | Phase::NeedsHuman { seat, .. } => Some(seat),
            Phase::Switching { from, .. } => Some(from),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn connected(m: &mut Machine, seat: &str, t: Instant) {
        assert_eq!(m.handle_at(Event::Call(seat.into()), t), vec![Effect::DriverCall(seat.into())]);
        m.handle_at(Event::ProbeInCall { seat: seat.into(), in_call: true }, t + Duration::from_secs(1));
        assert!(matches!(m.state.phase, Phase::InCall { .. }));
    }

    #[test]
    fn call_then_hangup() {
        let t = Instant::now();
        let mut m = Machine::new();
        connected(&mut m, "dot", t);
        assert!(m.state.expanded, "calling auto-expands the dock");
        m.handle_at(Event::Tick, t + Duration::from_secs(192));
        assert_eq!(m.state.phase, Phase::InCall { seat: "dot".into(), elapsed_secs: 191 });
        assert_eq!(m.handle_at(Event::Hangup, t + Duration::from_secs(200)), vec![Effect::DriverHangup("dot".into())]);
        assert_eq!(m.state.phase, Phase::Idle);
    }

    #[test]
    fn mute_is_system_level_and_survives_switch() {
        let t = Instant::now();
        let mut m = Machine::new();
        connected(&mut m, "dot", t);
        assert_eq!(m.handle_at(Event::ToggleMute, t), vec![Effect::SystemMute(true)]);
        m.handle_at(Event::Call("grok-bot".into()), t + Duration::from_secs(5));
        assert!(matches!(m.state.phase, Phase::Switching { .. }));
        let fx = m.handle_at(Event::Tick, t + Duration::from_secs(9));
        assert_eq!(fx, vec![Effect::DriverHangup("dot".into()), Effect::DriverCall("grok-bot".into())]);
        assert_eq!(m.state.phase, Phase::Calling { seat: "grok-bot".into() });
        assert!(m.state.muted, "mute state is kept across a switch");
    }

    #[test]
    fn the_microphone_comes_back_when_the_call_ends_and_idle_never_mutes_it() {
        let t = Instant::now();
        let mut m = Machine::new();
        assert!(m.handle_at(Event::ToggleMute, t).is_empty(), "no call, nothing to mute");
        assert!(m.handle_at(Event::SetMuted(true), t).is_empty());
        assert!(!m.state.muted);

        connected(&mut m, "dot", t);
        m.handle_at(Event::ToggleMute, t);
        assert_eq!(m.handle_at(Event::Hangup, t), vec![Effect::DriverHangup("dot".into()), Effect::SystemMute(false)]);
        assert!(!m.state.muted);

        connected(&mut m, "dot", t);
        m.handle_at(Event::SetMuted(true), t);
        let fx = m.handle_at(Event::ProbeInCall { seat: "dot".into(), in_call: false }, t + Duration::from_secs(9));
        assert_eq!(fx, vec![Effect::SystemMute(false)], "the far side hanging up unmutes too");
        assert_eq!(m.state.phase, Phase::Idle);
    }

    #[test]
    fn switch_can_be_undone_within_window() {
        let t = Instant::now();
        let mut m = Machine::new();
        connected(&mut m, "dot", t);
        m.handle_at(Event::Call("copilot".into()), t + Duration::from_secs(10));
        m.handle_at(Event::Tick, t + Duration::from_secs(11));
        assert_eq!(m.state.phase, Phase::Switching { from: "dot".into(), to: "copilot".into(), undo_secs_left: 2 });
        let fx = m.handle_at(Event::Undo, t + Duration::from_secs(12));
        assert!(fx.is_empty(), "undo never touches drivers");
        assert!(matches!(m.state.phase, Phase::InCall { ref seat, .. } if seat == "dot"));
    }

    #[test]
    fn tapping_same_seat_is_a_noop_and_remote_hangup_resets() {
        let t = Instant::now();
        let mut m = Machine::new();
        connected(&mut m, "dot", t);
        assert!(m.handle_at(Event::Call("dot".into()), t).is_empty());
        m.handle_at(Event::ProbeInCall { seat: "dot".into(), in_call: false }, t + Duration::from_secs(30));
        assert_eq!(m.state.phase, Phase::Idle);
    }

    #[test]
    fn call_timeout_asks_for_a_human() {
        let t = Instant::now();
        let mut m = Machine::new();
        m.handle_at(Event::Call("dot".into()), t);
        m.handle_at(Event::Tick, t + CALL_TIMEOUT + Duration::from_secs(1));
        assert!(matches!(m.state.phase, Phase::NeedsHuman { ref seat, .. } if seat == "dot"));
        assert_eq!(m.handle_at(Event::Hangup, t), vec![Effect::DriverHangup("dot".into())]);
        assert_eq!(m.state.phase, Phase::Idle);
    }

    #[test]
    fn calling_away_from_needs_human_hangs_that_seat_up_first() {
        let t = Instant::now();
        let mut m = Machine::new();
        m.handle_at(Event::Call("grok-a".into()), t);
        m.handle_at(Event::Tick, t + CALL_TIMEOUT + Duration::from_secs(1));
        let fx = m.handle_at(Event::Call("grok-b".into()), t + Duration::from_secs(20));
        assert_eq!(fx, vec![Effect::DriverHangup("grok-a".into()), Effect::DriverCall("grok-b".into())], "hang up, then dial, in that order");
        assert!(matches!(m.state.phase, Phase::Calling { ref seat } if seat == "grok-b"));
        // retrying the same seat just dials again
        let later = t + Duration::from_secs(21) + CALL_TIMEOUT;
        m.handle_at(Event::Tick, later);
        assert_eq!(m.handle_at(Event::Call("grok-b".into()), later + Duration::from_secs(1)), vec![Effect::DriverCall("grok-b".into())]);
    }

    #[test]
    fn a_late_confirmation_turns_needs_human_into_the_call() {
        let t = Instant::now();
        let mut m = Machine::new();
        m.handle_at(Event::Call("grok".into()), t);
        m.handle_at(Event::Tick, t + CALL_TIMEOUT + Duration::from_secs(1));
        m.handle_at(Event::ProbeInCall { seat: "other".into(), in_call: true }, t + Duration::from_secs(17));
        assert!(matches!(m.state.phase, Phase::NeedsHuman { .. }), "another seat's report changes nothing");
        m.handle_at(Event::ProbeInCall { seat: "grok".into(), in_call: true }, t + Duration::from_secs(18));
        assert!(matches!(m.state.phase, Phase::InCall { ref seat, .. } if seat == "grok"));
    }

    #[test]
    fn expanded_toggle_does_not_interrupt_call() {
        let t = Instant::now();
        let mut m = Machine::new();
        connected(&mut m, "dot", t);
        m.handle_at(Event::ToggleExpanded, t);
        assert!(!m.state.expanded);
        assert!(matches!(m.state.phase, Phase::InCall { .. }));
    }
}
