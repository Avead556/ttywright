use super::{
    Action, Command, Effect, Focus, PickerKind, RunFacts, RunOutcome, RunPanel, RunStreamMessage,
    RunTarget, ToolTab, Workspace, run,
};
use crate::core::file_op::FilePromptKind;
use crate::core::input::LineInput;
use crate::core::run_state::{ConfirmAnswer, RunChoice, RunEffect, RunEvent};

impl Workspace {
    pub(super) fn run_action_in_window(&mut self, action: Action, fx: &mut Vec<Effect>) {
        let may =
            self.run_window && matches!(action, Action::NextRunError | Action::PreviousRunError);
        if self.window && !may {
            return self.info("Running is in the main window".to_owned());
        }
        match action {
            Action::RunChoose => self.run_choose(fx),
            Action::RunAnything => {
                self.file_prompt_open(FilePromptKind::RunAnything { back: 0 }, "");
            }
            Action::RunTest => self.run_test_at_cursor(fx),
            Action::RerunLast => match self.run.last().cloned() {
                Some(target) => self.start_run(target, fx),
                None => self.run_choose(fx),
            },
            Action::StopRun => self.run_stop(fx),
            Action::ShowRun => {
                let showing = self.focus == Focus::Run
                    && self.tool_window_visible
                    && self.tool_tab == ToolTab::Run;
                if showing {
                    self.tool_window_visible = false;
                    self.focus = Focus::Editor;
                } else {
                    self.set_focus(Focus::Run);
                }
            }
            Action::PreviousRunError => self.run_next_location(false, fx),
            _ => self.run_next_location(true, fx),
        }
    }

    pub fn configure_run_history(&mut self, commands: Vec<String>) {
        self.run.configure_history(commands);
    }

    pub(super) fn run_anything_history_step(&mut self, older: bool) {
        let Some(prompt) = self.file_prompt.as_mut() else {
            return;
        };
        let FilePromptKind::RunAnything { back } = &mut prompt.kind else {
            return;
        };
        let text = self.run.history_at(back, older);
        let mut input = LineInput::default();
        input.insert(text);
        prompt.input = input;
    }

    pub(super) fn run_anything_confirmed(&mut self, text: String, fx: &mut Vec<Effect>) {
        if text.is_empty() {
            if let Some(prompt) = self.file_prompt.as_mut() {
                prompt.error = Some("Type a command".to_owned());
            }
            return;
        }
        self.file_prompt = None;
        fx.push(Effect::SaveRunHistory {
            commands: self.run.remember(&text),
        });
        let target = RunTarget {
            name: text.clone(),
            command: run::RunCommand::UserShell(text),
            cwd: self.root.clone(),
            source: run::RunSource::Typed,
        };
        self.start_run(target, fx);
    }

    pub(super) fn run_choose(&mut self, fx: &mut Vec<Effect>) {
        self.autosave(fx);
        fx.push(Effect::Run(RunEffect::Detect {
            root: self.root.clone(),
            file: self.active().map(|(_, d)| d.path().to_path_buf()),
        }));
    }

    pub(super) fn run_facts_read(&mut self, facts: &RunFacts, fx: &mut Vec<Effect>) {
        match self.run.facts_read(facts, &self.root, self.trusted) {
            RunChoice::Test(Some(target)) => self.start_run(target, fx),
            RunChoice::Test(None) => self.info("No known way to run this test".to_owned()),
            RunChoice::AskTrust => {
                self.trust_ask(Some("Running here runs this project's code".to_owned()));
            }
            RunChoice::Nothing => self.info(
                "Nothing to run here: no known project file, and no runner for this file"
                    .to_owned(),
            ),
            RunChoice::Pick => self.open_picker(PickerKind::Run),
        }
    }

    pub(super) fn start_run(&mut self, target: RunTarget, fx: &mut Vec<Effect>) {
        self.picker = None;
        self.autosave(fx);
        let start = self.run.start(target, &self.root);
        if let (true, RunEffect::Start { id, target, .. }) = (self.run_watched, &start) {
            fx.push(Effect::RunStreamSend(RunStreamMessage::Started {
                id: *id,
                target: target.clone(),
            }));
        }
        self.tool_window_visible = true;
        self.tool_tab = ToolTab::Run;
        if matches!(
            self.focus,
            Focus::FindInFiles
                | Focus::Problems
                | Focus::Git
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
            self.focus = Focus::Run;
        }
        fx.push(Effect::Run(start));
    }

    pub(super) fn run_close_tab(&mut self, fx: &mut Vec<Effect>) {
        fx.extend(self.run.close_active().map(Effect::Run));
    }

    pub(super) fn run_test_at_cursor(&mut self, fx: &mut Vec<Effect>) {
        let found = self.active().and_then(|(view, d)| {
            let line = d.text().char_to_line(view.cursor.idx);
            let path = d.path().to_path_buf();
            crate::core::run_test::test_at(&path, &d.text().to_string(), line).map(|t| (path, t))
        });
        match found {
            Some((path, test)) => {
                self.run.want_test(path, test);
                self.run_choose(fx);
            }
            None => self.info("No test here: put the cursor in one".to_owned()),
        }
    }

