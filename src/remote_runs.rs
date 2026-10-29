use regex::Regex;
use serde::Deserialize;
use serde_json::Value;
use std::{
    collections::HashSet,
    fs::{self, OpenOptions},
    io::{Read, Write},
    os::unix::fs::OpenOptionsExt,
    path::PathBuf,
    process::{Command, Stdio},
    sync::mpsc,
    time::{Duration, Instant},
};

#[derive(Clone, Debug)]
pub struct Config {
    pub enabled: bool,
    pub gh_path: String,
    pub run_limit: usize,
    pub sources: Vec<Source>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Source {
    pub name: String,
    pub ticket_pattern: String,
    pub repos: Vec<String>,
    #[serde(default)]
    pub events: Vec<String>,
    #[serde(default = "default_title_match")]
    pub title_match: String,
    #[serde(default)]
    pub pr_label: Option<String>,
    #[serde(default)]
    pub output_job: Option<String>,
    #[serde(default)]
    pub output_step: Option<String>,
    #[serde(default)]
    pub kinds: Vec<Kind>,
    #[serde(skip)]
    ticket_regex: Option<Regex>,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Kind {
    pub label: String,
    pub title_regex: String,
    #[serde(skip)]
    regex: Option<Regex>,
}

fn default_title_match() -> String {
    "{key}\\b".into()
}
fn default_gh() -> String {
    "gh".into()
}
fn default_limit() -> usize {
    30
}
fn default_enabled() -> bool {
    true
}

impl Config {
    pub fn parse(value: &Value) -> Result<Self, String> {
        if value.is_null() {
            return Ok(Self {
                enabled: false,
                gh_path: default_gh(),
                run_limit: default_limit(),
                sources: vec![],
            });
        }
        #[derive(Deserialize)]
        struct Raw {
            #[serde(default = "default_enabled")]
            enabled: bool,
            #[serde(default = "default_gh")]
            gh_path: String,
            #[serde(default = "default_limit")]
            run_limit: usize,
            #[serde(default)]
            sources: Vec<Source>,
        }
        let raw: Raw = serde_json::from_value(value.clone())
            .map_err(|_| "Remote runs configuration has invalid fields".to_string())?;
        safe_text(&raw.gh_path, "GitHub CLI path")?;
        if raw.gh_path.trim().is_empty() {
            return Err("GitHub CLI path must not be empty".into());
        }
        if !(1..=1000).contains(&raw.run_limit) {
            return Err("Remote run_limit must be between 1 and 1000".into());
        }
        if raw.sources.len() > 32 {
            return Err("Remote runs supports at most 32 sources".into());
        }
        let mut sources = raw.sources;
        let mut names = HashSet::new();
        for source in &mut sources {
            let context = format!("Remote source '{}'", source.name);
            safe_text(&source.name, "Source name")?;
            if source.name.trim().is_empty() || !names.insert(source.name.clone()) {
                return Err(format!("{context}: name must be nonempty and unique"));
            }
            if source.repos.is_empty() || source.repos.len() > 32 {
                return Err(format!("{context}: provide between 1 and 32 repositories"));
            }
            for repo in &source.repos {
                validate_repo(repo).map_err(|e| format!("{context}: {e}"))?;
            }
            if source.events.len() > 16
                || source.events.iter().any(|event| {
                    event.is_empty()
                        || !event
                            .bytes()
                            .all(|b| b.is_ascii_alphanumeric() || b == b'_')
                })
            {
                return Err(format!("{context}: invalid event filter"));
            }
            source.ticket_regex = Some(
                compile_regex(&format!("\\A(?:{})\\z", source.ticket_pattern))
                    .map_err(|_| format!("{context}: invalid ticket_pattern regex"))?,
            );
            compile_regex(&source.title_match.replace("{key}", "TICKET-1"))
                .map_err(|_| format!("{context}: invalid title_match regex"))?;
            if !source.title_match.contains("{key}") {
                return Err(format!("{context}: title_match must include {{key}}"));
            }
            for kind in &mut source.kinds {
                safe_text(&kind.label, "Kind label")?;
                kind.regex = Some(compile_regex(&kind.title_regex).map_err(|_| {
                    format!("{context}: invalid title_regex for kind '{}'", kind.label)
                })?);
            }
            for text in [&source.pr_label, &source.output_job, &source.output_step]
                .into_iter()
                .flatten()
            {
                safe_text(text, &context)?;
            }
        }
        Ok(Self {
            enabled: raw.enabled,
            gh_path: raw.gh_path,
            run_limit: raw.run_limit,
            sources,
        })
    }

    pub fn visible(&self) -> bool {
        self.enabled && !self.sources.is_empty()
    }

    pub fn tasks(&self, key: &str) -> Result<Vec<LookupTask>, String> {
        safe_text(key, "Ticket key")?;
        if key.len() > 256 || key.is_empty() {
            return Err("Enter a ticket key of at most 256 bytes".into());
        }
        let mut tasks = Vec::new();
        if self.visible() {
            for source in &self.sources {
                if source
                    .ticket_regex
                    .as_ref()
                    .is_some_and(|pattern| pattern.is_match(key))
                {
                    for repo in &source.repos {
                        tasks.push(LookupTask {
                            repo: repo.clone(),
                            source: source.clone(),
                        });
                    }
                }
            }
        }
        if tasks.is_empty() {
            Err("No configured source matches this complete ticket key".into())
        } else {
            Ok(tasks)
        }
    }
}

fn compile_regex(pattern: &str) -> Result<Regex, regex::Error> {
    regex::RegexBuilder::new(pattern)
        .size_limit(1024 * 1024)
        .build()
}

fn safe_text(value: &str, name: &str) -> Result<(), String> {
    if value.len() > 8192 || value.chars().any(char::is_control) {
        return Err(format!(
            "{name} contains control characters or exceeds 8192 bytes"
        ));
    }
    Ok(())
}

fn validate_repo(repo: &str) -> Result<(), String> {
    let mut parts = repo.split('/');
    let valid = |part: &str| {
        !part.is_empty()
            && part != "."
            && part != ".."
            && !part.starts_with('-')
            && part
                .bytes()
                .all(|b| b.is_ascii_alphanumeric() || matches!(b, b'-' | b'_' | b'.'))
    };
    if parts.next().is_some_and(valid) && parts.next().is_some_and(valid) && parts.next().is_none()
    {
        Ok(())
    } else {
        Err("Repository must be an owner/name pair".into())
    }
}

pub trait Gh: Send + Sync {
    fn run(&self, args: &[String]) -> Result<String, String>;
}

pub struct GhCli {
    path: String,
}

impl GhCli {
    pub fn new(path: String) -> Self {
        Self { path }
    }
}

const GH_MISSING: &str = "GitHub CLI is unavailable; install gh or correct gh_path in Settings";
const GH_AUTH: &str = "GitHub CLI is not authenticated; run gh auth login in your terminal";
const GH_FORBIDDEN: &str =
    "GitHub denied access; check account permissions and organization authorization";
const GH_REPO: &str = "Repository was not found or is inaccessible to your GitHub account";
const GH_NETWORK: &str = "GitHub could not be reached; check your network and retry";
const GH_FAILED: &str =
    "GitHub CLI request failed; check authentication, repository access, and CLI compatibility";
const GH_TIMEOUT: &str = "GitHub CLI request timed out after 20 seconds";
const INVALID_METADATA: &str = "GitHub CLI returned invalid metadata";

fn safe_gh_error(error: &str) -> &str {
    if [
        GH_MISSING,
        GH_AUTH,
        GH_FORBIDDEN,
        GH_REPO,
        GH_NETWORK,
        GH_FAILED,
        GH_TIMEOUT,
        INVALID_METADATA,
    ]
    .contains(&error)
    {
        error
    } else {
        GH_FAILED
    }
}

fn classify_failure(code: Option<i32>, diagnostics: &[u8]) -> &'static str {
    let diagnostic = String::from_utf8_lossy(diagnostics).to_ascii_lowercase();
    if code == Some(127) {
        GH_MISSING
    } else if diagnostic.contains("http 401")
        || diagnostic.contains("gh auth login")
        || diagnostic.contains("not logged")
    {
        GH_AUTH
    } else if diagnostic.contains("http 404")
        || diagnostic.contains("could not resolve to a repository")
    {
        GH_REPO
    } else if diagnostic.contains("http 403") || diagnostic.contains("resource not accessible") {
        GH_FORBIDDEN
    } else if diagnostic.contains("error connecting")
        || diagnostic.contains("could not resolve host")
        || diagnostic.contains("network is unreachable")
    {
        GH_NETWORK
    } else {
        GH_FAILED
    }
}

fn capture_output(
    reader: impl Read + Send + 'static,
    limit: usize,
) -> mpsc::Receiver<std::io::Result<Vec<u8>>> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let result = reader
            .take((limit + 1) as u64)
            .read_to_end(&mut bytes)
            .map(|_| bytes);
        let _ = sender.send(result);
    });
    receiver
}

