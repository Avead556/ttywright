use super::{Command, Effect, Workspace};
use crate::core::action::Action;

const MAX_STEPS: usize = 10_000;

#[derive(Debug, Default)]
pub(super) struct Macros {
    recording: Option<Vec<Command>>,
    last: Vec<Command>,
    playing: bool,
}

fn recordable(command: &Command) -> bool {
    match command {
        Command::Insert(_)
        | Command::Newline
        | Command::DeleteBackward
        | Command::DeleteForward
        | Command::Move(_)
        | Command::Select(_) => true,
        Command::Run(action) => matches!(
            action,
            Action::Undo | Action::Redo | Action::Find | Action::FindNext | Action::FindPrevious
        ),
        _ => false,
    }
}

impl Workspace {
    pub fn is_recording_macro(&self) -> bool {
        self.macros.recording.is_some()
    }

    pub(super) fn macro_note(&mut self, command: &Command) {
        if self.macros.playing || !recordable(command) {
            return;
        }
        if let Some(steps) = self.macros.recording.as_mut()
            && steps.len() < MAX_STEPS
        {
            steps.push(command.clone());
        }
    }

    pub(super) fn macro_toggle(&mut self) {
        match self.macros.recording.take() {
            None => {
                self.macros.recording = Some(Vec::new());
                self.info("Recording a macro: the same command stops it".to_owned());
            }
            Some(steps) if steps.is_empty() => self.info("Nothing was recorded".to_owned()),
            Some(steps) => {
                self.info(format!("Macro recorded: {} steps", steps.len()));
                self.macros.last = steps;
            }
        }
    }

    pub(super) fn macro_play(&mut self, fx: &mut Vec<Effect>) {
        if self.macros.last.is_empty() {
            return self.info("No macro yet: record one first".to_owned());
        }
        if self.macros.playing {
            return;
        }
        self.macros.playing = true;
        for step in self.macros.last.clone() {
            fx.extend(self.dispatch(step));
        }
        self.macros.playing = false;
    }
}
