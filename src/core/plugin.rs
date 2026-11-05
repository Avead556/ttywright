use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::command::Command;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginCommand {
    pub id: String,
    pub title: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginInfo {
    pub name: String,
    pub commands: Vec<PluginCommand>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginContext {
    pub path: Option<PathBuf>,
    pub text: Option<String>,
    pub cursor: Option<usize>,
    pub selection: Option<(usize, usize)>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginReport {
    pub commands: Vec<Command>,
    pub status: Option<String>,
}

pub const fn acts_at_the_cursor(command: &Command) -> bool {
    matches!(
        command,
        Command::Insert(_)
            | Command::Newline
            | Command::Indent
            | Command::Outdent
            | Command::DeleteBackward
            | Command::DeleteForward
            | Command::Move(_)
            | Command::Select(_)
            | Command::SelectTo { .. }
            | Command::SelectWord
    )
}

pub fn clean_text(text: &str) -> String {
    const LIMIT: usize = 300;
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(LIMIT)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::action::Action;

    #[test]
    fn typing_is_about_the_cursor_and_actions_are_not() {
        assert!(acts_at_the_cursor(&Command::Newline));
        assert!(!acts_at_the_cursor(&Command::Run(Action::Quit)));
    }

    #[test]
    fn screen_text_is_one_short_line() {
        assert_eq!(clean_text("a\u{1b}[2J\nb"), "a [2J b");
        assert_eq!(clean_text(&"x".repeat(1000)).chars().count(), 300);
    }
}
