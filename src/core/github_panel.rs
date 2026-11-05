use super::action::Action;
use super::cli::{CliEffect, CliOutcome, CliTool, PanelCx, PanelReply, clean_typed, safe_word};
use super::command::{Command, Motion};
use super::github::{
    GhCompose, GhComposing, GhConfirm, GhKind, GhMerge, GhRequest, GhReview, GhView, GhWrite,
    GitHubPanel, parse_account, parse_detail, parse_items, parse_pr_diff, parse_repo,
    parse_review_comments, plain_failure,
};
use super::input::LineInput;

const PR_FIELDS: &str = "number,title,state,author,headRefName,isDraft,isCrossRepository";
const ISSUE_FIELDS: &str = "number,title,state,author,labels";
const PR_VIEW_FIELDS: &str = "number,title,state,author,body,url,headRefName,headRefOid,\
    baseRefName,isDraft,isCrossRepository,reviewDecision,mergeable,statusCheckRollup,comments,reviews";
const ISSUE_VIEW_FIELDS: &str = "number,title,state,author,body,url,labels,comments";

fn words(list: &[&str]) -> Vec<String> {
    list.iter().map(|w| (*w).to_owned()).collect()
}

type Out = Vec<PanelReply>;

impl GitHubPanel {
    pub fn open(&mut self, cx: &PanelCx, out: &mut Out) {
        if !self.loaded {
            self.refresh(cx, out);
        }
    }

    fn refresh(&mut self, cx: &PanelCx, out: &mut Out) {
        if self.pending.any(|r| r == GhRequest::Repo) {
            return;
        }
        self.loaded = true;
        self.note = None;
        self.ask(GhRequest::Account, words(&["api", "user"]), None, cx, out);
        let args = words(&[
            "repo",
            "view",
            "--json",
            "nameWithOwner,url,defaultBranchRef",
        ]);
        self.ask(GhRequest::Repo, args, None, cx, out);
    }

