use super::{Focus, ToolTab, Workspace};
use crate::core::Problem;
use crate::core::Severity;
use crate::core::analysis::{AnalysisConfirm, AnalysisEvent, AnalysisReply};
use crate::core::command::{Command, Effect, Motion};

impl Workspace {
    pub const fn analysis_confirm(&self) -> Option<&AnalysisConfirm> {
        self.analysis.confirm()
    }

    pub fn analysis_running(&self) -> Vec<&'static str> {
        self.analysis.running()
    }

    pub(super) fn analysis_problems(&self) -> impl Iterator<Item = Problem> + '_ {
        self.analysis.findings().map(|(tool, f)| Problem {
            path: f.path.clone(),
            line: f.line,
            col: f.col,
            severity: f.severity,
            message: f.message.clone(),
            source: tool.name(),
        })
    }

    pub(super) fn analysis_count(&self, severity: Severity) -> usize {
        self.analysis.count(severity)
    }

    fn analysis_reply(&mut self, reply: AnalysisReply, fx: &mut Vec<Effect>) {
        match reply {
            AnalysisReply::Nothing => {}
            AnalysisReply::Info(text) => self.info(text),
            AnalysisReply::Error(text) => self.error(text),
            AnalysisReply::Send(effect) => fx.push(Effect::Analysis(effect)),
            AnalysisReply::Started { effect, note } => {
                self.info(note);
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Problems;
                self.focus = Focus::Problems;
                fx.push(Effect::Analysis(effect));
            }
        }
    }

    pub(super) fn analysis_ask(&mut self, fx: &mut Vec<Effect>) {
        if self.window {
            return self.info("Analysis runs from the main window".to_owned());
        }
        let reply = self.analysis.ask(self.root.clone());
        self.analysis_reply(reply, fx);
    }

    pub(super) fn analysis_stop(&mut self, fx: &mut Vec<Effect>) {
        let reply = self.analysis.stop();
        self.analysis_reply(reply, fx);
    }

    pub(super) fn analysis_handle(&mut self, event: AnalysisEvent, fx: &mut Vec<Effect>) {
        let reply = self.analysis.handle(event, self.root.clone());
        self.analysis_reply(reply, fx);
    }

    pub(super) fn analysis_confirm_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        if self.analysis.confirm().is_none() {
            return;
        }
        match input {
            Command::Insert(text) if text == "y" || text == "Y" => {
                let reply = self.analysis.accept(self.root.clone());
                self.analysis_reply(reply, fx);
            }
            Command::Move(Motion::Up) => self.analysis.scroll_confirm(-1),
            Command::Move(Motion::Down) => self.analysis.scroll_confirm(1),
            _ => {
                self.analysis.decline();
                self.info("Not run".to_owned());
            }
        }
    }
}
