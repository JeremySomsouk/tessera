//! Conservative keyboard compatibility for visible agent choice prompts.
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
        Self::parse_codex(lines).or_else(|| Self::parse_claude(lines))
    }

    fn parse_codex(lines: &[String]) -> Option<Self> {
        let header = lines.iter().rposition(|line| {
            let line = line.trim();
            line.starts_with("Question ") && line.contains('/') && line.ends_with(" unanswered)")
        })?;
        let footer = lines
            .iter()
            .enumerate()
            .skip(header + 1)
            .find_map(|(row, _)| {
                let footer = footer_text(lines, row);
                (footer.contains("tab to add notes")
                    && footer.contains("enter to submit answer")
                    && footer.contains("esc to interrupt"))
                .then_some(row)
            })?;
        Self::parse_choices(lines, header + 1, footer, '›')
    }

    fn parse_claude(lines: &[String]) -> Option<Self> {
        let footer = (0..lines.len()).rev().find(|&row| {
            let text = footer_text(lines, row);
            text.contains("enter to select")
                && (text.contains("to navigate") || text.contains("tab/arrow keys"))
                && text.contains("esc to cancel")
        })?;
        let start = lines[..footer].iter().rposition(|line| {
            line.trim_start()
                .trim_start_matches(['❯', '>'])
                .trim_start()
                .starts_with("1. ")
        })?;
        let marker = if lines[start..footer]
            .iter()
            .any(|line| line.trim_start().starts_with('❯'))
        {
            '❯'
        } else {
            '>'
        };
        let prompt = Self::parse_choices(lines, start, footer, marker)?;
        let selected = prompt
            .choices
            .iter()
            .find(|choice| choice.number == prompt.selected)?;
        if lines[selected.row].contains("Type something") {
            return None;
        }
        Some(prompt)
    }

    fn parse_choices(lines: &[String], start: usize, footer: usize, marker: char) -> Option<Self> {
        let mut choices = Vec::new();
        let mut selected = None;
        for (row, line) in lines.iter().enumerate().take(footer).skip(start) {
            let trimmed = line.trim_start();
            let marked = trimmed.starts_with(marker);
            let text = if marked {
                trimmed.strip_prefix(marker)?.trim_start()
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

fn footer_text(lines: &[String], row: usize) -> String {
    let first = lines[row].trim().to_lowercase();
    if !["tab ", "enter ", "press ", "↑/↓"]
        .iter()
        .any(|prefix| first.starts_with(prefix))
    {
        return String::new();
    }
    lines[row..lines.len().min(row + 3)]
        .iter()
        .take_while(|line| !line.trim().is_empty())
        .flat_map(|line| line.split_whitespace())
        .collect::<Vec<_>>()
        .join(" ")
        .to_lowercase()
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
    fn claude_fixture() -> Vec<String> {
        [
            "Which approach should we use?",
            "❯ 1. First",
            "     Description of first choice",
            "  2. Second",
            "  3. Type something.",
            "  4. Chat about this",
            "Enter to select · ↑/↓ to navigate · Esc to cancel",
        ]
        .into_iter()
        .map(String::from)
        .collect()
    }
    #[test]
    fn recognizes_claude_numbered_choices_and_footer_variants() {
        let mut lines = claude_fixture();
        let prompt = Prompt::parse(&lines).unwrap();
        assert_eq!(prompt.selected, 1);
        assert_eq!(prompt.choices[1].row, 3);
        assert_eq!(prompt.choices.len(), 4);
        lines[6] = "Enter to select · Tab/Arrow keys to navigate · Esc to cancel".into();
        assert!(Prompt::parse(&lines).is_some());
        lines[1] = "> 1. First".into();
        assert!(Prompt::parse(&lines).is_some());
        lines[0] = "Do you want to proceed?".into();
        lines[1] = "  1. Yes".into();
        lines[3] = "❯ 2. No".into();
        assert_eq!(Prompt::parse(&lines).unwrap().selected, 2);
    }
    #[test]
    fn rejects_claude_output_and_text_entry() {
        let mut lines = claude_fixture();
        lines[6] = "Enter to submit · Esc to cancel".into();
        assert!(Prompt::parse(&lines).is_none());
        lines = claude_fixture();
        lines[1] = "  1. First".into();
        assert!(Prompt::parse(&lines).is_none());
        lines = claude_fixture();
        lines[3] = "❯ 2. Second".into();
        assert!(Prompt::parse(&lines).is_none());
        lines = claude_fixture();
        lines[3] = "  5. Second".into();
        assert!(Prompt::parse(&lines).is_none());
        lines = claude_fixture();
        lines[1] = "  1. First".into();
        lines[4] = "❯ 3. Type something.".into();
        assert!(Prompt::parse(&lines).is_none());
    }
    #[test]
    fn option_labels_with_keyboard_words_are_not_footer_rows() {
        let mut lines = fixture();
        lines[5] = "   3. Enter another answer".into();
        assert_eq!(Prompt::parse(&lines).unwrap().choices.len(), 3);
    }

    #[test]
    fn recognizes_codex_footer_across_terminal_rows() {
        let mut lines = fixture();
        lines.truncate(6);
        lines.extend([
            "tab to add notes | enter to submit answer |".into(),
            "esc to interrupt".into(),
        ]);
        assert_eq!(Prompt::parse(&lines).unwrap().choices.len(), 3);
    }

    #[test]
    fn recognizes_claude_footer_across_terminal_rows() {
        let mut lines = claude_fixture();
        lines.truncate(6);
        lines.extend([
            "Enter to select · Tab/Arrow keys".into(),
            "to navigate · Esc to cancel".into(),
        ]);
        assert_eq!(Prompt::parse(&lines).unwrap().choices.len(), 4);
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
