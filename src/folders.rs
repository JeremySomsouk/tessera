use eframe::egui;
use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        Arc, Condvar, Mutex,
        atomic::{AtomicU64, Ordering},
    },
    time::{Duration, Instant},
};

const DEBOUNCE: Duration = Duration::from_millis(180);
const MAX_RESULTS: usize = 80;
const MAX_ROOTS: usize = 16;

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct FolderResult {
    pub path: PathBuf,
    pub name: String,
}

impl FolderResult {
    fn new(path: PathBuf) -> Self {
        let name = path
            .file_name()
            .unwrap_or(path.as_os_str())
            .to_string_lossy()
            .into_owned();
        Self { path, name }
    }
}

#[derive(Clone)]
struct Request {
    generation: u64,
    query: String,
    roots: Vec<String>,
    ready_at: Instant,
}

#[derive(Default)]
struct Response {
    generation: u64,
    results: Vec<FolderResult>,
    notice: Option<String>,
    error: Option<String>,
}

#[derive(Default)]
struct WorkerState {
    pending: Option<Request>,
    response: Option<Response>,
    stop: bool,
}

#[derive(Default)]
struct Shared {
    state: Mutex<WorkerState>,
    wake: Condvar,
    generation: AtomicU64,
}

pub struct FolderSearch {
    shared: Arc<Shared>,
    input: Option<(String, Vec<String>)>,
    available: bool,
    pub results: Vec<FolderResult>,
    pub loading: bool,
    pub notice: Option<String>,
    pub error: Option<String>,
}

impl FolderSearch {
    pub fn new(ctx: egui::Context) -> Self {
        let shared = Arc::new(Shared::default());
        let worker_shared = shared.clone();
        let worker = std::thread::Builder::new()
            .name("folder-search".into())
            .spawn(move || worker(worker_shared, ctx));
        Self {
            shared,
            input: None,
            available: worker.is_ok(),
            results: Vec::new(),
            loading: false,
            notice: None,
            error: worker
                .err()
                .map(|error| format!("Cannot start folder search: {error}")),
        }
    }

    pub fn set_query(&mut self, query: &str, roots: &[String]) {
        if self
            .input
            .as_ref()
            .is_some_and(|(previous, previous_roots)| previous == query && previous_roots == roots)
        {
            return;
        }
        self.cancel();
        self.input = Some((query.to_owned(), roots.to_vec()));
        if !self.available {
            self.error = Some("Folder search is unavailable. Restart Tessera to try again.".into());
            return;
        }
        if query.trim().is_empty() {
            self.notice = Some("Type a folder name or paste an existing directory path.".into());
            return;
        }
        if query.len() > 4096 {
            self.error = Some("The folder name or path is too long (maximum 4096 bytes).".into());
            return;
        }
        self.loading = true;
        let request = Request {
            generation: self.shared.generation.load(Ordering::Acquire),
            query: query.to_owned(),
            roots: if roots.is_empty() {
                default_roots()
            } else {
                roots.to_vec()
            },
            ready_at: Instant::now() + DEBOUNCE,
        };
        self.shared.state.lock().unwrap().pending = Some(request);
        self.shared.wake.notify_one();
    }

    pub fn poll(&mut self) -> bool {
        let response = self.shared.state.lock().unwrap().response.take();
        response.is_some_and(|response| self.accept(response))
    }

    fn accept(&mut self, response: Response) -> bool {
        if response.generation != self.shared.generation.load(Ordering::Acquire) {
            return false;
        }
        self.results = response.results;
        self.notice = response.notice;
        self.error = response.error;
        self.loading = false;
        true
    }

    pub fn cancel(&mut self) {
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        let mut state = self.shared.state.lock().unwrap();
        state.pending = None;
        state.response = None;
        self.input = None;
        self.results.clear();
        self.loading = false;
        self.notice = None;
        self.error = None;
        self.shared.wake.notify_one();
    }
}

impl Drop for FolderSearch {
    fn drop(&mut self) {
        self.shared.generation.fetch_add(1, Ordering::AcqRel);
        self.shared.state.lock().unwrap().stop = true;
        self.shared.wake.notify_one();
    }
}

pub fn default_roots() -> Vec<String> {
    vec![std::env::var("HOME").unwrap_or_else(|_| ".".into())]
}

