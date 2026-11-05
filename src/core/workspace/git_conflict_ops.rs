use std::path::PathBuf;

use super::*;
use crate::core::command::Effect;
use crate::core::conflict::{
    ConflictChoice, ConflictRegion, conflict_at, conflict_regions, resolution, step_conflict,
};
use crate::core::git::{GitChange, GitFile, GitOp, GitOperation, GitTab};
use crate::core::git_command::GitEffect;

impl Workspace {
    fn git_selected_is_active(&self, file: &GitFile) -> bool {
        let top = self.repo.top_or(&self.root);
        self.active()
            .is_some_and(|(_, d)| d.path() == top.join(&file.path))
    }

    fn active_conflicts(&self) -> Vec<ConflictRegion> {
        self.active()
            .map(|(_, d)| conflict_regions(d.text()))
            .unwrap_or_default()
    }

    pub fn conflict_position(&self, doc: DocumentId, line: usize) -> Option<(usize, usize)> {
        let d = self.documents.get(&doc)?;
        if self.git_change(d.path()) != Some(GitChange::Conflicted) {
            return None;
        }
        let regions = conflict_regions(d.text());
        let at = regions.iter().position(|r| line <= r.end);
        Some((at.map_or(regions.len(), |i| i + 1), regions.len()))
    }

    pub(super) fn git_step_conflict(&mut self, forward: bool, _fx: &mut [Effect]) {
        let Some((view, doc)) = self.active() else {
            return self.info("Open a file with conflicts".to_owned());
        };
        let line = doc.text().char_to_line(view.cursor.idx);
        match step_conflict(&conflict_regions(doc.text()), line, forward) {
            Some(target) => self.git_put_cursor(target),
            None => self.info("No conflict markers in this file".to_owned()),
        }
    }

    pub(super) fn git_put_cursor(&mut self, line: usize) {
        let at = self.active().map(|(_, d)| d.text().line_to_char(line));
        if let Some(at) = at {
            self.set_active_cursor(Cursor::at(at));
        }
    }

    pub(super) fn git_accept(&mut self, choice: ConflictChoice) {
        let Some((view, doc)) = self.active() else {
            return self.info("Open a file with conflicts".to_owned());
        };
        let line = doc.text().char_to_line(view.cursor.idx);
        let Some(region) = conflict_at(&conflict_regions(doc.text()), line) else {
            return self.info("No conflict here or below the cursor".to_owned());
        };
        let (from, to, kept) = resolution(doc.text(), &region, choice);
        self.close_undo_group();
        self.edit(from, to, &kept, false);
        self.close_undo_group();
        self.set_active_cursor(Cursor::at(from));
        let left = self.active_conflicts().len();
        let text = if left == 0 {
            "No conflicts left in this file: Space marks it resolved".to_owned()
        } else {
            format!("{left} conflicts left in this file")
        };
        self.git_notice(text.clone(), false);
        self.info(text);
    }

    pub(super) fn git_conflict_step_key(&mut self, forward: bool, fx: &mut Vec<Effect>) {
        let Some(file) = self
            .git_panel
            .as_ref()
            .and_then(|p| p.selected_file().cloned())
        else {
            return;
        };
        if self.git_selected_is_active(&file) {
            self.git_step_conflict(forward, fx);
        } else {
            self.git_open_conflict(&file, fx);
        }
    }

    pub(super) fn git_conflict_key(&mut self, choice: ConflictChoice, fx: &mut Vec<Effect>) {
        let Some(file) = self
            .git_panel
            .as_ref()
            .and_then(|p| p.selected_file().cloned())
        else {
            return;
        };
        if file.code.index == 'D' || file.code.worktree == 'D' {
            return self.git_notice(
                "One side deleted it, so there are no markers: Space keeps the folder as it is",
                true,
            );
        }
        if !self.git_selected_is_active(&file) {
            self.git_open_conflict(&file, fx);
            return self.git_notice(
                "Opened at its first conflict: look, then press again",
                false,
            );
        }
        self.git_accept(choice);
    }

    pub(super) fn git_open_conflict(&mut self, file: &GitFile, fx: &mut Vec<Effect>) {
        let top = self.repo.top_or(&self.root);
        let path = top.join(&file.path);
        if self.window {
            fx.push(Effect::OpenInMainWindow {
                root: self.git_root(),
                path,
                at: None,
            });
            return;
        }
        self.git_conflict_jump = Some((path.clone(), self.focus));
        self.open_file(path, None, fx);
    }

    pub(super) fn git_conflict_catch_up(&mut self) {
        let Some((path, focus)) = self.git_conflict_jump.clone() else {
            return;
        };
        let (line, here) = match self.active() {
            Some((_, d)) if d.path() == path => {
                (conflict_regions(d.text()).first().map(|r| r.start), true)
            }
            _ => (None, false),
        };
        if here {
            self.git_conflict_jump = None;
            self.focus = focus;
            if let Some(line) = line {
                self.git_put_cursor(line);
            }
        }
    }

