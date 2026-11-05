use super::*;
use crate::core::command::{Command, Effect, Motion};
use crate::core::git::{CommitMode, GitChange, GitField, GitFile, GitNotice, LinePick};
use crate::core::git_command::GitEffect;

impl Workspace {
    pub(super) fn git_save_first(&mut self, fx: &mut Vec<Effect>) -> bool {
        let at_odds = self
            .documents
            .values()
            .any(|d| d.is_dirty() && d.conflict().is_some());
        if at_odds {
            self.git_notice(
                "A file changed on disk has unsaved edits: resolve that first",
                true,
            );
            return false;
        }
        self.autosave(fx);
        true
    }

    pub(super) fn git_begin(&mut self, fx: &mut Vec<Effect>) -> bool {
        if self.git_panel.as_ref().is_none_or(|p| p.busy) || !self.git_save_first(fx) {
            return false;
        }
        if let Some(panel) = self.git_panel.as_mut() {
            panel.busy = true;
            panel.notice = None;
        }
        true
    }

    pub(super) fn git_hunks_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        let last = panel.hunks.len().saturating_sub(1);
        match input {
            Command::Escape | Command::Outdent => panel.field = GitField::List,
            Command::Indent => panel.field = GitField::Message,
            Command::Move(Motion::Up) => panel.select_hunk(panel.hunk_selected.saturating_sub(1)),
            Command::Move(Motion::Down) => panel.select_hunk((panel.hunk_selected + 1).min(last)),
            Command::Insert(s) if s == " " => self.git_move_hunk(fx),
            Command::Newline => self.git_enter_lines(),
            Command::SelectGitRow { .. } => {
                panel.field = GitField::List;
                self.git_files_input(input, fx);
            }
            _ => {}
        }
    }

    fn git_move_hunk(&mut self, fx: &mut Vec<Effect>) {
        let Some((file, hunk)) = self.git_panel.as_ref().and_then(|p| {
            Some((
                p.selected_file()?.clone(),
                p.hunks.get(p.hunk_selected)?.clone(),
            ))
        }) else {
            return;
        };
        if self.git_begin(fx) {
            fx.push(Effect::Git(GitEffect::ApplyHunk {
                root: self.git_root(),
                file,
                hunk,
                lines: None,
            }));
        }
    }

    fn git_enter_lines(&mut self) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        let Some(hunk) = panel.hunks.get(panel.hunk_selected) else {
            return;
        };
        if hunk.whole || hunk.patch.starts_with(b"diff ") {
            return self.git_notice("This moves as a whole: it has no lines to pick", true);
        }
        panel.line_pick = Some(LinePick::default());
        panel.field = GitField::HunkLines;
        panel.show_line_cursor();
    }

    pub(super) fn git_lines_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        let count = panel.changed_lines();
        let Some(pick) = panel.line_pick.as_mut() else {
            panel.field = GitField::Hunks;
            return;
        };
        match input {
            Command::Escape => {
                panel.line_pick = None;
                panel.field = GitField::Hunks;
            }
            Command::Move(Motion::Up) => {
                pick.cursor = pick.cursor.saturating_sub(1);
                panel.show_line_cursor();
            }
            Command::Move(Motion::Down) => {
                pick.cursor = (pick.cursor + 1).min(count.saturating_sub(1));
                panel.show_line_cursor();
            }
            Command::Insert(s) => match s.to_lowercase().as_str() {
                " " => {
                    let at = pick.cursor;
                    if !pick.picked.remove(&at) {
                        pick.picked.insert(at);
                    }
                    pick.cursor = (at + 1).min(count.saturating_sub(1));
                    panel.show_line_cursor();
                }
                "a" => {
                    if pick.picked.len() == count {
                        pick.picked.clear();
                    } else {
                        pick.picked = (0..count).collect();
                    }
                }
                _ => {}
            },
            Command::Newline => self.git_move_lines(fx),
            _ => {}
        }
    }

    fn git_move_lines(&mut self, fx: &mut Vec<Effect>) {
        let Some((file, hunk, lines)) = self.git_panel.as_ref().and_then(|p| {
            let pick = p.line_pick.as_ref()?;
            let lines: Vec<usize> = if pick.picked.is_empty() {
                vec![pick.cursor]
            } else {
                pick.picked.iter().copied().collect()
            };
            Some((
                p.selected_file()?.clone(),
                p.hunks.get(p.hunk_selected)?.clone(),
                lines,
            ))
        }) else {
            return;
        };
        if self.git_begin(fx) {
            if let Some(panel) = self.git_panel.as_mut() {
                panel.line_pick = None;
                panel.field = GitField::Hunks;
            }
            fx.push(Effect::Git(GitEffect::ApplyHunk {
                root: self.git_root(),
                file,
                hunk,
                lines: Some(lines),
            }));
        }
    }

    pub(super) fn git_stage_selected(&mut self, fx: &mut Vec<Effect>) {
        let Some(file) = self
            .git_panel
            .as_ref()
            .and_then(|p| p.selected_file().cloned())
        else {
            return;
        };
        let stage = file.code.unstaged();
        if self.git_begin(fx) {
            fx.push(Effect::Git(GitEffect::StageFiles {
                root: self.git_root(),
                files: vec![file],
                stage,
            }));
        }
    }

    pub(super) fn git_stage_all(&mut self, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_ref() else {
            return;
        };
        let movable = panel
            .files
            .iter()
            .filter(|f| f.change != GitChange::Conflicted);
        let stage = movable.clone().any(|f| f.code.unstaged());
        let files: Vec<GitFile> = movable
            .filter(|f| {
                if stage {
                    f.code.unstaged()
                } else {
                    f.code.staged()
                }
            })
            .cloned()
            .collect();
        if !files.is_empty() && self.git_begin(fx) {
            fx.push(Effect::Git(GitEffect::StageFiles {
                root: self.git_root(),
                files,
                stage,
            }));
        }
    }

    pub(super) fn git_stage_done(&mut self, result: Result<(), String>, fx: &mut Vec<Effect>) {
        if let Some(panel) = self.git_panel.as_mut() {
            panel.busy = false;
            if let Err(text) = result {
                panel.notice = Some(GitNotice { text, error: true });
            }
        }
        fx.push(Effect::Git(GitEffect::Status {
            root: self.git_root(),
        }));
    }

    pub(super) fn git_toggle_mode(&mut self) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        let forced = panel.repo.operation.is_some()
            || panel
                .files
                .iter()
                .any(|f| f.change == GitChange::Conflicted);
        if forced {
            return self.git_notice("This can only be finished from what is staged", true);
        }
        panel.mode_choice = Some(match panel.commit_mode() {
            CommitMode::Ticked => CommitMode::Staged,
            CommitMode::Staged => CommitMode::Ticked,
        });
    }

    pub(super) fn git_commit_staged(&mut self, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_ref() else {
            return;
        };
        let message = panel.message.text().trim().to_owned();
        let amend = panel.amend;
        let staged_id = panel.repo.staged_id.clone();
        let nothing = !panel.files.iter().any(|f| f.code.staged());
        if panel
            .files
            .iter()
            .any(|f| f.change == GitChange::Conflicted)
        {
            return self.git_notice("Resolve the conflicts first", true);
        }
        if nothing && !amend && panel.repo.operation.is_none() {
            return self.git_notice("Nothing staged: Space stages, S commits ticked files", true);
        }
        if message.is_empty() {
            return self.git_notice("Write a commit message", true);
        }
        if self.git_checks_wanted(true, &[], &[], fx) {
            return;
        }
        if let Some(panel) = self.git_panel.as_mut() {
            panel.checks_passed = false;
        }
        if self.git_begin(fx) {
            fx.push(Effect::Git(GitEffect::CommitStaged {
                root: self.git_root(),
                message,
                amend,
                staged_id,
            }));
        }
    }
}
