use crate::remote_runs::{Gh, parallel_map};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Config {
    pub gh_path: String,
    pub keyword: String,
    pub extra: String,
    pub ignore: String,
    pub minutes: u64,
    pub idle_days: u64,
    pub bot_first: bool,
    pub review_source: ReviewSource,
    pub review_repos: String,
    pub review_teams: String,
    pub review_exclude_prefixes: String,
    pub review_max_age_days: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            gh_path: "gh".into(),
            keyword: String::new(),
            extra: String::new(),
            ignore: String::new(),
            minutes: 3,
            idle_days: 3,
            bot_first: false,
            review_source: ReviewSource::Github,
            review_repos: String::new(),
            review_teams: String::new(),
            review_exclude_prefixes: String::new(),
            review_max_age_days: 60,
        }
    }
}

#[derive(Clone, Copy, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ReviewSource {
    #[default]
    Github,
    Repositories,
}

impl Config {
    pub fn validate(&self) -> Result<(), String> {
        if self.gh_path.trim().is_empty() || self.gh_path.chars().any(char::is_control) {
            return Err("Provide a GitHub CLI path".into());
        }
        if !(1..=60).contains(&self.minutes) {
            return Err("Refresh interval must be between 1 and 60 minutes".into());
        }
        if !(1..=90).contains(&self.idle_days) {
            return Err("Idle threshold must be between 1 and 90 days".into());
        }
        if self.review_max_age_days > 3650 {
            return Err("Maximum review age must be between 0 and 3650 days".into());
        }
        for value in [
            &self.keyword,
            &self.extra,
            &self.ignore,
            &self.review_exclude_prefixes,
        ] {
            if value.len() > 4096 || value.chars().any(char::is_control) {
                return Err("Filters must be single lines of at most 4096 characters".into());
            }
        }
        let repos = scope_list(&self.review_repos)?;
        scope_list(&self.review_teams)?;
        if self.review_source == ReviewSource::Repositories && repos.is_empty() {
            return Err("Add at least one review repository in owner/repository format".into());
        }
        Ok(())
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord)]
pub enum Lane {
    Review,
    Me,
    Others,
    Ready,
    Draft,
}
impl Lane {
    pub const ALL: [Self; 5] = [
        Self::Review,
        Self::Me,
        Self::Others,
        Self::Ready,
        Self::Draft,
    ];
    pub fn label(self) -> &'static str {
        match self {
            Self::Review => "Needs my review",
            Self::Me => "Needs me",
            Self::Others => "Waiting on others",
            Self::Ready => "Ready to merge",
            Self::Draft => "Drafts",
        }
    }
}

#[derive(Clone, Debug)]
pub struct Blocker {
    pub label: String,
    pub url: String,
}
#[derive(Clone, Debug)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    pub url: String,
    pub repo: String,
    pub updated_at: String,
    pub is_bot: bool,
    pub lane: Lane,
    pub blockers: Vec<Blocker>,
    pub review_url: String,
    pub failed_check_url: Option<String>,
    pub feedback_url: Option<String>,
}

const PR_FIELDS: &str = r#"
number title url state isDraft createdAt updatedAt mergeable reviewDecision mergeStateStatus
repository { nameWithOwner }
author { __typename }
commits(last: 1) { nodes { commit { statusCheckRollup { contexts(first: 100) {
pageInfo { hasNextPage }
nodes { __typename ... on CheckRun { name status conclusion detailsUrl }
... on StatusContext { context state targetUrl } }
} } } } }
reviewThreads(first: 100) { pageInfo { hasNextPage } nodes {
isResolved comments(first: 1) { nodes { url } }
} }
"#;
const INVALID: &str = "GitHub returned incomplete or invalid pull request data; refresh to retry";

fn required<'a>(value: &'a Value, key: &str) -> Result<&'a str, String> {
    value[key].as_str().ok_or_else(|| INVALID.into())
}
fn connection(value: &Value) -> Result<(&[Value], bool), String> {
    let nodes = value["nodes"].as_array().ok_or(INVALID)?;
    let more = value["pageInfo"]["hasNextPage"].as_bool().ok_or(INVALID)?;
    Ok((nodes, more))
}
fn safe_link(value: &Value, fallback: &str) -> String {
    value
        .as_str()
        .filter(|url| valid_url(url))
        .unwrap_or(fallback)
        .into()
}
fn valid_url(url: &str) -> bool {
    url.starts_with("https://")
        && url.len() > 8
        && !url.chars().any(|c| c.is_whitespace() || c.is_control())
}

