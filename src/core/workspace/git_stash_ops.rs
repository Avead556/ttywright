use super::*;
use crate::core::command::{Command, Effect, Motion};
use crate::core::git::{GitDiffSource, GitField, GitNotice, GitOp, GitStash, GitTab};
use crate::core::git_command::GitEffect;
use crate::core::input::LineInput;

impl Workspace {
    pub(super) fn git_show_stash(&mut self, fx: &mut Vec<Effect>) {
        self.git_show(GitTab::Stash, fx);
        fx.push(Effect::Git(GitEffect::ListStashes {
            root: self.git_root(),
        }));
    }

    pub(super) fn git_stashes_read(
        &mut self,
        result: Result<Vec<GitStash>, String>,
        now: u64,
        fx: &mut Vec<Effect>,
    ) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        panel.now = now;
        match result {
            Ok(list) => {
                panel.stashes = list;
                panel.stash_selected = panel
                    .stash_selected
                    .min(panel.stashes.len().saturating_sub(1));
                if panel.tab == GitTab::Stash {
                    self.git_show_stash_diff(fx);
                }
            }
            Err(text) => panel.notice = Some(GitNotice { text, error: true }),
        }
    }

    fn git_show_stash_diff(&mut self, fx: &mut Vec<Effect>) {
        let source = self
            .git_panel
            .as_ref()
            .and_then(|p| p.stashes.get(p.stash_selected))
            .map(|s| GitDiffSource::Stash(s.hash.clone()));
        self.git_request_diff(source, true, fx);
    }

    pub(super) fn git_stash_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        panel.field = GitField::List;
        let last = panel.stashes.len().saturating_sub(1);
        let chosen = panel.stashes.get(panel.stash_selected).cloned();
        let target = match input {
            Command::Escape => return self.git_leave(),
            Command::Insert(s) => {
                match (s.to_lowercase().as_str(), chosen) {
                    ("n", _) => {
                        panel.field = GitField::StashMessage;
                        panel.notice = None;
                    }
                    ("c", _) => self.git_show(GitTab::Commit, fx),
                    ("l", _) => self.git_show(GitTab::Log, fx),
                    ("g", _) => self.git_show_branches(fx),
                    ("r", _) => self.git_show_stash(fx),
                    ("a", Some(entry)) => self.git_run_op(GitOp::StashApply(entry), Vec::new(), fx),
                    ("o", Some(entry)) => self.git_run_op(GitOp::StashPop(entry), Vec::new(), fx),
                    ("d", Some(entry)) => self.git_stash_drop(entry, fx),
                    _ => {}
                }
                return;
            }
            Command::Move(Motion::Up) => panel.stash_selected.saturating_sub(1),
            Command::Move(Motion::Down) => (panel.stash_selected + 1).min(last),
            Command::SelectGitRow { index } => index.min(last),
            _ => return,
        };
        if target != panel.stash_selected {
            panel.stash_selected = target;
            self.git_show_stash_diff(fx);
        }
    }

    fn git_stash_drop(&mut self, entry: GitStash, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        if panel.confirm_drop.as_ref() != Some(&entry.hash) {
            let text = format!(
                "Press D again to drop stash@{{{}}} ({}): its changes are lost",
                entry.index, entry.subject
            );
            panel.confirm_drop = Some(entry.hash);
            return self.git_notice(text, true);
        }
        panel.confirm_drop = None;
        self.git_run_op(GitOp::StashDrop(entry), Vec::new(), fx);
    }

    pub(super) fn git_stash_message_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        match input {
            Command::Escape => {
                panel.field = GitField::List;
                panel.stash_message = LineInput::default();
            }
            Command::Indent | Command::Outdent => panel.stash_untracked = !panel.stash_untracked,
            Command::Insert(s) => panel.stash_message.insert(&s),
            Command::DeleteBackward => panel.stash_message.delete_backward(),
            Command::DeleteForward => panel.stash_message.delete_forward(),
            Command::Move(motion) => move_input(&mut panel.stash_message, motion),
            Command::Newline => {
                let op = GitOp::StashPush {
                    message: panel.stash_message.text().to_owned(),
                    untracked: panel.stash_untracked,
                };
                panel.field = GitField::List;
                panel.stash_message = LineInput::default();
                panel.stash_selected = 0;
                self.git_run_op(op, Vec::new(), fx);
            }
            _ => {}
        }
    }
}
