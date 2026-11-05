use super::{
    Action, Command, CommitMode, ConflictChoice, Effect, Focus, GitChange, GitDiffSource, GitField,
    GitFile, GitNotice, GitPanel, GitRemoteOp, GitRepoState, GitTab, Motion, PathBuf, ToolTab,
    Workspace, move_input,
};
use crate::core::git_command::GitEffect;

impl Workspace {
    pub(super) fn git_root(&self) -> PathBuf {
        self.root.clone()
    }

    pub(super) fn open_git(&mut self, tab: GitTab, fx: &mut Vec<Effect>) {
        let showing = self.focus == Focus::Git
            && self.tool_window_visible
            && self.tool_tab == ToolTab::Git
            && self.git_panel.as_ref().is_some_and(|p| p.tab == tab);
        if showing && !self.window {
            self.tool_window_visible = false;
            self.focus = Focus::Editor;
            return;
        }
        self.git_show(tab, fx);
    }

    pub(super) fn in_commit_message(&self) -> bool {
        self.focus == Focus::Git
            && self
                .git_panel
                .as_ref()
                .is_some_and(|p| p.field == GitField::Message)
    }

    pub(super) fn git_show(&mut self, tab: GitTab, fx: &mut Vec<Effect>) {
        self.picker = None;
        self.autosave(fx);
        let root = self.git_root();
        let panel = self.git_panel.get_or_insert_with(|| GitPanel::new(tab));
        panel.tab = tab;
        panel.field = GitField::List;
        self.tool_window_visible = true;
        self.tool_tab = ToolTab::Git;
        self.focus = Focus::Git;
        fx.push(Effect::Git(GitEffect::Status { root: root.clone() }));
        fx.push(Effect::Git(GitEffect::Log { root }));
    }

    pub(super) fn git_leave(&mut self) {
        if !self.window {
            self.focus = Focus::Editor;
        }
    }

    pub(super) fn git_status_read(
        &mut self,
        branch: Option<String>,
        top: Option<PathBuf>,
        files: Result<Vec<GitFile>, String>,
        state: GitRepoState,
        fx: &mut Vec<Effect>,
    ) {
        let listed = files.as_deref().map_err(String::as_str);
        self.repo.status_read(branch, top, listed, state.operation);
        if let Ok(files) = &files {
            self.changelists.keep_only(files);
        }
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        panel.repo = state;
        match files {
            Ok(files) => {
                panel.unavailable = None;
                panel.set_files(files);
            }
            Err(reason) => panel.unavailable = Some(reason),
        }
        if panel.tab == GitTab::Commit {
            self.git_request_diff_of_file(false, fx);
        }
    }

