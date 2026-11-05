use crate::core::git_command::GitEffect;
use std::path::{Path, PathBuf};

use super::{Effect, Focus, Severity, Workspace};
use crate::core::command::{Command, Motion};
use crate::core::git::{CommitChecks, CommitMode, GitCompare, GitCompared, GitDiffSource, GitTab};
use crate::core::picker::PickerKind;

const NO_LIST: &str = "(no changelist)";

impl Workspace {
    pub fn configure_commit_checks(&mut self, checks: CommitChecks) {
        self.commit_checks = checks;
    }

    pub fn changelists(&self) -> &crate::core::git::Changelists {
        &self.changelists
    }

    pub(super) fn git_compare_ask(&mut self, path: Option<PathBuf>, fx: &mut Vec<Effect>) {
        let tab = self.git_panel.as_ref().map_or(GitTab::Commit, |p| p.tab);
        self.open_git(tab, fx);
        if let Some(panel) = self.git_panel.as_mut() {
            panel.compare_asked = Some(path);
        }
        self.git_ask_branches(fx);
    }

    pub(super) fn git_compare_path(&mut self, fx: &mut Vec<Effect>) {
        let Some(path) = self.file_target() else {
            return self.info("Open a file or choose one in the Project tree".to_owned());
        };
        self.git_compare_ask(Some(path), fx);
    }

    pub(super) fn git_compare_start(&mut self, base: String, fx: &mut Vec<Effect>) {
        let root = self.git_root();
        self.autosave(fx);
        self.git_show(GitTab::Compare, fx);
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        let path = panel.compare_target.take();
        panel.clear_diff();
        panel.compare = Some(GitCompare {
            base: base.clone(),
            path: path.clone(),
            found: GitCompared::default(),
            selected: 0,
            loading: true,
        });
        fx.push(Effect::Git(GitEffect::Compare { root, base, path }));
    }

