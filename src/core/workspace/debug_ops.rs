use std::path::{Path, PathBuf};

use ropey::Rope;

use super::{Focus, ToolTab, Workspace};
use crate::core::action::Action;
use crate::core::command::{Command, Effect};
use crate::core::debug::{
    Breakpoint, BreakpointState, ConfiguredDebug, DebugEvent, DebugPart, DebugSession, DebugState,
    VarRow,
};
use crate::core::debug_protocol::DebugReply;
use crate::core::file_op::FilePromptKind;
use crate::core::picker::PickerKind;
use crate::core::run::visible;
use crate::core::session::SavedBreakpoint;

fn is_step(action: Action) -> Option<&'static str> {
    match action {
        Action::DebugStepOver => Some("next"),
        Action::DebugStepInto => Some("stepIn"),
        Action::DebugStepOut => Some("stepOut"),
        _ => None,
    }
}

impl Workspace {
    pub fn debug(&self) -> &DebugState {
        &self.debug
    }

    pub fn debug_session(&self) -> Option<&DebugSession> {
        self.debug.session.as_ref()
    }

    pub fn debug_var_rows(&self) -> Vec<VarRow> {
        self.debug.var_rows()
    }

    pub fn breakpoint_at(&self, path: &Path, line: usize) -> Option<&Breakpoint> {
        self.debug.breakpoint_at(path, line)
    }

    pub fn debug_line(&self, path: &Path) -> Option<usize> {
        self.debug.stopped_line(path)
    }

    pub fn debug_inline_values(&self, path: &Path, line: usize, text: &str) -> Vec<String> {
        self.debug.inline_values(path, line, text)
    }

    pub fn configure_debug(&mut self, entries: Vec<ConfiguredDebug>) {
        self.debug.configure(entries);
    }

    fn debug_apply(&mut self, out: Vec<DebugReply>, fx: &mut Vec<Effect>) {
        for reply in out {
            match reply {
                DebugReply::Fx(effect) => fx.push(Effect::Debug(effect)),
                DebugReply::Info(text) => self.info(text),
                DebugReply::Error(text) => self.error(text),
                DebugReply::Pick(kind) => self.open_picker(kind),
                DebugReply::Show => {
                    self.tool_window_visible = true;
                    self.tool_tab = ToolTab::Debug;
                }
                DebugReply::Started => {
                    self.tool_window_visible = true;
                    self.tool_tab = ToolTab::Debug;
                    if matches!(
                        self.focus,
                        Focus::FindInFiles
                            | Focus::Problems
                            | Focus::Git
                            | Focus::Run
                            | Focus::Language
                            | Focus::Http
                            | Focus::Duplicates
                            | Focus::Todo
                            | Focus::Tests
                            | Focus::Dependencies
                            | Focus::Bookmarks
                            | Focus::Database
                            | Focus::Docker
                            | Focus::GitHub
                    ) {
                        self.focus = Focus::Debug;
                    }
                }
                DebugReply::OpenFile { path, line, col } => {
                    let keep = self.focus;
                    self.open_file(path, Some((line, col)), fx);
                    self.focus = keep;
                }
                DebugReply::Autosave => self.autosave(fx),
            }
        }
    }

    fn debug_do(
        &mut self,
        fx: &mut Vec<Effect>,
        f: impl FnOnce(&mut DebugState, &mut Vec<DebugReply>),
    ) {
        let mut out = Vec::new();
        f(&mut self.debug, &mut out);
        self.debug_apply(out, fx);
    }