fn classify(value: &Value, ignored: &[&str], requested: bool) -> Result<PullRequest, String> {
    let url = required(value, "url")?;
    if !valid_url(url) {
        return Err(INVALID.into());
    }
    let review_url = format!("{url}/files");
    let checks_url = format!("{url}/checks");
    let mut mine = Vec::new();
    let mut others = Vec::new();
    let blocker = |label: &str, url: &str| Blocker {
        label: label.into(),
        url: url.into(),
    };
    match required(value, "mergeable")? {
        "CONFLICTING" => mine.push(blocker("Merge conflict", &format!("{url}/conflicts"))),
        "UNKNOWN" => others.push(blocker("Mergeability computing", url)),
        "MERGEABLE" => {}
        _ => return Err(INVALID.into()),
    }
    let commits = value["commits"]["nodes"].as_array().ok_or(INVALID)?;
    let commit = commits
        .first()
        .ok_or(INVALID)?
        .get("commit")
        .ok_or(INVALID)?;
    let rollup = commit.get("statusCheckRollup").ok_or(INVALID)?;
    let mut failed_check_url = None;
    let mut running = 0;
    if !rollup.is_null() {
        let (checks, more) = connection(&rollup["contexts"])?;
        if more {
            others.push(blocker("More checks exist; confirm on GitHub", &checks_url));
        }
        for check in checks {
            let (name, state, link) = match required(check, "__typename")? {
                "StatusContext" => (
                    required(check, "context")?,
                    required(check, "state")?,
                    &check["targetUrl"],
                ),
                "CheckRun" => {
                    let status = required(check, "status")?;
                    (
                        required(check, "name")?,
                        if status == "COMPLETED" {
                            check["conclusion"].as_str().unwrap_or("UNKNOWN")
                        } else {
                            status
                        },
                        &check["detailsUrl"],
                    )
                }
                _ => return Err(INVALID.into()),
            };
            if ignored.iter().any(|part| name.contains(part)) {
                continue;
            }
            match state {
                "FAILURE" | "ERROR" | "TIMED_OUT" | "CANCELLED" | "ACTION_REQUIRED"
                | "STARTUP_FAILURE" => {
                    let link = safe_link(link, &checks_url);
                    failed_check_url.get_or_insert_with(|| link.clone());
                    mine.push(blocker(&format!("CI failed: {name}"), &link));
                }
                "SUCCESS" | "NEUTRAL" | "SKIPPED" => {}
                _ => running += 1,
            }
        }
    }
    if running > 0 {
        others.push(blocker(&format!("CI pending ({running})"), &checks_url));
    }
    let (threads, more) = connection(&value["reviewThreads"])?;
    if more {
        others.push(blocker(
            "More review threads exist; confirm on GitHub",
            &review_url,
        ));
    }
    let mut unresolved = Vec::new();
    for thread in threads {
        if !thread["isResolved"].as_bool().ok_or(INVALID)? {
            unresolved.push(thread);
        }
    }
    let mut feedback_url = None;
    if let Some(first) = unresolved.first() {
        let link = safe_link(&first["comments"]["nodes"][0]["url"], &review_url);
        mine.push(blocker(
            &format!("{} unresolved thread(s)", unresolved.len()),
            &link,
        ));
        feedback_url = Some(link);
    }
    match value.get("reviewDecision").ok_or(INVALID)? {
        Value::Null => {}
        Value::String(decision) if decision == "APPROVED" => {}
        Value::String(decision) if decision == "CHANGES_REQUESTED" => {
            mine.push(blocker("Changes requested", url));
            feedback_url.get_or_insert_with(|| url.into());
        }
        Value::String(decision) if decision == "REVIEW_REQUIRED" => {
            others.push(blocker("Review required", url))
        }
        _ => return Err(INVALID.into()),
    }
    match required(value, "mergeStateStatus")? {
        "CLEAN" | "HAS_HOOKS" | "DRAFT" => {}
        "BLOCKED" => others.push(blocker("Blocked by branch protection", url)),
        "BEHIND" => others.push(blocker("Branch is behind its base", url)),
        "DIRTY" => {
            if mine.is_empty() {
                mine.push(blocker("Merge conflict", url));
            }
        }
        _ => {
            if others.is_empty() {
                others.push(blocker("Merge readiness unconfirmed", url));
            }
        }
    }
    let draft = value["isDraft"].as_bool().ok_or(INVALID)?;
    let lane = if draft {
        Lane::Draft
    } else if requested {
        Lane::Review
    } else if !mine.is_empty() {
        Lane::Me
    } else if !others.is_empty() {
        Lane::Others
    } else {
        Lane::Ready
    };
    mine.extend(others);
    let updated_at = required(value, "updatedAt")?;
    if parse_timestamp(updated_at).is_none() {
        return Err(INVALID.into());
    }
    Ok(PullRequest {
        number: value["number"].as_u64().filter(|n| *n > 0).ok_or(INVALID)?,
        title: required(value, "title")?.into(),
        url: url.into(),
        repo: required(&value["repository"], "nameWithOwner")?.into(),
        updated_at: updated_at.into(),
        is_bot: value["author"]["__typename"] == "Bot",
        lane,
        blockers: mine,
        review_url,
        failed_check_url,
        feedback_url,
    })
}

