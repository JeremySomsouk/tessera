use super::{CLI_TIMEOUT, GH_FAILED, GH_RATE_LIMIT, GhFailure, MAX_OUTPUT, capture_output};
use std::{
    collections::{HashMap, HashSet},
    ffi::OsString,
    os::unix::ffi::OsStringExt,
    process::{Command, Stdio},
    sync::{
        Arc, Condvar, Mutex, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::{Duration, Instant},
};

const TTL: Duration = Duration::from_secs(45);
const CACHE_BYTES: usize = 16 * 1024 * 1024;

type Environment = Arc<Vec<(OsString, OsString)>>;
type EnvironmentState = Arc<(Mutex<Option<Environment>>, Condvar)>;

pub(super) fn environment(cancelled: &AtomicBool) -> Result<Environment, &'static str> {
    static STATE: OnceLock<EnvironmentState> = OnceLock::new();
    let state = STATE.get_or_init(|| {
        let state: EnvironmentState = Arc::new((Mutex::new(None), Condvar::new()));
        let worker = state.clone();
        std::thread::spawn(move || {
            let started = Instant::now();
            let values = resolve_environment().unwrap_or_else(|| {
                log::debug!("GitHub login environment unavailable; using inherited environment");
                std::env::vars_os().collect()
            });
            log::debug!(
                "GitHub login environment elapsed_ms={}",
                started.elapsed().as_millis()
            );
            *worker.0.lock().unwrap() = Some(Arc::new(values));
            worker.1.notify_all();
        });
        state
    });
    let mut values = state.0.lock().unwrap();
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err("Lookup cancelled");
        }
        if let Some(values) = &*values {
            return Ok(values.clone());
        }
        values = state
            .1
            .wait_timeout(values, Duration::from_millis(20))
            .unwrap()
            .0;
    }
}

fn resolve_environment() -> Option<Vec<(OsString, OsString)>> {
    let marker = format!("TESSERA_ENV_{}", uuid::Uuid::new_v4().simple());
    let script = format!("printf '\\0{marker}\\0'; exec /usr/bin/env -0");
    let mut child = Command::new(crate::terminal::agent_shell())
        .args(["-lic", &script])
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .ok()?;
    let stdout = capture_output(child.stdout.take()?, MAX_OUTPUT);
    let deadline = Instant::now() + CLI_TIMEOUT;
    loop {
        match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    return None;
                }
                let bytes = stdout.recv_timeout(Duration::from_millis(250)).ok()?.ok()?;
                if bytes.len() > MAX_OUTPUT {
                    return None;
                }
                let marker = format!("\0{marker}\0");
                let start = bytes
                    .windows(marker.len())
                    .position(|part| part == marker.as_bytes())?
                    + marker.len();
                return parse_environment(&bytes[start..]);
            }
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            _ => {
                let _ = child.kill();
                let _ = child.wait();
                return None;
            }
        }
    }
}

fn parse_environment(bytes: &[u8]) -> Option<Vec<(OsString, OsString)>> {
    let mut values = Vec::new();
    for entry in bytes
        .split(|byte| *byte == 0)
        .filter(|entry| !entry.is_empty())
    {
        let separator = entry.iter().position(|byte| *byte == b'=')?;
        if separator == 0 {
            return None;
        }
        values.push((
            OsString::from_vec(entry[..separator].to_vec()),
            OsString::from_vec(entry[separator + 1..].to_vec()),
        ));
    }
    (!values.is_empty()).then_some(values)
}

struct Gate {
    active: usize,
    backoff: Option<Instant>,
}
impl Gate {
    fn try_acquire(&mut self, now: Instant) -> Result<bool, &'static str> {
        if self.backoff.is_some_and(|until| until > now) {
            return Err(GH_RATE_LIMIT);
        }
        if self.active >= 4 {
            return Ok(false);
        }
        self.active += 1;
        Ok(true)
    }
}
static GATE: Mutex<Gate> = Mutex::new(Gate {
    active: 0,
    backoff: None,
});
pub(super) struct Permit;
impl Drop for Permit {
    fn drop(&mut self) {
        GATE.lock().unwrap().active -= 1;
    }
}

pub(super) fn acquire(cancelled: &AtomicBool) -> Result<Permit, &'static str> {
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err("Lookup cancelled");
        }
        {
            let mut gate = GATE.lock().map_err(|_| GH_FAILED)?;
            if gate.try_acquire(Instant::now())? {
                return Ok(Permit);
            }
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

pub(super) fn backoff() {
    GATE.lock().unwrap().backoff = Some(Instant::now() + Duration::from_secs(60));
}

#[derive(Clone, Hash, PartialEq, Eq)]
struct Key {
    path: String,
    args: Vec<String>,
}
struct Cached {
    at: Instant,
    text: String,
}
#[derive(Default)]
struct Cache {
    entries: HashMap<Key, Cached>,
    failures: HashMap<Key, (Instant, GhFailure)>,
}
impl Cache {
    fn prune(&mut self) {
        self.entries.retain(|_, value| value.at.elapsed() < TTL);
        self.failures.retain(|_, (at, _)| at.elapsed() < TTL);
    }
    fn insert(&mut self, key: Key, text: String) {
        self.prune();
        if text.len() > CACHE_BYTES {
            return;
        }
        while self.entries.len() >= 64
            || self
                .entries
                .values()
                .map(|value| value.text.len())
                .sum::<usize>()
                + text.len()
                > CACHE_BYTES
        {
            let Some(oldest) =
                self.entries
                    .iter()
                    .min_by_key(|(_, value)| value.at)
                    .map(|(key, _)| Key {
                        path: key.path.clone(),
                        args: key.args.clone(),
                    })
            else {
                break;
            };
            self.entries.remove(&oldest);
        }
        self.entries.insert(
            key,
            Cached {
                at: Instant::now(),
                text,
            },
        );
    }
}
fn cache() -> &'static Mutex<Cache> {
    static CACHE: OnceLock<Mutex<Cache>> = OnceLock::new();
    CACHE.get_or_init(Default::default)
}
fn key(path: &str, args: &[String]) -> Option<Key> {
    matches!(args.first().map(String::as_str), Some("run" | "pr"))
        .then_some(())
        .filter(|_| args.get(1).is_some_and(|arg| arg == "list"))
        .map(|_| Key {
            path: path.into(),
            args: args.to_vec(),
        })
}
type Flights = (Mutex<HashSet<Key>>, Condvar);
fn flights() -> &'static Flights {
    static FLIGHTS: OnceLock<Flights> = OnceLock::new();
    FLIGHTS.get_or_init(Default::default)
}