    pub(super) fn run_stop(&mut self, fx: &mut Vec<Effect>) {
        if self.window {
            return self.info("Stopping is in the main window".to_owned());
        }
        match self.run.stop() {
            Some(stop) => fx.push(Effect::Run(stop)),
            None => self.info("Nothing is running".to_owned()),
        }
    }

    pub(super) fn run_finished(&mut self, id: u64, outcome: RunOutcome, duration_ms: u64) {
        let Some(name) = self.run.finished(id, outcome.clone(), duration_ms) else {
            return;
        };
        let ok = outcome.is_success() || outcome == RunOutcome::Stopped;
        self.notify_run_finished(id, &name, &outcome.to_string(), ok);
        if self.tool_window_visible && self.tool_tab == ToolTab::Run {
            return;
        }
        if outcome.is_success() || outcome == RunOutcome::Stopped {
            self.info(format!("{name}: {outcome}"));
        } else {
            self.error(format!("{name}: {outcome}"));
        }
    }

    pub(super) fn run_confirm_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        match self.run.confirm_input(&input) {
            ConfirmAnswer::Run(target) => self.start_run(target, fx),
            ConfirmAnswer::Waiting => {}
            ConfirmAnswer::Declined => self.info("Not run".to_owned()),
        }
    }

    pub(super) fn run_pick(&mut self, index: usize, fx: &mut Vec<Effect>) {
        if let Some(target) = self.run.pick(index) {
            self.start_run(target, fx);
        }
    }

    pub(super) fn run_handle(&mut self, event: RunEvent, fx: &mut Vec<Effect>) {
        match event {
            RunEvent::FactsRead { facts } => self.run_facts_read(&facts, fx),
            RunEvent::Output {
                id,
                lines,
                partial,
                dropped,
            } => {
                if self.run_watched {
                    fx.push(Effect::RunStreamSend(RunStreamMessage::Output {
                        id,
                        lines: lines.clone(),
                        partial: partial.clone(),
                        dropped,
                    }));
                }
                if let Some(panel) = self.run.find_mut(id) {
                    self.tests.feed(id, panel.end_line(), &lines);
                    panel.push(lines, partial, dropped);
                }
            }
            RunEvent::Finished {
                id,
                outcome,
                duration_ms,
            } => {
                if self.run_watched {
                    fx.push(Effect::RunStreamSend(RunStreamMessage::Finished {
                        id,
                        outcome: outcome.clone(),
                        duration_ms,
                    }));
                }
                self.run_finished(id, outcome, duration_ms);
            }
        }
    }

    pub(super) fn run_open_line(&mut self, line: usize, fx: &mut Vec<Effect>) {
        let location = self
            .run
            .panel()
            .and_then(|p| p.line(line))
            .and_then(|l| l.location.clone());
        let Some(place) = location else {
            return;
        };
        let at = Some((place.line, place.col));
        if self.run_window {
            fx.push(Effect::OpenInMainWindow {
                root: self.git_root(),
                path: place.path,
                at,
            });
        } else {
            self.open_file(place.path, at, fx);
        }
    }

    pub(super) fn run_stream(&mut self, message: RunStreamMessage) {
        match message {
            RunStreamMessage::Snapshot(snapshot) => {
                self.run.replace_all(snapshot.map(RunPanel::from_snapshot));
            }
            RunStreamMessage::Started { id, target } => {
                self.run.replace_all(Some(RunPanel::new(id, target)));
            }
            RunStreamMessage::Output {
                id,
                lines,
                partial,
                dropped,
            } => {
                if let Some(panel) = self.run.find_mut(id) {
                    panel.push(lines, partial, dropped);
                }
            }
            RunStreamMessage::Finished {
                id,
                outcome,
                duration_ms,
            } => self.run_finished(id, outcome, duration_ms),
        }
    }

    pub(super) fn run_click(&mut self, line: usize, fx: &mut Vec<Effect>) {
        self.picker = None;
        self.set_focus(Focus::Run);
        if let Some(panel) = self.run.panel_mut() {
            panel.select_line(line);
        }
        self.run_open_line(line, fx);
    }

    pub(super) fn run_next_location(&mut self, forward: bool, fx: &mut Vec<Effect>) {
        let next = self.run.panel().and_then(|p| p.next_location(forward));
        let (Some(line), Some(panel)) = (next, self.run.panel_mut()) else {
            return self.info("No more errors in the run output".to_owned());
        };
        panel.select_line(line);
        self.tool_window_visible = true;
        self.tool_tab = ToolTab::Run;
        self.run_open_line(line, fx);
    }

    pub(super) fn run_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        match input {
            Command::Indent => self.run.step(true),
            Command::Outdent => self.run.step(false),
            Command::Insert(ref text) if text == "x" && !self.window => self.run_close_tab(fx),
            Command::Move(motion) => {
                self.run.motion(motion);
            }
            Command::Newline => {
                if let Some(line) = self.run.panel().and_then(RunPanel::selected) {
                    self.run_open_line(line, fx);
                }
            }
            Command::Insert(text) if self.run.panel().is_some() => match text.as_str() {
                "s" => self.run_stop(fx),
                "r" => self.run_action_in_window(Action::RerunLast, fx),
                "w" => self.run_action(Action::RunOutputWindow, fx),
                "n" => self.run_next_location(true, fx),
                "p" => self.run_next_location(false, fx),
                _ => {}
            },
            _ => {}
        }
    }
}
