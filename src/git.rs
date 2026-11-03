use std::{
    io::Read,
    path::{Path, PathBuf},
    process::{Command, Stdio},
    sync::mpsc::{Receiver, channel},
    time::{Duration, Instant},
};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub name: String,
    pub remote: bool,
}

#[derive(Debug)]
pub struct Repository {
    pub root: PathBuf,
    pub current: Option<String>,
    pub default_branch: Option<String>,
    pub branches: Vec<Branch>,
}

#[derive(Clone)]
pub enum Action {
    Refresh,
    Fetch,
    Main,
    Pull,
    Switch(Branch),
}

pub fn load(directory: &Path) -> Result<Repository, String> {
    let root = run(directory, &["rev-parse", "--show-toplevel"])?;
    let root = PathBuf::from(root.trim_end_matches('\n'));
    let current = run(&root, &["symbolic-ref", "--quiet", "--short", "HEAD"])
        .ok()
        .map(|value| value.trim().to_owned());
    let refs = run(
        &root,
        &[
            "for-each-ref",
            "--format=%(refname)%09%(symref)",
            "refs/heads/",
            "refs/remotes/origin/",
        ],
    )?;
    let mut branches = Vec::new();
    let mut default_branch = None;
    for line in refs.lines() {
        let Some((name, target)) = line.split_once('\t') else {
            continue;
        };
        if name == "refs/remotes/origin/HEAD" {
            default_branch = target
                .strip_prefix("refs/remotes/origin/")
                .map(str::to_owned);
        }
        if !target.is_empty() || name == "refs/remotes/origin/HEAD" {
            continue;
        }
        if let Some(name) = name.strip_prefix("refs/heads/") {
            branches.push(Branch {
                name: name.into(),
                remote: false,
            });
        } else if let Some(name) = name.strip_prefix("refs/remotes/origin/") {
            branches.push(Branch {
                name: name.into(),
                remote: true,
            });
        }
    }
    default_branch = default_branch
        .filter(|name| branches.iter().any(|b| b.name == *name))
        .or_else(|| {
            ["main", "trunk", "mainline", "default", "stable", "master"]
                .into_iter()
                .find(|name| branches.iter().any(|b| b.name == *name))
                .map(str::to_owned)
        });
    Ok(Repository {
        root,
        current,
        default_branch,
        branches,
    })
}

pub fn execute(directory: &Path, action: &Action) -> Result<Repository, String> {
    let repo = load(directory)?;
    match action {
        Action::Refresh => return Ok(repo),
        Action::Fetch => {
            run(&repo.root, &["fetch", "origin"])?;
        }
        Action::Pull => {
            run(
                &repo.root,
                &[
                    "-c",
                    "merge.autoStash=false",
                    "-c",
                    "rebase.autoStash=false",
                    "-c",
                    "pull.autoStash=false",
                    "pull",
                    "--ff-only",
                    "--no-rebase",
                ],
            )?;
        }
        Action::Main => {
            let name = repo.default_branch.as_ref().ok_or("Cannot find the default branch. Fetch origin and refresh, or choose a branch below.")?;
            let branch = repo.branches.iter().find(|b| b.name == *name).unwrap();
            switch(&repo, branch)?;
        }
        Action::Switch(branch) => switch(&repo, branch)?,
    }
    load(&repo.root)
}

fn switch(repo: &Repository, branch: &Branch) -> Result<(), String> {
    if !repo.branches.contains(branch) {
        return Err("This branch is no longer available. Refresh the branch list.".into());
    }
    if branch.remote
        && !repo
            .branches
            .iter()
            .any(|b| !b.remote && b.name == branch.name)
    {
        run(
            &repo.root,
            &[
                "switch",
                "--track",
                "-c",
                &branch.name,
                "--",
                &format!("refs/remotes/origin/{}", branch.name),
            ],
        )?;
    } else {
        run(&repo.root, &["switch", "--no-guess", "--", &branch.name])?;
    }
    Ok(())
}

fn capture(mut stream: impl Read + Send + 'static) -> Receiver<Result<Vec<u8>, String>> {
    let (sender, receiver) = channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        let mut buffer = [0; 8192];
        let result = loop {
            match stream.read(&mut buffer) {
                Ok(0) => break Ok(bytes),
                Ok(count) => {
                    let remaining = (4 * 1024 * 1024_usize).saturating_sub(bytes.len());
                    bytes.extend_from_slice(&buffer[..count.min(remaining)]);
                }
                Err(error) => break Err(error.to_string()),
            }
        };
        let _ = sender.send(result);
    });
    receiver
}