fn worker(shared: Arc<Shared>, ctx: egui::Context) {
    loop {
        let request = {
            let mut state = shared.state.lock().unwrap();
            loop {
                if state.stop {
                    return;
                }
                match &state.pending {
                    Some(request) if Instant::now() >= request.ready_at => {
                        break state.pending.take().unwrap();
                    }
                    Some(request) => {
                        let wait = request.ready_at.saturating_duration_since(Instant::now());
                        state = shared.wake.wait_timeout(state, wait).unwrap().0;
                    }
                    None => state = shared.wake.wait(state).unwrap(),
                }
            }
        };
        let cancelled = || shared.generation.load(Ordering::Acquire) != request.generation;
        let response = discover(&request, &cancelled);
        if !cancelled() {
            shared.state.lock().unwrap().response = Some(response);
            ctx.request_repaint();
        }
    }
}

fn explicit_path(query: &str) -> bool {
    matches!(query, "~" | "." | "..") || query.contains('/')
}

fn literal_path(input: &str, base: &Path, home: Option<&Path>) -> Result<PathBuf, String> {
    if input.is_empty() || input.chars().any(char::is_control) {
        return Err("Enter a folder path without control characters.".into());
    }
    if input == "~" || input.starts_with("~/") {
        let home = home.ok_or("Home directory is unavailable; enter an absolute path.")?;
        return Ok(home.join(
            input
                .strip_prefix("~/")
                .unwrap_or("")
                .trim_start_matches('/'),
        ));
    }
    if input.starts_with('~') {
        return Err("Use ~/ for your home directory, or enter an absolute path.".into());
    }
    let path = PathBuf::from(input);
    Ok(if path.is_absolute() {
        path
    } else {
        base.join(path)
    })
}

fn existing_directory(query: &str) -> Result<PathBuf, String> {
    let home = std::env::var_os("HOME").map(PathBuf::from);
    let path = literal_path(
        query,
        &crate::directories::default_directory(),
        home.as_deref(),
    )?;
    let metadata = std::fs::metadata(&path).map_err(|error| {
        format!(
            "Cannot open {}: {error}. Choose an existing, accessible folder.",
            path.display()
        )
    })?;
    if !metadata.is_dir() {
        return Err(format!("{} is not a folder.", path.display()));
    }
    std::fs::read_dir(&path).map_err(|error| {
        format!(
            "Cannot read {}: {error}. Check this folder's access permissions.",
            path.display()
        )
    })?;
    Ok(path)
}

fn discover(request: &Request, cancelled: &impl Fn() -> bool) -> Response {
    let mut response = Response {
        generation: request.generation,
        ..Default::default()
    };
    let query = request.query.trim();
    if explicit_path(query) {
        match existing_directory(&request.query) {
            Ok(path) => response.results.push(FolderResult::new(path)),
            Err(error) => response.error = Some(error),
        }
        return response;
    }
    let query = query.to_lowercase();
    let mut paths = HashSet::new();
    let mut notes = Vec::new();
    let mut roots = Vec::new();
    for root in request.roots.iter().take(MAX_ROOTS) {
        match existing_directory(root) {
            Ok(path) => roots.push(path),
            Err(error) => notes.push(error),
        }
    }
    if request.roots.len() > MAX_ROOTS {
        notes.push(format!(
            "Only the first {MAX_ROOTS} search roots were used."
        ));
    }
    for root in &roots {
        if cancelled() || paths.len() >= MAX_RESULTS {
            break;
        }
        platform::indexed(&query, root, &mut paths, &mut notes, cancelled);
    }
    if cancelled() {
        return response;
    }
    let stats = search_roots(&roots, &query, &mut paths, Limits::default(), cancelled);
    if stats.limited {
        notes.push("Search limit reached. Narrow the name, choose a more specific search root, or paste a path.".into());
    }
    if stats.inaccessible > 0 {
        notes.push(format!(
            "{} locations could not be read; check access permissions or paste an accessible path.",
            stats.inaccessible
        ));
    }
    notes.push(format!("Search is limited to {} root(s). Fallback scans up to 6 levels and 20,000 entries. Hidden and dependency/build folders are skipped; paste a path to open one.", roots.len()));
    response.results = paths.into_iter().map(FolderResult::new).collect();
    response.results.sort_by_cached_key(|folder| {
        let name = folder.name.to_lowercase();
        (
            name != query,
            !name.starts_with(&query),
            name,
            folder.path.clone(),
        )
    });
    response.results.truncate(MAX_RESULTS);
    response.notice = Some(notes.join(" "));
    response
}