const MAX_OUTPUT: usize = 8 * 1024 * 1024;
const CLI_TIMEOUT: Duration = Duration::from_secs(20);

impl Gh for GhCli {
    fn run(&self, args: &[String]) -> Result<String, String> {
        self.run_bounded(args, CLI_TIMEOUT)
    }
}

impl GhCli {
    fn run_bounded(&self, args: &[String], timeout: Duration) -> Result<String, String> {
        safe_text(&self.path, "GitHub CLI path")?;
        let marker = format!("TESSERA_GH_{}", uuid::Uuid::new_v4().simple());
        let script = format!(
            "printf '\\n{marker}\\n'; exec {}",
            shell_args(std::iter::once(self.path.as_str()).chain(args.iter().map(String::as_str)))
        );
        let mut command = Command::new(crate::terminal::agent_shell());
        command
            .args(["-lic", &script])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("GH_PROMPT_DISABLED", "1")
            .env("GH_PAGER", "cat");
        let mut child = command.spawn().map_err(|_| "Cannot start GitHub CLI in your login shell; check its installation and configured path".to_string())?;
        let stdout = capture_output(child.stdout.take().ok_or(GH_FAILED)?, MAX_OUTPUT);
        let stderr = capture_output(child.stderr.take().ok_or(GH_FAILED)?, 65536);
        let deadline = Instant::now() + timeout;
        loop {
            match child.try_wait() {
                Ok(Some(status)) => {
                    if !status.success() {
                        let diagnostics = stderr
                            .recv_timeout(Duration::from_millis(250))
                            .ok()
                            .and_then(Result::ok)
                            .unwrap_or_default();
                        return Err(classify_failure(status.code(), &diagnostics).into());
                    }
                    let bytes = stdout
                        .recv_timeout(Duration::from_millis(250))
                        .map_err(|_| "GitHub CLI output did not close".to_string())?
                        .map_err(|_| "Cannot read GitHub CLI output".to_string())?;
                    if bytes.len() > MAX_OUTPUT {
                        return Err("GitHub CLI response exceeds the 8 MiB limit".into());
                    }
                    let text = String::from_utf8(bytes)
                        .map_err(|_| "GitHub CLI returned invalid UTF-8".to_string())?;
                    return text
                        .split_once(&format!("\n{marker}\n"))
                        .map(|(_, text)| text.to_owned())
                        .ok_or_else(|| "Login shell did not return a GitHub CLI response".into());
                }
                Err(_) => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err("Cannot check GitHub CLI process".into());
                }
                Ok(None) if Instant::now() >= deadline => {
                    let _ = child.kill();
                    let _ = child.wait();
                    return Err(GH_TIMEOUT.into());
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(20)),
            }
        }
    }
}

