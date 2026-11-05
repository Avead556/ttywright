use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::{Cursor, DocumentId, Focus, Workspace};
use crate::core::command::{Command, Effect, Motion};
use crate::core::local_history::SnapshotLabel;
use crate::core::lsp::Severity;
use crate::core::mcp::{
    Decision, McpCall, McpProposal, ProposedEdit, diff_file, proposal_answer, propose_file,
    smallest_change,
};
use crate::core::run_output::RunStatus;

const MAX_CHARS: usize = 200_000;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct McpReview {
    pub file: usize,
    pub scroll: usize,
}

fn severity_name(severity: Severity) -> &'static str {
    match severity {
        Severity::Error => "error",
        Severity::Warning => "warning",
        Severity::Info => "info",
        Severity::Hint => "hint",
    }
}

impl Workspace {
    pub fn mcp_proposal(&self) -> Option<(&McpProposal, &McpReview)> {
        Some((self.mcp_proposals.first()?, self.mcp_review.as_ref()?))
    }

    pub fn mcp_pending(&self) -> usize {
        self.mcp_proposals.len()
    }

    pub(super) fn mcp_called(&mut self, id: u64, tool: &str, args: &Value, fx: &mut Vec<Effect>) {
        let call = match McpCall::parse(tool, args, &self.root) {
            Ok(call) => call,
            Err(message) => return Self::mcp_reply(id, Err(message), fx),
        };
        let paths: Vec<PathBuf> = call
            .paths()
            .into_iter()
            .filter(|p| !self.documents.values().any(|d| d.path() == p))
            .collect();
        if paths.is_empty() {
            self.mcp_answer(id, &call, &BTreeMap::new(), fx);
        } else {
            fx.push(Effect::McpReadFiles { id, call, paths });
        }
    }

    pub(super) fn mcp_files_read(
        &mut self,
        id: u64,
        call: &McpCall,
        disk: &BTreeMap<PathBuf, Result<String, String>>,
        fx: &mut Vec<Effect>,
    ) {
        self.mcp_answer(id, call, disk, fx);
    }

    fn mcp_reply(id: u64, result: Result<String, String>, fx: &mut Vec<Effect>) {
        fx.push(Effect::McpReply { id, result });
    }

    fn mcp_text(
        &self,
        path: &Path,
        disk: &BTreeMap<PathBuf, Result<String, String>>,
    ) -> Result<String, String> {
        if let Some(d) = self.documents.values().find(|d| d.path() == path) {
            return Ok(d.text().to_string());
        }
        disk.get(path)
            .cloned()
            .unwrap_or_else(|| Err(format!("Could not read {}", path.display())))
    }

    fn mcp_answer(
        &mut self,
        id: u64,
        call: &McpCall,
        disk: &BTreeMap<PathBuf, Result<String, String>>,
        fx: &mut Vec<Effect>,
    ) {
        let answer = match call {
            McpCall::OpenFiles => Ok(self.mcp_open_files()),
            McpCall::Selection => Ok(self.mcp_selection()),
            McpCall::ReadBuffer { path, from, to } => self
                .mcp_text(path, disk)
                .map(|text| self.mcp_read(path, &text, *from, *to)),
            McpCall::Diagnostics { path } => Ok(self.mcp_diagnostics(path.as_deref())),
            McpCall::OpenFile { path, line, column } => {
                let at = line.map(|l| (l.saturating_sub(1), column.unwrap_or(1).saturating_sub(1)));
                self.open_file(path.clone(), at, fx);
                Ok(json!({ "opened": path }))
            }
            McpCall::RunOutput { last } => Ok(self.mcp_run_output(*last)),
            McpCall::IdeDiagnostics => Ok(self.ide_diagnostics()),
            McpCall::WorkspaceFolders => {
                Ok(json!({ "success": true, "folders": [self.root], "rootPath": self.root }))
            }
            McpCall::CloseDiffs { tab } => {
                self.mcp_close_diffs(tab.as_deref(), fx);
                Ok(json!("TAB_CLOSED"))
            }
            McpCall::OpenDiff {
                path,
                contents,
                tab,
            } => {
                return match self.mcp_text(path, disk) {
                    Ok(original) => {
                        self.mcp_proposals.push(McpProposal {
                            id,
                            tab: Some(tab.clone()),
                            summary: format!("edit of {}", self.display_path(path).display()),
                            files: vec![diff_file(path, &original, contents.clone())],
                        });
                        if self.focus == Focus::Claude {
                            self.mcp_open_review();
                        } else {
                            self.info("Claude Code proposes an edit: F9".to_owned());
                        }
                    }
                    Err(message) => Self::mcp_reply(id, Err(message), fx),
                };
            }
            McpCall::ProposeEdit { summary, edits } => {
                return match self.mcp_build_proposal(id, summary, edits, disk) {
                    Ok(proposal) => self.mcp_queue(proposal),
                    Err(message) => Self::mcp_reply(id, Err(message), fx),
                };
            }
        };
        Self::mcp_reply(id, answer.map(|value| value.to_string()), fx);
    }