    fn lists(&mut self, cx: &PanelCx, out: &mut Out) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        for (request, kind, fields) in [
            (GhRequest::Prs, "pr", PR_FIELDS),
            (GhRequest::Issues, "issue", ISSUE_FIELDS),
        ] {
            let args = words(&[
                kind, "list", "-R", &repo, "--limit", "100", "--json", fields,
            ]);
            self.ask(request, args, None, cx, out);
        }
    }

    fn ask(
        &mut self,
        request: GhRequest,
        args: Vec<String>,
        input: Option<String>,
        cx: &PanelCx,
        out: &mut Out,
    ) {
        let id = self.pending.ask(request);
        out.push(PanelReply::Cli(CliEffect::Run {
            id,
            tool: CliTool::Gh,
            root: cx.root.clone(),
            args,
            input,
            in_project: true,
            write: request == GhRequest::Write,
        }));
    }

    pub fn answered(&mut self, id: u64, outcome: CliOutcome, cx: &PanelCx, out: &mut Out) {
        let Some(request) = self.pending.take(id) else {
            return;
        };
        if request == GhRequest::Write {
            return self.written(outcome, cx, out);
        }
        let read = match outcome {
            CliOutcome::Done(text) => self.read(request, &text, cx, out),
            CliOutcome::Missing => Err("gh is not installed".to_owned()),
            CliOutcome::Failed(text) => Err(plain_failure(&text)),
            CliOutcome::TimedOut => Err("gh did not answer in time".to_owned()),
            CliOutcome::TooLong => Err("gh printed too much".to_owned()),
        };
        if let Err(why) = read {
            self.note = Some(why);
        }
    }

    fn read(
        &mut self,
        request: GhRequest,
        text: &str,
        cx: &PanelCx,
        out: &mut Out,
    ) -> Result<(), String> {
        match request {
            GhRequest::Account => self.account = Some(parse_account(text)?),
            GhRequest::Repo => {
                let (repo, branch) = parse_repo(text)?;
                self.repo = Some(repo);
                self.default_branch = branch;
                self.lists(cx, out);
            }
            GhRequest::Prs => {
                self.prs = parse_items(text)?;
                self.selected_pr = self.selected_pr.min(self.prs.len().saturating_sub(1));
            }
            GhRequest::Issues => {
                self.issues = parse_items(text)?;
                self.selected_issue = self.selected_issue.min(self.issues.len().saturating_sub(1));
            }
            GhRequest::Detail(kind, number) => {
                let mut detail = parse_detail(kind, text)?;
                if detail.number == number {
                    if let Some(early) = self.early_review.take_if(|(n, _)| *n == number) {
                        detail.lines.extend(early.1);
                    }
                    self.detail = Some(detail);
                    self.detail_stale = false;
                }
            }
            GhRequest::ReviewComments(number) => {
                let lines = parse_review_comments(text)?;
                self.early_review = Some((number, lines));
                if let Some(detail) = self.detail.as_mut().filter(|d| d.number == number)
                    && !self.detail_stale
                    && let Some((_, lines)) = self.early_review.take()
                {
                    detail.lines.extend(lines);
                }
            }
            GhRequest::Diff(number) => {
                if self.detail.as_ref().is_some_and(|d| d.number == number) {
                    self.files = parse_pr_diff(text);
                    self.selected_file = 0;
                    self.diff_scroll = 0;
                }
            }
            GhRequest::Write => {}
        }
        Ok(())
    }

    fn written(&mut self, outcome: CliOutcome, cx: &PanelCx, out: &mut Out) {
        let failure = match outcome {
            CliOutcome::Done(_) => {
                out.push(PanelReply::Info("GitHub: done".to_owned()));
                out.push(PanelReply::RefreshGit);
                self.lists(cx, out);
                if let Some((kind, number)) = self.detail.as_ref().map(|d| (d.kind, d.number)) {
                    self.open_item(kind, number, cx, out);
                }
                return;
            }
            CliOutcome::Missing => "gh is not installed".to_owned(),
            CliOutcome::Failed(text) => format!("GitHub: {}", plain_failure(&text)),
            CliOutcome::TimedOut | CliOutcome::TooLong => {
                "gh was stopped before it finished: check on GitHub whether it went through"
                    .to_owned()
            }
        };
        out.push(PanelReply::Error(failure));
    }

    fn open_item(&mut self, kind: GhKind, number: u64, cx: &PanelCx, out: &mut Out) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        let fields = match kind {
            GhKind::Pr => PR_VIEW_FIELDS,
            GhKind::Issue => ISSUE_VIEW_FIELDS,
        };
        self.detail_stale = true;
        self.early_review = None;
        let text = number.to_string();
        let args = words(&[kind.word(), "view", &text, "-R", &repo, "--json", fields]);
        self.ask(GhRequest::Detail(kind, number), args, None, cx, out);
        if kind == GhKind::Pr {
            let path = format!("repos/{repo}/pulls/{number}/comments?per_page=100");
            let args = words(&["api", &path]);
            self.ask(GhRequest::ReviewComments(number), args, None, cx, out);
        }
    }

    pub fn pick(&mut self, index: usize) {
        match self.view {
            GhView::List => self.select(index),
            GhView::Files => {
                self.selected_file = index.min(self.files.len().saturating_sub(1));
                self.diff_scroll = 0;
            }
            GhView::Detail => {}
        }
    }

    pub fn scroll(&mut self, delta: isize) {
        match self.view {
            GhView::List => self.select(self.selected().saturating_add_signed(delta)),
            GhView::Detail => {
                let last = self
                    .detail
                    .as_ref()
                    .map_or(0, |d| d.lines.len().saturating_sub(1));
                self.scroll = self.scroll.saturating_add_signed(delta).min(last);
            }
            GhView::Files => {
                let last = self
                    .files
                    .get(self.selected_file)
                    .map_or(0, |f| f.rows.len().saturating_sub(1));
                self.diff_scroll = self.diff_scroll.saturating_add_signed(delta).min(last);
            }
        }
    }

    pub fn back(&mut self) -> bool {
        match self.view {
            GhView::List => false,
            GhView::Detail => {
                self.view = GhView::List;
                true
            }
            GhView::Files => {
                self.view = GhView::Detail;
                true
            }
        }
    }

    pub fn input(&mut self, input: &Command, cx: &PanelCx, out: &mut Out) {
        let rows = |rows: usize| isize::try_from(rows).unwrap_or(isize::MAX);
        match input {
            Command::Move(Motion::Up) if self.view == GhView::Files => self.step_file(-1),
            Command::Move(Motion::Down) if self.view == GhView::Files => self.step_file(1),
            Command::Move(Motion::Up) => self.scroll(-1),
            Command::Move(Motion::Down) => self.scroll(1),
            Command::Move(Motion::PageUp { rows: n }) => self.scroll(-rows(*n)),
            Command::Move(Motion::PageDown { rows: n }) => self.scroll(rows(*n)),
            Command::Indent if self.view == GhView::List => {
                self.showing_issues = !self.showing_issues;
            }
            Command::Newline if self.view == GhView::List => self.enter(cx, out),
            Command::Insert(letter) if letter == "r" => self.refresh(cx, out),
            Command::Insert(letter) if letter == "n" => self.new_pr(cx, out),
            Command::Insert(letter) if self.view == GhView::Detail => {
                self.detail_key(letter, cx, out);
            }
            _ => {}
        }
    }

    fn step_file(&mut self, delta: isize) {
        self.selected_file = self
            .selected_file
            .saturating_add_signed(delta)
            .min(self.files.len().saturating_sub(1));
        self.diff_scroll = 0;
    }

    fn enter(&mut self, cx: &PanelCx, out: &mut Out) {
        let Some(number) = self.items().get(self.selected()).map(|i| i.number) else {
            return;
        };
        let kind = if self.showing_issues {
            GhKind::Issue
        } else {
            GhKind::Pr
        };
        self.detail = None;
        self.files.clear();
        self.scroll = 0;
        self.view = GhView::Detail;
        self.open_item(kind, number, cx, out);
    }

    fn detail_key(&mut self, letter: &str, cx: &PanelCx, out: &mut Out) {
        let Some(detail) = self.detail.clone() else {
            return;
        };
        let is_pr = detail.kind == GhKind::Pr;
        let error = |text: &str| PanelReply::Error(text.to_owned());
        match letter {
            "c" => self.compose(GhComposing::Comment {
                kind: detail.kind,
                number: detail.number,
            }),
            "d" => self.ask_confirm(
                GhWrite::Close {
                    kind: detail.kind,
                    number: detail.number,
                },
                out,
            ),
            "a" if is_pr => self.compose(GhComposing::Review {
                number: detail.number,
                review: GhReview::Approve,
            }),
            "x" if is_pr => self.compose(GhComposing::Review {
                number: detail.number,
                review: GhReview::RequestChanges,
            }),
            "f" if is_pr => self.diff(detail.number, cx, out),
            "o" if is_pr => {
                if !safe_word(&detail.head) {
                    return out.push(error("That branch name cannot be used safely"));
                }
                self.ask_confirm(
                    GhWrite::Checkout {
                        number: detail.number,
                        branch: detail.head,
                        cross_repository: detail.cross_repository,
                    },
                    out,
                );
            }
            "m" if is_pr => {
                if detail.head_oid.is_empty() || !safe_word(&detail.head_oid) {
                    return out.push(error("gh did not say which commit this is"));
                }
                self.ask_confirm(
                    GhWrite::Merge {
                        number: detail.number,
                        head_oid: detail.head_oid,
                        method: GhMerge::Merge,
                    },
                    out,
                );
            }
            _ => {}
        }
    }

    fn diff(&mut self, number: u64, cx: &PanelCx, out: &mut Out) {
        let Some(repo) = self.repo.clone() else {
            return;
        };
        self.view = GhView::Files;
        if self.files.is_empty() {
            let text = number.to_string();
            let args = words(&["pr", "diff", &text, "-R", &repo]);
            self.ask(GhRequest::Diff(number), args, None, cx, out);
        }
    }

    fn new_pr(&mut self, cx: &PanelCx, out: &mut Out) {
        let head = cx.branch.clone().unwrap_or_default();
        let Some(base) = self.default_branch.clone() else {
            let why = "The repository's default branch is not known yet";
            return out.push(PanelReply::Info(why.to_owned()));
        };
        if !safe_word(&head) {
            let why = "This branch's name cannot be used safely";
            return out.push(PanelReply::Error(why.to_owned()));
        }
        if head == base {
            let why = format!("You are on {base}: switch to a branch first");
            return out.push(PanelReply::Info(why));
        }
        self.compose(GhComposing::PrTitle { head, base });
    }

    fn compose(&mut self, purpose: GhComposing) {
        self.compose = Some(GhCompose {
            purpose,
            input: LineInput::default(),
            error: None,
        });
    }

    fn ask_confirm(&mut self, write: GhWrite, out: &mut Out) {
        if self.repo.is_none() || self.account.is_none() {
            let why = "Not known yet which account and repository: press r";
            return out.push(PanelReply::Error(why.to_owned()));
        }
        if self.writing() {
            let why = "Another change is still on its way";
            return out.push(PanelReply::Info(why.to_owned()));
        }
        self.confirm = Some(GhConfirm {
            write,
            typed: None,
            scroll: 0,
        });
    }

    pub fn compose_input(&mut self, input: Command, out: &mut Out) {
        let Some(compose) = self.compose.as_mut() else {
            return;
        };
        match input {
            Command::Escape => self.compose = None,
            Command::Newline => self.composed(out),
            Command::Run(Action::ContextActions) => compose.input.insert_lines("\n"),
            Command::Insert(text) => {
                compose.input.insert_lines(&text);
                compose.error = None;
            }
            Command::DeleteBackward => compose.input.delete_backward(),
            Command::DeleteForward => compose.input.delete_forward(),
            Command::Move(motion) => compose.input.motion(motion),
            _ => {}
        }
    }

    fn composed(&mut self, out: &mut Out) {
        let Some(compose) = self.compose.as_mut() else {
            return;
        };
        let text = clean_typed(compose.input.text());
        let empty_is_fine = matches!(
            compose.purpose,
            GhComposing::Review {
                review: GhReview::Approve,
                ..
            } | GhComposing::PrBody { .. }
        );
        if text.trim().is_empty() && !empty_is_fine {
            compose.error = Some("Type something first".to_owned());
            return;
        }
        let write = match compose.purpose.clone() {
            GhComposing::Comment { kind, number } => GhWrite::Comment {
                kind,
                number,
                body: text,
            },
            GhComposing::Review { number, review } => GhWrite::Review {
                number,
                review,
                body: text,
            },
            GhComposing::PrTitle { head, base } => {
                compose.purpose = GhComposing::PrBody {
                    head,
                    base,
                    title: text.replace('\n', " "),
                };
                compose.input = LineInput::default();
                return;
            }
            GhComposing::PrBody { head, base, title } => GhWrite::CreatePr {
                title,
                body: text,
                head,
                base,
            },
        };
        self.compose = None;
        self.ask_confirm(write, out);
    }

    pub fn confirm_input(&mut self, input: Command, cx: &PanelCx, out: &mut Out) {
        let not_sent = || PanelReply::Info("Not sent".to_owned());
        let Some(confirm) = self.confirm.as_mut() else {
            return;
        };
        if let Some(typed) = confirm.typed.as_mut() {
            match input {
                Command::Insert(text) => typed.insert(&text),
                Command::DeleteBackward => typed.delete_backward(),
                Command::Newline => {
                    let wanted = confirm.write.second_answer().map(|n| n.to_string());
                    if wanted.as_deref() == Some(typed.text().trim()) {
                        self.send(cx, out);
                    }
                }
                Command::Escape => {
                    self.confirm = None;
                    out.push(not_sent());
                }
                _ => {}
            }
            return;
        }
        match input {
            Command::Insert(letter) if letter == "y" || letter == "Y" => {
                if confirm.write.second_answer().is_some() {
                    confirm.typed = Some(LineInput::default());
                } else {
                    self.send(cx, out);
                }
            }
            Command::Indent => {
                if let GhWrite::Merge { method, .. } = &mut confirm.write {
                    *method = method.next();
                }
            }
            Command::Move(Motion::Up) => confirm.scroll = confirm.scroll.saturating_sub(1),
            Command::Move(Motion::Down) => confirm.scroll = confirm.scroll.saturating_add(1),
            _ => {
                self.confirm = None;
                out.push(not_sent());
            }
        }
    }

    fn send(&mut self, cx: &PanelCx, out: &mut Out) {
        let Some(confirm) = self.confirm.take() else {
            return;
        };
        let Some(repo) = self.repo.clone() else {
            return;
        };
        out.push(PanelReply::Autosave);
        let args = confirm.write.args(&repo);
        self.ask(GhRequest::Write, args, confirm.write.input(), cx, out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn cx(branch: &str) -> PanelCx {
        PanelCx {
            root: "/p".into(),
            trusted: true,
            branch: Some(branch.to_owned()),
        }
    }

    fn ready() -> GitHubPanel {
        GitHubPanel {
            repo: Some("o/r".to_owned()),
            account: Some("me".to_owned()),
            default_branch: Some("main".to_owned()),
            ..GitHubPanel::default()
        }
    }

    fn writes(out: &[PanelReply]) -> Vec<u64> {
        out.iter()
            .filter_map(|r| match r {
                PanelReply::Cli(CliEffect::Run {
                    id, write: true, ..
                }) => Some(*id),
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_new_pr_is_refused_on_the_default_branch() {
        let mut panel = ready();
        let mut out = Vec::new();
        panel.input(&Command::Insert("n".to_owned()), &cx("main"), &mut out);
        assert_eq!(
            out,
            [PanelReply::Info(
                "You are on main: switch to a branch first".to_owned()
            )]
        );
        assert!(panel.compose.is_none());
    }

    #[test]
    fn a_new_pr_asks_title_then_body_then_saves_before_it_sends() {
        let mut panel = ready();
        let mut out = Vec::new();
        panel.input(&Command::Insert("n".to_owned()), &cx("fix"), &mut out);
        for text in ["Title", "Body"] {
            panel.compose_input(Command::Insert(text.to_owned()), &mut out);
            panel.compose_input(Command::Newline, &mut out);
        }
        assert!(out.is_empty(), "{out:?}");
        assert!(panel.confirm.is_some());
        panel.confirm_input(Command::Insert("y".to_owned()), &cx("fix"), &mut out);
        assert_eq!(out[0], PanelReply::Autosave);
        let [id] = writes(&out)[..] else {
            panic!("{out:?}");
        };

        let mut out = Vec::new();
        panel.answered(id, CliOutcome::Done(String::new()), &cx("fix"), &mut out);
        assert_eq!(
            out[..2],
            [
                PanelReply::Info("GitHub: done".to_owned()),
                PanelReply::RefreshGit
            ]
        );
    }

    #[test]
    fn a_write_that_ran_out_of_time_is_not_called_a_failure() {
        let mut panel = ready();
        let mut out = Vec::new();
        panel.input(&Command::Insert("n".to_owned()), &cx("fix"), &mut out);
        for text in ["T", ""] {
            panel.compose_input(Command::Insert(text.to_owned()), &mut out);
            panel.compose_input(Command::Newline, &mut out);
        }
        panel.confirm_input(Command::Insert("y".to_owned()), &cx("fix"), &mut out);
        let id = writes(&out)[0];
        let mut out = Vec::new();
        panel.answered(id, CliOutcome::TimedOut, &cx("fix"), &mut out);
        let [PanelReply::Error(text)] = &out[..] else {
            panic!("{out:?}");
        };
        assert!(text.contains("check on GitHub"), "{text}");
    }
}
