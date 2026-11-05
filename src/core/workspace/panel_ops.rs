use super::Workspace;
use crate::core::cli::{CliEvent, CliTool, PanelCx, PanelReply};
use crate::core::command::Effect;

impl Workspace {
    pub(super) fn panel_cx(&self) -> PanelCx {
        PanelCx {
            root: self.root.clone(),
            trusted: self.trusted,
            branch: self.repo.branch().map(str::to_owned),
        }
    }

    pub(super) fn panel_apply(&mut self, out: Vec<PanelReply>, fx: &mut Vec<Effect>) {
        for reply in out {
            match reply {
                PanelReply::Cli(effect) => fx.push(Effect::Cli(effect)),
                PanelReply::Info(text) => self.info(text),
                PanelReply::Error(text) => self.error(text),
                PanelReply::AskTrust(why) => self.trust_ask(Some(why)),
                PanelReply::Run(target) => self.start_run(target, fx),
                PanelReply::Terminal {
                    title,
                    command,
                    project_command,
                } => self.terminal_open(title, command, project_command, fx),
                PanelReply::RefreshGit => self.git_refresh_all(fx),
                PanelReply::Autosave => self.autosave(fx),
            }
        }
    }

    pub(super) fn cli_finished(&mut self, event: CliEvent, fx: &mut Vec<Effect>) {
        let CliEvent::Finished { id, tool, outcome } = event;
        let (cx, mut out) = (self.panel_cx(), Vec::new());
        match tool {
            CliTool::Docker => self.docker.answered(id, outcome, &cx, &mut out),
            CliTool::Gh => self.github.answered(id, outcome, &cx, &mut out),
        }
        self.panel_apply(out, fx);
    }
}
