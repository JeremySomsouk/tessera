use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct Revision {
    pub title: String,
    pub markdown: String,
    pub directory: String,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Launch {
    pub pane: Uuid,
    pub revision: usize,
    pub agent: String,
    #[serde(default)]
    pub exit_code: Option<u32>,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Proposal {
    pub base: usize,
    pub expected_draft: Revision,
    pub replacement: Revision,
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Specification {
    pub id: Uuid,
    pub draft: Revision,
    pub status: usize,
    pub revisions: Vec<Revision>,
    pub launches: Vec<Launch>,
    #[serde(default)]
    pub proposal: Option<Proposal>,
}
impl Specification {
    pub fn new(directory: String) -> Self {
        Self {
            id: Uuid::new_v4(),
            draft: Revision {
                title: "Untitled specification".into(),
                directory,
                ..Default::default()
            },
            status: 0,
            revisions: Vec::new(),
            launches: Vec::new(),
            proposal: None,
        }
    }
    pub fn begin_proposal(&mut self) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.proposal.is_none(),
            "Review or discard the existing proposal first"
        );
        let base = self.save_revision()?;
        self.proposal = Some(Proposal {
            base,
            expected_draft: self.draft.clone(),
            replacement: self.draft.clone(),
        });
        Ok(())
    }
    pub fn proposal_is_current(&self) -> bool {
        self.proposal.as_ref().is_some_and(|p| {
            self.revisions.len() == p.base
                && self.revisions.get(p.base.saturating_sub(1)) == Some(&p.expected_draft)
                && self.draft == p.expected_draft
        })
    }
    pub fn accept_proposal(
        &mut self,
        title: bool,
        markdown: bool,
        directory: bool,
    ) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.proposal_is_current(),
            "Proposal conflicts with newer edits; discard and prepare a new proposal"
        );
        anyhow::ensure!(
            title || markdown || directory,
            "Select at least one field to accept"
        );
        let p = self.proposal.as_ref().unwrap();
        let mut candidate = self.draft.clone();
        if title {
            candidate.title.clone_from(&p.replacement.title);
        }
        if markdown {
            candidate.markdown.clone_from(&p.replacement.markdown);
        }
        if directory {
            candidate.directory.clone_from(&p.replacement.directory);
        }
        anyhow::ensure!(
            !candidate.title.trim().is_empty() && !candidate.markdown.trim().is_empty(),
            "A title and specification are required"
        );
        anyhow::ensure!(
            candidate == self.draft || self.revisions.len() < 256,
            "Revision limit (256) reached"
        );
        self.draft = candidate;
        self.save_revision()?;
        self.proposal = None;
        Ok(())
    }
    pub fn restore_revision(&mut self, number: usize) -> anyhow::Result<()> {
        anyhow::ensure!(
            self.proposal.is_none(),
            "Review or discard the proposal before restoring"
        );
        let revision = self
            .revisions
            .get(number.wrapping_sub(1))
            .cloned()
            .ok_or_else(|| anyhow::anyhow!("Unknown revision"))?;
        // Preserve unsaved edits before replacing the draft, and reserve room for both snapshots.
        let unsaved = self.revisions.last() != Some(&self.draft);
        let required = usize::from(unsaved) + usize::from(revision != self.draft);
        anyhow::ensure!(
            self.revisions.len() + required <= 256,
            "Revision limit (256) reached"
        );
        self.save_revision()?;
        self.draft = revision;
        self.save_revision()?;
        Ok(())
    }
    pub fn save_revision(&mut self) -> anyhow::Result<usize> {
        anyhow::ensure!(
            !self.draft.title.trim().is_empty() && !self.draft.markdown.trim().is_empty(),
            "A title and specification are required"
        );
        if self.revisions.last().is_none_or(|r| {
            r.title != self.draft.title
                || r.markdown != self.draft.markdown
                || r.directory != self.draft.directory
        }) {
            anyhow::ensure!(self.revisions.len() < 256, "Revision limit (256) reached");
            self.revisions.push(self.draft.clone());
        }
        Ok(self.revisions.len())
    }
}
pub fn command(revision: &Revision, number: usize, codex: bool) -> anyhow::Result<String> {
    let prompt = format!(
        "Implement this specification (revision {number}).\n\n{}\n\n{}",
        revision.title, revision.markdown
    );
    anyhow::ensure!(
        prompt.len() <= 8000
            && !prompt
                .chars()
                .any(|c| c.is_control() && c != '\n' && c != '\t'),
        "Launch context must be at most 8000 bytes and contain no terminal control characters"
    );
    // A POSIX shell literal: spec text cannot become shell commands.
    let quoted = format!("'{}'", prompt.replace('\'', "'\"'\"'"));
    Ok(format!(
        "{} {}",
        if codex { "codex --no-daemon" } else { "claude" },
        quoted
    ))
}
#[cfg(test)]
mod tests {
    use super::*;
    fn ready() -> Specification {
        let mut s = Specification::new("/tmp".into());
        s.draft.markdown = "Original scope".into();
        s.save_revision().unwrap();
        s
    }
    #[test]
    fn proposal_round_trip_selective_acceptance_and_pinned_launch() {
        let mut s = ready();
        s.launches.push(Launch {
            pane: Uuid::new_v4(),
            revision: 1,
            agent: "Codex".into(),
            exit_code: None,
        });
        s.begin_proposal().unwrap();
        assert!(s.begin_proposal().is_err());
        let p = s.proposal.as_mut().unwrap();
        p.replacement.markdown = "Reviewed scope".into();
        p.replacement.title = "Proposed title".into();
        let mut s: Specification =
            serde_json::from_str(&serde_json::to_string(&s).unwrap()).unwrap();
        s.accept_proposal(false, true, false).unwrap();
        assert_eq!(s.draft.title, "Untitled specification");
        assert_eq!(s.draft.markdown, "Reviewed scope");
        assert_eq!(s.revisions[0].markdown, "Original scope");
        assert_eq!(s.launches[0].revision, 1);
        assert!(s.proposal.is_none());
    }
    #[test]
    fn conflicts_and_invalid_acceptance_are_non_destructive() {
        let mut s = ready();
        s.begin_proposal().unwrap();
        s.draft.markdown = "Concurrent edit".into();
        assert!(!s.proposal_is_current());
        assert!(s.accept_proposal(true, true, true).is_err());
        assert_eq!(s.draft.markdown, "Concurrent edit");
        s.draft.markdown = "Original scope".into();
        s.proposal.as_mut().unwrap().replacement.markdown.clear();
        assert!(s.accept_proposal(false, true, false).is_err());
        assert!(s.accept_proposal(false, false, false).is_err());
        assert_eq!(s.revisions.len(), 1);
        assert!(s.proposal.is_some());
    }
    #[test]
    fn restore_preserves_unsaved_work_and_capacity_failures() {
        let mut s = ready();
        s.draft.markdown = "Second scope".into();
        s.save_revision().unwrap();
        s.draft.markdown = "Unsaved work".into();
        s.restore_revision(1).unwrap();
        assert_eq!(s.revisions[2].markdown, "Unsaved work");
        assert_eq!(s.revisions[3].markdown, "Original scope");
        assert!(s.restore_revision(0).is_err());
        s.begin_proposal().unwrap();
        assert!(s.restore_revision(2).is_err());
        s.proposal = None;
        s.revisions.resize(256, s.draft.clone());
        s.draft.markdown = "Keep this".into();
        assert!(s.restore_revision(2).is_err());
        assert_eq!(s.draft.markdown, "Keep this");
        assert_eq!(s.revisions.len(), 256);
    }
    #[test]
    fn legacy_specification_without_proposal_loads() {
        let s = ready();
        let mut value = serde_json::to_value(s).unwrap();
        value.as_object_mut().unwrap().remove("proposal");
        let restored: Specification = serde_json::from_value(value).unwrap();
        assert!(restored.proposal.is_none());
    }
    #[test]
    fn revisions_are_immutable_and_deduplicated() {
        let mut spec = Specification::new("/tmp".into());
        assert!(spec.save_revision().is_err());
        spec.draft.markdown = "First scope".into();
        assert_eq!(spec.save_revision().unwrap(), 1);
        assert_eq!(spec.save_revision().unwrap(), 1);
        spec.draft.markdown = "New scope".into();
        assert_eq!(spec.save_revision().unwrap(), 2);
        assert_eq!(spec.revisions[0].markdown, "First scope");
        let restored: Specification =
            serde_json::from_str(&serde_json::to_string(&spec).unwrap()).unwrap();
        assert_eq!(restored.revisions.len(), 2);
    }
    #[test]
    fn shell_receives_exact_prompt_without_interpreting_specification() {
        let revision = Revision {
            title: "It's $(printf INJECTED)".into(),
            markdown: "`printf BAD`\nSecond line; echo BAD".into(),
            directory: "/tmp".into(),
        };
        let command = command(&revision, 3, false).unwrap();
        let output = std::process::Command::new("/bin/sh")
            .arg("-c")
            .arg(format!("claude() {{ printf '%s' \"$1\"; }}; {command}"))
            .output()
            .unwrap();
        assert!(output.status.success());
        assert_eq!(
            String::from_utf8(output.stdout).unwrap(),
            format!(
                "Implement this specification (revision 3).\n\n{}\n\n{}",
                revision.title, revision.markdown
            )
        );
    }
    #[test]
    fn launch_context_is_literal_and_bounded() {
        let r = Revision {
            title: "It's $(touch /tmp/no)".into(),
            markdown: "`false`\nScope".into(),
            directory: "/tmp".into(),
        };
        assert!(
            command(&r, 2, true)
                .unwrap()
                .starts_with("codex --no-daemon '")
        );
        assert!(
            command(
                &Revision {
                    markdown: "\x1b".into(),
                    ..r.clone()
                },
                1,
                false
            )
            .is_err()
        );
        assert!(
            command(
                &Revision {
                    markdown: "x".repeat(8001),
                    ..r
                },
                1,
                false
            )
            .is_err()
        );
    }
}
