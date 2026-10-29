use crate::remote_runs::{Config, RepoResult, Run};
use serde::{Deserialize, Serialize};

pub const MAX_TICKETS: usize = 32;
pub const REFRESH_SECONDS: u64 = 60;

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(default)]
pub struct Preferences {
    pub keys: Vec<String>,
    pub live: bool,
}

impl Preferences {
    pub fn add(&mut self, key: &str, config: &Config) -> Result<bool, String> {
        let key = key.trim();
        config.tasks(key)?;
        if self.keys.iter().any(|existing| existing == key) {
            return Ok(false);
        }
        if self.keys.len() >= MAX_TICKETS {
            return Err(format!(
                "Follow at most {MAX_TICKETS} tickets. Remove one to add another."
            ));
        }
        self.keys.push(key.to_owned());
        Ok(true)
    }

    pub fn normalize(&mut self) {
        let mut seen = std::collections::HashSet::new();
        self.keys = self
            .keys
            .iter()
            .map(|key| key.trim().to_owned())
            .filter(|key| {
                !key.is_empty()
                    && key.len() <= 256
                    && !key.chars().any(char::is_control)
                    && seen.insert(key.clone())
            })
            .take(MAX_TICKETS)
            .collect();
    }
}

#[derive(Clone, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub struct GroupId {
    pub source: String,
    pub ticket: String,
    pub repo: String,
}

#[derive(Default)]
pub struct Refresh {
    pub active: bool,
    pub next_at: u64,
    failures: u32,
    generation: u64,
}

impl Refresh {
    pub fn due(&self, now: u64, visible: bool, live: bool) -> bool {
        visible && live && !self.active && now >= self.next_at
    }

    pub fn start(&mut self) -> Option<u64> {
        if self.active {
            return None;
        }
        self.active = true;
        self.generation = self.generation.wrapping_add(1);
        Some(self.generation)
    }

    pub fn finish(&mut self, generation: u64, now: u64, failed: bool) -> bool {
        if !self.active || generation != self.generation {
            return false;
        }
        self.active = false;
        self.failures = if failed {
            (self.failures + 1).min(4)
        } else {
            0
        };
        self.next_at = now.saturating_add((REFRESH_SECONDS << self.failures).min(600));
        true
    }

    pub fn cancel(&mut self) {
        self.active = false;
        self.generation = self.generation.wrapping_add(1);
    }
}

#[derive(Clone)]
pub struct CachedRepo {
    pub result: RepoResult,
    pub last_success: Option<u64>,
    pub stale: bool,
    pub change: String,
    pub shared_sources: Vec<String>,
}

impl CachedRepo {
    pub fn new(result: RepoResult, at: u64) -> Self {
        let stale = !result.errors.is_empty();
        Self {
            result,
            last_success: (!stale).then_some(at),
            stale,
            change: "First observation".into(),
            shared_sources: Vec::new(),
        }
    }

    pub fn update(&mut self, mut result: RepoResult, at: u64) {
        self.stale = !result.errors.is_empty();
        if self.stale {
            // A partial response must not erase runs or PRs from the last successful lookup.
            for run in &self.result.runs {
                if !result.runs.iter().any(|new| new.id == run.id) {
                    result.runs.push(run.clone());
                }
            }
            for pr in &self.result.prs {
                if !result.prs.iter().any(|new| new.number == pr.number) {
                    result.prs.push(pr.clone());
                }
            }
        } else {
            self.last_success = Some(at);
        }
        let before = latest(&self.result.runs);
        let after = latest(&result.runs);
        self.change = match (before, after) {
            (Some(old), Some(new)) if old.id != new.id => "New run".into(),
            (Some(old), Some(new))
                if old.status != new.status || old.conclusion != new.conclusion =>
            {
                format!("Changed: {}", state(new))
            }
            (None, Some(_)) => "New run".into(),
            (Some(_), None) => "No runs in current lookup window".into(),
            _ => "No run state change".into(),
        };
        result.runs.sort_by(|a, b| {
            b.created_at
                .cmp(&a.created_at)
                .then_with(|| b.id.cmp(&a.id))
        });
        result.runs.dedup_by_key(|run| run.id);
        self.result = result;
    }
}

pub fn latest(runs: &[Run]) -> Option<&Run> {
    runs.iter().max_by(|a, b| {
        a.created_at
            .cmp(&b.created_at)
            .then_with(|| a.id.cmp(&b.id))
    })
}

pub fn priority(result: &RepoResult) -> u8 {
    if !result.errors.is_empty() {
        return 0;
    }
    match latest(&result.runs) {
        Some(run)
            if matches!(
                run.conclusion.as_deref(),
                Some("failure" | "timed_out" | "action_required")
            ) =>
        {
            1
        }
        Some(run) if run.status == "in_progress" => 2,
        Some(run) if run.status != "completed" => 3,
        _ => 4,
    }
}