    pub(super) fn debug_action(&mut self, action: Action, fx: &mut Vec<Effect>) {
        if self.window || self.run_window {
            return self.info("Debugging runs in the main window".to_owned());
        }
        match action {
            Action::ToggleBreakpoint => {
                let Some((path, line, at)) = self.cursor_place() else {
                    return self.info("Open a file first".to_owned());
                };
                self.debug_do(fx, |d, out| d.breakpoint_flip(path, line, Some(at), out));
            }
            Action::EditBreakpoint => self.breakpoint_prompt(false),
            Action::AddLogpoint => self.breakpoint_prompt(true),
            Action::ToggleBreakpointEnabled => {
                let Some((path, line, _)) = self.cursor_place() else {
                    return self.info("Open a file first".to_owned());
                };
                self.debug_do(fx, |d, out| d.breakpoint_enable(&path, line, out));
            }
            Action::ViewBreakpoints => {
                if self.debug.breakpoints.is_empty() {
                    self.info("No breakpoints yet".to_owned());
                } else {
                    self.open_picker(PickerKind::Breakpoints);
                }
            }
            Action::RemoveAllBreakpoints => self.debug_do(fx, DebugState::breakpoints_clear),
            Action::DebugChoose => self.debug_detect(false, fx),
            Action::DebugLast => self.debug_detect(true, fx),
            Action::DebugStop => self.debug_stop(fx),
            Action::DebugShow => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Debug;
                self.focus = Focus::Debug;
            }
            Action::ResumeProgram => {
                let paused = self.debug_session().is_some_and(DebugSession::is_paused);
                if paused && self.focus != Focus::Claude {
                    self.debug_do(fx, DebugState::resume);
                } else {
                    self.claude_toggle(fx);
                }
            }
            Action::DebugPause => self.debug_do(fx, DebugState::pause),
            Action::DebugRunToCursor => {
                let Some((path, line, _)) = self.cursor_place() else {
                    return self.info("Open a file first".to_owned());
                };
                self.debug_do(fx, |d, out| d.run_to_cursor(path, line, out));
            }
            Action::DebugEvaluate => self.debug_prompt(FilePromptKind::DebugEvaluate),
            Action::DebugAddWatch => self.debug_prompt(FilePromptKind::DebugWatch),
            Action::DebugExceptions => self.debug_do(fx, DebugState::toggle_exceptions),
            step => {
                if let Some(command) = is_step(step) {
                    self.debug_do(fx, |d, out| d.step(command, out));
                }
            }
        }
    }

    fn debug_detect(&mut self, last: bool, fx: &mut Vec<Effect>) {
        let file = self.active().map(|(_, d)| d.path().to_path_buf());
        let root = self.root.clone();
        self.debug_do(fx, |d, out| d.detect(last, &root, file, out));
    }

    fn debug_prompt(&mut self, kind: FilePromptKind) {
        let selected = self.active().and_then(|(view, d)| {
            let chars = view.cursor.selection()?;
            Some(d.text().slice(chars).to_string())
        });
        let text = selected.filter(|s| !s.contains('\n')).unwrap_or_default();
        self.file_prompt_open(kind, &text);
    }

    fn cursor_place(&self) -> Option<(PathBuf, usize, usize)> {
        let (view, d) = self.active()?;
        let line = d.text().char_to_line(view.cursor.idx);
        let at = d.text().line_to_char(line);
        Some((d.path().to_path_buf(), line, at))
    }

    pub(super) fn breakpoint_toggle_at(
        &mut self,
        view: super::ViewId,
        line: usize,
        fx: &mut Vec<Effect>,
    ) {
        let Some(d) = self
            .views
            .get(&view)
            .and_then(|v| self.documents.get(&v.doc))
        else {
            return;
        };
        if line >= d.text().len_lines() {
            return;
        }
        let (path, at) = (d.path().to_path_buf(), d.text().line_to_char(line));
        self.debug_do(fx, |d, out| d.breakpoint_flip(path, line, Some(at), out));
    }

    fn breakpoint_prompt(&mut self, log: bool) {
        let Some((path, line, at)) = self.cursor_place() else {
            return self.info("Open a file first".to_owned());
        };
        let text = self.debug.breakpoint_text(&path, line, at, log);
        let kind = if log {
            FilePromptKind::BreakpointLog { path, line }
        } else {
            FilePromptKind::BreakpointCondition { path, line }
        };
        self.file_prompt_open(kind, &text);
    }

    pub(super) fn debug_prompt_confirm(
        &mut self,
        kind: FilePromptKind,
        text: String,
        fx: &mut Vec<Effect>,
    ) {
        let text = text.trim().to_owned();
        match kind {
            FilePromptKind::BreakpointCondition { path, line } => {
                self.debug_do(fx, |d, out| {
                    d.breakpoint_set_text(&path, line, false, text, out);
                });
            }
            FilePromptKind::BreakpointLog { path, line } => {
                self.debug_do(fx, |d, out| {
                    d.breakpoint_set_text(&path, line, true, text, out)
                });
            }
            FilePromptKind::DebugEvaluate if !text.is_empty() => {
                self.debug_do(fx, |d, out| d.evaluate(text, out));
            }
            FilePromptKind::DebugWatch if !text.is_empty() => {
                self.debug_do(fx, |d, out| d.add_watch(text, out));
            }
            _ => {}
        }
    }

    pub(super) fn breakpoint_label(&self, b: &Breakpoint) -> String {
        let mut label = format!("{}:{}", self.display_path(&b.path).display(), b.line + 1);
        if !b.enabled {
            label.push_str("  [off]");
        }
        if let Some(condition) = &b.condition {
            label.push_str(&format!("  if {}", visible(condition)));
        }
        if let Some(message) = &b.log_message {
            label.push_str(&format!("  log {}", visible(message)));
        }
        if let BreakpointState::Pending(Some(why)) = &b.state {
            label.push_str(&format!("  ({why})"));
        }
        label
    }

    pub(super) fn breakpoint_jump(&mut self, index: usize, fx: &mut Vec<Effect>) {
        if let Some(b) = self.debug.breakpoints.get(index) {
            let place = Some((b.line, 0));
            let path = b.path.clone();
            self.open_file(path, place, fx);
        }
    }

    pub(super) fn breakpoints_changed(
        &mut self,
        path: &Path,
        text: &Rope,
        changes: &[crate::core::ChangeSet],
    ) {
        self.debug.breakpoints_changed(path, text, changes);
    }

    pub(super) fn breakpoints_sync(&mut self) {
        let documents = &self.documents;
        self.debug.breakpoints_sync(|path| {
            documents
                .values()
                .find(|d| d.path() == path)
                .map(|d| d.text())
        });
    }

    pub(super) fn saved_breakpoints(&self) -> Vec<SavedBreakpoint> {
        self.debug.saved_breakpoints()
    }

    pub(super) fn restore_breakpoints(&mut self, saved: Vec<SavedBreakpoint>) {
        self.debug.restore_breakpoints(saved);
    }

    pub(super) fn debug_file_saved(&mut self, path: &Path, fx: &mut Vec<Effect>) {
        self.debug_do(fx, |d, out| d.file_saved(path, out));
    }

    pub(super) fn debug_choice_labels(&self) -> Vec<String> {
        self.debug.choice_labels()
    }

    pub(super) fn debug_choice_taken(&mut self, index: usize, fx: &mut Vec<Effect>) {
        self.debug_do(fx, |d, out| d.choice_taken(index, out));
    }

    pub(super) fn debug_executable_labels(&self) -> Vec<String> {
        self.debug.executable_labels()
    }

    pub(super) fn debug_executable_taken(&mut self, index: usize, fx: &mut Vec<Effect>) {
        self.debug_do(fx, |d, out| d.executable_taken(index, out));
    }

    pub(super) fn debug_thread_labels(&self) -> Vec<String> {
        self.debug.thread_labels()
    }

    pub(super) fn debug_thread_taken(&mut self, index: usize, fx: &mut Vec<Effect>) {
        self.debug_do(fx, |d, out| d.thread_taken(index, out));
    }

    pub(super) fn debug_handle(&mut self, event: DebugEvent, fx: &mut Vec<Effect>) {
        let root = self.root.clone();
        self.debug_do(fx, |d, out| d.handle(event, &root, out));
    }

    pub(super) fn debug_stop(&mut self, fx: &mut Vec<Effect>) {
        self.debug_do(fx, DebugState::stop);
    }

    pub(super) fn debug_owns_stop(&self) -> bool {
        let showing = self.tool_tab == ToolTab::Debug && self.tool_window_visible;
        let run_going = self.run_panel().is_some_and(|p| p.is_active());
        self.debug_session().is_some_and(DebugSession::is_live) && (showing || !run_going)
    }

    pub fn can_stop(&self) -> bool {
        self.debug_session().is_some_and(DebugSession::is_live)
            || self.run_panel().is_some_and(|p| p.is_active())
    }

    pub fn last_run_name(&self) -> Option<&str> {
        self.run.last().map(|t| t.name.as_str())
    }

    pub(super) fn debug_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        match input {
            Command::Insert(text) if text == "r" => self.debug_detect(true, fx),
            Command::Insert(text) if text == "e" => {
                self.debug_prompt(FilePromptKind::DebugEvaluate);
            }
            Command::Insert(text) if text == "w" => self.debug_prompt(FilePromptKind::DebugWatch),
            input => self.debug_do(fx, |d, out| d.input(input, out)),
        }
    }

    pub(super) fn debug_row_clicked(
        &mut self,
        part: DebugPart,
        index: usize,
        open: bool,
        fx: &mut Vec<Effect>,
    ) {
        self.picker = None;
        self.focus = Focus::Debug;
        self.debug_do(fx, |d, out| d.row_clicked(part, index, open, out));
    }

    pub(super) fn debug_scroll_console(&mut self, delta: isize) {
        self.debug.scroll_console(delta);
    }
}
