use super::*;
use crate::core::command::{Command, Effect, Motion};
use crate::core::git::{GitBranch, GitField, GitOp, GitTab};
use crate::core::git_command::GitEffect;
use crate::core::picker::PickerKind;

impl Workspace {
    pub(super) fn git_show_branches(&mut self, fx: &mut Vec<Effect>) {
        self.git_show(GitTab::Branches, fx);
        fx.push(Effect::Git(GitEffect::ListBranches {
            root: self.git_root(),
        }));
    }

    pub(super) fn git_branches_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        panel.field = GitField::List;
        let last = self.repo.branches().len().saturating_sub(1);
        let chosen = self.repo.branches().get(panel.branch_selected).cloned();
        let current = chosen.as_ref().is_some_and(|b| self.repo.is_current(b));
        match input {
            Command::Escape => self.git_leave(),
            Command::Move(Motion::Up) => {
                panel.branch_selected = panel.branch_selected.saturating_sub(1);
            }
            Command::Move(Motion::Down) => {
                panel.branch_selected = (panel.branch_selected + 1).min(last);
            }
            Command::SelectGitRow { index } => panel.branch_selected = index.min(last),
            Command::Newline => {
                if let Some(branch) = chosen.filter(|_| !current) {
                    self.git_switch(branch, false, fx);
                }
            }
            Command::Insert(s) => match s.to_lowercase().as_str() {
                "n" => self.open_picker(PickerKind::Branches),
                "d" => {
                    if let Some(branch) = chosen {
                        self.git_branch_delete(&branch, false, fx);
                    }
                }
                "f" => {
                    let asked = panel.confirm_force.take();
                    if let Some(branch) = chosen.filter(|b| asked.as_ref() == Some(&b.name)) {
                        self.git_branch_delete(&branch, true, fx);
                    }
                }
                "m" => match chosen {
                    Some(branch) if !current => {
                        self.git_run_op(
                            GitOp::Merge {
                                branch: branch.name,
                            },
                            Vec::new(),
                            fx,
                        );
                    }
                    _ => self.git_notice("Choose another branch to merge into this one", true),
                },
                "c" => self.git_show(GitTab::Commit, fx),
                "l" => self.git_show(GitTab::Log, fx),
                "z" => self.git_show_stash(fx),
                "r" => self.git_show_branches(fx),
                _ => {}
            },
            _ => {}
        }
    }

    fn git_branch_delete(&mut self, branch: &GitBranch, force: bool, fx: &mut Vec<Effect>) {
        if branch.remote {
            return self.git_notice(
                "A remote branch is not deleted from here: that is a push",
                true,
            );
        }
        let op = GitOp::BranchDelete {
            name: branch.name.clone(),
            force,
        };
        self.git_run_op(op, Vec::new(), fx);
    }

    pub(super) fn git_ask_branches(&mut self, fx: &mut Vec<Effect>) {
        self.git_branch_picker_asked = true;
        self.autosave(fx);
        self.info("Reading branches…".to_owned());
        fx.push(Effect::Git(GitEffect::ListBranches {
            root: self.git_root(),
        }));
    }

    pub(super) fn git_branches_read(&mut self, result: Result<Vec<GitBranch>, String>) {
        match result {
            Ok(branches) => {
                self.repo.set_branches(branches);
                if let Some(panel) = self.git_panel.as_mut() {
                    panel.branch_selected = panel
                        .branch_selected
                        .min(self.repo.branches().len().saturating_sub(1));
                }
                if let Some(panel) = self.git_panel.as_mut()
                    && let Some(target) = panel.compare_asked.take()
                {
                    panel.compare_target = target;
                    self.git_branch_picker_asked = false;
                    self.status = None;
                    return self.open_picker(PickerKind::CompareBranches);
                }
                if std::mem::take(&mut self.git_branch_picker_asked) {
                    self.status = None;
                    if self.picker.is_none() && self.local_history.is_none() {
                        self.open_picker(PickerKind::Branches);
                    }
                }
            }
            Err(text) => self.error(text),
        }
    }

    pub(super) fn git_switch(&mut self, branch: GitBranch, create: bool, fx: &mut Vec<Effect>) {
        self.autosave(fx);
        fx.push(Effect::Git(GitEffect::Switch {
            root: self.git_root(),
            branch,
            create,
        }));
    }

    pub(super) fn git_switched(
        &mut self,
        name: &str,
        result: Result<(), String>,
        fx: &mut Vec<Effect>,
    ) {
        match result {
            Ok(()) => self.info(format!("On branch {name}")),
            Err(text) => self.error(format!("Could not switch: {text}")),
        }
        fx.extend(self.check_docs(|_| true));
        self.git_refresh_all(fx);
    }
}