#[derive(Clone, Copy)]
struct Limits {
    depth: usize,
    entries: usize,
    results: usize,
    duration: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            depth: 6,
            entries: 20_000,
            results: MAX_RESULTS,
            duration: Duration::from_millis(700),
        }
    }
}

#[derive(Default)]
struct ScanStats {
    limited: bool,
    inaccessible: usize,
}

fn excluded(path: &Path) -> bool {
    path.file_name().is_some_and(|name| {
        let name = name.to_string_lossy();
        name.starts_with('.')
            || matches!(
                name.as_ref(),
                "node_modules"
                    | "target"
                    | "build"
                    | "dist"
                    | "vendor"
                    | "Pods"
                    | "DerivedData"
                    | "__pycache__"
                    | "Library"
                    | "venv"
                    | "bower_components"
            )
    })
}

fn matches_name(path: &Path, query: &str) -> bool {
    path.file_name()
        .is_some_and(|name| name.to_string_lossy().to_lowercase().contains(query))
}

fn search_roots(
    roots: &[PathBuf],
    query: &str,
    results: &mut HashSet<PathBuf>,
    limits: Limits,
    cancelled: &impl Fn() -> bool,
) -> ScanStats {
    let deadline = Instant::now() + limits.duration;
    let mut queue: VecDeque<_> = roots.iter().cloned().map(|path| (path, 0)).collect();
    let mut visited = HashSet::new();
    let mut count = 0;
    let mut stats = ScanStats::default();
    while let Some((path, depth)) = queue.pop_front() {
        if cancelled() {
            break;
        }
        if count >= limits.entries || results.len() >= limits.results || Instant::now() >= deadline
        {
            stats.limited = true;
            break;
        }
        if !visited.insert(path.clone()) {
            continue;
        }
        if matches_name(&path, query) {
            results.insert(path.clone());
        }
        let entries = match std::fs::read_dir(&path) {
            Ok(entries) => entries,
            Err(_) => {
                stats.inaccessible += 1;
                continue;
            }
        };
        for entry in entries {
            if cancelled() {
                return stats;
            }
            if count >= limits.entries
                || results.len() >= limits.results
                || Instant::now() >= deadline
            {
                stats.limited = true;
                return stats;
            }
            count += 1;
            let entry = match entry {
                Ok(entry) => entry,
                Err(_) => {
                    stats.inaccessible += 1;
                    continue;
                }
            };
            let child = entry.path();
            if excluded(&child) {
                continue;
            }
            let kind = match entry.file_type() {
                Ok(kind) => kind,
                Err(_) => {
                    stats.inaccessible += 1;
                    continue;
                }
            };
            if kind.is_dir() || (kind.is_symlink() && child.is_dir()) {
                if matches_name(&child, query) {
                    results.insert(child.clone());
                }
                if kind.is_dir() {
                    if depth + 1 < limits.depth {
                        queue.push_back((child, depth + 1));
                    } else {
                        stats.limited = true;
                    }
                }
            }
        }
    }
    stats
}

#[cfg(not(target_os = "macos"))]
mod platform {
    use super::*;

    pub fn indexed(
        _: &str,
        _: &Path,
        _: &mut HashSet<PathBuf>,
        _: &mut Vec<String>,
        _: &impl Fn() -> bool,
    ) {
    }
}

#[cfg(target_os = "macos")]
mod platform {
    use super::*;
    use std::{
        ffi::OsString,
        io::Read,
        os::{fd::AsRawFd, unix::ffi::OsStringExt},
        process::{Command, Stdio},
    };

    fn predicate(query: &str) -> String {
        let escaped = query.replace('\\', "\\\\").replace('"', "\\\"");
        format!("kMDItemContentTypeTree == \"public.folder\" && kMDItemFSName == \"*{escaped}*\"cd")
    }

    fn search_command(query: &str, root: &Path) -> Command {
        let mut command = Command::new("/usr/bin/mdfind");
        command
            .arg("-0")
            .arg("-onlyin")
            .arg(root)
            .arg(predicate(query));
        command
    }

