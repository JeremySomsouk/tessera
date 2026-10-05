use crate::model::{Agent, Session, SessionState};
use std::{
    collections::HashMap,
    path::Path,
    process::Command,
    sync::mpsc::{Receiver, SyncSender, sync_channel},
    time::{Duration, Instant},
};
use uuid::Uuid;

pub struct Discovery {
    requests: SyncSender<Vec<(Uuid, u32)>>,
    results: Receiver<Vec<(Uuid, u32, Agent)>>,
    next_scan: Instant,
}

impl Discovery {
    pub fn start(ctx: eframe::egui::Context) -> Self {
        let (requests, receiver) = sync_channel::<Vec<(Uuid, u32)>>(1);
        let (sender, results) = sync_channel(1);
        std::thread::spawn(move || {
            while let Ok(roots) = receiver.recv() {
                let Ok(output) = Command::new("/bin/ps")
                    .args(["-x", "-o", "pid=,ppid=,comm="])
                    .output()
                else {
                    continue;
                };
                if !output.status.success() || output.stdout.len() > 4 * 1024 * 1024 {
                    continue;
                }
                let Ok(text) = std::str::from_utf8(&output.stdout) else {
                    continue;
                };
                if sender.send(parse_processes(text, &roots)).is_err() {
                    break;
                }
                ctx.request_repaint();
            }
        });
        Self {
            requests,
            results,
            next_scan: Instant::now(),
        }
    }

    pub fn scan(&mut self, roots: Vec<(Uuid, u32)>) -> Option<Vec<(Uuid, u32, Agent)>> {
        let result = self.results.try_recv().ok();
        if Instant::now() >= self.next_scan {
            let _ = self.requests.try_send(roots);
            self.next_scan = Instant::now() + Duration::from_secs(2);
        }
        result
    }
}

fn parse_processes(text: &str, roots: &[(Uuid, u32)]) -> Vec<(Uuid, u32, Agent)> {
    let mut processes = HashMap::new();
    for line in text.lines() {
        let Some((pid, remainder)) = line.trim().split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(pid) = pid.parse::<u32>() else {
            continue;
        };
        let Some((parent, command)) = remainder.trim_start().split_once(char::is_whitespace) else {
            continue;
        };
        let Ok(parent) = parent.parse::<u32>() else {
            continue;
        };
        processes.insert(pid, (parent, command.trim()));
    }
    let roots: HashMap<_, _> = roots.iter().map(|&(pane, pid)| (pid, pane)).collect();
    let mut found = Vec::new();
    for (&pid, &(_, command)) in &processes {
        let agent = match Path::new(command)
            .file_name()
            .and_then(|name| name.to_str())
        {
            Some("codex") => Agent::Codex,
            Some("claude") => Agent::Claude,
            _ => continue,
        };
        let mut ancestor = pid;
        for _ in 0..64 {
            if let Some(&pane) = roots.get(&ancestor) {
                found.push((pane, pid, agent));
                break;
            }
            let Some(&(parent, _)) = processes.get(&ancestor) else {
                break;
            };
            if parent == ancestor {
                break;
            }
            ancestor = parent;
        }
    }
    found.sort_by_key(|&(_, pid, _)| pid);
    found
}

pub fn reconcile(sessions: &mut Vec<Session>, found: &[(Uuid, u32, Agent)], now: u64) -> bool {
    let mut changed = false;
    for session in sessions
        .iter_mut()
        .filter(|session| session.observed_process.is_some())
    {
        if session.state == SessionState::Unknown
            && !found.iter().any(|&(pane, pid, agent)| {
                pane == session.pane
                    && Some(pid) == session.observed_process
                    && agent == session.agent
            })
        {
            session.state = SessionState::Ended;
            session.updated = now;
            changed = true;
        }
    }
    for &(pane, pid, agent) in found {
        if sessions.iter().any(|session| {
            session.pane == pane
                && session.agent == agent
                && !matches!(
                    session.state,
                    SessionState::Ended | SessionState::Disconnected
                )
        }) {
            continue;
        }
        if sessions.len() >= 256 {
            if let Some(index) = sessions
                .iter()
                .position(|s| matches!(s.state, SessionState::Ended | SessionState::Disconnected))
            {
                sessions.remove(index);
            } else {
                continue;
            }
        }
        let mut session = Session::new(pane, format!("process-{pid}-{now}"));
        session.agent = agent;
        session.observed_process = Some(pid);
        session.updated = now;
        sessions.push(session);
        changed = true;
    }
    changed
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovers_only_agents_descended_from_live_panes() {
        let pane = Uuid::new_v4();
        let text = " 10  1 /bin/zsh\n 11   10 /Applications/My App/codex\n 12 11 /bin/codex-code-mode-host\n 20 1 codex\n 21 20 claude\n 30 30 codex\n malformed\n";
        assert_eq!(
            parse_processes(text, &[(pane, 10)]),
            vec![(pane, 11, Agent::Codex)]
        );
        assert!(parse_processes(text, &[]).is_empty());
        assert!(parse_processes("31 32 codex\n32 31 zsh", &[(pane, 10)]).is_empty());
    }

    #[test]
    fn discovery_does_not_invent_activity_or_duplicate_hook_sessions() {
        let pane = Uuid::new_v4();
        let found = [(pane, 11, Agent::Codex)];
        let mut sessions = Vec::new();
        assert!(reconcile(&mut sessions, &found, 1));
        assert_eq!(sessions[0].state, SessionState::Unknown);
        assert!(sessions[0].history.is_empty());
        assert!(!reconcile(&mut sessions, &found, 2));
        assert!(reconcile(&mut sessions, &[], 3));
        assert_eq!(sessions[0].state, SessionState::Ended);
        let mut tracked = Session::new(pane, "real-session".into());
        tracked.agent = Agent::Codex;
        tracked.state = SessionState::Input;
        sessions.push(tracked);
        assert!(!reconcile(&mut sessions, &found, 4));
        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[1].state, SessionState::Input);
    }
}