#[derive(Clone, Debug)]
pub struct LookupTask {
    pub repo: String,
    pub source: Source,
}

#[derive(Clone, Debug, Default)]
pub struct RepoResult {
    pub repo: String,
    pub source_name: String,
    pub runs: Vec<Run>,
    pub prs: Vec<PullRequest>,
    pub errors: Vec<String>,
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Run {
    #[serde(rename = "databaseId")]
    pub id: u64,
    #[serde(rename = "displayTitle")]
    pub title: String,
    pub status: String,
    pub conclusion: Option<String>,
    #[serde(rename = "createdAt")]
    pub created_at: String,
    pub url: String,
    #[serde(rename = "workflowName")]
    pub workflow_name: String,
    pub event: String,
    #[serde(skip)]
    pub kind: Option<String>,
}

#[derive(Clone, Debug)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub head_ref_name: String,
    pub state: String,
    pub auto_merge_enabled: bool,
}

impl PullRequest {
    pub fn needs_warning(&self) -> bool {
        self.state != "OPEN" || self.auto_merge_enabled
    }
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct PrJson {
    number: u64,
    title: String,
    url: String,
    head_ref_name: String,
    state: String,
    auto_merge_request: Option<Value>,
}

pub fn lookup(gh: &dyn Gh, task: &LookupTask, key: &str, limit: usize) -> RepoResult {
    let mut result = RepoResult {
        repo: task.repo.clone(),
        source_name: task.source.name.clone(),
        ..Default::default()
    };
    let title_regex = match compile_regex(
        &task
            .source
            .title_match
            .replace("{key}", &regex::escape(key)),
    ) {
        Ok(regex) => regex,
        Err(_) => {
            result.errors.push("Invalid run title matcher".into());
            return result;
        }
    };
    let events: Vec<Option<&str>> = if task.source.events.is_empty() {
        vec![None]
    } else {
        task.source
            .events
            .iter()
            .map(|event| Some(event.as_str()))
            .collect()
    };
    let mut seen = HashSet::new();
    for event in events {
        let mut args = strings(&[
            "run",
            "list",
            "--repo",
            &task.repo,
            "--limit",
            &limit.clamp(1, 1000).to_string(),
            "--json",
            "databaseId,displayTitle,status,conclusion,createdAt,url,workflowName,event",
        ]);
        if let Some(event) = event {
            args.extend(strings(&["--event", event]));
        }
        match gh.run(&args).and_then(|text| {
            serde_json::from_str::<Vec<Run>>(&text).map_err(|_| INVALID_METADATA.into())
        }) {
            Ok(runs) => {
                if runs.len() >= limit.clamp(1, 1000) {
                    result.errors.push(format!(
                        "{}: run lookup window reached {}{}; older matching runs may be omitted",
                        task.repo,
                        limit.clamp(1, 1000),
                        event
                            .map(|event| format!(" for {event}"))
                            .unwrap_or_default()
                    ));
                }
                for mut run in runs {
                    if run.id > 0 && title_regex.is_match(&run.title) && seen.insert(run.id) {
                        run.kind = task
                            .source
                            .kinds
                            .iter()
                            .find(|kind| {
                                kind.regex
                                    .as_ref()
                                    .is_some_and(|regex| regex.is_match(&run.title))
                            })
                            .map(|kind| kind.label.clone());
                        result.runs.push(run);
                    }
                }
            }
            Err(error) => result.errors.push(format!(
                "{}: cannot list runs{}: {}",
                task.repo,
                event
                    .map(|event| format!(" for {event}"))
                    .unwrap_or_default(),
                safe_gh_error(&error)
            )),
        }
    }
    result.runs.sort_by(|a, b| {
        b.created_at
            .cmp(&a.created_at)
            .then_with(|| b.id.cmp(&a.id))
    });
    let mut args = strings(&[
        "pr",
        "list",
        "--repo",
        &task.repo,
        "--state",
        "all",
        "--limit",
        "1000",
        "--json",
        "number,title,url,headRefName,state,autoMergeRequest",
    ]);
    if let Some(label) = &task.source.pr_label
        && !label.is_empty()
    {
        args.extend(strings(&["--label", label]));
    }
    let ticket = compile_regex(&format!(
        "(?:^|[^A-Za-z0-9]){}(?:[^A-Za-z0-9]|$)",
        regex::escape(key)
    ));
    let mut seen_prs = HashSet::new();
    for title_search in [true, false] {
        let mut query = args.clone();
        if title_search {
            query.extend(strings(&["--search", &format!("{key} in:title")]));
        }
        match gh.run(&query).and_then(|text| {
            serde_json::from_str::<Vec<PrJson>>(&text).map_err(|_| INVALID_METADATA.into())
        }) {
            Ok(prs) => {
                if prs.len() >= 1000 {
                    result.errors.push(format!(
                        "{}: PR {} reached 1000 results; older matching PRs may be omitted",
                        task.repo,
                        if title_search {
                            "title search"
                        } else {
                            "branch scan"
                        }
                    ));
                }
                if let Ok(pattern) = &ticket {
                    for pr in prs {
                        if (pattern.is_match(&pr.title) || pattern.is_match(&pr.head_ref_name))
                            && seen_prs.insert(pr.number)
                        {
                            result.prs.push(PullRequest {
                                number: pr.number,
                                title: pr.title,
                                url: pr.url,
                                head_ref_name: pr.head_ref_name,
                                state: pr.state,
                                auto_merge_enabled: pr.auto_merge_request.is_some(),
                            });
                        }
                    }
                }
            }
            Err(error) => result.errors.push(format!(
                "{}: cannot {}: {}",
                task.repo,
                if title_search {
                    "search pull request titles"
                } else {
                    "scan pull request branches"
                },
                safe_gh_error(&error)
            )),
        }
    }
    result
}

#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Job {
    #[serde(rename = "databaseId")]
    pub id: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
    pub steps: Vec<Step>,
}
#[derive(Clone, Debug, Default, Deserialize)]
#[serde(default)]
pub struct Step {
    pub number: u64,
    pub name: String,
    pub status: String,
    pub conclusion: Option<String>,
}