    pub fn indexed(
        query: &str,
        root: &Path,
        results: &mut HashSet<PathBuf>,
        notes: &mut Vec<String>,
        cancelled: &impl Fn() -> bool,
    ) {
        let mut child = match search_command(query, root)
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::null())
            .spawn()
        {
            Ok(child) => child,
            Err(_) => {
                notes.push("Spotlight is unavailable; using search roots.".into());
                return;
            }
        };
        let mut stdout = child.stdout.take().unwrap();
        let fd = stdout.as_raw_fd();
        // Nonblocking reads let the single worker cancel and time out Spotlight.
        let flags = unsafe { libc::fcntl(fd, libc::F_GETFL) };
        if flags == -1 || unsafe { libc::fcntl(fd, libc::F_SETFL, flags | libc::O_NONBLOCK) } == -1
        {
            let _ = child.kill();
            let _ = child.wait();
            notes.push("Spotlight could not be read; using search roots.".into());
            return;
        }
        let deadline = Instant::now() + Duration::from_millis(650);
        let mut pending = Vec::new();
        let mut bytes = 0;
        let mut candidates = 0;
        let mut limited = false;
        let mut failed = false;
        'read: loop {
            if cancelled() {
                break;
            }
            if Instant::now() >= deadline
                || bytes >= 1024 * 1024
                || candidates >= 2500
                || results.len() >= MAX_RESULTS
            {
                limited = true;
                break;
            }
            let mut buffer = [0; 8192];
            match stdout.read(&mut buffer) {
                Ok(0) => break,
                Ok(read) => {
                    bytes += read;
                    pending.extend_from_slice(&buffer[..read]);
                    while let Some(end) = pending.iter().position(|byte| *byte == 0) {
                        let path =
                            PathBuf::from(OsString::from_vec(pending.drain(..end).collect()));
                        pending.remove(0);
                        candidates += 1;
                        if path.starts_with(root)
                            && !path
                                .components()
                                .any(|part| excluded(Path::new(part.as_os_str())))
                            && matches_name(&path, query)
                            && path.is_dir()
                        {
                            results.insert(path);
                        }
                        if cancelled() || results.len() >= MAX_RESULTS || candidates >= 2500 {
                            limited = true;
                            break 'read;
                        }
                    }
                }
                Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(_) => {
                    failed = true;
                    break;
                }
            }
        }
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    failed |= !status.success();
                    break;
                }
                Ok(None) if !limited && !cancelled() && Instant::now() < deadline => {
                    std::thread::sleep(Duration::from_millis(5));
                }
                Ok(None) => {
                    limited |= Instant::now() >= deadline;
                    let _ = child.kill();
                    break;
                }
                Err(_) => {
                    failed = true;
                    let _ = child.kill();
                    break;
                }
            }
        }
        let status = child.wait();
        failed |= !limited && !cancelled() && status.is_ok_and(|status| !status.success());
        notes.push(
            if failed {
                "Spotlight search failed; using search roots."
            } else if limited {
                "Spotlight search was bounded; more indexed matches may exist."
            } else {
                "Spotlight searched within the configured root; unindexed folders rely on bounded search."
            }
            .into(),
        );
    }

    #[cfg(test)]
    mod tests {
        use super::*;
        #[test]
        fn spotlight_command_is_limited_to_the_selected_root() {
            let root = Path::new("/Users/example/projects/My Work");
            let command = search_command("folder", root);
            let args: Vec<_> = command.get_args().collect();
            assert_eq!(args[0], "-0");
            assert_eq!(args[1], "-onlyin");
            assert_eq!(args[2], root.as_os_str());
            assert_eq!(args[3], predicate("folder").as_str());
        }

        #[test]
        fn spotlight_query_quotes_literal_input() {
            assert_eq!(
                predicate("a\"b\\c"),
                "kMDItemContentTypeTree == \"public.folder\" && kMDItemFSName == \"*a\\\"b\\\\c*\"cd"
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scan(root: &Path, query: &str, limits: Limits) -> (HashSet<PathBuf>, ScanStats) {
        let mut results = HashSet::new();
        let stats = search_roots(&[root.to_owned()], query, &mut results, limits, &|| false);
        (results, stats)
    }

    #[test]
    fn discovery_stays_inside_configured_roots_and_reports_invalid_roots() {
        let parent = tempfile::tempdir().unwrap();
        let root = parent.path().join("selected");
        let inside = root.join("tessera-scope-match");
        let outside = parent.path().join("tessera-scope-match");
        std::fs::create_dir_all(&inside).unwrap();
        std::fs::create_dir(&outside).unwrap();
        let mut request = Request {
            generation: 1,
            query: "tessera-scope-match".into(),
            roots: vec![root.to_string_lossy().into_owned()],
            ready_at: Instant::now(),
        };
        let response = discover(&request, &|| false);
        assert_eq!(response.results, vec![FolderResult::new(inside)]);
        request.roots = vec![parent.path().join("missing").to_string_lossy().into_owned()];
        let response = discover(&request, &|| false);
        assert!(response.results.is_empty());
        assert!(response.notice.unwrap().contains("Cannot open"));
        request.query = outside.to_string_lossy().into_owned();
        assert_eq!(
            discover(&request, &|| false).results,
            vec![FolderResult::new(outside)]
        );
    }

    #[test]
    fn fallback_finds_duplicates_unicode_and_literal_punctuation() {
        let root = tempfile::tempdir().unwrap();
        for directory in [
            "one/My Étude's project",
            "two/My Étude's project",
            "three/other",
        ] {
            std::fs::create_dir_all(root.path().join(directory)).unwrap();
        }
        let (results, _) = scan(root.path(), "étude's", Limits::default());
        assert_eq!(results.len(), 2);
        assert!(results.contains(&root.path().join("one/My Étude's project")));
        assert!(results.contains(&root.path().join("two/My Étude's project")));
    }

    #[test]
    fn fallback_skips_hidden_dependencies_and_build_trees() {
        let root = tempfile::tempdir().unwrap();
        for directory in [
            ".hidden/find-me",
            "node_modules/find-me",
            "target/find-me",
            "src/find-me",
        ] {
            std::fs::create_dir_all(root.path().join(directory)).unwrap();
        }
        let (results, _) = scan(root.path(), "find-me", Limits::default());
        assert_eq!(results, HashSet::from([root.path().join("src/find-me")]));
    }

    #[test]
    fn bounded_scan_reports_depth_entry_and_result_limits() {
        let root = tempfile::tempdir().unwrap();
        std::fs::create_dir_all(root.path().join("one/two/find-me")).unwrap();
        let (results, stats) = scan(
            root.path(),
            "find-me",
            Limits {
                depth: 1,
                ..Limits::default()
            },
        );
        assert!(results.is_empty());
        assert!(stats.limited);
        let (_, stats) = scan(
            root.path(),
            "",
            Limits {
                entries: 0,
                ..Limits::default()
            },
        );
        assert!(stats.limited);
        let (results, stats) = scan(
            root.path(),
            "",
            Limits {
                results: 1,
                ..Limits::default()
            },
        );
        assert_eq!(results.len(), 1);
        assert!(stats.limited);
    }

    #[test]
    fn explicit_paths_validate_without_creating_or_canonicalizing() {
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("My Étude's folder");
        std::fs::create_dir(&path).unwrap();
        assert_eq!(existing_directory(path.to_str().unwrap()).unwrap(), path);
        let missing = root.path().join("missing");
        assert!(existing_directory(missing.to_str().unwrap()).is_err());
        assert!(!missing.exists());
        let file = root.path().join("file");
        std::fs::write(&file, "keep").unwrap();
        assert!(existing_directory(file.to_str().unwrap()).is_err());
        for path in [
            "/tmp/folder",
            "~/folder",
            "~",
            "./folder",
            "../folder",
            "parent/folder",
        ] {
            assert!(explicit_path(path));
        }
        assert!(!explicit_path("folder"));
    }

    #[test]
    fn explicit_paths_and_search_roots_preserve_significant_whitespace() {
        let root = tempfile::tempdir().unwrap();
        let plain = root.path().join("folder");
        let spaced = root.path().join("folder ");
        std::fs::create_dir(&plain).unwrap();
        std::fs::create_dir(&spaced).unwrap();
        std::fs::create_dir(spaced.join("found")).unwrap();
        let request = Request {
            generation: 1,
            query: spaced.to_str().unwrap().into(),
            roots: vec![],
            ready_at: Instant::now(),
        };
        let response = discover(&request, &|| false);
        assert!(response.error.is_none());
        assert_eq!(response.results, vec![FolderResult::new(spaced.clone())]);
        let resolved_root = existing_directory(spaced.to_str().unwrap()).unwrap();
        assert_eq!(resolved_root, spaced);
        assert_eq!(
            scan(&resolved_root, "found", Limits::default()).0,
            HashSet::from([spaced.join("found")])
        );
    }

    #[test]
    fn home_and_relative_paths_expand_without_trimming_folder_names() {
        let base = Path::new("/workspace");
        let home = Some(Path::new("/home/example"));
        for (input, expected) in [
            ("~/ project ", "/home/example/ project "),
            ("~", "/home/example"),
            ("./ project ", "/workspace/ project "),
            (" parent/folder ", "/workspace/ parent/folder "),
            ("/ project ", "/ project "),
        ] {
            assert_eq!(
                literal_path(input, base, home).unwrap(),
                Path::new(expected)
            );
        }
        assert!(literal_path("~/folder", base, None).is_err());
        assert!(literal_path("~someone/folder", base, home).is_err());
        assert!(literal_path("/folder\n", base, home).is_err());
    }

    #[cfg(unix)]
    #[test]
    fn symlinks_can_match_but_are_never_traversed() {
        let root = tempfile::tempdir().unwrap();
        let outside = tempfile::tempdir().unwrap();
        std::fs::create_dir(outside.path().join("find-hidden")).unwrap();
        let link = root.path().join("find-link");
        std::os::unix::fs::symlink(outside.path(), &link).unwrap();
        std::os::unix::fs::symlink(root.path(), root.path().join("cycle")).unwrap();
        let (results, stats) = scan(root.path(), "find", Limits::default());
        assert_eq!(results, HashSet::from([link.clone()]));
        assert!(!stats.limited);
        assert_eq!(existing_directory(link.to_str().unwrap()).unwrap(), link);
    }

    #[test]
    fn cancellation_and_stale_responses_do_not_replace_current_results() {
        let mut search = FolderSearch::new(egui::Context::default());
        search.set_query("old", &["/tmp".into()]);
        let old = search.shared.generation.load(Ordering::Acquire);
        search.set_query("new", &["/tmp".into()]);
        let current = search.shared.generation.load(Ordering::Acquire);
        assert_ne!(old, current);
        assert!(!search.accept(Response {
            generation: old,
            results: vec![FolderResult::new("/old".into())],
            ..Default::default()
        }));
        assert!(search.loading);
        assert!(search.results.is_empty());
        assert!(search.accept(Response {
            generation: current,
            results: vec![FolderResult::new("/new".into())],
            ..Default::default()
        }));
        assert_eq!(search.results[0].path, Path::new("/new"));
        search.cancel();
        assert!(!search.accept(Response {
            generation: current,
            ..Default::default()
        }));
        assert!(!search.loading);
        assert!(search.results.is_empty());
    }

    #[test]
    fn unchanged_queries_do_not_restart_debounce_and_root_edits_do() {
        let mut search = FolderSearch::new(egui::Context::default());
        search.set_query("query", &["/tmp".into()]);
        let first = search.shared.generation.load(Ordering::Acquire);
        search.set_query("query", &["/tmp".into()]);
        assert_eq!(first, search.shared.generation.load(Ordering::Acquire));
        search.set_query("query", &["/other".into()]);
        assert_ne!(first, search.shared.generation.load(Ordering::Acquire));
    }

    #[test]
    fn worker_coalesces_requests_and_delivers_the_latest_explicit_path() {
        let root = tempfile::tempdir().unwrap();
        let first = root.path().join("first");
        let second = root.path().join("second");
        std::fs::create_dir(&first).unwrap();
        std::fs::create_dir(&second).unwrap();
        let mut search = FolderSearch::new(egui::Context::default());
        search.set_query(first.to_str().unwrap(), &[]);
        search.set_query(second.to_str().unwrap(), &[]);
        let deadline = Instant::now() + Duration::from_secs(3);
        while search.loading && Instant::now() < deadline {
            search.poll();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(!search.loading);
        assert!(search.error.is_none());
        assert_eq!(search.results, vec![FolderResult::new(second)]);
    }

    #[test]
    fn inaccessible_root_is_reported_and_does_not_hide_other_roots() {
        let root = tempfile::tempdir().unwrap();
        let valid = root.path().join("find-me");
        std::fs::create_dir(&valid).unwrap();
        let mut results = HashSet::new();
        let stats = search_roots(
            &[root.path().join("missing"), root.path().to_owned()],
            "find-me",
            &mut results,
            Limits::default(),
            &|| false,
        );
        assert_eq!(stats.inaccessible, 1);
        assert_eq!(results, HashSet::from([valid]));
    }

    #[test]
    fn cancelled_scan_stops_before_reading_roots() {
        let mut results = HashSet::new();
        let stats = search_roots(
            &[PathBuf::from("/does-not-exist")],
            "",
            &mut results,
            Limits::default(),
            &|| true,
        );
        assert!(results.is_empty());
        assert_eq!(stats.inaccessible, 0);
    }
}
