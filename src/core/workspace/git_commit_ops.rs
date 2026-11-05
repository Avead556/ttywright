use std::path::{Path, PathBuf};

use super::*;
use crate::core::command::{Command, Effect};
use crate::core::git::{CommitMode, GitChange, GitField, GitFile};
use crate::core::git_command::GitEffect;

impl Workspace {
    pub(super) fn git_message_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        match input {
            Command::Escape | Command::Indent => panel.field = GitField::List,
            Command::Outdent => {
                panel.field = if panel.hunks.is_empty() {
                    GitField::List
                } else {
                    GitField::Hunks
                };
            }
            Command::Insert(s) => panel.message.insert_lines(&s),
            Command::DeleteBackward => panel.message.delete_backward(),
            Command::DeleteForward => panel.message.delete_forward(),
            Command::Move(motion) => move_input(&mut panel.message, motion),
            Command::Newline => self.git_commit(fx),
            Command::SelectGitRow { .. } => {
                panel.field = GitField::List;
                self.git_files_input(input, fx);
            }
            _ => {}
        }
    }

    pub(super) fn git_commit(&mut self, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_ref() else {
            return;
        };
        if panel.busy {
            return;
        }
        if panel.commit_mode() == CommitMode::Staged {
            return self.git_commit_staged(fx);
        }
        let message = panel.message.text().trim().to_owned();
        if panel.checked.is_empty() && !panel.amend {
            return self.git_notice("Tick the files to commit", true);
        }
        if message.is_empty() {
            return self.git_notice("Write a commit message", true);
        }
        let chosen: Vec<&GitFile> = panel
            .files
            .iter()
            .filter(|f| panel.checked.contains(&f.path))
            .collect();
        let untracked: Vec<PathBuf> = chosen
            .iter()
            .filter(|f| f.change == GitChange::Untracked)
            .map(|f| f.path.clone())
            .collect();
        let mut paths: Vec<PathBuf> = chosen.iter().map(|f| f.path.clone()).collect();
        paths.extend(chosen.iter().filter_map(|f| f.old_path.clone()));
        let conflicted = self
            .documents
            .values()
            .any(|d| d.is_dirty() && d.conflict().is_some());
        if conflicted {
            return self.git_notice(
                "A file changed on disk has unsaved edits: resolve that first",
                true,
            );
        }
        self.autosave(fx);
        if self.git_checks_wanted(false, &paths, &untracked, fx) {
            return;
        }
        let root = self.git_root();
        let amend = self.git_panel.as_ref().is_some_and(|p| p.amend);
        fx.push(Effect::Git(GitEffect::Commit {
            root,
            message,
            paths,
            untracked,
            amend,
        }));
        if let Some(panel) = self.git_panel.as_mut() {
            panel.busy = true;
            panel.notice = None;
            panel.checks_passed = false;
        }
    }

    pub(super) fn git_committed(&mut self, result: Result<String, String>, fx: &mut Vec<Effect>) {
        if let Some(panel) = self.git_panel.as_mut() {
            panel.busy = false;
        }
        match result {
            Ok(summary) => {
                if let Some(panel) = self.git_panel.as_mut() {
                    panel.message = crate::core::input::LineInput::default();
                    panel.field = GitField::List;
                    panel.amend = false;
                    panel.mode_choice = None;
                }
                self.git_notice(format!("Committed {summary}"), false);
                self.info(format!("Committed {summary}"));
            }
            Err(text) => {
                self.git_notice(text.clone(), true);
                self.error(format!("Commit failed: {text}"));
            }
        }
        self.git_refresh_all(fx);
        fx.push(Effect::Git(GitEffect::Log {
            root: self.git_root(),
        }));
    }

    pub(super) fn git_refresh_all(&mut self, fx: &mut Vec<Effect>) {
        self.git_blame_expire();
        let root = self.git_root();
        fx.push(Effect::Git(GitEffect::Status { root: root.clone() }));
        for d in self.documents.values() {
            fx.push(Effect::Git(GitEffect::Base {
                root: root.clone(),
                path: d.path().to_path_buf(),
            }));
        }
    }

    pub(super) fn git_toggle_amend(&mut self, fx: &mut Vec<Effect>) {
        let root = self.git_root();
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        panel.amend = !panel.amend;
        if panel.amend {
            fx.push(Effect::Git(GitEffect::ReadLastCommit { root }));
        } else {
            panel.message = crate::core::input::LineInput::default();
            panel.notice = None;
        }
    }

    pub(super) fn git_last_commit(&mut self, result: Result<(String, bool), String>) {
        let Some(panel) = self.git_panel.as_mut().filter(|p| p.amend) else {
            return;
        };
        match result {
            Ok((message, pushed)) => {
                if panel.message.text().is_empty() {
                    panel.message.insert_lines(message.trim_end());
                }
                panel.field = GitField::Message;
                if pushed {
                    self.git_notice(
                        "This commit is already pushed: amending it needs a force push",
                        true,
                    );
                }
            }
            Err(text) => {
                panel.amend = false;
                self.git_notice(text, true);
            }
        }
    }

    pub(super) fn git_rollback(&mut self, fx: &mut Vec<Effect>) {
        let top = self.repo.top_or(&self.root);
        let conflicted = self
            .documents
            .values()
            .any(|d| d.is_dirty() && d.conflict().is_some());
        let root = self.git_root();
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        let Some(file) = panel.files.get(panel.file_selected).cloned() else {
            return;
        };
        if matches!(file.change, GitChange::Untracked | GitChange::Conflicted) {
            panel.confirm_rollback = None;
            let reason = if file.change == GitChange::Untracked {
                "An untracked file has nothing to roll back to"
            } else {
                "Resolve the conflict first"
            };
            return self.git_notice(reason, true);
        }
        if panel.confirm_rollback.as_ref() != Some(&file.path) {
            let text = format!("Press U again to roll back {}", file.path.display());
            panel.confirm_rollback = Some(file.path);
            return self.git_notice(text, true);
        }
        panel.confirm_rollback = None;
        if conflicted {
            return self.git_notice(
                "A file changed on disk has unsaved edits: resolve that first",
                true,
            );
        }
        self.autosave(fx);
        let absolute = top.join(&file.path);
        fx.push(Effect::Git(GitEffect::Rollback {
            root,
            file,
            absolute,
        }));
        if let Some(panel) = self.git_panel.as_mut() {
            panel.busy = true;
        }
    }

    pub(super) fn git_rolled_back(
        &mut self,
        path: &Path,
        result: Result<(), String>,
        fx: &mut Vec<Effect>,
    ) {
        if let Some(panel) = self.git_panel.as_mut() {
            panel.busy = false;
        }
        match result {
            Ok(()) => self.git_notice(
                format!(
                    "Rolled back {} (the old text is in Local History)",
                    path.display()
                ),
                false,
            ),
            Err(text) => self.git_notice(text, true),
        }
        fx.extend(self.check_docs(|_| true));
        self.git_refresh_all(fx);
    }
}
