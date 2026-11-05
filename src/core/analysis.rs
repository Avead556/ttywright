use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::lsp::Severity;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AnalysisTool {
    Clippy,
    Staticcheck,
    Ruff,
    Eslint,
}

impl AnalysisTool {
    pub fn name(self) -> &'static str {
        match self {
            Self::Clippy => "clippy",
            Self::Staticcheck => "staticcheck",
            Self::Ruff => "ruff",
            Self::Eslint => "eslint",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisCommand {
    pub tool: AnalysisTool,
    pub program: PathBuf,
    pub args: Vec<String>,
    pub from_project: bool,
}

impl AnalysisCommand {
    pub fn command_line(&self) -> String {
        let mut line = self.program.display().to_string();
        for arg in &self.args {
            line.push(' ');
            line.push_str(arg);
        }
        line
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct AnalysisPlan {
    pub commands: Vec<AnalysisCommand>,
    pub missing: Vec<String>,
    pub approved: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Finding {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub severity: Severity,
    pub message: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalysisOutcome {
    Found(Vec<Finding>),
    Failed(String),
    Stopped,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AnalysisConfirm {
    pub commands: Vec<AnalysisCommand>,
    pub scroll: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalysisEffect {
    Detect {
        root: PathBuf,
    },
    Start {
        root: PathBuf,
        commands: Vec<AnalysisCommand>,
    },
    Stop,
}

impl AnalysisEffect {
    pub const fn needs_trust(&self) -> bool {
        match self {
            Self::Detect { .. } | Self::Start { .. } => true,
            Self::Stop => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum AnalysisEvent {
    Planned {
        plan: AnalysisPlan,
    },
    Finished {
        tool: AnalysisTool,
        outcome: AnalysisOutcome,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AnalysisReply {
    Nothing,
    Info(String),
    Error(String),
    Send(AnalysisEffect),
    Started {
        effect: AnalysisEffect,
        note: String,
    },
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AnalysisState {
    findings: std::collections::BTreeMap<AnalysisTool, Vec<Finding>>,
    running: std::collections::BTreeSet<AnalysisTool>,
    detecting: bool,
    confirm: Option<AnalysisConfirm>,
    missing: Vec<String>,
}

impl AnalysisState {
    pub const fn confirm(&self) -> Option<&AnalysisConfirm> {
        self.confirm.as_ref()
    }

    pub fn running(&self) -> Vec<&'static str> {
        self.running.iter().map(|t| t.name()).collect()
    }

    pub fn findings(&self) -> impl Iterator<Item = (AnalysisTool, &Finding)> {
        self.findings
            .iter()
            .flat_map(|(tool, found)| found.iter().map(|f| (*tool, f)))
    }

    pub fn count(&self, severity: Severity) -> usize {
        self.findings()
            .filter(|(_, f)| f.severity == severity)
            .count()
    }

    pub fn ask(&mut self, root: PathBuf) -> AnalysisReply {
        if !self.running.is_empty() || self.detecting {
            return AnalysisReply::Info("Analysis is already running".to_owned());
        }
        self.detecting = true;
        AnalysisReply::Send(AnalysisEffect::Detect { root })
    }

    pub fn stop(&self) -> AnalysisReply {
        if self.running.is_empty() {
            return AnalysisReply::Info("No analysis is running".to_owned());
        }
        AnalysisReply::Send(AnalysisEffect::Stop)
    }

    pub fn handle(&mut self, event: AnalysisEvent, root: PathBuf) -> AnalysisReply {
        match event {
            AnalysisEvent::Planned { plan } => self.planned(plan, root),
            AnalysisEvent::Finished { tool, outcome } => self.finished(tool, outcome),
        }
    }

    fn planned(&mut self, plan: AnalysisPlan, root: PathBuf) -> AnalysisReply {
        if !std::mem::take(&mut self.detecting) {
            return AnalysisReply::Nothing;
        }
        if plan.commands.is_empty() {
            return AnalysisReply::Info(if plan.missing.is_empty() {
                "No linter is known for this project".to_owned()
            } else {
                format!("Not installed: {}", plan.missing.join(", "))
            });
        }
        if plan.approved {
            return self.start(plan.commands, &plan.missing, root);
        }
        self.missing = plan.missing;
        self.confirm = Some(AnalysisConfirm {
            commands: plan.commands,
            scroll: 0,
        });
        AnalysisReply::Nothing
    }

    pub fn accept(&mut self, root: PathBuf) -> AnalysisReply {
        let Some(confirm) = self.confirm.take() else {
            return AnalysisReply::Nothing;
        };
        let missing = std::mem::take(&mut self.missing);
        self.start(confirm.commands, &missing, root)
    }

    pub fn decline(&mut self) {
        self.confirm = None;
        self.missing.clear();
    }

    pub fn scroll_confirm(&mut self, delta: isize) {
        if let Some(confirm) = self.confirm.as_mut() {
            confirm.scroll = confirm.scroll.saturating_add_signed(delta);
        }
    }

    fn start(
        &mut self,
        commands: Vec<AnalysisCommand>,
        missing: &[String],
        root: PathBuf,
    ) -> AnalysisReply {
        self.running = commands.iter().map(|c| c.tool).collect();
        let tail = if missing.is_empty() {
            String::new()
        } else {
            format!("; not installed: {}", missing.join(", "))
        };
        AnalysisReply::Started {
            note: format!("Analyzing with {}{tail}", self.running().join(", ")),
            effect: AnalysisEffect::Start { root, commands },
        }
    }

    fn finished(&mut self, tool: AnalysisTool, outcome: AnalysisOutcome) -> AnalysisReply {
        self.running.remove(&tool);
        match outcome {
            AnalysisOutcome::Found(findings) => {
                self.findings.insert(tool, findings);
            }
            AnalysisOutcome::Failed(message) => {
                return AnalysisReply::Error(format!("{}: {message}", tool.name()));
            }
            AnalysisOutcome::Stopped => {
                return AnalysisReply::Info(format!("{} stopped", tool.name()));
            }
        }
        if !self.running.is_empty() {
            return AnalysisReply::Nothing;
        }
        let total: usize = self.findings.values().map(Vec::len).sum();
        AnalysisReply::Info(format!("Analysis finished: {total} found"))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn command(tool: AnalysisTool) -> AnalysisCommand {
        AnalysisCommand {
            tool,
            program: PathBuf::from("/bin/x"),
            args: Vec::new(),
            from_project: false,
        }
    }

    fn finding(severity: Severity) -> Finding {
        Finding {
            path: PathBuf::from("a.rs"),
            line: 0,
            col: 0,
            severity,
            message: "m".to_owned(),
        }
    }

    fn plan(approved: bool) -> AnalysisPlan {
        AnalysisPlan {
            commands: vec![command(AnalysisTool::Clippy), command(AnalysisTool::Ruff)],
            missing: vec!["eslint".to_owned()],
            approved,
        }
    }

    #[test]
    fn an_unapproved_plan_waits_for_yes_and_names_what_is_missing() {
        let mut a = AnalysisState::default();
        assert!(matches!(
            a.ask("/p".into()),
            AnalysisReply::Send(AnalysisEffect::Detect { .. })
        ));
        assert!(
            matches!(a.ask("/p".into()), AnalysisReply::Info(_)),
            "one at a time"
        );
        let reply = a.handle(AnalysisEvent::Planned { plan: plan(false) }, "/p".into());
        assert_eq!(reply, AnalysisReply::Nothing);
        assert_eq!(a.confirm().map(|c| c.commands.len()), Some(2));
        let AnalysisReply::Started { note, .. } = a.accept("/p".into()) else {
            panic!("starts on yes");
        };
        assert_eq!(note, "Analyzing with clippy, ruff; not installed: eslint");
        assert!(a.confirm().is_none());
    }

    #[test]
    fn a_plan_nobody_waits_for_is_dropped() {
        let mut a = AnalysisState::default();
        let reply = a.handle(AnalysisEvent::Planned { plan: plan(true) }, "/p".into());
        assert_eq!(reply, AnalysisReply::Nothing);
        assert!(a.running().is_empty());
    }

    #[test]
    fn findings_count_once_every_tool_has_answered() {
        let mut a = AnalysisState::default();
        a.ask("/p".into());
        a.handle(AnalysisEvent::Planned { plan: plan(true) }, "/p".into());
        assert!(matches!(
            a.stop(),
            AnalysisReply::Send(AnalysisEffect::Stop)
        ));
        let found =
            AnalysisOutcome::Found(vec![finding(Severity::Error), finding(Severity::Warning)]);
        let reply = a.handle(
            AnalysisEvent::Finished {
                tool: AnalysisTool::Clippy,
                outcome: found,
            },
            "/p".into(),
        );
        assert_eq!(reply, AnalysisReply::Nothing, "ruff is still working");
        let reply = a.handle(
            AnalysisEvent::Finished {
                tool: AnalysisTool::Ruff,
                outcome: AnalysisOutcome::Failed("x".into()),
            },
            "/p".into(),
        );
        assert_eq!(reply, AnalysisReply::Error("ruff: x".to_owned()));
        assert_eq!(a.count(Severity::Error), 1);
        assert!(
            matches!(a.stop(), AnalysisReply::Info(_)),
            "nothing left to stop"
        );
    }
}