    fn mcp_open_files(&self) -> Value {
        let active = self.active().map(|(_, d)| d.path().to_path_buf());
        let mut files: Vec<Value> = self
            .documents
            .values()
            .map(|d| {
                json!({
                    "path": d.path(),
                    "unsaved_changes": d.is_dirty(),
                    "active": active.as_deref() == Some(d.path()),
                })
            })
            .collect();
        files.sort_by_key(|f| f["path"].as_str().map(str::to_owned));
        json!({ "project": self.root, "files": files })
    }

    fn mcp_selection(&self) -> Value {
        let Some((view, d)) = self.active() else {
            return json!({ "file": null });
        };
        let text = d.text();
        let place = |char_index: usize| {
            let line = text.char_to_line(char_index);
            json!({ "line": line + 1, "column": char_index - text.line_to_char(line) + 1 })
        };
        let span = view.cursor.selection();
        let selected = span.clone().map(|s| {
            text.slice(s.start..s.end.min(s.start + MAX_CHARS))
                .to_string()
        });
        json!({
            "file": d.path(),
            "cursor": place(view.cursor.idx),
            "start": span.clone().map(|s| place(s.start)),
            "end": span.map(|s| place(s.end)),
            "text": selected,
        })
    }

    fn mcp_read(&self, path: &Path, text: &str, from: Option<usize>, to: Option<usize>) -> Value {
        let total = text.split_inclusive('\n').count();
        let first = from.unwrap_or(1).max(1);
        let last = to.unwrap_or(total).max(first);
        let wanted: String = text
            .split_inclusive('\n')
            .skip(first - 1)
            .take(last - first + 1)
            .collect();
        let truncated = wanted.chars().count() > MAX_CHARS;
        let unsaved = self
            .documents
            .values()
            .any(|d| d.path() == path && d.is_dirty());
        json!({
            "path": path,
            "unsaved_changes": unsaved,
            "total_lines": total,
            "from_line": first,
            "truncated": truncated,
            "text": wanted.chars().take(MAX_CHARS).collect::<String>(),
        })
    }

    fn ide_diagnostics(&self) -> Value {
        let mut files: BTreeMap<PathBuf, Vec<Value>> = BTreeMap::new();
        for problem in self.problems() {
            let place = json!({ "line": problem.line, "character": problem.col });
            let severity = match problem.severity {
                Severity::Error => "Error",
                Severity::Warning => "Warning",
                Severity::Info => "Information",
                Severity::Hint => "Hint",
            };
            files.entry(problem.path).or_default().push(json!({
                "message": problem.message,
                "severity": severity,
                "range": { "start": place, "end": place },
            }));
        }
        let list: Vec<Value> = files
            .into_iter()
            .map(|(path, diagnostics)| {
                json!({ "uri": format!("file://{}", path.display()), "diagnostics": diagnostics })
            })
            .collect();
        json!(list)
    }

    fn mcp_diagnostics(&self, only: Option<&Path>) -> Value {
        let items: Vec<Value> = self
            .problems()
            .into_iter()
            .filter(|p| only.is_none_or(|o| o == p.path))
            .map(|p| {
                json!({
                    "path": p.path,
                    "line": p.line + 1,
                    "column": p.col + 1,
                    "severity": severity_name(p.severity),
                    "message": p.message,
                })
            })
            .collect();
        json!({ "diagnostics": items })
    }

    fn mcp_run_output(&self, last: usize) -> Value {
        let Some(panel) = self.run_panel() else {
            return json!({ "running": false, "lines": [] });
        };
        let first = panel.first_line();
        let count = (first..).take_while(|n| panel.line(*n).is_some()).count();
        let lines: Vec<&str> = (first + count.saturating_sub(last)..first + count)
            .filter_map(|n| panel.line(n).map(|l| l.text.as_str()))
            .collect();
        let exit = match &panel.status {
            RunStatus::Finished { outcome, .. } => Some(format!("{outcome:?}")),
            _ => None,
        };
        json!({
            "name": panel.target.name,
            "running": panel.is_active(),
            "finished": exit,
            "lines": lines,
        })
    }