pub fn jobs(gh: &dyn Gh, repo: &str, id: u64) -> Result<Vec<Job>, String> {
    validate_target(repo, id)?;
    #[derive(Deserialize)]
    struct Jobs {
        jobs: Vec<Job>,
    }
    let response = gh
        .run(&strings(&[
            "run",
            "view",
            &id.to_string(),
            "--repo",
            repo,
            "--json",
            "jobs",
        ]))
        .map_err(|error| format!("Cannot load jobs: {}", safe_gh_error(&error)))?;
    serde_json::from_str::<Jobs>(&response)
        .map(|response| response.jobs)
        .map_err(|_| "GitHub CLI returned invalid job metadata".into())
}

fn strings(parts: &[&str]) -> Vec<String> {
    parts.iter().map(|part| (*part).to_owned()).collect()
}
fn quote(value: &str) -> String {
    format!("'{}'", value.replace('\'', "'\\''"))
}
fn shell_args<'a>(parts: impl IntoIterator<Item = &'a str>) -> String {
    parts.into_iter().map(quote).collect::<Vec<_>>().join(" ")
}
fn validate_target(repo: &str, id: u64) -> Result<(), String> {
    validate_repo(repo)?;
    if id == 0 {
        Err("GitHub identity must be a positive number".into())
    } else {
        Ok(())
    }
}

pub fn log_command(
    gh_path: &str,
    repo: &str,
    id: u64,
    job: Option<u64>,
    step: Option<&str>,
) -> Result<String, String> {
    validate_target(repo, id)?;
    safe_text(gh_path, "GitHub CLI path")?;
    let mut args = strings(&[
        gh_path,
        "run",
        "view",
        &id.to_string(),
        "--repo",
        repo,
        "--log",
    ]);
    if let Some(job) = job {
        if job == 0 {
            return Err("Job identity must be positive".into());
        }
        args.extend(strings(&["--job", &job.to_string()]));
    }
    let mut pipeline = shell_args(args.iter().map(String::as_str));
    if let Some(step) = step {
        safe_text(step, "Output step")?;
        pipeline.push_str(&format!(" | TESSERA_LOG_STEP={} awk -F '\\t' 'BEGIN {{ wanted = ENVIRON[\"TESSERA_LOG_STEP\"] }} $2 == wanted {{ print }}'",quote(step)));
    }
    pipeline.push_str(" | ${PAGER:-less -R}");
    Ok(format!(
        "PAGER=\"${{PAGER:-less -R}}\" bash -o pipefail -c {}",
        quote(&pipeline)
    ))
}

pub fn watch_command(gh_path: &str, repo: &str, id: u64) -> Result<String, String> {
    validate_target(repo, id)?;
    safe_text(gh_path, "GitHub CLI path")?;
    Ok(shell_args([
        gh_path,
        "run",
        "watch",
        &id.to_string(),
        "--repo",
        repo,
    ]))
}

#[derive(Clone, Debug)]
pub struct CommentPreview {
    repo: String,
    pr: u64,
    body: String,
}
pub struct ConfirmedComment {
    preview: CommentPreview,
}

impl CommentPreview {
    pub fn new(repo: &str, pr: u64, body: String) -> Result<Self, String> {
        validate_target(repo, pr)?;
        if body.trim().is_empty() || body.len() > 65536 || body.contains('\0') {
            return Err("Comment must contain text, contain no NUL, and fit within 64 KiB".into());
        }
        Ok(Self {
            repo: repo.into(),
            pr,
            body,
        })
    }
    pub fn repo(&self) -> &str {
        &self.repo
    }
    pub fn pr(&self) -> u64 {
        self.pr
    }
    pub fn body(&self) -> &str {
        &self.body
    }
    pub fn confirm(self) -> ConfirmedComment {
        ConfirmedComment { preview: self }
    }
}

