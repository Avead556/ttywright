use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::db::is_unsafe_on_screen;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum CliTool {
    Docker,
    Gh,
}

impl CliTool {
    pub const fn program(self) -> &'static str {
        match self {
            Self::Docker => "docker",
            Self::Gh => "gh",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CliOutcome {
    Missing,
    Done(String),
    Failed(String),
    TimedOut,
    TooLong,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CliEffect {
    Run {
        id: u64,
        tool: CliTool,
        root: PathBuf,
        args: Vec<String>,
        input: Option<String>,
        in_project: bool,
        write: bool,
    },
}

impl CliEffect {
    pub const fn needs_trust(&self) -> bool {
        match self {
            Self::Run { in_project, .. } => *in_project,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CliEvent {
    Finished {
        id: u64,
        tool: CliTool,
        outcome: CliOutcome,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PanelCx {
    pub root: PathBuf,
    pub trusted: bool,
    pub branch: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PanelReply {
    Cli(CliEffect),
    Info(String),
    Error(String),
    AskTrust(String),
    Run(super::run::RunTarget),
    Terminal {
        title: String,
        command: Vec<String>,
        project_command: bool,
    },
    RefreshGit,
    Autosave,
}

#[derive(Debug)]
pub struct CliAsks<R> {
    next: u64,
    pending: Vec<(u64, R)>,
}

impl<R> Default for CliAsks<R> {
    fn default() -> Self {
        Self {
            next: 1,
            pending: Vec::new(),
        }
    }
}

impl<R: Copy> CliAsks<R> {
    pub fn ask(&mut self, request: R) -> u64 {
        let id = self.next;
        self.next += 1;
        self.pending.push((id, request));
        id
    }

    pub fn take(&mut self, id: u64) -> Option<R> {
        let found = self.pending.iter().position(|(i, _)| *i == id)?;
        Some(self.pending.remove(found).1)
    }

    pub fn is_empty(&self) -> bool {
        self.pending.is_empty()
    }

    pub fn any(&self, mut f: impl FnMut(R) -> bool) -> bool {
        self.pending.iter().any(|(_, r)| f(*r))
    }
}

pub fn clean_line(text: &str) -> String {
    text.chars()
        .filter(|c| *c != '\r')
        .map(|c| match c {
            '\t' | '\n' => ' ',
            c if is_unsafe_on_screen(c) => '�',
            c => c,
        })
        .collect()
}

pub fn clean_block(text: &str) -> Vec<String> {
    text.split('\n').map(clean_line).collect()
}

pub fn clean_typed(text: &str) -> String {
    clean_block(text).join("\n").trim_end().to_owned()
}

pub fn safe_word(word: &str) -> bool {
    !word.is_empty()
        && !word.starts_with('-')
        && word.len() <= 255
        && !word.contains("..")
        && word.chars().all(|c| {
            c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-' | '/' | ':' | '@' | '+')
        })
}

pub fn redact(text: &str) -> String {
    const PREFIXES: [&str; 6] = ["ghp_", "gho_", "ghu_", "ghs_", "ghr_", "github_pat_"];
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some((at, prefix)) = PREFIXES
        .iter()
        .filter_map(|p| rest.find(p).map(|at| (at, *p)))
        .min()
    {
        out.push_str(&rest[..at]);
        out.push_str("***");
        let after = &rest[at + prefix.len()..];
        let end = after
            .find(|c: char| !(c.is_ascii_alphanumeric() || c == '_'))
            .unwrap_or(after.len());
        rest = &after[end..];
    }
    out.push_str(rest);
    out
}

pub fn command_text(tool: CliTool, args: &[String]) -> String {
    let mut text = tool.program().to_owned();
    for arg in args {
        text.push(' ');
        if arg
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "._-/:@+=".contains(c))
            && !arg.is_empty()
        {
            text.push_str(arg);
        } else {
            text.push('\'');
            text.push_str(&clean_line(arg).replace('\'', "'\\''"));
            text.push('\'');
        }
    }
    text
}

pub fn main_complaint(text: &str) -> String {
    let lines = || text.lines().map(str::trim).filter(|l| !l.is_empty());
    let chosen = lines()
        .rfind(|l| !l.contains("level=warning"))
        .or_else(|| lines().next_back())
        .unwrap_or("it failed");
    clean_line(chosen).chars().take(300).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn an_answer_is_taken_once_and_only_by_the_one_who_asked() {
        let mut asks = CliAsks::default();
        let a = asks.ask('a');
        let b = asks.ask('b');
        assert_ne!(a, b);
        assert!(asks.any(|r| r == 'b'));
        assert_eq!(asks.take(b), Some('b'));
        assert_eq!(asks.take(b), None, "answered already");
        assert_eq!(asks.take(99), None, "never asked here");
        assert!(!asks.is_empty());
        assert_eq!(asks.take(a), Some('a'));
        assert!(asks.is_empty());
    }

    #[test]
    fn nothing_on_a_line_can_steer_the_terminal() {
        let hostile = "a\u{1b}]0;title\u{7}b\u{202e}c\td\r";
        let clean = clean_line(hostile);
        assert!(!clean.chars().any(is_unsafe_on_screen), "{clean:?}");
        assert!(!clean.contains('\r'));
    }

    #[test]
    fn option_like_and_odd_names_are_not_words() {
        for good in ["web", "feature/x-1", "owner/repo", "nginx:1.2", "a@b+c"] {
            assert!(safe_word(good), "{good}");
        }
        for bad in [
            "", "-x", "--force", "a b", "a;b", "a\nb", "$(x)", "a..b", "`x`",
        ] {
            assert!(!safe_word(bad), "{bad}");
        }
    }

    #[test]
    fn tokens_are_taken_out_of_what_a_tool_says() {
        let text = "bad ghp_abcDEF123 and github_pat_11A_b2, gho_x";
        assert_eq!(redact(text), "bad *** and ***, ***");
        assert_eq!(redact("nothing here"), "nothing here");
    }

    #[test]
    fn a_command_reads_as_it_would_be_typed() {
        let args = ["compose".to_owned(), "down".to_owned(), "a b".to_owned()];
        assert_eq!(
            command_text(CliTool::Docker, &args),
            "docker compose down 'a b'"
        );
    }

    #[test]
    fn the_complaint_is_the_error_and_not_the_warning_before_or_after_it() {
        let text = "time=x level=warning msg=\"y\"\nno configuration file provided: not found\n";
        assert_eq!(
            main_complaint(text),
            "no configuration file provided: not found"
        );
        assert_eq!(main_complaint(""), "it failed");
    }
}