fn graphql(gh: &impl Gh, query: String, variables: &[(&str, String)]) -> Result<Value, String> {
    let mut args = vec![
        "api".into(),
        "graphql".into(),
        "-f".into(),
        format!(
            "query={}",
            query.split_whitespace().collect::<Vec<_>>().join(" ")
        ),
    ];
    for (name, value) in variables {
        args.extend(["-f".into(), format!("{name}={value}")]);
    }
    let response: Value =
        serde_json::from_str(&gh.run_detailed(&args).map_err(|error| error.message())?)
            .map_err(|_| INVALID)?;
    if response.get("errors").is_some_and(|errors| {
        !errors.is_null() && errors.as_array().is_none_or(|items| !items.is_empty())
    }) || !response["data"].is_object()
    {
        return Err(
            "GitHub could not load all pull requests; check account access and search filters"
                .into(),
        );
    }
    Ok(response["data"].clone())
}

fn search(gh: &impl Gh, config: &Config, filter: &str) -> Result<Vec<Value>, String> {
    let keyword = config.keyword.replace(['"', '\\'], "");
    let title = if keyword.trim().is_empty() {
        String::new()
    } else {
        format!("\"{}\" in:title", keyword.trim())
    };
    let q = format!(
        "is:open is:pr archived:false {title} sort:updated-desc {} {filter}",
        config.extra.trim()
    );
    search_query(gh, &q)
}

fn search_query(gh: &impl Gh, q: &str) -> Result<Vec<Value>, String> {
    search_fields(gh, q, PR_FIELDS)
}

fn search_fields(gh: &impl Gh, q: &str, fields: &str) -> Result<Vec<Value>, String> {
    let mut cursor = String::new();
    let mut results = Vec::new();
    for _ in 0..100 {
        let after = if cursor.is_empty() {
            ""
        } else {
            ", after:$cursor"
        };
        let cursor_parameter = if cursor.is_empty() {
            ""
        } else {
            ", $cursor:String!"
        };
        let query = format!(
            "query($q:String!{cursor_parameter}) {{ search(query:$q,type:ISSUE,first:10{after}) {{ issueCount pageInfo {{ hasNextPage endCursor }} nodes {{ ... on PullRequest {{ {fields} }} }} }} }}"
        );
        let mut variables = vec![("q", q.to_owned())];
        if !cursor.is_empty() {
            variables.push(("cursor", cursor.clone()));
        }
        if gh.cancelled() {
            return Err("Refresh cancelled".into());
        }
        let data = graphql(gh, query, &variables)?;
        let total = data["search"]["issueCount"].as_u64().ok_or(INVALID)?;
        if total > 1000 {
            return Err(
                "More than 1000 pull requests match; narrow the title or extra filters".into(),
            );
        }
        let (nodes, more) = connection(&data["search"])?;
        results.extend(nodes.iter().cloned());
        if !more {
            return Ok(results);
        }
        let next = required(&data["search"]["pageInfo"], "endCursor")?;
        if next.is_empty() || next == cursor {
            return Err(INVALID.into());
        }
        cursor = next.into();
    }
    Err("More than 1000 pull requests match; narrow the title or extra filters".into())
}

fn scope_list(raw: &str) -> Result<Vec<String>, String> {
    let values: BTreeSet<_> = raw
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .map(str::to_ascii_lowercase)
        .collect();
    if values.len() > 32 {
        return Err("Configure at most 32 repositories or teams".into());
    }
    for value in &values {
        let Some((owner, name)) = value.split_once('/') else {
            return Err(
                "Repositories and teams must use owner/name format, separated by commas".into(),
            );
        };
        if owner.is_empty()
            || name.is_empty()
            || matches!(name, "." | "..")
            || !owner
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || c == b'-')
            || !owner.as_bytes()[0].is_ascii_alphanumeric()
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'-' | b'_' | b'.'))
        {
            return Err(
                "Repositories and teams must use owner/name format, separated by commas".into(),
            );
        }
    }
    Ok(values.into_iter().collect())
}

#[derive(Clone, Debug)]
pub struct FetchError {
    pub scope: String,
    pub message: String,
}

#[derive(Default)]
pub struct FetchResult {
    pub prs: Vec<PullRequest>,
    pub errors: Vec<FetchError>,
}