struct PrivateBody {
    path: PathBuf,
}
impl Drop for PrivateBody {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
    }
}

pub fn post_comment(gh: &dyn Gh, comment: &ConfirmedComment) -> Result<(), String> {
    let path =
        std::env::temp_dir().join(format!("tessera-comment-{}", uuid::Uuid::new_v4().simple()));
    let mut output = OpenOptions::new()
        .create_new(true)
        .write(true)
        .mode(0o600)
        .open(&path)
        .map_err(|_| "Cannot create private comment file".to_string())?;
    let file = PrivateBody { path };
    output
        .write_all(comment.preview.body.as_bytes())
        .map_err(|_| "Cannot write private comment file".to_string())?;
    drop(output);
    let path = file
        .path
        .to_str()
        .ok_or("Comment file path cannot be represented")?;
    gh.run(&strings(&[
        "pr",
        "comment",
        &comment.preview.pr.to_string(),
        "--repo",
        &comment.preview.repo,
        "--body-file",
        path,
    ]))
    .map(|_| ())
    .map_err(|error| format!("Comment could not be posted: {}", safe_gh_error(&error)))
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    #[test]
    fn enabled_sources_make_remote_runs_visible() {
        let config = Config::parse(&json!({
            "enabled": true,
            "sources": [{"name": "Atlas", "ticket_pattern": "ATL-[0-9]+", "repos": ["fiction/atlas"]}]
        })).unwrap();
        assert!(config.visible());
    }
}

#[cfg(test)]
mod contract_tests {
    use super::*;
    use serde_json::json;
    use std::{collections::VecDeque, sync::Mutex};

