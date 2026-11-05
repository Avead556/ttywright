use super::{Focus, ToolTab, Workspace};
use crate::core::command::{Command, Effect};
use crate::core::docker::DockerPanel;

impl Workspace {
    pub const fn docker_panel(&self) -> &DockerPanel {
        &self.docker
    }

    pub(super) fn docker_show(&mut self, fx: &mut Vec<Effect>) {
        if self.window {
            return self.info("Docker is in the main window".to_owned());
        }
        self.tool_window_visible = true;
        self.tool_tab = ToolTab::Docker;
        self.focus = Focus::Docker;
        let (cx, mut out) = (self.panel_cx(), Vec::new());
        self.docker.refresh(&cx, &mut out);
        self.panel_apply(out, fx);
    }

    pub(super) fn docker_pick(&mut self, section: usize, index: usize) {
        self.docker.pick(section, index);
    }

    pub(super) fn docker_input(&mut self, input: &Command, fx: &mut Vec<Effect>) {
        let (cx, mut out) = (self.panel_cx(), Vec::new());
        self.docker.input(input, &cx, &mut out);
        self.panel_apply(out, fx);
    }

    pub(super) fn docker_confirm_input(&mut self, input: &Command, fx: &mut Vec<Effect>) {
        let (cx, mut out) = (self.panel_cx(), Vec::new());
        self.docker.confirm_input(input, &cx, &mut out);
        self.panel_apply(out, fx);
    }
}
