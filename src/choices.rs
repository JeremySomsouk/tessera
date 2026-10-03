//! Conservative keyboard compatibility for Codex's request_user_input prompt.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Choice {
    pub row: usize,
    pub start: usize,
    pub end: usize,
    pub number: usize,
}
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Prompt {
    pub selected: usize,
    pub choices: Vec<Choice>,
}
impl Prompt {
    pub fn parse(lines: &[String]) -> Option<Self> {
        let header = lines.iter().rposition(|line| {
            let line = line.trim();
            line.starts_with("Question ") && line.contains('/') && line.ends_with(" unanswered)")
        })?;
        let footer = lines
            .iter()
            .enumerate()
            .skip(header + 1)
            .find_map(|(row, line)| {
                (line.contains("tab to add notes")
                    && line.contains("enter to submit answer")
                    && line.contains("esc to interrupt"))
                .then_some(row)
            })?;
        let mut choices = Vec::new();
        let mut selected = None;
        for (row, line) in lines.iter().enumerate().take(footer).skip(header + 1) {
            let trimmed = line.trim_start();
            let marked = trimmed.starts_with('›');
            let text = if marked {
                trimmed.strip_prefix('›')?.trim_start()
            } else {
                trimmed
            };
            let Some((number, label)) = text.split_once(". ") else {
                continue;
            };
            let Ok(number) = number.parse::<usize>() else {
                continue;
            };
            if number != choices.len() + 1 || number > 64 || label.trim().is_empty() {
                return None;
            }
            if marked && selected.replace(number).is_some() {
                return None;
            }
            choices.push(Choice {
                row,
                start: line.chars().count() - trimmed.chars().count(),
                end: line.trim_end().chars().count(),
                number,
            });
        }
        if choices.len() < 2 {
            return None;
        }
        Some(Self {
            selected: selected?,
            choices,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn fixture() -> Vec<String> {
        [
            "Question 1/1 (1 unanswered)",
            "Choose an option.",
            " › 1. First",
            "   2. Second",
            "     wrapped description",
            "   3. None of the above",
            "tab to add notes | enter to submit answer | esc to interrupt",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }
    #[test]
    fn recognizes_choices_with_wrapped_descriptions() {
        let prompt = Prompt::parse(&fixture()).unwrap();
        assert_eq!(prompt.selected, 1);
        assert_eq!(
            prompt.choices[1],
            Choice {
                row: 3,
                start: 3,
                end: 12,
                number: 2
            }
        );
    }
    #[test]
    fn rejects_lists_notes_and_ambiguous_selection() {
        let mut lines = fixture();
        lines[0] = "Ordinary numbered output".into();
        assert!(Prompt::parse(&lines).is_none());
        lines = fixture();
        lines[6] = "tab to switch | enter to submit answer | esc to interrupt".into();
        assert!(Prompt::parse(&lines).is_none());
        lines = fixture();
        lines[3] = " › 2. Second".into();
        assert!(Prompt::parse(&lines).is_none());
        lines = fixture();
        lines.push("Question 2/2 (1 unanswered)".into());
        assert!(Prompt::parse(&lines).is_none());
    }
}
