use crate::remote_runs::Gh;
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
number title url state isDraft updatedAt mergeable reviewDecision mergeStateStatus
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
        format!("query={query}"),
    ];
    for (name, value) in variables {
        args.extend(["-f".into(), format!("{name}={value}")]);
    }
    let response: Value = serde_json::from_str(&gh.run(&args)?).map_err(|_| INVALID)?;
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
    let mut cursor = String::new();
    let mut results = Vec::new();
    for _ in 0..20 {
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
            "query($q:String!{cursor_parameter}) {{ search(query:$q,type:ISSUE,first:50{after}) {{ issueCount pageInfo {{ hasNextPage endCursor }} nodes {{ ... on PullRequest {{ {PR_FIELDS} }} }} }} }}"
        );
        let mut variables = vec![("q", q.clone())];
        if !cursor.is_empty() {
            variables.push(("cursor", cursor.clone()));
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

fn repository_targets_at(
    gh: &impl Gh,
    config: &Config,
    now: u64,
) -> Result<Vec<(String, u64)>, String> {
    let username = gh
        .run(&["api".into(), "user".into(), "--jq".into(), ".login".into()])?
        .trim()
        .to_ascii_lowercase();
    if username.is_empty() {
        return Err("Could not determine the GitHub CLI account; run gh auth status".into());
    }
    let teams = scope_list(&config.review_teams)?;
    let prefixes: Vec<_> = config
        .review_exclude_prefixes
        .split(',')
        .map(str::trim)
        .filter(|s| !s.is_empty())
        .collect();
    let mut targets = Vec::new();
    for repo in scope_list(&config.review_repos)? {
        let output = gh.run(&[
            "api".into(),
            format!("repos/{repo}/pulls?state=open&per_page=100"),
            "--paginate".into(),
            "--slurp".into(),
        ])?;
        let pages: Vec<Vec<Value>> = serde_json::from_str(&output).map_err(|_| INVALID)?;
        for pr in pages.iter().flatten() {
            if pr["draft"].as_bool().ok_or(INVALID)?
                || required(&pr["user"], "login")?.eq_ignore_ascii_case(&username)
            {
                continue;
            }
            let title = required(pr, "title")?;
            if prefixes.iter().any(|prefix| title.starts_with(prefix)) {
                continue;
            }
            let created = parse_timestamp(required(pr, "created_at")?).ok_or(INVALID)?;
            if now.saturating_sub(created) / 86400 > config.review_max_age_days {
                continue;
            }
            let reviewers = pr["requested_reviewers"].as_array().ok_or(INVALID)?;
            let requested_teams = pr["requested_teams"].as_array().ok_or(INVALID)?;
            let owner = repo.split('/').next().unwrap_or_default();
            let direct = reviewers.iter().any(|r| {
                r["login"]
                    .as_str()
                    .is_some_and(|name| name.eq_ignore_ascii_case(&username))
            });
            let team = requested_teams.iter().any(|t| {
                t["slug"].as_str().is_some_and(|slug| {
                    teams.contains(&format!("{owner}/{}", slug.to_ascii_lowercase()))
                })
            });
            if direct || team {
                targets.push((repo.clone(), pr["number"].as_u64().ok_or(INVALID)?));
            }
        }
    }
    Ok(targets)
}

pub fn fetch(gh: &impl Gh, config: &Config) -> Result<Vec<PullRequest>, String> {
    let now = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs();
    fetch_at(gh, config, now)
}

fn fetch_at(gh: &impl Gh, config: &Config, now: u64) -> Result<Vec<PullRequest>, String> {
    config.validate()?;
    let assigned = search(gh, config, "assignee:@me")?;
    let authored = search(gh, config, "author:@me")?;
    let requested = match config.review_source {
        ReviewSource::Github => search(gh, config, "user-review-requested:@me")?,
        ReviewSource::Repositories => {
            let mut requested = Vec::new();
            for (repo, number) in repository_targets_at(gh, config, now)? {
                if let Some(known) = assigned.iter().chain(&authored).find(|pr| {
                    pr["number"] == number
                        && pr["repository"]["nameWithOwner"]
                            .as_str()
                            .is_some_and(|name| name.eq_ignore_ascii_case(&repo))
                }) {
                    requested.push(known.clone());
                } else {
                    let (owner, name) = repo.split_once('/').ok_or(INVALID)?;
                    let query = format!(
                        "query($owner:String!,$repo:String!) {{ repository(owner:$owner,name:$repo) {{ pullRequest(number:{number}) {{ {PR_FIELDS} }} }} }}"
                    );
                    let data =
                        graphql(gh, query, &[("owner", owner.into()), ("repo", name.into())])?;
                    let pr = &data["repository"]["pullRequest"];
                    if required(pr, "state")? == "OPEN"
                        && !pr["isDraft"].as_bool().ok_or(INVALID)?
                    {
                        requested.push(pr.clone());
                    }
                }
            }
            requested
        }
    };
    let review_urls: BTreeSet<_> = requested
        .iter()
        .map(|pr| required(pr, "url").map(str::to_ascii_lowercase))
        .collect::<Result<_, _>>()?;
    let mut unique = BTreeMap::new();
    for pr in assigned.into_iter().chain(authored).chain(requested) {
        if required(&pr, "state")? == "OPEN" {
            unique.insert(required(&pr, "url")?.to_ascii_lowercase(), pr);
        }
    }
    let ignored: Vec<_> = config
        .ignore
        .split(',')
        .map(str::trim)
        .filter(|part| !part.is_empty())
        .collect();
    let mut prs = unique
        .into_iter()
        .map(|(url, pr)| classify(&pr, &ignored, review_urls.contains(&url)))
        .collect::<Result<Vec<_>, _>>()?;
    prs.sort_by(|a, b| {
        b.updated_at
            .cmp(&a.updated_at)
            .then_with(|| a.url.cmp(&b.url))
    });
    Ok(prs)
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

    #[test]
    fn fetch_deduplicates_assignments_and_gives_direct_reviews_precedence() {
        let gh = FakeGh::new(vec![
            page(vec![pr(1)]),
            page(vec![pr(1), pr(2)]),
            page(vec![pr(1)]),
        ]);
        let config = Config {
            keyword: "my\"keyword".into(),
            extra: "repo:acme/app".into(),
            ..Default::default()
        };
        let result = fetch(&gh, &config).unwrap();
        assert_eq!(result.len(), 2);
        assert_eq!(result[0].lane, Lane::Review);
        assert_eq!(result[1].lane, Lane::Ready);
        let calls = gh.calls.lock().unwrap();
        assert!(
            calls[2]
                .iter()
                .any(|arg| arg.contains("user-review-requested:@me"))
        );
        assert!(
            calls[0]
                .iter()
                .any(|arg| arg.contains("\"mykeyword\" in:title") && arg.contains("repo:acme/app"))
        );
    }

    #[test]
    fn fetch_rejects_graphql_errors_instead_of_clearing_the_dashboard() {
        let gh = FakeGh::new(vec![
            json!({"data":{"search":{"nodes":[]}},"errors":[{"message":"denied"}]}),
        ]);
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
                .any(|arg| arg.contains("type:ISSUE,first:50"))
        );
        let gh = FakeGh::new(vec![first.clone(), first]);
        assert!(search(&gh, &Config::default(), "author:@me").is_err());
    }

    #[test]
    fn repository_reviews_match_organization_teams_and_ignore_owned_filters() {
        let mut raw = json!({"number":3,"title":"Review this","created_at":"2026-10-29T00:00:00Z","draft":false,"user":{"login":"someone"},"requested_reviewers":[],"requested_teams":[{"slug":"platform"}]});
        let mut requested = pr(3);
        requested["repository"]["nameWithOwner"] = json!("acme/app");
        let mut own = raw.clone();
        own["number"] = json!(4);
        own["user"]["login"] = json!("myself");
        let mut draft = raw.clone();
        draft["number"] = json!(5);
        draft["draft"] = json!(true);
        let mut excluded = raw.clone();
        excluded["number"] = json!(6);
        excluded["title"] = json!("Skip: generated");
        raw["number"] = json!(7);
        raw["created_at"] = json!("2020-01-01T00:00:00Z");
        let gh = FakeGh::new(vec![
            page(vec![]),
            page(vec![]),
            json!("placeholder"),
            json!([[
                json!({"number":3,"title":"Review this","created_at":"2026-10-29T00:00:00Z","draft":false,"user":{"login":"someone"},"requested_reviewers":[],"requested_teams":[{"slug":"platform"}]}),
                own,
                draft,
                excluded,
                raw
            ]]),
            json!({"data":{"repository":{"pullRequest":requested}}}),
        ]);
        gh.outputs.lock().unwrap()[2] = "myself\n".into();
        let config = Config {
            review_source: ReviewSource::Repositories,
            review_repos: "Acme/app".into(),
            review_teams: "acme/platform".into(),
            review_exclude_prefixes: "Skip:".into(),
            keyword: "does not match".into(),
            review_max_age_days: 60,
            ..Default::default()
        };
        let result = fetch_at(
            &gh,
            &config,
            parse_timestamp("2026-10-29T10:00:00Z").unwrap(),
        )
        .unwrap();
        assert_eq!(result.len(), 1);
        assert_eq!(result[0].lane, Lane::Review);
        assert!(gh.calls.lock().unwrap()[3].contains(&"--paginate".into()));
        let gh = FakeGh::new(vec![
            json!("placeholder"),
            json!([[{"number":1,"title":"Review","created_at":"2026-10-29T00:00:00Z","draft":false,"user":{"login":"someone"},"requested_reviewers":[],"requested_teams":[{"slug":"platform"}]}]]),
        ]);
        gh.outputs.lock().unwrap()[0] = "myself".into();
        let config = Config {
            review_teams: "other/platform".into(),
            ..config
        };
        assert!(
            repository_targets_at(
                &gh,
                &config,
                parse_timestamp("2026-10-29T10:00:00Z").unwrap()
            )
            .unwrap()
            .is_empty()
        );
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