fn review_since(now: u64, max_age_days: u64) -> String {
    let mut days = now.saturating_sub((max_age_days + 1) * 86400) / 86400;
    let leap = |year: u64| {
        year.is_multiple_of(4) && (!year.is_multiple_of(100) || year.is_multiple_of(400))
    };
    let mut year = 1970;
    while days >= 365 + u64::from(leap(year)) {
        days -= 365 + u64::from(leap(year));
        year += 1;
    }
    let months = [
        31,
        28 + u64::from(leap(year)),
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
    let mut month = 0;
    while days >= months[month] {
        days -= months[month];
        month += 1;
    }
    format!("{year:04}-{:02}-{:02}", month + 1, days + 1)
}

pub fn fetch(gh: &impl Gh, config: &Config) -> Result<FetchResult, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    let started = std::time::Instant::now();
    let result = fetch_at(gh, config, now);
    log::debug!(
        "merge radar refresh elapsed_ms={} prs={} failures={}",
        started.elapsed().as_millis(),
        result.as_ref().map_or(0, |result| result.prs.len()),
        result.as_ref().map_or(1, |result| result.errors.len())
    );
    result
}

fn fetch_at(gh: &impl Gh, config: &Config, now: u64) -> Result<FetchResult, String> {
    config.validate()?;
    let mut queries = vec![
        (
            "Assigned pull requests".to_owned(),
            "assignee:@me".to_owned(),
            false,
        ),
        (
            "Authored pull requests".to_owned(),
            "author:@me".to_owned(),
            false,
        ),
    ];
    match config.review_source {
        ReviewSource::Github => queries.push((
            "Review requests".into(),
            "user-review-requested:@me".into(),
            true,
        )),
        ReviewSource::Repositories => {
            let teams = scope_list(&config.review_teams)?;
            let since = review_since(now, config.review_max_age_days);
            for repo in scope_list(&config.review_repos)? {
                let owner = repo.split('/').next().ok_or(INVALID)?;
                let requesters = std::iter::once("user-review-requested:@me".to_owned()).chain(
                    teams
                        .iter()
                        .filter(|team| team.split('/').next() == Some(owner))
                        .map(|team| format!("team-review-requested:{team}")),
                );
                for requester in requesters {
                    queries.push((repo.clone(), format!("is:open is:pr repo:{repo} draft:false -author:@me created:>={since} sort:updated-desc {requester}"), true));
                }
            }
        }
    }
    let responses = parallel_map(&queries, |(scope, query, review)| {
        if gh.cancelled() {
            return Err("Refresh cancelled".into());
        }
        let started = std::time::Instant::now();
        let result = if *review && config.review_source == ReviewSource::Repositories {
            search_fields(
                gh,
                query,
                &PR_FIELDS.replace("reviewDecision mergeStateStatus", ""),
            )
        } else {
            search(gh, config, query)
        };
        log::debug!(
            "merge radar scope={} review={} elapsed_ms={} success={}",
            scope,
            review,
            started.elapsed().as_millis(),
            result.is_ok()
        );
        result
    });
    if gh.cancelled() {
        return Err("Refresh cancelled".into());
    }
    let mut result = FetchResult::default();
    let mut unique = BTreeMap::new();
    let mut review_urls = BTreeSet::new();
    let mut successes = 0;
    let prefixes: Vec<_> = config
        .review_exclude_prefixes
        .split(',')
        .map(str::trim)
        .filter(|prefix| !prefix.is_empty())
        .collect();
    for ((scope, _, requested), response) in queries.into_iter().zip(responses) {
        let nodes = match response {
            Ok(nodes) => {
                successes += 1;
                nodes
            }
            Err(message) => {
                result.errors.push(FetchError { scope, message });
                continue;
            }
        };
        for mut pr in nodes {
            let checked = (|| {
                if required(&pr, "state")? != "OPEN" {
                    return Ok(None);
                }
                if requested && config.review_source == ReviewSource::Repositories {
                    if pr["isDraft"].as_bool().ok_or(INVALID)?
                        || prefixes.iter().any(|prefix| {
                            pr["title"]
                                .as_str()
                                .is_some_and(|title| title.starts_with(prefix))
                        })
                    {
                        return Ok(None);
                    }
                    let created = parse_timestamp(required(&pr, "createdAt")?).ok_or(INVALID)?;
                    if now.saturating_sub(created) / 86400 > config.review_max_age_days {
                        return Ok(None);
                    }
                }
                required(&pr, "url").map(|url| Some(url.to_ascii_lowercase()))
            })();
            match checked {
                Ok(Some(url)) => {
                    if requested {
                        review_urls.insert(url.clone());
                        if config.review_source == ReviewSource::Repositories {
                            pr["reviewDecision"] = unique
                                .get(&url)
                                .map(|(_, known): &(String, Value)| known["reviewDecision"].clone())
                                .unwrap_or(Value::Null);
                            pr["mergeStateStatus"] = unique
                                .get(&url)
                                .map(|(_, known): &(String, Value)| {
                                    known["mergeStateStatus"].clone()
                                })
                                .unwrap_or_else(|| Value::String("UNKNOWN".into()));
                        }
                    }
                    unique.insert(url, (scope.clone(), pr));
                }
                Ok(None) => {}
                Err(message) => result.errors.push(FetchError {
                    scope: scope.clone(),
                    message,
                }),
            }
        }
    }
    let ignored: Vec<_> = config
        .ignore
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    for (url, (scope, pr)) in unique {
        match classify(&pr, &ignored, review_urls.contains(&url)) {
            Ok(pr) => result.prs.push(pr),
            Err(message) => result.errors.push(FetchError { scope, message }),
        }
    }
    result.errors.sort_by(|a, b| {
        a.scope
            .cmp(&b.scope)
            .then_with(|| a.message.cmp(&b.message))
    });
    result
        .errors
        .dedup_by(|a, b| a.scope == b.scope && a.message == b.message);
    if successes == 0 {
        return Err(result
            .errors
            .iter()
            .map(|error| format!("{}: {}", error.scope, error.message))
            .collect::<Vec<_>>()
            .join("\n"));
    }
    result.prs.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.url.cmp(&b.url))
    });
    Ok(result)
}