fn run(directory: &Path, args: &[&str]) -> Result<String, String> {
    let mut command = Command::new("git");
    command
        .args(args)
        .current_dir(directory)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_PAGER", "cat")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped());
    for variable in [
        "GIT_DIR",
        "GIT_WORK_TREE",
        "GIT_INDEX_FILE",
        "GIT_COMMON_DIR",
    ] {
        command.env_remove(variable);
    }
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    let mut child = command
        .spawn()
        .map_err(|error| format!("Cannot start Git: {error}"))?;
    let stdout = capture(child.stdout.take().unwrap());
    let stderr = capture(child.stderr.take().unwrap());
    let deadline = Instant::now() + Duration::from_secs(60);
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break status,
            Ok(None) if Instant::now() < deadline => std::thread::sleep(Duration::from_millis(20)),
            result => {
                #[cfg(unix)]
                unsafe {
                    libc::kill(-(child.id() as i32), libc::SIGKILL);
                }
                let _ = child.kill();
                let _ = child.wait();
                return Err(match result {
                    Err(error) => format!("Cannot check Git: {error}"),
                    _ => "Git timed out after 60 seconds. Check connectivity and authentication in a shell, then refresh to inspect the repository.".into(),
                });
            }
        }
    };
    let read = |receiver: Receiver<Result<Vec<u8>, String>>| {
        receiver
            .recv_timeout(Duration::from_secs(1))
            .map_err(|_| "Git output did not close".to_string())?
    };
    let stdout = read(stdout)?;
    let stderr = read(stderr)?;
    if !status.success() {
        let detail = String::from_utf8_lossy(&stderr);
        return Err(if detail.trim().is_empty() {
            format!("Git failed ({status})")
        } else {
            detail.trim().to_owned()
        });
    }
    String::from_utf8(stdout)
        .map_err(|_| "Git returned a name or path that is not valid UTF-8".into())
}

#[cfg(test)]
pub(crate) mod tests {
    use super::*;
    use std::process::Command;

    pub(crate) fn git(path: &Path, args: &[&str]) -> String {
        let output = Command::new("git")
            .args([
                "-c",
                "user.name=Tessera Test",
                "-c",
                "user.email=test@example.invalid",
            ])
            .args(args)
            .current_dir(path)
            .output()
            .unwrap();
        assert!(
            output.status.success(),
            "{}",
            String::from_utf8_lossy(&output.stderr)
        );
        String::from_utf8(output.stdout).unwrap().trim().to_owned()
    }

