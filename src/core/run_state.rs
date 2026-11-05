use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::command::{Command, Motion};
use super::run::{self, ConfiguredRun, RunFacts, RunSource, RunTarget};
use super::run_output::{RunLine, RunOutcome, RunPanel, RunStatus};
use super::run_test::{TestAt, test_target};

const HISTORY_MAX: usize = 100;
const SIDE_STEP: usize = 8;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunEffect {
    Detect {
        root: PathBuf,
        file: Option<PathBuf>,
    },
    Start {
        id: u64,
        target: RunTarget,
        root: PathBuf,
        replaces: Option<u64>,
    },
    Stop {
        id: u64,
    },
}

impl RunEffect {
    pub const fn needs_trust(&self) -> bool {
        match self {
            Self::Start { target, .. } => {
                !matches!(target.source, RunSource::UserConfig | RunSource::Typed)
            }
            Self::Detect { .. } | Self::Stop { .. } => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunEvent {
    FactsRead {
        facts: RunFacts,
    },
    Output {
        id: u64,
        lines: Vec<RunLine>,
        partial: String,
        dropped: usize,
    },
    Finished {
        id: u64,
        outcome: RunOutcome,
        duration_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RunConfirm {
    pub target: RunTarget,
    pub scroll: usize,
}

#[derive(Debug, PartialEq, Eq)]
pub enum RunChoice {
    Test(Option<RunTarget>),
    AskTrust,
    Nothing,
    Pick,
}

#[derive(Debug, PartialEq, Eq)]
pub enum ConfirmAnswer {
    Run(RunTarget),
    Waiting,
    Declined,
}

pub struct RunState {
    tabs: Vec<RunPanel>,
    active: usize,
    next_id: u64,
    last: Option<RunTarget>,
    configured: Vec<ConfiguredRun>,
    targets: Vec<RunTarget>,
    confirm: Option<RunConfirm>,
    test_wanted: Option<(PathBuf, TestAt)>,
    history: Vec<String>,
}

impl Default for RunState {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            next_id: 1,
            last: None,
            configured: Vec::new(),
            targets: Vec::new(),
            confirm: None,
            test_wanted: None,
            history: Vec::new(),
        }
    }
}

impl RunState {
    pub fn configure(&mut self, entries: Vec<ConfiguredRun>) {
        self.configured = entries;
    }

    pub fn configure_history(&mut self, commands: Vec<String>) {
        self.history = commands;
    }

    pub fn tabs(&self) -> &[RunPanel] {
        &self.tabs
    }

    pub const fn active(&self) -> usize {
        self.active
    }

    pub fn panel(&self) -> Option<&RunPanel> {
        self.tabs.get(self.active)
    }

    pub fn panel_mut(&mut self) -> Option<&mut RunPanel> {
        self.tabs.get_mut(self.active)
    }

    pub fn find_mut(&mut self, id: u64) -> Option<&mut RunPanel> {
        self.tabs.iter_mut().find(|p| p.id == id)
    }

    pub fn position(&self, id: u64) -> Option<usize> {
        self.tabs.iter().position(|p| p.id == id)
    }

    pub fn show_line(&mut self, index: usize, line: usize) {
        if let Some(panel) = self.tabs.get_mut(index) {
            self.active = index;
            panel.select_line(line);
        }
    }

    pub fn last(&self) -> Option<&RunTarget> {
        self.last.as_ref()
    }

    pub fn targets(&self) -> &[RunTarget] {
        &self.targets
    }

    pub fn confirm(&self) -> Option<&RunConfirm> {
        self.confirm.as_ref()
    }

    pub fn select(&mut self, index: usize) -> bool {
        let found = index < self.tabs.len();
        if found {
            self.active = index;
        }
        found
    }

    pub fn step(&mut self, forward: bool) {
        let count = self.tabs.len();
        if count > 0 {
            let step = if forward { 1 } else { count - 1 };
            self.active = (self.active + step) % count;
        }
    }

    pub fn pick(&mut self, index: usize) -> Option<RunTarget> {
        let target = self.targets.get(index)?.clone();
        if target.needs_confirmation() {
            self.confirm = Some(RunConfirm { target, scroll: 0 });
            None
        } else {
            Some(target)
        }
    }

    pub fn confirm_input(&mut self, input: &Command) -> ConfirmAnswer {
        let Some(confirm) = self.confirm.as_mut() else {
            return ConfirmAnswer::Declined;
        };
        match input {
            Command::Insert(text) if text == "y" || text == "Y" => {
                ConfirmAnswer::Run(confirm.target.clone())
            }
            Command::Move(Motion::Up) => {
                confirm.scroll = confirm.scroll.saturating_sub(1);
                ConfirmAnswer::Waiting
            }
            Command::Move(Motion::Down) => {
                confirm.scroll = confirm.scroll.saturating_add(1);
                ConfirmAnswer::Waiting
            }
            _ => {
                self.confirm = None;
                ConfirmAnswer::Declined
            }
        }
    }

    pub fn want_test(&mut self, path: PathBuf, test: TestAt) {
        self.test_wanted = Some((path, test));
    }

    pub fn facts_read(&mut self, facts: &RunFacts, root: &Path, trusted: bool) -> RunChoice {
        if let Some((path, test)) = self.test_wanted.take() {
            return RunChoice::Test(test_target(root, &path, &test, facts));
        }
        let configured = |from_project: bool| {
            self.configured
                .iter()
                .filter(move |e| e.from_project == from_project)
                .map(|e| e.target(root))
        };
        let mut targets: Vec<RunTarget> = configured(false).collect();
        if trusted {
            targets.extend(run::detect(root, facts));
            targets.extend(configured(true));
        } else if targets.is_empty() {
            return RunChoice::AskTrust;
        }
        self.targets = targets;
        if self.targets.is_empty() {
            RunChoice::Nothing
        } else {
            RunChoice::Pick
        }
    }

    pub fn history_at(&self, back: &mut usize, older: bool) -> &str {
        let count = self.history.len();
        *back = if older {
            (*back + 1).min(count)
        } else {
            back.saturating_sub(1)
        };
        count
            .checked_sub(*back)
            .filter(|_| *back > 0)
            .map_or("", |i| self.history[i].as_str())
    }

    pub fn remember(&mut self, text: &str) -> Vec<String> {
        self.history.retain(|c| c != text);
        self.history.push(text.to_owned());
        let extra = self.history.len().saturating_sub(HISTORY_MAX);
        self.history.drain(..extra);
        self.history.clone()
    }

    pub fn start(&mut self, target: RunTarget, root: &Path) -> RunEffect {
        self.confirm = None;
        let id = self.next_id;
        self.next_id += 1;
        self.last = Some(target.clone());
        let panel = RunPanel::new(id, target.clone());
        let same = self.tabs.iter().position(|tab| tab.target == target);
        let replaces = same.map(|index| self.tabs[index].id);
        match same {
            Some(index) => {
                self.tabs[index] = panel;
                self.active = index;
            }
            None => {
                self.tabs.push(panel);
                self.active = self.tabs.len() - 1;
            }
        }
        RunEffect::Start {
            id,
            target,
            root: root.to_path_buf(),
            replaces,
        }
    }

    pub fn close_active(&mut self) -> Option<RunEffect> {
        if self.active >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(self.active);
        self.active = self.active.min(self.tabs.len().saturating_sub(1));
        tab.is_active().then_some(RunEffect::Stop { id: tab.id })
    }

    pub fn stop(&mut self) -> Option<RunEffect> {
        let panel = self.panel_mut().filter(|p| p.is_active())?;
        panel.status = RunStatus::Stopping;
        Some(RunEffect::Stop { id: panel.id })
    }

    pub fn finished(&mut self, id: u64, outcome: RunOutcome, duration_ms: u64) -> Option<String> {
        let panel = self.find_mut(id)?;
        panel.status = RunStatus::Finished {
            outcome,
            duration_ms,
        };
        Some(run::visible(&panel.target.name))
    }

    pub fn replace_all(&mut self, panel: Option<RunPanel>) {
        self.tabs = panel.into_iter().collect();
        self.active = 0;
    }

    pub fn motion(&mut self, motion: Motion) -> bool {
        let Some(panel) = self.panel_mut() else {
            return false;
        };
        let rows = |rows: usize| isize::try_from(rows).unwrap_or(isize::MAX);
        match motion {
            Motion::Up => panel.select_by(-1),
            Motion::Down => panel.select_by(1),
            Motion::PageUp { rows: r } => panel.select_by(-rows(r)),
            Motion::PageDown { rows: r } => panel.select_by(rows(r)),
            Motion::LineStart | Motion::DocStart => {
                panel.left = 0;
                panel.select_first();
            }
            Motion::LineEnd | Motion::DocEnd => {
                panel.left = 0;
                panel.follow_end();
            }
            Motion::Left => panel.left = panel.left.saturating_sub(SIDE_STEP),
            Motion::Right => panel.left = panel.left.saturating_add(SIDE_STEP),
            _ => return false,
        }
        true
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::run::RunCommand;

    fn target(name: &str, source: RunSource) -> RunTarget {
        RunTarget {
            name: name.to_owned(),
            command: RunCommand::UserShell(name.to_owned()),
            cwd: PathBuf::from("/p"),
            source,
        }
    }

    #[test]
    fn running_the_same_target_again_reuses_its_tab_and_replaces_the_run() {
        let mut state = RunState::default();
        let root = PathBuf::from("/p");
        let first = state.start(target("a", RunSource::Typed), &root);
        state.start(target("b", RunSource::Typed), &root);
        let again = state.start(target("a", RunSource::Typed), &root);
        assert!(matches!(first, RunEffect::Start { id: 1, .. }));
        assert!(
            matches!(
                again,
                RunEffect::Start {
                    id: 3,
                    replaces: Some(1),
                    ..
                }
            ),
            "{again:?}"
        );
        assert_eq!(state.tabs().len(), 2);
        assert_eq!(state.active(), 0);
        assert!(!again.needs_trust(), "typed by the user");
        assert_eq!(state.stop(), Some(RunEffect::Stop { id: 3 }));
        assert_eq!(state.close_active(), Some(RunEffect::Stop { id: 3 }));
        assert_eq!(
            state.finished(2, RunOutcome::Stopped, 1).as_deref(),
            Some("b")
        );
        assert_eq!(state.stop(), None, "finished");
    }

    #[test]
    fn the_history_keeps_each_command_once_and_the_newest_last() {
        let mut state = RunState::default();
        state.remember("a");
        state.remember("b");
        assert_eq!(state.remember("a"), ["b", "a"]);
        let mut back = 0;
        assert_eq!(state.history_at(&mut back, true), "a");
        assert_eq!(state.history_at(&mut back, true), "b");
        assert_eq!(state.history_at(&mut back, true), "b", "the oldest stays");
        state.history_at(&mut back, false);
        assert_eq!(state.history_at(&mut back, false), "");
    }

    #[test]
    fn only_y_runs_what_the_project_asked_for() {
        let mut state = RunState {
            targets: vec![target("p", RunSource::ProjectConfig)],
            ..RunState::default()
        };
        assert_eq!(state.pick(0), None, "read first");
        assert_eq!(
            state.confirm_input(&Command::Move(Motion::Down)),
            ConfirmAnswer::Waiting
        );
        assert_eq!(state.confirm().map(|c| c.scroll), Some(1));
        assert_eq!(
            state.confirm_input(&Command::Newline),
            ConfirmAnswer::Declined
        );
        assert!(state.confirm().is_none());
        state.pick(0);
        assert!(matches!(
            state.confirm_input(&Command::Insert("y".to_owned())),
            ConfirmAnswer::Run(_)
        ));
    }
}