    pub(super) fn git_compared(
        &mut self,
        base: &str,
        path: Option<&Path>,
        result: Result<GitCompared, String>,
        fx: &mut Vec<Effect>,
    ) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        let Some(compare) = panel
            .compare
            .as_mut()
            .filter(|c| c.base == base && c.path.as_deref() == path)
        else {
            return;
        };
        compare.loading = false;
        match result {
            Ok(found) => {
                compare.found = found;
                compare.selected = 0;
                self.git_compare_show_row(fx);
            }
            Err(text) => self.git_notice(text, true),
        }
    }

    fn git_compare_show_row(&mut self, fx: &mut Vec<Effect>) {
        let source = self.git_panel.as_ref().and_then(|p| {
            let compare = p.compare.as_ref()?;
            if let Some(entry) = compare.selected_commit() {
                return Some(GitDiffSource::WholeCommit(entry.commit.hash.clone()));
            }
            Some(GitDiffSource::Compare {
                base: compare.base.clone(),
                worktree: compare.path.is_some(),
                file: compare.selected_file()?.clone(),
            })
        });
        self.git_request_diff(source, true, fx);
    }

    pub(super) fn git_compare_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        let Some(compare) = panel.compare.as_mut() else {
            return;
        };
        let last = compare.row_count().saturating_sub(1);
        let target = match input {
            Command::Escape => return self.git_leave(),
            Command::Insert(s) => {
                return match s.to_lowercase().as_str() {
                    "l" => self.git_show(GitTab::Log, fx),
                    "c" => self.git_show(GitTab::Commit, fx),
                    "z" => self.git_show_stash(fx),
                    "g" => self.git_show_branches(fx),
                    _ => {}
                };
            }
            Command::Newline => {
                let file = compare.selected_file().map(|f| f.path.clone());
                return self.git_open_file(file, fx);
            }
            Command::Move(Motion::Up) => compare.selected.saturating_sub(1),
            Command::Move(Motion::Down) => (compare.selected + 1).min(last),
            Command::SelectGitRow { index } => index.min(last),
            _ => return,
        };
        if target != compare.selected {
            compare.selected = target;
            self.git_compare_show_row(fx);
        }
    }

    pub(super) fn changelist_rows(&self) -> Vec<String> {
        let mut rows = vec![NO_LIST.to_owned()];
        rows.extend(self.changelists.names().map(str::to_owned));
        rows
    }

    pub(super) fn changelist_ask(&mut self) {
        let Some(path) = self
            .git_panel
            .as_ref()
            .and_then(|p| p.selected_file())
            .map(|f| f.path.clone())
        else {
            return;
        };
        self.changelist_target = Some(path);
        self.open_picker(PickerKind::Changelists);
    }

    pub(super) fn changelist_chosen(&mut self, chosen: Option<usize>, typed: &str) {
        let Some(path) = self.changelist_target.take() else {
            return;
        };
        let rows = self.changelist_rows();
        match chosen.and_then(|i| rows.get(i)) {
            Some(row) if row == NO_LIST => self.changelists.assign(&path, None),
            Some(name) => self.changelists.assign(&path, Some(name)),
            None if !typed.is_empty() => {
                let name: String = typed.chars().filter(|c| !c.is_control()).collect();
                self.changelists.assign(&path, Some(&name));
            }
            None => {}
        }
    }

    pub(super) fn changelist_tick_next(&mut self) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        let current = panel.changelist_ticked.as_deref();
        let Some(name) = self.changelists.next_after(current).map(str::to_owned) else {
            return self.git_notice("No changelists yet: E moves a file to one", true);
        };
        let files = self
            .changelists
            .files_of(&name)
            .cloned()
            .unwrap_or_default();
        panel.checked = files;
        panel.changelist_ticked = Some(name.clone());
        if panel.commit_mode() == CommitMode::Staged && panel.repo.operation.is_none() {
            panel.mode_choice = Some(CommitMode::Ticked);
        }
        let count = panel.checked.len();
        self.git_notice(format!("Changelist {name}: {count} ticked"), false);
    }

    pub(super) fn git_checks_wanted(
        &mut self,
        staged: bool,
        paths: &[PathBuf],
        untracked: &[PathBuf],
        fx: &mut Vec<Effect>,
    ) -> bool {
        if !self.commit_checks.any() {
            return false;
        }
        if self.git_panel.as_ref().is_none_or(|p| p.checks_passed) {
            return false;
        }
        self.autosave(fx);
        let root = self.git_root();
        let Some(panel) = self.git_panel.as_mut() else {
            return false;
        };
        panel.busy = true;
        panel.notice = None;
        fx.push(Effect::Git(GitEffect::CommitChecks {
            root,
            staged,
            paths: paths.to_vec(),
            untracked: untracked.to_vec(),
            todo: self.commit_checks.todo,
            command: self.commit_checks.command.clone(),
        }));
        true
    }

    fn problems_being_committed(&self) -> Vec<String> {
        let Some(panel) = self.git_panel.as_ref() else {
            return Vec::new();
        };
        let top = self.repo.top_or(&self.root);
        let staged = panel.commit_mode() == CommitMode::Staged;
        panel
            .files
            .iter()
            .filter(|f| {
                if staged {
                    f.code.staged()
                } else {
                    panel.checked.contains(&f.path)
                }
            })
            .filter_map(|f| {
                let items = self.diagnostics.get(&top.join(&f.path))?;
                let errors = items
                    .iter()
                    .filter(|d| d.raw.severity == Severity::Error)
                    .count();
                (errors > 0).then(|| format!("{errors} errors in {}", f.path.display()))
            })
            .collect()
    }

    pub(super) fn git_checks_done(&mut self, mut warnings: Vec<String>, fx: &mut Vec<Effect>) {
        if self.commit_checks.problems {
            warnings.extend(self.problems_being_committed());
        }
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        panel.busy = false;
        if warnings.is_empty() {
            panel.checks_passed = true;
            return self.git_commit(fx);
        }
        panel.confirm_checks = true;
        panel.field = crate::core::git::GitField::Message;
        self.focus = Focus::Git;
        self.git_notice(
            format!(
                "{}. Y commits anyway, any other key goes back",
                warnings.join("; ")
            ),
            true,
        );
    }
}
