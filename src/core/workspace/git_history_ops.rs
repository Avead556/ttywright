use std::path::{Path, PathBuf};

use super::*;
use crate::core::command::{Command, Effect, Motion};
use crate::core::git::{GitDiffSource, GitField, GitFileCommit, GitNotice, GitTab};
use crate::core::git_command::GitEffect;

impl Workspace {
    pub(super) fn git_show_history(&mut self, fx: &mut Vec<Effect>) {
        let path = self
            .active()
            .map(|(_, d)| d.path().to_path_buf())
            .or_else(|| self.git_panel.as_ref()?.history_path.clone());
        let Some(path) = path else {
            self.info("Open a file to see its history".to_owned());
            return self.git_notice("Open a file to see its history", true);
        };
        self.git_show(GitTab::History, fx);
        let root = self.git_root();
        if let Some(panel) = self.git_panel.as_mut() {
            if panel.history_path.as_ref() != Some(&path) {
                panel.history.clear();
                panel.history_selected = 0;
                panel.clear_diff();
            }
            panel.history_path = Some(path.clone());
            panel.history_loading = true;
        }
        fx.push(Effect::Git(GitEffect::FileHistory { root, path }));
    }

    pub(super) fn git_history_read(
        &mut self,
        path: &Path,
        result: Result<Vec<GitFileCommit>, String>,
        now: u64,
        fx: &mut Vec<Effect>,
    ) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        if panel.history_path.as_deref() != Some(path) {
            return;
        }
        panel.history_loading = false;
        panel.now = now;
        match result {
            Ok(list) => {
                panel.history = list;
                panel.history_selected = panel
                    .history_selected
                    .min(panel.history.len().saturating_sub(1));
                self.git_show_history_diff(true, fx);
            }
            Err(text) => {
                panel.history.clear();
                panel.clear_diff();
                panel.notice = Some(GitNotice { text, error: true });
            }
        }
    }

    fn git_show_history_diff(&mut self, fresh: bool, fx: &mut Vec<Effect>) {
        let source = self.git_panel.as_ref().and_then(|p| {
            let entry = p.history.get(p.history_selected)?;
            Some(GitDiffSource::Commit {
                hash: entry.commit.hash.clone(),
                file: entry.file.clone(),
            })
        });
        self.git_request_diff(source, fresh, fx);
    }

    pub(super) fn git_history_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        panel.field = GitField::List;
        let last = panel.history.len().saturating_sub(1);
        let target = match input {
            Command::Escape => return self.git_leave(),
            Command::Insert(s) => {
                return match s.to_lowercase().as_str() {
                    "l" => self.git_show(GitTab::Log, fx),
                    "c" => self.git_show(GitTab::Commit, fx),
                    "z" => self.git_show_stash(fx),
                    "g" => self.git_show_branches(fx),
                    "r" => self.git_show_history(fx),
                    _ => {}
                };
            }
            Command::Newline => {
                let path = panel.history_path.clone();
                if let Some(path) = path {
                    self.git_open_absolute(path, fx);
                }
                return;
            }
            Command::Move(Motion::Up) => panel.history_selected.saturating_sub(1),
            Command::Move(Motion::Down) => (panel.history_selected + 1).min(last),
            Command::Move(Motion::DocStart) => 0,
            Command::Move(Motion::DocEnd) => last,
            Command::SelectGitRow { index } => index.min(last),
            _ => return,
        };
        if target != panel.history_selected {
            panel.history_selected = target;
            self.git_show_history_diff(true, fx);
        }
    }

    fn git_open_absolute(&mut self, path: PathBuf, fx: &mut Vec<Effect>) {
        if self.window {
            fx.push(Effect::OpenInMainWindow {
                root: self.git_root(),
                path,
                at: None,
            });
        } else {
            self.open_file(path, None, fx);
        }
    }
}