    fn mcp_build_proposal(
        &self,
        id: u64,
        summary: &str,
        edits: &[ProposedEdit],
        disk: &BTreeMap<PathBuf, Result<String, String>>,
    ) -> Result<McpProposal, String> {
        let mut paths: Vec<&PathBuf> = edits.iter().map(|e| &e.path).collect();
        paths.dedup();
        paths.sort();
        paths.dedup();
        let mut files = Vec::new();
        for path in paths {
            let original = self.mcp_text(path, disk)?;
            let own: Vec<&ProposedEdit> = edits.iter().filter(|e| &e.path == path).collect();
            files.push(propose_file(path, &original, &own)?);
        }
        Ok(McpProposal {
            id,
            tab: None,
            summary: summary.to_owned(),
            files,
        })
    }

    fn mcp_queue(&mut self, proposal: McpProposal) {
        self.mcp_proposals.push(proposal);
        self.info("Claude proposes an edit: Review Claude Edits in the Claude menu".to_owned());
        self.notify(
            crate::core::notify::NotifyLevel::Warning,
            "Claude proposes an edit",
            "",
            Some(crate::core::action::Action::ClaudeReview),
            true,
        );
    }

    pub(super) fn mcp_open_review(&mut self) {
        if self.mcp_proposals.is_empty() {
            return self.info("Nothing from Claude to review".to_owned());
        }
        self.mcp_review = Some(McpReview::default());
    }