pub(super) struct Flight(Key);
impl Drop for Flight {
    fn drop(&mut self) {
        flights().0.lock().unwrap().remove(&self.0);
        flights().1.notify_all();
    }
}

pub(super) fn flight(
    path: &str,
    args: &[String],
    cancelled: &AtomicBool,
) -> Result<Option<Flight>, &'static str> {
    let Some(key) = key(path, args) else {
        return Ok(None);
    };
    let mut active = flights().0.lock().unwrap();
    loop {
        if cancelled.load(Ordering::Acquire) {
            return Err("Lookup cancelled");
        }
        if active.insert(key.clone()) {
            return Ok(Some(Flight(key)));
        }
        active = flights()
            .1
            .wait_timeout(active, Duration::from_millis(20))
            .unwrap()
            .0;
    }
}

pub(super) fn cached(
    path: &str,
    args: &[String],
    since: Option<Instant>,
) -> Option<Result<String, GhFailure>> {
    let key = key(path, args)?;
    let mut cache = cache().lock().unwrap();
    cache.prune();
    if let Some(value) = cache.entries.get(&key)
        && since.is_none_or(|since| value.at >= since)
    {
        return Some(Ok(value.text.clone()));
    }
    if let Some(since) = since
        && let Some((at, failure)) = cache.failures.get(&key)
        && *at >= since
    {
        return Some(Err(failure.clone()));
    }
    None
}

pub(super) fn remember(
    path: &str,
    args: &[String],
    result: &Result<String, GhFailure>,
    since: Option<Instant>,
) {
    let Some(key) = key(path, args) else { return };
    let mut cache = cache().lock().unwrap();
    match result {
        Ok(text) => {
            cache.failures.remove(&key);
            cache.insert(key, text.clone());
        }
        Err(error) if since.is_some() => {
            cache.prune();
            if cache.failures.len() >= 64 {
                let oldest = cache
                    .failures
                    .iter()
                    .min_by_key(|(_, (at, _))| *at)
                    .map(|(key, _)| key.clone());
                if let Some(oldest) = oldest {
                    cache.failures.remove(&oldest);
                }
            }
            cache.failures.insert(key, (Instant::now(), error.clone()));
        }
        Err(_) => {}
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::ffi::OsStrExt;
    #[test]
    fn environment_preserves_newlines_equals_and_non_utf8_values() {
        let values = parse_environment(b"PATH=/a:/b\0NAME=line\nwith=equals\0RAW=\xff\0").unwrap();
        assert_eq!(values[0].1, "/a:/b");
        assert_eq!(values[1].1, "line\nwith=equals");
        assert_eq!(values[2].1.as_bytes(), b"\xff");
        assert!(parse_environment(b"bad entry\0").is_none());
    }
    #[test]
    fn gate_bounds_active_calls_and_refuses_requests_until_cooldown_expires() {
        let now = Instant::now();
        let mut gate = Gate {
            active: 0,
            backoff: None,
        };
        for _ in 0..4 {
            assert_eq!(gate.try_acquire(now), Ok(true));
        }
        assert_eq!(gate.try_acquire(now), Ok(false));
        gate.active -= 1;
        gate.backoff = Some(now + Duration::from_secs(60));
        assert_eq!(
            gate.try_acquire(now + Duration::from_secs(59)),
            Err(GH_RATE_LIMIT)
        );
        assert_eq!(gate.try_acquire(now + Duration::from_secs(60)), Ok(true));
    }

    #[test]
    fn cancellation_stops_a_waiter_on_an_identical_inflight_request() {
        let path = uuid::Uuid::new_v4().to_string();
        let args = vec!["run".into(), "list".into()];
        let hold = flight(&path, &args, &AtomicBool::new(false)).unwrap();
        let cancelled = AtomicBool::new(false);
        std::thread::scope(|scope| {
            let waiting = scope.spawn(|| flight(&path, &args, &cancelled));
            cancelled.store(true, Ordering::Release);
            assert!(waiting.join().unwrap().is_err());
        });
        drop(hold);
    }

    #[test]
    fn cache_expiry_and_argument_keys_prevent_wrong_results() {
        let mut cache = Cache::default();
        let k = |repo: &str| Key {
            path: "gh".into(),
            args: vec![repo.into()],
        };
        cache.insert(k("one"), "first".into());
        cache.insert(k("two"), "second".into());
        assert_eq!(cache.entries[&k("one")].text, "first");
        cache.entries.get_mut(&k("one")).unwrap().at = Instant::now() - TTL;
        cache.prune();
        assert!(!cache.entries.contains_key(&k("one")));
        assert_eq!(cache.entries[&k("two")].text, "second");
    }
}