    pub(crate) fn fixture() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let source = temp.path().join("source");
        let work = temp.path().join("work 'with spaces'");
        std::fs::create_dir(&source).unwrap();
        git(&source, &["init", "-b", "main"]);
        std::fs::write(source.join("file"), "initial\n").unwrap();
        git(&source, &["add", "."]);
        git(&source, &["commit", "-m", "initial"]);
        git(
            temp.path(),
            &["clone", "--bare", source.to_str().unwrap(), "origin.git"],
        );
        git(
            &source,
            &[
                "remote",
                "add",
                "origin",
                temp.path().join("origin.git").to_str().unwrap(),
            ],
        );
        git(
            temp.path(),
            &[
                "clone",
                temp.path().join("origin.git").to_str().unwrap(),
                work.to_str().unwrap(),
            ],
        );
        (temp, source, work)
    }

    #[test]
    fn loads_repository_from_nested_directory_and_excludes_remote_head() {
        let (_temp, _, work) = fixture();
        let nested = work.join("nested");
        std::fs::create_dir(&nested).unwrap();
        let repo = load(&nested).unwrap();
        assert_eq!(repo.root, work.canonicalize().unwrap());
        assert_eq!(repo.current.as_deref(), Some("main"));
        assert_eq!(repo.default_branch.as_deref(), Some("main"));
        assert_eq!(
            repo.branches,
            vec![
                Branch {
                    name: "main".into(),
                    remote: false
                },
                Branch {
                    name: "main".into(),
                    remote: true
                },
            ]
        );
    }

    #[test]
    fn fetch_discovers_remote_branch_and_switch_creates_tracking_branch() {
        let (_temp, source, work) = fixture();
        git(&source, &["checkout", "-b", "feature/search;$literal"]);
        git(&source, &["push", "origin", "HEAD"]);
        let repo = execute(&work, &Action::Fetch).unwrap();
        let branch = repo
            .branches
            .into_iter()
            .find(|b| b.name == "feature/search;$literal")
            .unwrap();
        assert!(branch.remote);
        let repo = execute(&work, &Action::Switch(branch)).unwrap();
        assert_eq!(repo.current.as_deref(), Some("feature/search;$literal"));
        assert_eq!(
            git(&work, &["rev-parse", "--abbrev-ref", "@{upstream}"]),
            "origin/feature/search;$literal"
        );
        assert_eq!(
            execute(&work, &Action::Main).unwrap().current.as_deref(),
            Some("main")
        );
    }

    #[test]
    fn pull_fast_forwards_but_refuses_divergence_and_preserves_work() {
        let (_temp, source, work) = fixture();
        std::fs::write(source.join("file"), "remote\n").unwrap();
        git(&source, &["commit", "-am", "remote"]);
        git(&source, &["push", "origin", "main"]);
        execute(&work, &Action::Pull).unwrap();
        assert_eq!(
            std::fs::read_to_string(work.join("file")).unwrap(),
            "remote\n"
        );
        std::fs::write(work.join("local"), "local\n").unwrap();
        git(&work, &["add", "."]);
        git(&work, &["commit", "-m", "local"]);
        let head = git(&work, &["rev-parse", "HEAD"]);
        std::fs::write(source.join("another"), "remote\n").unwrap();
        git(&source, &["add", "."]);
        git(&source, &["commit", "-m", "diverged"]);
        git(&source, &["push", "origin", "main"]);
        assert!(execute(&work, &Action::Pull).is_err());
        assert_eq!(git(&work, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            std::fs::read_to_string(work.join("local")).unwrap(),
            "local\n"
        );
    }

    #[test]
    fn pull_never_autostashes_dirty_work_even_when_enabled_in_config() {
        let (_temp, source, work) = fixture();
        git(&work, &["config", "pull.autoStash", "true"]);
        let head = git(&work, &["rev-parse", "HEAD"]);
        std::fs::write(work.join("file"), "unsaved\n").unwrap();
        std::fs::write(source.join("file"), "remote\n").unwrap();
        git(&source, &["commit", "-am", "remote"]);
        git(&source, &["push", "origin", "main"]);
        assert!(execute(&work, &Action::Pull).is_err());
        assert_eq!(git(&work, &["rev-parse", "HEAD"]), head);
        assert_eq!(
            std::fs::read_to_string(work.join("file")).unwrap(),
            "unsaved\n"
        );
        assert!(git(&work, &["diff", "--name-only", "--diff-filter=U"]).is_empty());
    }

    #[test]
    fn remote_checkout_uses_the_remote_ref_when_a_local_name_collides() {
        let (_temp, source, work) = fixture();
        git(&source, &["checkout", "-b", "topic"]);
        std::fs::write(source.join("file"), "topic\n").unwrap();
        git(&source, &["commit", "-am", "topic"]);
        git(&source, &["push", "origin", "topic"]);
        git(&work, &["branch", "origin/topic"]);
        execute(&work, &Action::Fetch).unwrap();
        execute(
            &work,
            &Action::Switch(Branch {
                name: "topic".into(),
                remote: true,
            }),
        )
        .unwrap();
        assert_eq!(
            std::fs::read_to_string(work.join("file")).unwrap(),
            "topic\n"
        );
        assert_eq!(
            git(&work, &["rev-parse", "--symbolic-full-name", "@{upstream}"]),
            "refs/remotes/origin/topic"
        );
    }

    #[test]
    fn switch_reports_dirty_checkout_failure_without_discarding_changes() {
        let (_temp, source, work) = fixture();
        git(&source, &["checkout", "-b", "feature"]);
        std::fs::write(source.join("file"), "feature\n").unwrap();
        git(&source, &["commit", "-am", "feature"]);
        git(&source, &["push", "origin", "feature"]);
        execute(&work, &Action::Fetch).unwrap();
        std::fs::write(work.join("file"), "unsaved\n").unwrap();
        assert!(
            execute(
                &work,
                &Action::Switch(Branch {
                    name: "feature".into(),
                    remote: true
                })
            )
            .is_err()
        );
        assert_eq!(git(&work, &["branch", "--show-current"]), "main");
        assert_eq!(
            std::fs::read_to_string(work.join("file")).unwrap(),
            "unsaved\n"
        );
    }
}