    pub(super) fn git_log_read(
        &mut self,
        result: Result<Vec<crate::core::git::GitCommit>, String>,
        now: u64,
        fx: &mut Vec<Effect>,
    ) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        panel.now = now;
        match result {
            Ok(commits) => panel.set_commits(commits),
            Err(reason) => panel.unavailable = Some(reason),
        }
        let selected = panel.selected_commit().map(|c| c.hash.clone());
        if panel.tab == GitTab::Log && selected != panel.commit_shown {
            self.git_load_commit(fx);
        }
    }

    pub(super) fn git_commit_files_read(
        &mut self,
        hash: &str,
        result: Result<Vec<GitFile>, String>,
        fx: &mut Vec<Effect>,
    ) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        panel.commit_loading = None;
        if panel.selected_commit().is_none_or(|c| c.hash != hash) {
            return self.git_load_commit(fx);
        }
        match result {
            Ok(files) => {
                panel.commit_files = files;
                panel.commit_file_selected = 0;
                panel.commit_shown = Some(hash.to_owned());
                self.git_show_commit_file_diff(fx);
            }
            Err(text) => panel.notice = Some(GitNotice { text, error: true }),
        }
    }

    pub(super) fn git_load_commit(&mut self, fx: &mut Vec<Effect>) {
        let root = self.git_root();
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        panel.commit_files.clear();
        panel.commit_shown = None;
        panel.clear_diff();
        panel.diff_request = None;
        if panel.commit_loading.is_some() {
            return;
        }
        if let Some(hash) = panel.selected_commit().map(|c| c.hash.clone()) {
            panel.commit_loading = Some(hash.clone());
            fx.push(Effect::Git(GitEffect::CommitFiles { root, hash }));
        }
    }

    pub(super) fn git_request_diff(
        &mut self,
        source: Option<GitDiffSource>,
        fresh: bool,
        fx: &mut Vec<Effect>,
    ) {
        let id = u64::from(self.alloc_id());
        let root = self.git_root();
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        if fresh || source.is_none() {
            panel.clear_diff();
        }
        panel.diff_request = None;
        if let Some(source) = source {
            panel.diff_request = Some(id);
            fx.push(Effect::Git(GitEffect::Diff { id, root, source }));
        }
    }

    pub(super) fn git_show_file_diff(&mut self, fx: &mut Vec<Effect>) {
        self.git_request_diff_of_file(true, fx);
    }

    pub(super) fn git_request_diff_of_file(&mut self, fresh: bool, fx: &mut Vec<Effect>) {
        let id = u64::from(self.alloc_id());
        let root = self.git_root();
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        let file = panel.selected_file().cloned();
        if fresh || file.is_none() {
            panel.clear_diff();
        }
        panel.diff_request = None;
        if let Some(file) = file {
            panel.diff_request = Some(id);
            fx.push(Effect::Git(GitEffect::Hunks { id, root, file }));
        }
    }

    pub(super) fn git_show_commit_file_diff(&mut self, fx: &mut Vec<Effect>) {
        let source = self.git_panel.as_ref().and_then(|p| {
            let file = p.commit_files.get(p.commit_file_selected)?.clone();
            let hash = p.selected_commit()?.hash.clone();
            Some(GitDiffSource::Commit { hash, file })
        });
        self.git_request_diff(source, true, fx);
    }

    pub(super) fn git_notice(&mut self, text: impl Into<String>, error: bool) {
        if let Some(panel) = self.git_panel.as_mut() {
            panel.notice = Some(GitNotice {
                text: text.into(),
                error,
            });
        }
    }

    pub(super) fn git_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        if std::mem::take(&mut panel.confirm_checks) {
            panel.notice = None;
            if matches!(&input, Command::Insert(s) if s.eq_ignore_ascii_case("y")) {
                panel.checks_passed = true;
                self.git_commit(fx);
            }
            return;
        }
        let key = match &input {
            Command::Insert(s) => s.to_lowercase(),
            _ => String::new(),
        };
        if key != "x" {
            panel.confirm_abort = false;
        }
        if key != " " {
            panel.confirm_resolve = None;
        }
        if key != "d" {
            panel.confirm_drop = None;
        }
        if key != "f" {
            panel.confirm_force = None;
        }
        let typing = matches!(
            panel.field,
            GitField::StashMessage | GitField::Message | GitField::Filter
        );
        if key == "v" && !typing {
            panel.side_by_side = !panel.side_by_side;
            return;
        }
        let typing = panel.field == GitField::StashMessage;
        let max_scroll = panel.diff.len().saturating_sub(1);
        match input {
            _ if typing => {}
            Command::Move(Motion::PageUp { rows }) if panel.field != GitField::Message => {
                panel.diff_scroll = panel.diff_scroll.saturating_sub(rows);
                return;
            }
            Command::Move(Motion::PageDown { rows }) if panel.field != GitField::Message => {
                panel.diff_scroll = (panel.diff_scroll + rows).min(max_scroll);
                return;
            }
            _ => {}
        }
        match (panel.tab, panel.field) {
            (GitTab::Commit, GitField::Message) => self.git_message_input(input, fx),
            (GitTab::Commit, GitField::Hunks) => self.git_hunks_input(input, fx),
            (GitTab::Commit, GitField::HunkLines) => self.git_lines_input(input, fx),
            (GitTab::Commit, _) => self.git_files_input(input, fx),
            (GitTab::History, _) => self.git_history_input(input, fx),
            (GitTab::Compare, _) => self.git_compare_input(input, fx),
            (GitTab::Stash, GitField::StashMessage) => self.git_stash_message_input(input, fx),
            (GitTab::Stash, _) => self.git_stash_input(input, fx),
            (GitTab::Branches, _) => self.git_branches_input(input, fx),
            (GitTab::Log, GitField::Filter) => self.git_filter_input(input, fx),
            (GitTab::Log, GitField::CommitFiles) => self.git_commit_files_input(input, fx),
            (GitTab::Log, _) => self.git_log_input(input, fx),
        }
    }

    pub(super) fn git_files_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        let last = panel.files.len().saturating_sub(1);
        if !matches!(&input, Command::Insert(_)) {
            panel.confirm_rollback = None;
        }
        let target = match input {
            Command::Escape => return self.git_leave(),
            Command::Indent | Command::Outdent => {
                panel.field = if panel.hunks.is_empty() {
                    GitField::Message
                } else {
                    GitField::Hunks
                };
                return;
            }
            Command::Insert(s) => {
                let staging = panel.commit_mode() == CommitMode::Staged;
                let conflict = panel
                    .selected_file()
                    .is_some_and(|f| f.change == GitChange::Conflicted);
                match s.to_lowercase().as_str() {
                    " " if conflict => self.git_mark_resolved(fx),
                    "o" if conflict => self.git_conflict_key(ConflictChoice::Ours, fx),
                    "t" if conflict => self.git_conflict_key(ConflictChoice::Theirs, fx),
                    "b" if conflict => self.git_conflict_key(ConflictChoice::Both, fx),
                    "n" if conflict => self.git_conflict_step_key(true, fx),
                    "p" if conflict => self.git_conflict_step_key(false, fx),
                    "c" => self.git_continue(fx),
                    "x" => self.git_abort(fx),
                    "z" => self.git_show_stash(fx),
                    "g" => self.git_show_branches(fx),
                    " " if staging => self.git_stage_selected(fx),
                    "a" if staging => self.git_stage_all(fx),
                    " " => {
                        if let Some(path) =
                            panel.files.get(panel.file_selected).map(|f| f.path.clone())
                            && !panel.checked.remove(&path)
                        {
                            panel.checked.insert(path);
                        }
                    }
                    "a" => {
                        if panel.checked.len() == panel.files.len() {
                            panel.checked.clear();
                        } else {
                            panel.checked = panel.files.iter().map(|f| f.path.clone()).collect();
                        }
                    }
                    "s" => self.git_toggle_mode(),
                    "e" => self.changelist_ask(),
                    "j" => self.changelist_tick_next(),
                    "l" => self.git_show(GitTab::Log, fx),
                    "h" => self.git_show_history(fx),
                    "r" => self.git_show(GitTab::Commit, fx),
                    "m" => self.git_toggle_amend(fx),
                    "u" => return self.git_rollback(fx),
                    "b" => self.git_ask_branches(fx),
                    "w" => self.run_action(Action::GitLogWindow, fx),
                    _ => {}
                }
                if let Some(panel) = self.git_panel.as_mut() {
                    panel.confirm_rollback = None;
                }
                return;
            }
            Command::Newline => {
                let chosen = panel.files.get(panel.file_selected).cloned();
                if let Some(file) = chosen
                    .as_ref()
                    .filter(|f| f.change == GitChange::Conflicted)
                {
                    return self.merge_open_file(file, fx);
                }
                return self.git_open_file(chosen.map(|f| f.path), fx);
            }
            Command::Move(Motion::Up) => panel.file_selected.saturating_sub(1),
            Command::Move(Motion::Down) => (panel.file_selected + 1).min(last),
            Command::SelectGitRow { index } => index.min(last),
            _ => return,
        };
        if target != panel.file_selected {
            panel.file_selected = target;
            self.git_show_file_diff(fx);
        }
    }

    pub(super) fn git_log_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        let last = panel.visible.len().saturating_sub(1);
        let target = match input {
            Command::Escape => return self.git_leave(),
            Command::Indent | Command::Outdent => {
                if !panel.commit_files.is_empty() {
                    panel.field = GitField::CommitFiles;
                }
                return;
            }
            Command::Insert(s) => {
                match s.to_lowercase().as_str() {
                    "/" => panel.field = GitField::Filter,
                    "c" => self.git_show(GitTab::Commit, fx),
                    "h" => self.git_show_history(fx),
                    "z" => self.git_show_stash(fx),
                    "g" => self.git_show_branches(fx),
                    "r" => self.git_show(GitTab::Log, fx),
                    "w" => self.run_action(Action::GitLogWindow, fx),
                    "y" => {
                        if let Some(hash) = panel.selected_commit().map(|c| c.hash.clone()) {
                            self.git_notice(format!("Copied {hash}"), false);
                            fx.push(Effect::WriteClipboard { text: hash });
                        }
                    }
                    _ => {}
                }
                return;
            }
            Command::Move(Motion::Up) => panel.commit_selected.saturating_sub(1),
            Command::Move(Motion::Down) => (panel.commit_selected + 1).min(last),
            Command::Move(Motion::DocStart) => 0,
            Command::Move(Motion::DocEnd) => last,
            Command::SelectGitRow { index } => index.min(last),
            _ => return,
        };
        if target != panel.commit_selected {
            panel.commit_selected = target;
            self.git_load_commit(fx);
        }
    }

    pub(super) fn git_filter_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        match input {
            Command::Escape => {
                panel.filter = crate::core::input::LineInput::default();
                panel.field = GitField::List;
            }
            Command::Newline | Command::Indent | Command::Outdent => {
                panel.field = GitField::List;
                return;
            }
            Command::Insert(s) => panel.filter.insert(&s),
            Command::DeleteBackward => panel.filter.delete_backward(),
            Command::DeleteForward => panel.filter.delete_forward(),
            Command::Move(Motion::Up | Motion::Down) => return,
            Command::Move(motion) => return move_input(&mut panel.filter, motion),
            _ => return,
        }
        let before = panel.selected_commit().map(|c| c.hash.clone());
        panel.refilter();
        if panel.selected_commit().map(|c| c.hash.clone()) != before {
            self.git_load_commit(fx);
        }
    }

    pub(super) fn git_commit_files_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(panel) = self.git_panel.as_mut() else {
            return;
        };
        let last = panel.commit_files.len().saturating_sub(1);
        let target = match input {
            Command::Escape => return self.git_leave(),
            Command::Indent | Command::Outdent => {
                panel.field = GitField::List;
                return;
            }
            Command::Newline => {
                let file = panel
                    .commit_files
                    .get(panel.commit_file_selected)
                    .filter(|f| f.change != GitChange::Deleted)
                    .map(|f| f.path.clone());
                return self.git_open_file(file, fx);
            }
            Command::Move(Motion::Up) => panel.commit_file_selected.saturating_sub(1),
            Command::Move(Motion::Down) => (panel.commit_file_selected + 1).min(last),
            Command::SelectGitRow { .. } => {
                panel.field = GitField::List;
                return self.git_log_input(input, fx);
            }
            _ => return,
        };
        if target != panel.commit_file_selected {
            panel.commit_file_selected = target;
            self.git_show_commit_file_diff(fx);
        }
    }

    pub(super) fn git_open_file(&mut self, file: Option<PathBuf>, fx: &mut Vec<Effect>) {
        let Some(file) = file else {
            return;
        };
        let top = self.repo.top_or(&self.root);
        if self.window {
            fx.push(Effect::OpenInMainWindow {
                root: self.git_root(),
                path: top.join(file),
                at: None,
            });
        } else {
            self.open_file(top.join(file), None, fx);
        }
    }

    pub(super) fn git_remote(&mut self, op: GitRemoteOp, fx: &mut Vec<Effect>) {
        self.picker = None;
        let name = match op {
            GitRemoteOp::Push => "Pushing…",
            GitRemoteOp::Pull => "Pulling…",
        };
        self.info(name.to_owned());
        self.git_notice(name, false);
        if let Some(panel) = self.git_panel.as_mut() {
            panel.busy = true;
        }
        fx.push(Effect::Git(GitEffect::Remote {
            root: self.git_root(),
            op,
        }));
    }

    pub(super) fn git_remote_done(
        &mut self,
        op: GitRemoteOp,
        result: Result<String, String>,
        fx: &mut Vec<Effect>,
    ) {
        let name = match op {
            GitRemoteOp::Push => "Push",
            GitRemoteOp::Pull => "Pull",
        };
        if let Some(panel) = self.git_panel.as_mut() {
            panel.busy = false;
        }
        match result {
            Ok(summary) => {
                self.info(format!("{name}: {summary}"));
                self.git_notice(format!("{name}: {summary}"), false);
                self.notify(
                    crate::core::notify::NotifyLevel::Success,
                    name,
                    &summary,
                    None,
                    false,
                );
            }
            Err(text) => {
                self.error(format!("{name} failed: {text}"));
                self.git_notice(format!("{name} failed: {text}"), true);
                self.notify(
                    crate::core::notify::NotifyLevel::Error,
                    &format!("{name} failed"),
                    &text,
                    Some(Action::GitLog),
                    true,
                );
            }
        }
        let root = self.git_root();
        fx.push(Effect::Git(GitEffect::Status { root: root.clone() }));
        if self.git_panel.is_some() {
            fx.push(Effect::Git(GitEffect::Log { root }));
        }
    }
}