    struct Recorded {
        replies: Mutex<VecDeque<Result<String, String>>>,
        calls: Mutex<Vec<Vec<String>>>,
    }
    impl Recorded {
        fn new(replies: Vec<Result<String, String>>) -> Self {
            Self {
                replies: Mutex::new(replies.into()),
                calls: Mutex::new(Vec::new()),
            }
        }
    }
    impl Gh for Recorded {
        fn run(&self, args: &[String]) -> Result<String, String> {
            self.calls.lock().unwrap().push(args.to_vec());
            self.replies
                .lock()
                .unwrap()
                .pop_front()
                .expect("unexpected gh request")
        }
    }
    fn config() -> Config {
        Config::parse(&json!({"enabled":true,"sources":[{"name":"Atlas","ticket_pattern":"ATL-[0-9]+","repos":["fiction/atlas"],"events":["workflow_dispatch","push"],"pr_label":"remote","kinds":[{"label":"Review","title_regex":"^Review"}]}]})).unwrap()
    }
    #[test]
    fn matching_anchors_every_source_and_escapes_title_key() {
        let c = Config::parse(&json!({"enabled":true,"sources":[
            {"name":"Broad","ticket_pattern":"ATL-[0-9]+","repos":["fiction/one"]},
            {"name":"Specific","ticket_pattern":"ATL-42","repos":["fiction/two"]},
            {"name":"Literal","ticket_pattern":"A\\+[0-9]+","repos":["fiction/three"]}
        ]}))
        .unwrap();
        assert_eq!(c.tasks("ATL-42").unwrap().len(), 2);
        assert!(c.tasks("prefix ATL-42").is_err());
        let task = c.tasks("A+42").unwrap().remove(0);
        let gh=Recorded::new(vec![Ok("[{\"databaseId\":1,\"displayTitle\":\"A+42\",\"createdAt\":\"2026-10-29\"},{\"databaseId\":2,\"displayTitle\":\"AAAA42\",\"createdAt\":\"2026-10-29\"}]".into()),Ok("[]".into()),Ok("[]".into())]);
        let r = lookup(&gh, &task, "A+42", 20);
        assert_eq!(r.runs.iter().map(|r| r.id).collect::<Vec<_>>(), vec![1]);
    }
    #[test]
    fn invalid_regex_is_a_named_config_error_and_null_is_hidden() {
        assert!(!Config::parse(&Value::Null).unwrap().visible());
        let err = Config::parse(
            &json!({"sources":[{"name":"Broken","ticket_pattern":"[","repos":["fiction/atlas"]}]}),
        )
        .err()
        .unwrap();
        assert!(err.contains("Broken"));
        assert!(
            !Config::parse(&json!({"enabled":false,"sources":[]}))
                .unwrap()
                .visible()
        );
    }
    #[test]
    fn runs_are_event_filtered_deduplicated_sorted_and_classified() {
        let c = config();
        let task = c.tasks("ATL-42").unwrap().remove(0);
        let gh = Recorded::new(vec![
            Ok(include_str!("../tests/fixtures/remote_runs/runs.json").into()),
            Ok(include_str!("../tests/fixtures/remote_runs/runs.json").into()),
            Ok(include_str!("../tests/fixtures/remote_runs/prs.json").into()),
            Ok(include_str!("../tests/fixtures/remote_runs/prs.json").into()),
        ]);
        let r = lookup(&gh, &task, "ATL-42", 20);
        assert!(r.errors.is_empty(), "{:?}", r.errors);
        assert_eq!(
            r.runs.iter().map(|r| r.id).collect::<Vec<_>>(),
            vec![102, 101]
        );
        assert_eq!(r.runs[0].kind.as_deref(), Some("Review"));
        assert_eq!(
            r.prs.iter().map(|p| p.number).collect::<Vec<_>>(),
            vec![7, 8, 10]
        );
        assert!(!r.prs[0].needs_warning());
        assert!(r.prs[1].needs_warning());
        assert!(r.prs[2].needs_warning());
        let calls = gh.calls.lock().unwrap();
        assert!(
            calls[0]
                .windows(2)
                .any(|x| x == ["--event", "workflow_dispatch"])
        );
        assert!(calls[1].windows(2).any(|x| x == ["--event", "push"]));
        assert!(calls[2].windows(2).any(|x| x == ["--label", "remote"]));
    }
    #[test]
    fn query_failure_keeps_other_event_and_pr_results_without_leaking_diagnostics() {
        let c = config();
        let task = c.tasks("ATL-42").unwrap().remove(0);
        let gh = Recorded::new(vec![
            Err("token SECRET".into()),
            Ok(include_str!("../tests/fixtures/remote_runs/runs.json").into()),
            Ok(include_str!("../tests/fixtures/remote_runs/prs.json").into()),
            Ok(include_str!("../tests/fixtures/remote_runs/prs.json").into()),
        ]);
        let r = lookup(&gh, &task, "ATL-42", 20);
        assert_eq!(r.runs.len(), 2);
        assert_eq!(r.prs.len(), 3);
        assert_eq!(r.errors.len(), 1);
        assert!(!r.errors[0].contains("SECRET"));
        let gh = Recorded::new(vec![
            Ok("[]".into()),
            Ok("[]".into()),
            Err("auth SECRET".into()),
            Ok("[]".into()),
        ]);
        assert_eq!(lookup(&gh, &task, "ATL-42", 20).errors.len(), 1);
    }
    #[test]
    fn title_search_finds_older_prs_and_merges_branch_matches() {
        let c = config();
        let task = c.tasks("ATL-42").unwrap().remove(0);
        let older = json!([{"number":77,"title":"Older ATL-42", "url":"https://github.com/fiction/atlas/pull/77", "headRefName":"previous", "state":"OPEN", "autoMergeRequest":null}]).to_string();
        let gh = Recorded::new(vec![
            Ok("[]".into()),
            Ok("[]".into()),
            Ok(older),
            Ok(include_str!("../tests/fixtures/remote_runs/prs.json").into()),
        ]);
        let result = lookup(&gh, &task, "ATL-42", 20);
        assert_eq!(
            result.prs.iter().map(|p| p.number).collect::<Vec<_>>(),
            vec![77, 7, 8, 10]
        );
        assert!(
            gh.calls.lock().unwrap()[2]
                .windows(2)
                .any(|args| args == ["--search", "ATL-42 in:title"])
        );
    }
    #[test]
    fn full_event_windows_report_limits_and_keep_each_events_matches() {
        let c = config();
        let task = c.tasks("ATL-42").unwrap().remove(0);
        let first = json!([{"databaseId":102,"displayTitle":"Review ATL-42", "createdAt":"2026-10-29T12:00:00Z"}]).to_string();
        let second = json!([{"databaseId":101,"displayTitle":"Implement ATL-42", "createdAt":"2026-10-29T11:00:00Z"}]).to_string();
        let gh = Recorded::new(vec![
            Ok(first),
            Ok(second),
            Ok("[]".into()),
            Ok("[]".into()),
        ]);
        let result = lookup(&gh, &task, "ATL-42", 1);
        assert_eq!(result.runs.len(), 2);
        assert_eq!(result.errors.len(), 2);
        assert!(result.errors.iter().all(|error| error.contains("window")));
    }
    #[test]
    fn pr_query_failure_keeps_the_other_search_and_a_safe_recovery_reason() {
        let c = config();
        let task = c.tasks("ATL-42").unwrap().remove(0);
        for title_fails in [true, false] {
            let metadata = Ok(include_str!("../tests/fixtures/remote_runs/prs.json").into());
            let failure = Err(GH_AUTH.into());
            let (title, branch) = if title_fails {
                (failure, metadata)
            } else {
                (metadata, failure)
            };
            let gh = Recorded::new(vec![Ok("[]".into()), Ok("[]".into()), title, branch]);
            let result = lookup(&gh, &task, "ATL-42", 20);
            assert_eq!(result.prs.len(), 3);
            assert_eq!(result.errors.len(), 1);
            assert!(result.errors[0].contains("not authenticated"));
        }
    }
    #[test]
    fn pr_query_caps_are_visible_after_deduplication() {
        let c = config();
        let task = c.tasks("ATL-42").unwrap().remove(0);
        let pr = json!({"number":7,"title":"ATL-42", "url":"https://github.com/fiction/atlas/pull/7", "headRefName":"feature/ATL-42", "state":"OPEN", "autoMergeRequest":null});
        let metadata = serde_json::to_string(&vec![pr; 1000]).unwrap();
        let gh = Recorded::new(vec![
            Ok("[]".into()),
            Ok("[]".into()),
            Ok(metadata.clone()),
            Ok(metadata),
        ]);
        let result = lookup(&gh, &task, "ATL-42", 20);
        assert_eq!(result.prs.len(), 1);
        assert_eq!(result.errors.len(), 2);
        assert!(result.errors.iter().all(|error| error.contains("1000")));
    }
    #[test]
    fn job_and_step_metadata_parse_recorded_response() {
        let gh = Recorded::new(vec![Ok(include_str!(
            "../tests/fixtures/remote_runs/jobs.json"
        )
        .into())]);
        let list = jobs(&gh, "fiction/atlas", 102).unwrap();
        assert_eq!(list[0].id, 501);
        assert_eq!(list[0].steps[1].name, "Agent output");
    }
    #[test]
    fn log_pipeline_preserves_shell_literals_pager_step_and_producer_failure() {
        use std::{fs, os::unix::fs::PermissionsExt, process::Command};
        let dir = tempfile::tempdir().unwrap();
        let gh = dir.path().join("fake gh's");
        fs::write(&gh,"#!/bin/sh\nprintf 'job\\tOther\\twrong\\njob\\tAgent output\\tright\\n'\nexit \"${FAKE_EXIT:-0}\"\n").unwrap();
        fs::set_permissions(&gh, fs::Permissions::from_mode(0o700)).unwrap();
        let cmd = log_command(
            gh.to_str().unwrap(),
            "fiction/atlas",
            102,
            Some(501),
            Some("Agent output"),
        )
        .unwrap();
        let out = Command::new("/bin/sh")
            .args(["-c", &cmd])
            .env("PAGER", "cat")
            .output()
            .unwrap();
        assert!(
            out.status.success(),
            "{}",
            String::from_utf8_lossy(&out.stderr)
        );
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "job\tAgent output\tright\n"
        );
        let out = Command::new("/bin/sh")
            .args(["-c", &cmd])
            .env("PAGER", "cat")
            .env("FAKE_EXIT", "9")
            .output()
            .unwrap();
        assert_eq!(out.status.code(), Some(9));
        let out = Command::new("/bin/sh")
            .args(["-c", &format!("PAGER=cat; {cmd}")])
            .env_remove("PAGER")
            .output()
            .unwrap();
        assert!(out.status.success());
        assert_eq!(
            String::from_utf8_lossy(&out.stdout),
            "job\tAgent output\tright\n"
        );
        assert!(log_command("gh", "fiction/atlas", 0, None, None).is_err());
    }
    #[test]
    fn preview_confirmation_posts_private_file_and_removes_it_on_failure() {
        use std::{fs, os::unix::fs::PermissionsExt};
        struct Inspect {
            path: Mutex<Option<std::path::PathBuf>>,
            fail: bool,
        }
        impl Gh for Inspect {
            fn run(&self, args: &[String]) -> Result<String, String> {
                let path = std::path::PathBuf::from(args.last().unwrap());
                assert_eq!(fs::read_to_string(&path).unwrap(), "Please review ATL-42.");
                assert_eq!(
                    fs::metadata(&path).unwrap().permissions().mode() & 0o777,
                    0o600
                );
                *self.path.lock().unwrap() = Some(path);
                if self.fail {
                    Err("SECRET body".into())
                } else {
                    Ok(String::new())
                }
            }
        }
        assert!(CommentPreview::new("fiction/atlas", 0, "body".into()).is_err());
        for fail in [false, true] {
            let gh = Inspect {
                path: Mutex::new(None),
                fail,
            };
            let preview =
                CommentPreview::new("fiction/atlas", 7, "Please review ATL-42.".into()).unwrap();
            assert_eq!(preview.body(), "Please review ATL-42.");
            let result = post_comment(&gh, &preview.confirm());
            assert_eq!(result.is_err(), fail);
            assert!(!gh.path.lock().unwrap().as_ref().unwrap().exists());
            if let Err(err) = result {
                assert!(!err.contains("SECRET"));
            }
        }
    }
}

