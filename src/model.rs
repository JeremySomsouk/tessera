use serde::{Deserialize, Serialize};
use std::collections::VecDeque;
use uuid::Uuid;

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum SessionState {
    #[default]
    Unknown,
    Running,
    Permission,
    Input,
    Idle,
    Ended,
    Disconnected,
}
impl SessionState {
    pub fn attention(self) -> bool {
        matches!(self, Self::Permission | Self::Input)
    }
    pub fn label(self) -> &'static str {
        match self {
            Self::Unknown => "Untracked",
            Self::Running => "Running",
            Self::Permission => "Permission needed",
            Self::Input => "Input needed",
            Self::Idle => "Response finished",
            Self::Ended => "Ended",
            Self::Disconnected => "Disconnected",
        }
    }
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct HookEvent {
    pub id: Uuid,
    pub pane: Uuid,
    pub sequence: u128,
    pub session_id: String,
    pub hook_event_name: String,
    pub detail: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Activity {
    pub sequence: u128,
    pub at: u64,
    pub kind: String,
    pub detail: String,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Session {
    pub pane: Uuid,
    pub session_id: String,
    pub state: SessionState,
    pub last_sequence: u128,
    pub history: VecDeque<Activity>,
    pub seen: u64,
    #[serde(default)]
    pub seen_sequence: u128,
    pub updated: u64,
    #[serde(skip)]
    recent: VecDeque<Uuid>,
}
impl Session {
    pub fn new(pane: Uuid, session_id: String) -> Self {
        Self {
            pane,
            session_id,
            state: SessionState::Unknown,
            last_sequence: 0,
            history: VecDeque::new(),
            seen: 0,
            seen_sequence: 0,
            updated: 0,
            recent: VecDeque::new(),
        }
    }
    pub fn key(&self) -> String {
        format!("{}/{}", self.pane, self.session_id)
    }
    pub fn apply(&mut self, e: HookEvent, now: u64) -> bool {
        if e.pane != self.pane
            || e.session_id != self.session_id
            || self.recent.contains(&e.id)
            || e.sequence <= self.last_sequence
        {
            return false;
        }
        self.last_sequence = e.sequence;
        self.recent.push_back(e.id);
        if self.recent.len() > 256 {
            self.recent.pop_front();
        }
        self.state = match e.hook_event_name.as_str() {
            "SessionStart" | "UserPromptSubmit" | "PreToolUse" | "PostToolUse"
            | "PostToolUseFailure" => SessionState::Running,
            "PermissionRequest" => SessionState::Permission,
            "Notification" if e.detail.starts_with("permission_prompt") => SessionState::Permission,
            "Notification" if e.detail.starts_with("idle_prompt") => SessionState::Input,
            "Stop" => SessionState::Idle,
            "SessionEnd" => SessionState::Ended,
            _ => self.state,
        };
        self.updated = now;
        self.history.push_back(Activity {
            sequence: e.sequence,
            at: now,
            kind: e.hook_event_name,
            detail: e.detail,
        });
        if self.history.len() > 128 {
            self.history.pop_front();
        }
        true
    }
}
#[derive(Clone, Copy, Debug, Default, Serialize, Deserialize, PartialEq)]
pub enum TaskState {
    #[default]
    Implementing,
    ReviewRequested,
    Accepted,
}
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Task {
    pub id: Uuid,
    pub title: String,
    pub directory: String,
    pub state: TaskState,
}
#[cfg(test)]
mod tests {
    use super::*;
    fn event(pane: Uuid, seq: u128, name: &str) -> HookEvent {
        HookEvent {
            id: Uuid::new_v4(),
            pane,
            sequence: seq,
            session_id: "a".into(),
            hook_event_name: name.into(),
            detail: String::new(),
        }
    }
    #[test]
    fn stop_is_idle_and_not_acceptance() {
        let p = Uuid::new_v4();
        let mut s = Session::new(p, "a".into());
        s.apply(event(p, 1, "Stop"), 1);
        assert_eq!(s.state, SessionState::Idle);
    }
    #[test]
    fn duplicates_and_delayed_events_do_not_rewind() {
        let p = Uuid::new_v4();
        let mut s = Session::new(p, "a".into());
        let e = event(p, 2, "PermissionRequest");
        assert!(s.apply(e.clone(), 1));
        assert!(!s.apply(e, 2));
        assert!(!s.apply(event(p, 1, "Stop"), 3));
        assert_eq!(s.state, SessionState::Permission);
    }
    #[test]
    fn bounded_history_and_identity() {
        let p = Uuid::new_v4();
        let mut s = Session::new(p, "a".into());
        assert!(!s.apply(event(Uuid::new_v4(), 1, "Stop"), 1));
        for n in 1..300 {
            s.apply(event(p, n, "PreToolUse"), n as u64);
        }
        assert_eq!(s.history.len(), 128);
    }
}
