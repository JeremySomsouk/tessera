use serde::{Deserialize, Serialize};
use uuid::Uuid;

#[derive(Clone, Default, Serialize, Deserialize)]
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
}
#[derive(Clone, Serialize, Deserialize)]
pub struct Specification {
    pub id: Uuid,
    pub draft: Revision,
    pub status: usize,
    pub revisions: Vec<Revision>,
    pub launches: Vec<Launch>,
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
        }
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