pub fn state(run: &Run) -> String {
    match run.conclusion.as_deref().filter(|value| !value.is_empty()) {
        Some(conclusion) => format!("{} / {conclusion}", run.status),
        None => run.status.clone(),
    }
}

pub fn elapsed(run: &Run, now: u64) -> String {
    let start = crate::remote_runs::created_at_seconds(&run.started_at);
    let end = if run.status == "completed" {
        crate::remote_runs::created_at_seconds(&run.updated_at)
    } else {
        Some(now)
    };
    match (start, end) {
        (Some(start), Some(end)) if end >= start => {
            let seconds = end - start;
            format!(
                "{}m {}s elapsed{}",
                seconds / 60,
                seconds % 60,
                if run.status == "completed" {
                    " (to last update)"
                } else {
                    ""
                }
            )
        }
        _ => "Elapsed unavailable".into(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    #[test]
    fn tracking_round_trip_is_bounded_and_contains_preferences_only() {
        let config = Config::parse(&json!({"sources":[{"name":"bot","repos":["acme/api"],"ticket_pattern":"ACME-[0-9]+"}]})).unwrap();
        let mut preferences = Preferences::default();
        assert!(preferences.add(" ACME-42 ", &config).unwrap());
        assert!(!preferences.add("ACME-42", &config).unwrap());
        assert!(preferences.add("other", &config).is_err());
        let json = serde_json::to_value(&preferences).unwrap();
        assert_eq!(json, json!({"keys":["ACME-42"],"live":false}));
        let restored: Preferences = serde_json::from_value(json).unwrap();
        assert_eq!(restored.keys, preferences.keys);
    }
    #[test]
    fn scheduler_uses_controlled_time_visibility_backoff_and_generation() {
        let mut refresh = Refresh::default();
        assert!(!refresh.due(0, false, true));
        assert!(!refresh.due(0, true, false));
        let first = refresh.start().unwrap();
        assert!(refresh.start().is_none());
        assert!(!refresh.due(99, true, true));
        refresh.cancel();
        assert!(!refresh.finish(first, 0, false));
        let second = refresh.start().unwrap();
        assert!(refresh.finish(second, 10, true));
        assert!(!refresh.due(129, true, true));
        assert!(refresh.due(130, true, true));
        for _ in 0..8 {
            let generation = refresh.start().unwrap();
            refresh.finish(generation, 10, true);
        }
        assert_eq!(refresh.next_at, 610);
        let generation = refresh.start().unwrap();
        refresh.finish(generation, 10, false);
        assert_eq!(refresh.next_at, 70);
    }
    #[test]
    fn failed_partial_refresh_preserves_data_and_success_timestamp() {
        let run = Run {
            id: 1,
            status: "in_progress".into(),
            ..Default::default()
        };
        let mut cached = CachedRepo::new(
            RepoResult {
                runs: vec![run],
                ..Default::default()
            },
            100,
        );
        cached.update(
            RepoResult {
                errors: vec!["Network unavailable".into()],
                ..Default::default()
            },
            200,
        );
        assert_eq!(cached.result.runs.len(), 1);
        assert_eq!(cached.last_success, Some(100));
        assert!(cached.stale);
        cached.update(RepoResult::default(), 300);
        assert_eq!(cached.last_success, Some(300));
        assert!(!cached.stale);
        assert!(cached.result.runs.is_empty());
    }
    #[test]
    fn elapsed_uses_metadata_and_never_turns_completion_into_acceptance() {
        let mut run = Run {
            status: "completed".into(),
            conclusion: Some("success".into()),
            started_at: "2026-10-29T12:00:00Z".into(),
            updated_at: "2026-10-29T12:01:30Z".into(),
            ..Default::default()
        };
        assert_eq!(elapsed(&run, 0), "1m 30s elapsed (to last update)");
        assert_eq!(state(&run), "completed / success");
        run.started_at.clear();
        assert_eq!(elapsed(&run, 0), "Elapsed unavailable");
    }
    #[test]
    fn bounded_lookup_warning_is_successful_and_does_not_mark_metadata_stale() {
        let mut cached = CachedRepo::new(
            RepoResult {
                warnings: vec!["Lookup window reached".into()],
                ..Default::default()
            },
            100,
        );
        assert!(!cached.stale);
        assert_eq!(cached.last_success, Some(100));
        cached.update(
            RepoResult {
                warnings: vec!["Lookup window reached".into()],
                ..Default::default()
            },
            200,
        );
        assert!(!cached.stale);
        assert_eq!(cached.last_success, Some(200));
    }
}
