use super::{Focus, ToolTab, Workspace};
use crate::core::command::{Command, Effect};
use crate::core::github::GitHubPanel;

impl Workspace {
    pub const fn github_panel(&self) -> &GitHubPanel {
        &self.github
    }

    pub(super) fn github_show(&mut self, fx: &mut Vec<Effect>) {
        if self.window {
            return self.info("GitHub is in the main window".to_owned());
        }
        self.tool_window_visible = true;
        self.tool_tab = ToolTab::GitHub;
        self.focus = Focus::GitHub;
        let (cx, mut out) = (self.panel_cx(), Vec::new());
        self.github.open(&cx, &mut out);
        self.panel_apply(out, fx);
    }

    pub(super) fn github_pick(&mut self, index: usize) {
        self.github.pick(index);
    }

    pub(super) fn github_scroll(&mut self, delta: isize) {
        self.github.scroll(delta);
    }

    pub(super) fn github_back(&mut self) -> bool {
        self.focus == Focus::GitHub && self.github.back()
    }

    pub(super) fn github_input(&mut self, input: &Command, fx: &mut Vec<Effect>) {
        let (cx, mut out) = (self.panel_cx(), Vec::new());
        self.github.input(input, &cx, &mut out);
        self.panel_apply(out, fx);
    }

    pub(super) fn github_compose_input(&mut self, input: Command) {
        let mut out = Vec::new();
        self.github.compose_input(input, &mut out);
        self.panel_apply(out, &mut Vec::new());
    }

    pub(super) fn github_confirm_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let (cx, mut out) = (self.panel_cx(), Vec::new());
        self.github.confirm_input(input, &cx, &mut out);
        self.panel_apply(out, fx);
    }
}
