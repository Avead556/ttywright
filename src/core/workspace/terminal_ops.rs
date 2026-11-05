use super::{Focus, ToolTab, Workspace};
use crate::core::command::Effect;
use crate::core::terminal::{TerminalEvent, TerminalTab};

impl Workspace {
    pub fn terminal_tabs(&self) -> &[TerminalTab] {
        self.terminal.tabs()
    }

    pub fn terminal_active(&self) -> usize {
        self.terminal.active()
    }

    pub fn keys_screen(&self) -> Option<&vt100::Screen> {
        match self.focus {
            Focus::Claude => self.claude.as_ref().map(|panel| panel.screen.screen()),
            Focus::Terminal => self.terminal.screen(),
            _ => None,
        }
    }

    fn terminal_show(&mut self) {
        self.tool_window_visible = true;
        self.tool_tab = ToolTab::Terminal;
        self.focus = Focus::Terminal;
    }

    pub(super) fn terminal_toggle(&mut self, fx: &mut Vec<Effect>) {
        if self.focus == Focus::Terminal {
            self.focus = Focus::Editor;
            return;
        }
        if self.terminal.is_empty() {
            self.terminal_new(fx);
        } else {
            self.terminal_show();
        }
    }

    pub(super) fn terminal_new(&mut self, fx: &mut Vec<Effect>) {
        let title = self.terminal.shell_title();
        self.terminal_open(title, Vec::new(), false, fx);
    }

    pub(super) fn terminal_open(
        &mut self,
        title: String,
        command: Vec<String>,
        project_command: bool,
        fx: &mut Vec<Effect>,
    ) {
        if self.window {
            return self.info("The terminal is in the main window".to_owned());
        }
        self.terminal_show();
        let started = self
            .terminal
            .open(title, command, project_command, &self.root);
        fx.extend(started.map(Effect::Terminal));
    }

    pub(super) fn terminal_sized(&mut self, rows: u16, columns: u16, fx: &mut Vec<Effect>) {
        let effects = self.terminal.sized(rows, columns, &self.root);
        fx.extend(effects.into_iter().map(Effect::Terminal));
    }

    pub(super) fn terminal_typed(&mut self, bytes: Vec<u8>, fx: &mut Vec<Effect>) {
        fx.extend(self.terminal.typed(bytes).map(Effect::Terminal));
    }

    pub(super) fn terminal_scrolled(&mut self, delta: isize) {
        self.terminal.scrolled(delta);
    }

    pub(super) fn terminal_select(&mut self, index: usize) {
        if self.terminal.select(index) {
            self.terminal_show();
        }
    }

    pub(super) fn terminal_step(&mut self, forward: bool) {
        if self.terminal.step(forward) {
            self.terminal_show();
        }
    }

    pub(super) fn terminal_close(&mut self, index: usize, fx: &mut Vec<Effect>) {
        let closed = self.terminal.close(index);
        self.terminal_closed(closed.is_some());
        fx.extend(closed.map(Effect::Terminal));
    }

    fn terminal_closed(&mut self, closed: bool) {
        if closed && self.terminal.is_empty() {
            if self.tool_tab == ToolTab::Terminal {
                self.tool_window_visible = false;
            }
            if self.focus == Focus::Terminal {
                self.focus = Focus::Editor;
            }
        }
    }

    pub(super) fn terminal_handle(&mut self, event: TerminalEvent, fx: &mut Vec<Effect>) {
        let ended = matches!(event, TerminalEvent::Ended { .. });
        let reply = self.terminal.handle(event);
        if let Some(message) = reply.failure {
            self.error(message);
        }
        if ended {
            self.terminal_closed(reply.effect.is_some());
        }
        fx.extend(reply.effect.map(Effect::Terminal));
    }
}