    pub(super) fn git_mark_resolved(&mut self, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_ref() else {
            return;
        };
        let Some(file) = panel.selected_file().cloned() else {
            return;
        };
        let confirmed = panel.confirm_resolve.as_ref() == Some(&file.path);
        let deleted = file.code.index == 'D' || file.code.worktree == 'D';
        if !confirmed && !deleted {
            let open = self.git_selected_is_active(&file);
            let left = if open {
                self.active_conflicts().len()
            } else {
                0
            };
            if !open || left > 0 {
                if !open {
                    self.git_open_conflict(&file, fx);
                }
                let what = if open {
                    format!("{left} conflicts are still in it")
                } else {
                    "It has not been looked at".to_owned()
                };
                if let Some(panel) = self.git_panel.as_mut() {
                    panel.confirm_resolve = Some(file.path);
                }
                return self.git_notice(
                    format!("{what}: press Space again to mark it resolved as it is"),
                    true,
                );
            }
        }
        if let Some(panel) = self.git_panel.as_mut() {
            panel.confirm_resolve = None;
        }
        if self.git_begin(fx) {
            fx.push(Effect::Git(GitEffect::StageFiles {
                root: self.git_root(),
                files: vec![file],
                stage: true,
            }));
        }
    }

    fn git_operation_now(&self) -> Option<GitOperation> {
        self.git_panel
            .as_ref()
            .and_then(|p| p.repo.operation)
            .or(self.repo.operation())
    }

    pub(super) fn git_run_op(&mut self, op: GitOp, keep: Vec<PathBuf>, fx: &mut Vec<Effect>) {
        if self.git_begin(fx) {
            fx.push(Effect::Git(GitEffect::Op {
                root: self.git_root(),
                op,
                keep,
            }));
        }
    }

    pub(super) fn git_continue(&mut self, fx: &mut Vec<Effect>) {
        self.git_make_panel(fx);
        match self.git_operation_now() {
            Some(operation) => self.git_run_op(GitOp::Continue(operation), Vec::new(), fx),
            None => self.git_notice("Nothing to continue", true),
        }
    }

    pub(super) fn git_abort(&mut self, fx: &mut Vec<Effect>) {
        self.git_make_panel(fx);
        let Some(operation) = self.git_operation_now() else {
            let text = if self
                .git_panel
                .as_ref()
                .is_some_and(|p| p.files.iter().any(|f| f.change == GitChange::Conflicted))
            {
                "Nothing to abort: these conflicts came from a stash, which is kept"
            } else {
                "Nothing to abort"
            };
            return self.git_notice(text, true);
        };
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        if !panel.confirm_abort {
            panel.confirm_abort = true;
            let name = match operation {
                GitOperation::Merging => "merge",
                GitOperation::Rebasing => "rebase",
                GitOperation::CherryPicking => "cherry-pick",
                GitOperation::Reverting => "revert",
            };
            let count = panel.files.len();
            return self.git_notice(
                format!(
                    "Press X again to abort the {name}: everything resolved or edited in \
                     {count} files since it began is discarded (kept in Local History)"
                ),
                true,
            );
        }
        panel.confirm_abort = false;
        let top = self.repo.top_or(&self.root);
        let keep = panel.files.iter().map(|f| top.join(&f.path)).collect();
        self.git_run_op(GitOp::Abort(operation), keep, fx);
    }

    fn git_make_panel(&mut self, fx: &mut Vec<Effect>) {
        let showing = self.focus == crate::core::workspace::Focus::Git
            && self
                .git_panel
                .as_ref()
                .is_some_and(|p| p.tab == GitTab::Commit);
        if !showing {
            self.git_show(GitTab::Commit, fx);
        }
    }

    pub(super) fn git_op_done(
        &mut self,
        op: &GitOp,
        result: Result<String, String>,
        fx: &mut Vec<Effect>,
    ) {
        if let Some(panel) = self.git_panel.as_mut() {
            panel.busy = false;
        }
        let conflict = result
            .as_ref()
            .is_err_and(|text| text.contains("CONFLICT") || text.contains("conflict"));
        match result {
            Ok(words) => {
                let text = if words.is_empty() {
                    "Done".to_owned()
                } else {
                    words
                };
                self.git_notice(text, false);
            }
            Err(text) => {
                if let GitOp::BranchDelete { name, force: false } = op
                    && let Some(panel) = self.git_panel.as_mut()
                    && text.contains("not fully merged")
                {
                    panel.confirm_force = Some(name.clone());
                    let warned = format!(
                        "Press F to force: commits only {name} has become unreachable.  {text}"
                    );
                    self.git_notice(warned, true);
                } else {
                    self.git_notice(text, true);
                }
            }
        }
        fx.extend(self.check_docs(|_| true));
        self.git_refresh_all(fx);
        let root = self.git_root();
        fx.push(Effect::Git(GitEffect::Log { root: root.clone() }));
        match self.git_panel.as_ref().map(|p| p.tab) {
            Some(GitTab::Stash) => {
                fx.push(Effect::Git(GitEffect::ListStashes { root: root.clone() }))
            }
            Some(GitTab::Branches) => {
                fx.push(Effect::Git(GitEffect::ListBranches { root: root.clone() }))
            }
            _ => {}
        }
        if conflict {
            let notice = self.git_panel.as_ref().and_then(|p| p.notice.clone());
            self.git_show(GitTab::Commit, fx);
            if let Some(panel) = self.git_panel.as_mut() {
                panel.notice = notice;
            }
        }
    }
}