pub fn created_at_seconds(timestamp: &str) -> Option<u64> {
    if !timestamp.is_ascii() || timestamp.len() < 20 {
        return None;
    }
    let number = |start, end| {
        let text = timestamp.get(start..end)?;
        if !text.bytes().all(|byte| byte.is_ascii_digit()) {
            return None;
        }
        text.parse::<i64>().ok()
    };
    let bytes = timestamp.as_bytes();
    if bytes[4] != b'-'
        || bytes[7] != b'-'
        || bytes[10] != b'T'
        || bytes[13] != b':'
        || bytes[16] != b':'
    {
        return None;
    }
    let (year, month, day, hour, minute, second) = (
        number(0, 4)?,
        number(5, 7)?,
        number(8, 10)?,
        number(11, 13)?,
        number(14, 16)?,
        number(17, 19)?,
    );
    if !(1970..=9999).contains(&year)
        || !(1..=12).contains(&month)
        || !(0..24).contains(&hour)
        || !(0..60).contains(&minute)
        || !(0..60).contains(&second)
    {
        return None;
    }
    let leap = |year| year % 4 == 0 && (year % 100 != 0 || year % 400 == 0);
    let months = [
        31,
        if leap(year) { 29 } else { 28 },
        31,
        30,
        31,
        30,
        31,
        31,
        30,
        31,
        30,
        31,
    ];
    if day < 1 || day > months[(month - 1) as usize] {
        return None;
    }
    let mut zone = &timestamp[19..];
    if let Some(fraction) = zone.strip_prefix('.') {
        let digits = fraction.bytes().take_while(u8::is_ascii_digit).count();
        if digits == 0 {
            return None;
        }
        zone = &fraction[digits..];
    }
    let offset = if zone == "Z" {
        0
    } else {
        let z = zone.as_bytes();
        if z.len() != 6 || !matches!(z[0], b'+' | b'-') || z[3] != b':' {
            return None;
        }
        if !zone[1..3]
            .bytes()
            .chain(zone[4..6].bytes())
            .all(|byte| byte.is_ascii_digit())
        {
            return None;
        }
        let hours = zone[1..3].parse::<i64>().ok()?;
        let minutes = zone[4..6].parse::<i64>().ok()?;
        if hours > 23 || minutes > 59 {
            return None;
        }
        (hours * 3600 + minutes * 60) * if z[0] == b'+' { 1 } else { -1 }
    };
    let leap_days = |year: i64| (year - 1) / 4 - (year - 1) / 100 + (year - 1) / 400;
    let days = (year - 1970) * 365 + leap_days(year) - leap_days(1970)
        + months[..(month - 1) as usize].iter().sum::<i64>()
        + day
        - 1;
    u64::try_from(days * 86400 + hour * 3600 + minute * 60 + second - offset).ok()
}

