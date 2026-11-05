use serde_json::Value;

use super::cli::{clean_block, clean_line, main_complaint, safe_word};
use super::git::parse_unified;
use super::input::LineInput;
use super::local_history::{DiffKind, DiffRow};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhKind {
    Pr,
    Issue,
}

impl GhKind {
    pub const fn word(self) -> &'static str {
        match self {
            Self::Pr => "pr",
            Self::Issue => "issue",
        }
    }

    pub const fn label(self) -> &'static str {
        match self {
            Self::Pr => "pull request",
            Self::Issue => "issue",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhItem {
    pub number: u64,
    pub title: String,
    pub state: String,
    pub author: String,
    pub note: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhDetail {
    pub kind: GhKind,
    pub number: u64,
    pub title: String,
    pub state: String,
    pub head: String,
    pub head_oid: String,
    pub cross_repository: bool,
    pub lines: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GhFileDiff {
    pub path: String,
    pub added: usize,
    pub removed: usize,
    pub rows: Vec<DiffRow>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhReview {
    Approve,
    RequestChanges,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhMerge {
    Merge,
    Squash,
    Rebase,
}

impl GhMerge {
    const fn flag(self) -> &'static str {
        match self {
            Self::Merge => "--merge",
            Self::Squash => "--squash",
            Self::Rebase => "--rebase",
        }
    }

    pub const fn next(self) -> Self {
        match self {
            Self::Merge => Self::Squash,
            Self::Squash => Self::Rebase,
            Self::Rebase => Self::Merge,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhWrite {
    Comment {
        kind: GhKind,
        number: u64,
        body: String,
    },
    Review {
        number: u64,
        review: GhReview,
        body: String,
    },
    CreatePr {
        title: String,
        body: String,
        head: String,
        base: String,
    },
    Merge {
        number: u64,
        head_oid: String,
        method: GhMerge,
    },
    Close {
        kind: GhKind,
        number: u64,
    },
    Checkout {
        number: u64,
        branch: String,
        cross_repository: bool,
    },
}

impl GhWrite {
    pub fn args(&self, repo: &str) -> Vec<String> {
        let list =
            |words: &[&str]| -> Vec<String> { words.iter().map(|w| (*w).to_owned()).collect() };
        let repo_flag = ["-R", repo];
        match self {
            Self::Comment { kind, number, .. } => {
                let number = number.to_string();
                let mut args = list(&[kind.word(), "comment", &number]);
                args.extend(list(&repo_flag));
                args.extend(list(&["--body-file", "-"]));
                args
            }
            Self::Review {
                number,
                review,
                body,
            } => {
                let number = number.to_string();
                let flag = match review {
                    GhReview::Approve => "--approve",
                    GhReview::RequestChanges => "--request-changes",
                };
                let mut args = list(&["pr", "review", &number, flag]);
                args.extend(list(&repo_flag));
                if !body.is_empty() {
                    args.extend(list(&["--body-file", "-"]));
                }
                args
            }
            Self::CreatePr {
                title, head, base, ..
            } => {
                let mut args = list(&["pr", "create"]);
                args.extend(list(&repo_flag));
                args.extend(list(&["--head", head, "--base", base]));
                args.push(format!("--title={title}"));
                args.extend(list(&["--body-file", "-"]));
                args
            }
            Self::Merge {
                number,
                head_oid,
                method,
            } => {
                let number = number.to_string();
                let mut args = list(&["pr", "merge", &number, method.flag()]);
                args.extend(list(&repo_flag));
                args.extend(list(&["--match-head-commit", head_oid]));
                args
            }
            Self::Close { kind, number } => {
                let number = number.to_string();
                let mut args = list(&[kind.word(), "close", &number]);
                args.extend(list(&repo_flag));
                args
            }
            Self::Checkout { number, .. } => {
                let number = number.to_string();
                let mut args = list(&["pr", "checkout", &number]);
                args.extend(list(&repo_flag));
                args
            }
        }
    }

    pub fn input(&self) -> Option<String> {
        match self {
            Self::Comment { body, .. }
            | Self::Review { body, .. }
            | Self::CreatePr { body, .. }
                if !body.is_empty() =>
            {
                Some(body.clone())
            }
            Self::CreatePr { .. } => Some(String::new()),
            _ => None,
        }
    }

    pub fn text(&self) -> Vec<String> {
        match self {
            Self::Comment { body, .. } | Self::Review { body, .. } => clean_block(body),
            Self::CreatePr { title, body, .. } => {
                let mut lines = vec![format!("Title: {title}"), String::new()];
                lines.extend(clean_block(body));
                lines
            }
            _ => Vec::new(),
        }
    }

    pub fn headline(&self) -> String {
        match self {
            Self::Comment { kind, number, .. } => {
                format!("Post this comment on {} #{number}", kind.label())
            }
            Self::Review {
                number,
                review: GhReview::Approve,
                ..
            } => format!("Approve pull request #{number}"),
            Self::Review { number, .. } => format!("Request changes on pull request #{number}"),
            Self::CreatePr { head, base, .. } => {
                format!("Open a pull request from {head} into {base}")
            }
            Self::Merge { number, method, .. } => format!(
                "Merge pull request #{number} with {}",
                method.flag().trim_start_matches('-')
            ),
            Self::Close { kind, number } => format!("Close {} #{number}", kind.label()),
            Self::Checkout {
                number,
                branch,
                cross_repository,
            } => {
                let from = if *cross_repository {
                    " from a fork"
                } else {
                    ""
                };
                format!("Check out pull request #{number}{from}: branch {branch}")
            }
        }
    }

    pub const fn is_local(&self) -> bool {
        matches!(self, Self::Checkout { .. })
    }

    pub const fn second_answer(&self) -> Option<u64> {
        match self {
            Self::Merge { number, .. } | Self::Close { number, .. } => Some(*number),
            _ => None,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum GhComposing {
    Comment {
        kind: GhKind,
        number: u64,
    },
    Review {
        number: u64,
        review: GhReview,
    },
    PrTitle {
        head: String,
        base: String,
    },
    PrBody {
        head: String,
        base: String,
        title: String,
    },
}

impl GhComposing {
    pub fn title(&self) -> String {
        match self {
            Self::Comment { kind, number } => format!("Comment on {} #{number}", kind.label()),
            Self::Review {
                number,
                review: GhReview::Approve,
            } => format!("Approve #{number}: comment, may be empty"),
            Self::Review { number, .. } => format!("Request changes on #{number}: why"),
            Self::PrTitle { head, base } => format!("New pull request {head} → {base}: title"),
            Self::PrBody { head, base, .. } => {
                format!("New pull request {head} → {base}: description")
            }
        }
    }
}

#[derive(Debug, Clone)]
pub struct GhCompose {
    pub purpose: GhComposing,
    pub input: LineInput,
    pub error: Option<String>,
}

#[derive(Debug, Clone)]
pub struct GhConfirm {
    pub write: GhWrite,
    pub typed: Option<LineInput>,
    pub scroll: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GhRequest {
    Account,
    Repo,
    Prs,
    Issues,
    Detail(GhKind, u64),
    ReviewComments(u64),
    Diff(u64),
    Write,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum GhView {
    #[default]
    List,
    Detail,
    Files,
}

#[derive(Debug, Default)]
pub struct GitHubPanel {
    pub view: GhView,
    pub showing_issues: bool,
    pub prs: Vec<GhItem>,
    pub issues: Vec<GhItem>,
    pub selected_pr: usize,
    pub selected_issue: usize,
    pub account: Option<String>,
    pub repo: Option<String>,
    pub default_branch: Option<String>,
    pub note: Option<String>,
    pub detail: Option<GhDetail>,
    pub scroll: usize,
    pub files: Vec<GhFileDiff>,
    pub selected_file: usize,
    pub diff_scroll: usize,
    pub compose: Option<GhCompose>,
    pub confirm: Option<GhConfirm>,
    pub pending: super::cli::CliAsks<GhRequest>,
    pub loaded: bool,
    pub early_review: Option<(u64, Vec<String>)>,
    pub detail_stale: bool,
}

impl GitHubPanel {
    pub fn items(&self) -> &[GhItem] {
        if self.showing_issues {
            &self.issues
        } else {
            &self.prs
        }
    }

    pub fn selected(&self) -> usize {
        if self.showing_issues {
            self.selected_issue
        } else {
            self.selected_pr
        }
    }

    pub fn select(&mut self, index: usize) {
        let last = self.items().len().saturating_sub(1);
        let index = index.min(last);
        if self.showing_issues {
            self.selected_issue = index;
        } else {
            self.selected_pr = index;
        }
    }

    pub fn busy(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn writing(&self) -> bool {
        self.pending.any(|r| r == GhRequest::Write)
    }
}

fn text(value: &Value, key: &str) -> String {
    clean_line(value.get(key).and_then(Value::as_str).unwrap_or_default())
}

fn login(value: &Value) -> String {
    clean_line(
        value
            .pointer("/author/login")
            .or_else(|| value.pointer("/user/login"))
            .and_then(Value::as_str)
            .unwrap_or("ghost"),
    )
}

fn parsed(json: &str) -> Result<Value, String> {
    serde_json::from_str(json).map_err(|_| "gh printed something that is not JSON".to_owned())
}

pub fn parse_items(json: &str) -> Result<Vec<GhItem>, String> {
    let value = parsed(json)?;
    Ok(value
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter_map(|row| {
            let labels: Vec<String> = row
                .get("labels")
                .and_then(Value::as_array)
                .map_or(&[][..], Vec::as_slice)
                .iter()
                .map(|l| text(l, "name"))
                .collect();
            let note = if row.get("headRefName").is_some() {
                text(row, "headRefName")
            } else {
                labels.join(", ")
            };
            Some(GhItem {
                number: row.get("number")?.as_u64()?,
                title: text(row, "title"),
                state: text(row, "state"),
                author: login(row),
                note,
            })
        })
        .collect())
}

pub fn parse_account(json: &str) -> Result<String, String> {
    let value = parsed(json)?;
    let name = value
        .get("login")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if safe_word(name) {
        Ok(name.to_owned())
    } else {
        Err("gh did not say which account it is".to_owned())
    }
}

pub fn parse_repo(json: &str) -> Result<(String, Option<String>), String> {
    let value = parsed(json)?;
    let name = value
        .get("nameWithOwner")
        .and_then(Value::as_str)
        .unwrap_or_default();
    if !safe_word(name) || name.matches('/').count() != 1 {
        return Err("gh named a repository that cannot be used".to_owned());
    }
    let branch = value
        .pointer("/defaultBranchRef/name")
        .and_then(Value::as_str)
        .filter(|b| safe_word(b))
        .map(str::to_owned);
    Ok((name.to_owned(), branch))
}

fn push_block(lines: &mut Vec<String>, body: &str) {
    lines.extend(clean_block(body.trim_end()));
}

fn push_comments(lines: &mut Vec<String>, value: &Value, key: &str) {
    for comment in value
        .get(key)
        .and_then(Value::as_array)
        .map_or(&[][..], Vec::as_slice)
    {
        let body = comment.get("body").and_then(Value::as_str).unwrap_or("");
        let state = text(comment, "state");
        if body.is_empty() && state.is_empty() {
            continue;
        }
        lines.push(String::new());
        lines.push(if state.is_empty() {
            format!("── {} commented", login(comment))
        } else {
            format!("── {} reviewed: {state}", login(comment))
        });
        push_block(lines, body);
    }
}

fn check_line(check: &Value) -> String {
    let name = {
        let name = text(check, "name");
        if name.is_empty() {
            text(check, "context")
        } else {
            name
        }
    };
    let result = ["conclusion", "state", "status"]
        .iter()
        .map(|k| text(check, k))
        .find(|t| !t.is_empty())
        .unwrap_or_else(|| "UNKNOWN".to_owned());
    format!("  {result}  {name}")
}

pub fn parse_detail(kind: GhKind, json: &str) -> Result<GhDetail, String> {
    let value = parsed(json)?;
    let number = value
        .get("number")
        .and_then(Value::as_u64)
        .ok_or("gh named no number")?;
    let mut lines = vec![
        format!("#{number} {}", text(&value, "title")),
        format!(
            "{}  by {}  {}",
            text(&value, "state"),
            login(&value),
            text(&value, "url")
        ),
    ];
    let head = text(&value, "headRefName");
    if kind == GhKind::Pr {
        lines.push(format!("{head} → {}", text(&value, "baseRefName")));
        let review = text(&value, "reviewDecision");
        lines.push(format!(
            "review: {}  mergeable: {}{}",
            if review.is_empty() { "none" } else { &review },
            text(&value, "mergeable"),
            if value.get("isDraft").and_then(Value::as_bool) == Some(true) {
                "  draft"
            } else {
                ""
            }
        ));
        let checks = value
            .get("statusCheckRollup")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice);
        if !checks.is_empty() {
            lines.push(String::new());
            lines.push(format!("Checks ({})", checks.len()));
            lines.extend(checks.iter().map(check_line));
        }
    }
    lines.push(String::new());
    push_block(
        &mut lines,
        value
            .get("body")
            .and_then(Value::as_str)
            .unwrap_or_default(),
    );
    push_comments(&mut lines, &value, "reviews");
    push_comments(&mut lines, &value, "comments");
    Ok(GhDetail {
        kind,
        number,
        title: text(&value, "title"),
        state: text(&value, "state"),
        head,
        head_oid: text(&value, "headRefOid"),
        cross_repository: value.get("isCrossRepository").and_then(Value::as_bool) == Some(true),
        lines,
    })
}

pub fn parse_review_comments(json: &str) -> Result<Vec<String>, String> {
    let value = parsed(json)?;
    let mut lines = Vec::new();
    for comment in value.as_array().map_or(&[][..], Vec::as_slice) {
        let line = comment
            .get("line")
            .or_else(|| comment.get("original_line"))
            .and_then(Value::as_u64)
            .map_or_else(String::new, |n| format!(":{n}"));
        lines.push(String::new());
        lines.push(format!(
            "── {} on {}{line}",
            login(comment),
            text(comment, "path")
        ));
        push_block(
            &mut lines,
            comment
                .get("body")
                .and_then(Value::as_str)
                .unwrap_or_default(),
        );
    }
    Ok(lines)
}

pub fn parse_pr_diff(text: &str) -> Vec<GhFileDiff> {
    let mut sections: Vec<String> = Vec::new();
    for line in text.split_inclusive('\n') {
        if line.starts_with("diff --git ") || sections.is_empty() {
            sections.push(String::new());
        }
        if let Some(section) = sections.last_mut() {
            section.push_str(line);
        }
    }
    sections
        .iter()
        .filter(|s| s.starts_with("diff --git "))
        .map(|section| {
            let mut rows = parse_unified(section);
            for row in &mut rows {
                row.text = clean_line(&row.text);
            }
            let count = |kind| rows.iter().filter(|r| r.kind == kind).count();
            GhFileDiff {
                path: diff_path(section),
                added: count(DiffKind::Added),
                removed: count(DiffKind::Removed),
                rows,
            }
        })
        .collect()
}

fn diff_path(section: &str) -> String {
    let named = |prefix: &str, side: &str| {
        section
            .lines()
            .take_while(|l| !l.starts_with("@@ "))
            .find_map(|l| l.strip_prefix(prefix))
            .filter(|p| *p != "/dev/null")
            .map(|p| p.strip_prefix(side).unwrap_or(p).to_owned())
    };
    let path = named("+++ ", "b/")
        .or_else(|| named("--- ", "a/"))
        .or_else(|| {
            let header = section.lines().next()?;
            header.rsplit(" b/").next().map(str::to_owned)
        })
        .unwrap_or_default();
    clean_line(&path)
}

pub fn plain_failure(said: &str) -> String {
    let lower = said.to_lowercase();
    if lower.contains("gh auth login") || lower.contains("not logged") {
        return "Not logged in: run `gh auth login` in the Terminal panel".to_owned();
    }
    if lower.contains("not a git repository")
        || lower.contains("no git remotes")
        || lower.contains("none of the git remotes")
        || lower.contains("could not determine")
    {
        return "This folder has no GitHub repository".to_owned();
    }
    main_complaint(said)
}

#[cfg(test)]
mod tests {
    use super::*;

    const PR_LIST: &str = include_str!("../../tests/fixtures/github/pr-list.json");
    const ISSUE_LIST: &str = include_str!("../../tests/fixtures/github/issue-list.json");
    const REPO: &str = include_str!("../../tests/fixtures/github/repo.json");
    const PR_VIEW: &str = include_str!("../../tests/fixtures/github/pr-view.json");
    const ISSUE_VIEW: &str = include_str!("../../tests/fixtures/github/issue-view.json");
    const PR_DIFF: &str = include_str!("../../tests/fixtures/github/pr.diff");
    const COMMENTS: &str = include_str!("../../tests/fixtures/github/review-comments.json");

    #[test]
    fn real_answers_of_gh_are_read() {
        let prs = parse_items(PR_LIST).unwrap();
        assert_eq!(prs.len(), 2);
        assert!(prs[0].number > 0 && !prs[0].author.is_empty() && !prs[0].note.is_empty());
        let issues = parse_items(ISSUE_LIST).unwrap();
        assert_eq!(issues.len(), 2);
        assert_eq!(
            parse_repo(REPO).unwrap(),
            ("cli/cli".to_owned(), Some("trunk".to_owned()))
        );
        let pr = parse_detail(GhKind::Pr, PR_VIEW).unwrap();
        assert_eq!(pr.number, 14879);
        assert_eq!(pr.head_oid.len(), 40);
        assert!(
            pr.lines.iter().any(|l| l.contains("SUCCESS")),
            "{:?}",
            pr.lines
        );
        assert!(pr.lines.iter().any(|l| l.contains("reviewed:")));
        let issue = parse_detail(GhKind::Issue, ISSUE_VIEW).unwrap();
        assert_eq!(issue.head, "");
        let files = parse_pr_diff(PR_DIFF);
        assert_eq!(files[0].path, "git/client_test.go");
        assert!(files[0].added > 0);
        let comments = parse_review_comments(COMMENTS).unwrap();
        assert!(
            comments.iter().any(|l| l.contains("git/client_test.go")),
            "{comments:?}"
        );
    }

    #[test]
    fn nothing_hostile_survives_reading() {
        let escape = "\\u001b]0;pwned\\u0007\\u001b[2J\\u202e";
        let json = format!(
            r#"{{"number":1,"title":"t{escape}","state":"OPEN{escape}","author":{{"login":"a{escape}"}},
            "body":"b{escape}\nsecond","headRefName":"h{escape}","baseRefName":"m{escape}","url":"u{escape}",
            "comments":[{{"author":{{"login":"c{escape}"}},"body":"x{escape}"}}],
            "statusCheckRollup":[{{"name":"n{escape}","conclusion":"S{escape}"}}]}}"#
        );
        let detail = parse_detail(GhKind::Pr, &json).unwrap();
        let shown = format!("{detail:?}");
        for line in detail.lines {
            assert!(
                !line.contains('\u{1b}') && !line.contains('\u{202e}'),
                "{line:?}"
            );
        }
        assert!(!shown.contains('\u{7}'));
        let rows = parse_items(&format!("[{json}]")).unwrap();
        assert!(!rows[0].title.contains('\u{1b}') && !rows[0].note.contains('\u{1b}'));
        let diff = "diff --git a/x b/x\n+++ b/x\u{1b}[2J\n@@ -1 +1 @@\n+a\u{1b}]0;t\u{7}\n";
        let files = parse_pr_diff(diff);
        assert!(!files[0].path.contains('\u{1b}'));
        assert!(!files[0].rows[0].text.contains('\u{1b}'));
    }

    #[test]
    fn a_line_of_content_cannot_start_a_file_in_a_diff() {
        let diff =
            "diff --git a/x b/x\n--- a/x\n+++ b/x\n@@ -1 +1,2 @@\n a\n+diff --git a/evil b/evil\n";
        let files = parse_pr_diff(diff);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].added, 1);
    }

    #[test]
    fn a_repository_or_account_with_an_odd_name_is_refused() {
        for bad in ["--x/y", "a b/c", "no-slash", "a/b/c", "a/$(x)"] {
            let json = format!(r#"{{"nameWithOwner":"{bad}"}}"#);
            assert!(parse_repo(&json).is_err(), "{bad}");
        }
        assert!(parse_account(r#"{"login":"--admin"}"#).is_err());
        assert_eq!(
            parse_account(r#"{"login":"me","email":"x@y"}"#),
            Ok("me".to_owned())
        );
    }

    #[test]
    fn every_write_is_exactly_this_command_and_text_goes_by_stdin() {
        let repo = "o/r";
        let cases = [
            (
                GhWrite::Comment {
                    kind: GhKind::Issue,
                    number: 7,
                    body: "--rm -rf".into(),
                },
                "issue comment 7 -R o/r --body-file -",
                Some("--rm -rf"),
            ),
            (
                GhWrite::Review {
                    number: 7,
                    review: GhReview::Approve,
                    body: String::new(),
                },
                "pr review 7 --approve -R o/r",
                None,
            ),
            (
                GhWrite::Review {
                    number: 7,
                    review: GhReview::RequestChanges,
                    body: "no".into(),
                },
                "pr review 7 --request-changes -R o/r --body-file -",
                Some("no"),
            ),
            (
                GhWrite::CreatePr {
                    title: "--draft".into(),
                    body: String::new(),
                    head: "f".into(),
                    base: "main".into(),
                },
                "pr create -R o/r --head f --base main --title=--draft --body-file -",
                Some(""),
            ),
            (
                GhWrite::Merge {
                    number: 7,
                    head_oid: "abc".into(),
                    method: GhMerge::Squash,
                },
                "pr merge 7 --squash -R o/r --match-head-commit abc",
                None,
            ),
            (
                GhWrite::Close {
                    kind: GhKind::Pr,
                    number: 7,
                },
                "pr close 7 -R o/r",
                None,
            ),
            (
                GhWrite::Checkout {
                    number: 7,
                    branch: "f".into(),
                    cross_repository: false,
                },
                "pr checkout 7 -R o/r",
                None,
            ),
        ];
        for (write, command, input) in cases {
            assert_eq!(write.args(repo).join(" "), command);
            assert_eq!(write.input().as_deref(), input, "{command}");
        }
    }

    #[test]
    fn only_merge_and_close_take_a_second_answer_and_only_checkout_is_local() {
        let close = GhWrite::Close {
            kind: GhKind::Issue,
            number: 3,
        };
        assert_eq!(close.second_answer(), Some(3));
        let comment = GhWrite::Comment {
            kind: GhKind::Pr,
            number: 3,
            body: "x".into(),
        };
        assert_eq!(comment.second_answer(), None);
        assert!(!comment.is_local());
        let checkout = GhWrite::Checkout {
            number: 3,
            branch: "f".into(),
            cross_repository: true,
        };
        assert!(checkout.is_local());
        assert!(checkout.headline().contains("fork"));
    }

    #[test]
    fn failures_are_said_plainly() {
        let real = "To get started with GitHub CLI, please run:  gh auth login
\
                    Alternatively, populate the GH_TOKEN environment variable.
";
        assert!(plain_failure(real).starts_with("Not logged in"));
        assert!(
            plain_failure("failed to run git: fatal: not a git repository")
                .contains("no GitHub repository")
        );
        assert_eq!(plain_failure("HTTP 502"), "HTTP 502");
    }
}