    pub(super) fn mcp_review_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(review) = self.mcp_review.as_mut() else {
            return;
        };
        let count = self.mcp_proposals.first().map_or(0, |p| p.files.len());
        let scroll = |review: &mut McpReview, by: isize| {
            review.scroll = review.scroll.saturating_add_signed(by);
        };
        match input {
            Command::Escape => self.mcp_review = None,
            Command::Move(Motion::Up) => scroll(review, -1),
            Command::Move(Motion::Down) => scroll(review, 1),
            Command::Move(Motion::PageUp { rows }) => {
                scroll(review, -isize::try_from(rows).unwrap_or(1));
            }
            Command::Move(Motion::PageDown { rows }) => {
                scroll(review, isize::try_from(rows).unwrap_or(1));
            }
            Command::Indent if count > 0 => {
                review.file = (review.file + 1) % count;
                review.scroll = 0;
            }
            Command::Outdent if count > 0 => {
                review.file = (review.file + count - 1) % count;
                review.scroll = 0;
            }
            Command::Insert(s) => match s.to_lowercase().as_str() {
                "a" => self.mcp_decide(true, false, fx),
                "r" => self.mcp_decide(false, false, fx),
                "y" => self.mcp_decide(true, true, fx),
                "d" => self.mcp_decide(false, true, fx),
                _ => {}
            },
            _ => {}
        }
    }

    fn mcp_decide(&mut self, accept: bool, all: bool, fx: &mut Vec<Effect>) {
        let Some(chosen) = self.mcp_review.as_ref().map(|r| r.file) else {
            return;
        };
        let Some(count) = self.mcp_proposals.first().map(|p| p.files.len()) else {
            return;
        };
        let targets: Vec<usize> = (0..count)
            .filter(|i| all || *i == chosen)
            .filter(|i| {
                self.mcp_proposals
                    .first()
                    .is_some_and(|p| p.files[*i].decision.is_none())
            })
            .collect();
        for index in targets {
            let decision = if accept {
                self.mcp_apply(index, fx)
            } else {
                Decision::Rejected
            };
            if let Some(file) = self.mcp_proposals.first_mut().map(|p| &mut p.files[index]) {
                file.decision = Some(decision);
            }
        }
        if let (Some(review), Some(proposal)) =
            (self.mcp_review.as_mut(), self.mcp_proposals.first())
            && let Some(next) = (0..count)
                .map(|n| (chosen + n) % count)
                .find(|i| proposal.files[*i].decision.is_none())
        {
            if next != review.file {
                review.scroll = 0;
            }
            review.file = next;
        }
        self.mcp_settle(fx);
    }

    fn mcp_apply(&mut self, index: usize, fx: &mut Vec<Effect>) -> Decision {
        let Some(file) = self.mcp_proposals.first().map(|p| &p.files[index]) else {
            return Decision::Rejected;
        };
        let path = file.path.clone();
        if self.mcp_proposals.first().is_some_and(|p| p.tab.is_some()) {
            let unsaved = self
                .documents
                .values()
                .any(|d| d.path() == path && d.is_dirty());
            return if unsaved {
                Decision::Stale
            } else {
                Decision::Accepted
            };
        }
        if self.documents.values().any(|d| d.path() == path) {
            let (original, proposed) = (file.original.clone(), file.proposed.clone());
            return self.mcp_edit(&path, &original, &proposed, fx);
        }
        self.return_to = self.return_to.or(self.active_view());
        fx.push(Effect::ReadFile { path, at: None });
        Decision::Applying
    }

    fn mcp_edit(
        &mut self,
        path: &Path,
        original: &str,
        proposed: &str,
        fx: &mut Vec<Effect>,
    ) -> Decision {
        let Some((doc, d)) = self
            .documents
            .iter_mut()
            .find(|(_, d)| d.path() == path)
            .map(|(id, d)| (*id, d))
        else {
            return Decision::Stale;
        };
        if *d.text() != original {
            return Decision::Stale;
        }
        fx.push(Effect::Snapshot {
            path: path.to_path_buf(),
            bytes: d.to_bytes(),
            label: SnapshotLabel::BeforeAgent,
        });
        self.replace_text_keeping_cursor(doc, original, proposed);
        Decision::Accepted
    }

    pub(super) fn replace_text_keeping_cursor(&mut self, doc: DocumentId, old: &str, new: &str) {
        let (from, to, text) = smallest_change(old, new);
        let Some(d) = self.documents.get_mut(&doc) else {
            return;
        };
        d.close_undo_group();
        let applied = d.edit(from, to, &text, false);
        d.close_undo_group();
        let active = self
            .active_view()
            .filter(|v| self.views.get(v).is_some_and(|view| view.doc == doc));
        let cursor = active
            .and_then(|v| self.views.get(&v))
            .map(|v| v.cursor.idx);
        self.after_change(active, doc, &applied);
        if let Some(at) = cursor {
            let inserted = text.chars().count();
            let moved = if at <= from {
                at
            } else if at >= to {
                at - (to - from) + inserted
            } else {
                from + inserted
            };
            self.set_active_cursor(Cursor::at(moved));
        }
    }

    pub(super) fn mcp_file_read(&mut self, path: &Path, failed: bool, fx: &mut Vec<Effect>) {
        let waiting: Vec<(usize, usize, String, String)> = self
            .mcp_proposals
            .iter()
            .enumerate()
            .flat_map(|(p, proposal)| {
                proposal
                    .files
                    .iter()
                    .enumerate()
                    .filter(|(_, file)| {
                        file.path == path && file.decision == Some(Decision::Applying)
                    })
                    .map(move |(f, file)| (p, f, file.original.clone(), file.proposed.clone()))
            })
            .collect();
        if waiting.is_empty() {
            return;
        }
        for (p, f, original, proposed) in waiting {
            let decision = if failed {
                Decision::Stale
            } else {
                self.mcp_edit(path, &original, &proposed, fx)
            };
            self.mcp_proposals[p].files[f].decision = Some(decision);
        }
        self.mcp_settle(fx);
    }

    fn mcp_settle(&mut self, fx: &mut Vec<Effect>) {
        let settled = |p: &McpProposal| {
            p.files
                .iter()
                .all(|f| f.decision.is_some_and(|d| d != Decision::Applying))
        };
        let (done, left): (Vec<_>, Vec<_>) = std::mem::take(&mut self.mcp_proposals)
            .into_iter()
            .partition(settled);
        self.mcp_proposals = left;
        for proposal in &done {
            Self::mcp_reply(proposal.id, Ok(proposal_answer(proposal)), fx);
        }
        let open = |p: &McpProposal| p.files.iter().any(|f| f.decision.is_none());
        if !done.is_empty()
            && let Some(review) = self.mcp_review.as_mut()
        {
            *review = McpReview::default();
        }
        if !self.mcp_proposals.iter().any(open) {
            self.mcp_review = None;
        }
        let applying = self.mcp_proposals.iter().any(|p| {
            p.files
                .iter()
                .any(|f| f.decision == Some(Decision::Applying))
        });
        if !applying && let Some(view) = self.return_to.take() {
            self.focus_view(view, None);
        }
    }

    pub(super) fn mcp_close_diffs(&mut self, tab: Option<&str>, fx: &mut Vec<Effect>) {
        let closing =
            |p: &McpProposal| p.tab.is_some() && (tab.is_none() || p.tab.as_deref() == tab);
        let (closed, kept): (Vec<_>, Vec<_>) = std::mem::take(&mut self.mcp_proposals)
            .into_iter()
            .partition(closing);
        self.mcp_proposals = kept;
        for proposal in closed {
            let answer = serde_json::to_string(&["TAB_CLOSED"]).unwrap_or_default();
            Self::mcp_reply(proposal.id, Ok(answer), fx);
        }
        if self.mcp_proposals.is_empty() {
            self.mcp_review = None;
        } else if let Some(review) = self.mcp_review.as_mut() {
            *review = McpReview::default();
        }
    }

    pub(super) fn claude_new_window(&mut self, fx: &mut Vec<Effect>) {
        if self.window {
            return self.info("Claude Code opens from the main window".to_owned());
        }
        fx.push(Effect::OpenClaude {
            root: self.root.clone(),
        });
    }
}