pub use crate::remote_runs::created_at_seconds as parse_timestamp;

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;
    use std::sync::Mutex;

    fn pr(number: u64) -> Value {
        json!({"number":number,"title":"Improve search","url":format!("https://github.com/acme/app/pull/{number}"),"state":"OPEN","isDraft":false,"updatedAt":"2026-10-29T10:00:00Z","mergeable":"MERGEABLE","reviewDecision":"APPROVED","mergeStateStatus":"CLEAN","repository":{"nameWithOwner":"acme/app"},"author":{"__typename":"User"},"commits":{"nodes":[{"commit":{"statusCheckRollup":{"contexts":{"pageInfo":{"hasNextPage":false},"nodes":[]}}}}]},"reviewThreads":{"pageInfo":{"hasNextPage":false},"nodes":[]}})
    }

    #[test]
    fn review_and_draft_precedence_preserve_actionable_blockers() {
        let mut value = pr(1);
        value["mergeable"] = json!("CONFLICTING");
        assert_eq!(classify(&value, &[], false).unwrap().lane, Lane::Me);
        let review = classify(&value, &[], true).unwrap();
        assert_eq!(review.lane, Lane::Review);
        assert_eq!(review.blockers[0].label, "Merge conflict");
        value["isDraft"] = json!(true);
        assert_eq!(classify(&value, &[], true).unwrap().lane, Lane::Draft);
    }

    #[test]
    fn incomplete_checks_threads_and_unknown_merge_state_are_not_ready() {
        for path in ["checks", "threads", "merge"] {
            let mut value = pr(1);
            match path {
                "checks" => {
                    value["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["pageInfo"]
                        ["hasNextPage"] = json!(true)
                }
                "threads" => value["reviewThreads"]["pageInfo"]["hasNextPage"] = json!(true),
                _ => value["mergeStateStatus"] = json!("UNKNOWN"),
            }
            assert_eq!(classify(&value, &[], false).unwrap().lane, Lane::Others);
        }
        let mut value = pr(1);
        value.as_object_mut().unwrap().remove("reviewThreads");
        assert!(classify(&value, &[], false).is_err());
        assert_eq!(classify(&pr(1), &[], false).unwrap().lane, Lane::Ready);
    }

    #[test]
    fn ignored_checks_and_safe_action_links() {
        let mut value = pr(1);
        value["commits"]["nodes"][0]["commit"]["statusCheckRollup"]["contexts"]["nodes"] = json!([
            {"__typename":"CheckRun","name":"optional CI","status":"COMPLETED","conclusion":"FAILURE","detailsUrl":"https://ci.example/run/1"},
            {"__typename":"StatusContext","context":"tests","state":"FAILURE","targetUrl":"javascript:alert(1)"}
        ]);
        let result = classify(&value, &["optional"], false).unwrap();
        assert_eq!(result.lane, Lane::Me);
        assert_eq!(result.blockers.len(), 1);
        assert_eq!(
            result.failed_check_url.as_deref(),
            Some("https://github.com/acme/app/pull/1/checks")
        );
        let result = classify(&value, &["optional", "tests"], false).unwrap();
        assert_eq!(result.lane, Lane::Ready);
        value["reviewThreads"]["nodes"] = json!([{"isResolved":false,"comments":{"nodes":[{"url":"https://github.com/acme/app/pull/1#discussion_r1"}]}}]);
        assert_eq!(
            classify(&value, &["optional", "tests"], false)
                .unwrap()
                .feedback_url
                .as_deref(),
            Some("https://github.com/acme/app/pull/1#discussion_r1")
        );
    }

    struct FakeGh {
        outputs: Mutex<std::collections::VecDeque<String>>,
        calls: Mutex<Vec<Vec<String>>>,
    }
    impl FakeGh {
        fn new(outputs: Vec<Value>) -> Self {
            Self {
                outputs: Mutex::new(outputs.into_iter().map(|v| v.to_string()).collect()),
                calls: Default::default(),
            }
        }
    }
    impl Gh for FakeGh {
        fn run(&self, args: &[String]) -> Result<String, String> {
            self.calls.lock().unwrap().push(args.to_vec());
            self.outputs
                .lock()
                .unwrap()
                .pop_front()
                .ok_or_else(|| "Unexpected request".into())
        }
    }
    fn page(nodes: Vec<Value>) -> Value {
        json!({"data":{"search":{"issueCount":nodes.len(),"nodes":nodes,"pageInfo":{"hasNextPage":false,"endCursor":null}}}})
    }

    struct RoutedGh {
        replies: Vec<(String, Result<Value, String>)>,
        calls: Mutex<Vec<Vec<String>>>,
        active: std::sync::atomic::AtomicUsize,
        peak: std::sync::atomic::AtomicUsize,
        reverse: bool,
    }
    impl RoutedGh {
        fn new(replies: Vec<(&str, Result<Value, String>)>) -> Self {
            Self {
                replies: replies
                    .into_iter()
                    .map(|(key, reply)| (key.into(), reply))
                    .collect(),
                calls: Default::default(),
                active: Default::default(),
                peak: Default::default(),
                reverse: false,
            }
        }
    }
    impl Gh for RoutedGh {
        fn run(&self, args: &[String]) -> Result<String, String> {
            use std::sync::atomic::Ordering::SeqCst;
            self.calls.lock().unwrap().push(args.to_vec());
            let active = self.active.fetch_add(1, SeqCst) + 1;
            self.peak.fetch_max(active, SeqCst);
            let query = args
                .iter()
                .find_map(|arg| arg.strip_prefix("q="))
                .unwrap_or("");
            let reply = self.replies.iter().position(|(key, _)| query.contains(key));
            let index = reply.unwrap_or(0);
            let delay = if self.reverse {
                self.replies.len() - index
            } else {
                index + 1
            };
            std::thread::sleep(std::time::Duration::from_millis(delay as u64 * 10));
            self.active.fetch_sub(1, SeqCst);
            reply
                .map(|index| self.replies[index].1.clone().map(|value| value.to_string()))
                .unwrap_or_else(|| Err("Unexpected request".into()))
        }
    }

    #[test]
    fn concurrent_fetch_is_deterministic_and_preserves_lanes() {
        let mut snapshots = Vec::new();
        for reverse in [false, true] {
            let mut gh = RoutedGh::new(vec![
                ("assignee:@me", Ok(page(vec![pr(1)]))),
                ("author:@me", Ok(page(vec![pr(1), pr(2)]))),
                ("user-review-requested:@me", Ok(page(vec![pr(1)]))),
            ]);
            gh.reverse = reverse;
            let result = fetch(
                &gh,
                &Config {
                    keyword: "my\"keyword".into(),
                    extra: "repo:acme/app".into(),
                    ..Default::default()
                },
            )
            .unwrap();
            assert_eq!(result.prs.len(), 2);
            assert_eq!(result.prs[0].lane, Lane::Review);
            assert_eq!(result.prs[1].lane, Lane::Ready);
            assert!(gh.peak.load(std::sync::atomic::Ordering::SeqCst) > 1);
            snapshots.push(
                result
                    .prs
                    .iter()
                    .map(|pr| (pr.url.clone(), pr.lane))
                    .collect::<Vec<_>>(),
            );
            let calls = gh.calls.lock().unwrap();
            assert!(
                calls
                    .iter()
                    .flatten()
                    .any(|arg| arg.contains("\"mykeyword\" in:title")
                        && arg.contains("repo:acme/app"))
            );
        }
        assert_eq!(snapshots[0], snapshots[1]);
    }

    #[test]
    fn fetch_rejects_total_failure_instead_of_clearing_the_dashboard() {
        let failed = json!({"data":{"search":{"nodes":[]}},"errors":[{"message":"denied"}]});
        let gh = RoutedGh::new(vec![("q", Ok(failed))]);
        assert!(fetch(&gh, &Config::default()).is_err());
    }

    #[test]
    fn search_paginates_and_rejects_stuck_cursors() {
        let mut first = page(vec![pr(1)]);
        first["data"]["search"]["pageInfo"] = json!({"hasNextPage":true,"endCursor":"next"});
        let gh = FakeGh::new(vec![first.clone(), page(vec![pr(2)])]);
        assert_eq!(
            search(&gh, &Config::default(), "author:@me").unwrap().len(),
            2
        );
        assert!(gh.calls.lock().unwrap()[1].contains(&"cursor=next".into()));
        assert!(
            gh.calls.lock().unwrap()[0]
                .iter()
                .any(|arg| arg.contains("type:ISSUE,first:10"))
        );
        let gh = FakeGh::new(vec![first.clone(), first]);
        assert!(search(&gh, &Config::default(), "author:@me").is_err());
    }

    #[test]
    fn repository_reviews_filter_on_server_and_ignore_owned_filters() {
        let mut requested = pr(3);
        requested["createdAt"] = json!("2026-10-29T00:00:00Z");
        let mut draft = requested.clone();
        draft["number"] = json!(4);
        draft["isDraft"] = json!(true);
        let mut excluded = requested.clone();
        excluded["number"] = json!(5);
        excluded["title"] = json!("Skip: generated");
        let mut old = requested.clone();
        old["number"] = json!(6);
        old["createdAt"] = json!("2020-01-01T00:00:00Z");
        let gh = RoutedGh::new(vec![
            ("assignee:@me", Ok(page(vec![]))),
            (" author:@me", Ok(page(vec![]))),
            (
                "user-review-requested:@me",
                Ok(page(vec![requested.clone(), draft, excluded, old])),
            ),
            (
                "team-review-requested:acme/platform",
                Ok(page(vec![requested])),
            ),
        ]);
        let config = Config {
            review_source: ReviewSource::Repositories,
            review_repos: "Acme/app".into(),
            review_teams: "acme/platform, other/platform".into(),
            review_exclude_prefixes: "Skip:".into(),
            keyword: "does not match".into(),
            ..Default::default()
        };
        let result = fetch_at(
            &gh,
            &config,
            parse_timestamp("2026-10-29T10:00:00Z").unwrap(),
        )
        .unwrap();
        assert_eq!(result.prs.len(), 1);
        assert_eq!(result.prs[0].lane, Lane::Review);
        assert!(result.errors.is_empty());
        let calls = gh.calls.lock().unwrap();
        assert_eq!(calls.len(), 4, "full PR fields eliminate per-PR requests");
        for query in calls
            .iter()
            .flatten()
            .filter_map(|arg| arg.strip_prefix("q="))
            .filter(|q| q.contains("review-requested:"))
        {
            assert!(
                query.contains("repo:acme/app")
                    && query.contains("draft:false")
                    && query.contains("-author:@me")
                    && query.contains("created:>=2026-08-29")
            );
            assert!(!query.contains("does not match") && !query.contains("other/platform"));
        }
        assert!(
            calls
                .iter()
                .flatten()
                .any(|arg| arg.contains("reviewThreads(first: 100)"))
        );
    }

    #[test]
    fn failing_repository_keeps_successes_and_identifies_failed_scope() {
        let mut requested = pr(3);
        requested["createdAt"] = json!("2026-10-29T00:00:00Z");
        let gh = RoutedGh::new(vec![
            ("assignee:@me", Ok(page(vec![pr(1)]))),
            (" author:@me", Ok(page(vec![]))),
            ("repo:acme/app", Ok(page(vec![requested]))),
            ("repo:acme/broken", Err("network SECRET".into())),
        ]);
        let result = fetch_at(
            &gh,
            &Config {
                review_source: ReviewSource::Repositories,
                review_repos: "acme/app,acme/broken".into(),
                ..Default::default()
            },
            parse_timestamp("2026-10-29T10:00:00Z").unwrap(),
        )
        .unwrap();
        assert_eq!(result.prs.len(), 2);
        assert_eq!(result.prs[0].lane, Lane::Ready);
        assert_eq!(result.prs[1].lane, Lane::Review);
        assert_eq!(result.errors.len(), 1);
        assert_eq!(result.errors[0].scope, "acme/broken");
        assert!(!result.errors[0].message.contains("SECRET"));
    }

    #[test]
    fn repository_reviews_reuse_known_readiness_and_mark_unknown_readiness() {
        let mut known = pr(1);
        known["mergeStateStatus"] = json!("BEHIND");
        known["reviewDecision"] = json!("CHANGES_REQUESTED");
        let mut first = pr(1);
        first.as_object_mut().unwrap().remove("mergeStateStatus");
        first.as_object_mut().unwrap().remove("reviewDecision");
        first["createdAt"] = json!("2026-10-29T00:00:00Z");
        let mut second = first.clone();
        second["number"] = json!(2);
        second["url"] = json!("https://github.com/acme/app/pull/2");
        let gh = RoutedGh::new(vec![
            ("assignee:@me", Ok(page(vec![known]))),
            (" author:@me", Ok(page(vec![]))),
            ("user-review-requested:@me", Ok(page(vec![first, second]))),
        ]);
        let result = fetch_at(
            &gh,
            &Config {
                review_source: ReviewSource::Repositories,
                review_repos: "acme/app".into(),
                ..Default::default()
            },
            parse_timestamp("2026-10-29T10:00:00Z").unwrap(),
        )
        .unwrap();
        assert_eq!(result.prs.len(), 2);
        assert!(result.errors.is_empty());
        assert_eq!(result.prs[0].lane, Lane::Review);
        assert_eq!(result.prs[0].blockers[0].label, "Changes requested");
        assert_eq!(result.prs[0].blockers[1].label, "Branch is behind its base");
        assert_eq!(result.prs[1].lane, Lane::Review);
        assert_eq!(
            result.prs[1].blockers[0].label,
            "Merge readiness unconfirmed"
        );
        let calls = gh.calls.lock().unwrap();
        let repository_call = calls
            .iter()
            .find(|args| args.iter().any(|arg| arg.contains("q=is:open is:pr repo:")))
            .unwrap();
        assert!(
            !repository_call
                .iter()
                .any(|arg| arg.contains("mergeStateStatus"))
        );
    }

    #[test]
    fn cancellation_releases_active_requests_and_skips_queued_repositories() {
        use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
        struct BlockingGh {
            cancelled: AtomicBool,
            calls: AtomicUsize,
            started: std::sync::mpsc::Sender<()>,
        }
        impl Gh for BlockingGh {
            fn cancelled(&self) -> bool {
                self.cancelled.load(Ordering::Acquire)
            }
            fn run(&self, _: &[String]) -> Result<String, String> {
                self.calls.fetch_add(1, Ordering::AcqRel);
                self.started.send(()).unwrap();
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(2);
                while !self.cancelled() && std::time::Instant::now() < deadline {
                    std::thread::sleep(std::time::Duration::from_millis(1));
                }
                Err("Cancelled".into())
            }
        }
        let (sender, receiver) = std::sync::mpsc::channel();
        let gh = BlockingGh {
            cancelled: AtomicBool::new(false),
            calls: AtomicUsize::new(0),
            started: sender,
        };
        let config = Config {
            review_source: ReviewSource::Repositories,
            review_repos: (0..12)
                .map(|i| format!("acme/repo{i}"))
                .collect::<Vec<_>>()
                .join(","),
            ..Default::default()
        };
        std::thread::scope(|scope| {
            let refresh = scope.spawn(|| fetch(&gh, &config));
            for _ in 0..4 {
                receiver
                    .recv_timeout(std::time::Duration::from_secs(2))
                    .unwrap();
            }
            gh.cancelled.store(true, Ordering::Release);
            assert_eq!(
                refresh.join().unwrap().err().as_deref(),
                Some("Refresh cancelled")
            );
        });
        assert_eq!(gh.calls.load(Ordering::Acquire), 4);
    }

    #[test]
    fn server_age_filter_keeps_the_entire_last_eligible_day() {
        let now = parse_timestamp("2026-10-29T10:00:00Z").unwrap();
        assert_eq!(review_since(now, 20), "2026-10-08");
        assert_eq!(review_since(now, 0), "2026-10-28");
        assert_eq!(
            review_since(parse_timestamp("2024-03-01T00:00:00Z").unwrap(), 0),
            "2024-02-29"
        );
        assert_eq!(review_since(0, 3650), "1970-01-01");
    }

    #[test]
    fn settings_validate_scope_and_refresh_interval() {
        assert!(Config::default().validate().is_ok());
        assert!(
            Config {
                minutes: 0,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        assert!(
            Config {
                review_source: ReviewSource::Repositories,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        for idle_days in [0, 91] {
            assert!(
                Config {
                    idle_days,
                    ..Default::default()
                }
                .validate()
                .is_err()
            );
        }
        for idle_days in [1, 90] {
            assert!(
                Config {
                    idle_days,
                    ..Default::default()
                }
                .validate()
                .is_ok()
            );
        }
        for review_max_age_days in [0, 3650] {
            assert!(
                Config {
                    review_max_age_days,
                    ..Default::default()
                }
                .validate()
                .is_ok()
            );
        }
        assert!(
            Config {
                review_max_age_days: 3651,
                ..Default::default()
            }
            .validate()
            .is_err()
        );
        for scope in [
            "acme/..",
            "acme/repo/extra",
            "-acme/repo",
            "acme/repo?query",
        ] {
            assert!(scope_list(scope).is_err());
        }
        assert_eq!(scope_list("Acme/App, acme/app").unwrap(), vec!["acme/app"]);
    }
}