#[cfg(test)]
mod timestamp_tests {
    use super::*;
    #[test]
    fn github_timestamps_validate_leap_days_and_offsets() {
        assert_eq!(created_at_seconds("1970-01-01T00:00:00Z"), Some(0));
        assert_eq!(created_at_seconds("1970-01-01T01:00:00+01:00"), Some(0));
        assert_eq!(
            created_at_seconds("1972-03-01T00:00:00.123Z"),
            Some(68256000)
        );
        assert_eq!(created_at_seconds("2023-02-29T00:00:00Z"), None);
        assert_eq!(created_at_seconds("2024-13-01T00:00:00Z"), None);
        assert_eq!(created_at_seconds("2024-01-01T25:00:00Z"), None);
        assert_eq!(created_at_seconds("a"), None);
        assert_eq!(created_at_seconds("1970-+1-01T00:00:00Z"), None);
        assert_eq!(created_at_seconds("1970-01-01T00:00:00+-1:00"), None);
    }
}

#[cfg(test)]
mod cli_tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    fn executable(directory: &std::path::Path, text: &str) -> GhCli {
        let path = directory.join("fake gh's");
        fs::write(&path, text).unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o700)).unwrap();
        GhCli::new(path.to_str().unwrap().into())
    }
    #[test]
    fn cli_passes_literal_arguments_and_suppresses_raw_failure_output() {
        let dir = tempfile::tempdir().unwrap();
        let cli = executable(dir.path(), "#!/bin/sh\nprintf '%s\\n' \"$@\"\n");
        assert_eq!(
            cli.run(&strings(&[
                "with space",
                "with'apostrophe",
                "$(printf injected)"
            ]))
            .unwrap(),
            "with space\nwith'apostrophe\n$(printf injected)\n"
        );
        let cli = executable(
            dir.path(),
            "#!/bin/sh\nprintf 'SECRET stdout'\nprintf 'SECRET stderr' >&2\nexit 1\n",
        );
        let error = cli.run(&[]).unwrap_err();
        assert!(!error.contains("SECRET"));
    }
    #[test]
    fn cli_classifies_safe_recovery_actions_without_raw_diagnostics() {
        let dir = tempfile::tempdir().unwrap();
        for (diagnostic, code, expected) in [
            ("HTTP 401: SECRET credential", 1, "not authenticated"),
            ("HTTP 404: SECRET", 1, "not found"),
            ("HTTP 403: SECRET", 1, "denied access"),
            ("SECRET command not found", 127, "unavailable"),
        ] {
            let cli = executable(
                dir.path(),
                &format!(
                    "#!/bin/sh\nprintf '%s' '{}' >&2\nexit {}\n",
                    diagnostic, code
                ),
            );
            let error = cli.run(&[]).unwrap_err();
            assert!(error.contains(expected), "{error}");
            assert!(!error.contains("SECRET"));
        }
    }
    #[test]
    fn cli_enforces_deadline_and_output_bound() {
        let dir = tempfile::tempdir().unwrap();
        let cli = executable(dir.path(), "#!/bin/sh\nexec sleep 10\n");
        let start = Instant::now();
        assert!(
            cli.run_bounded(&[], Duration::from_millis(100))
                .unwrap_err()
                .contains("timed out")
        );
        assert!(start.elapsed() < Duration::from_secs(2));
        let cli = executable(dir.path(), "#!/bin/sh\nhead -c 8500000 /dev/zero\nexit 0\n");
        assert!(cli.run(&[]).is_err());
    }
    #[test]
    fn watch_arguments_remain_literal_in_the_shell() {
        let dir = tempfile::tempdir().unwrap();
        let cli = executable(dir.path(), "#!/bin/sh\nprintf '%s\\n' \"$@\"\n");
        let command = watch_command(&cli.path, "fiction/atlas", 102).unwrap();
        let result = Command::new("/bin/sh")
            .args(["-c", &command])
            .output()
            .unwrap();
        assert!(result.status.success());
        assert_eq!(
            String::from_utf8(result.stdout).unwrap(),
            "run\nwatch\n102\n--repo\nfiction/atlas\n"
        );
    }
}
