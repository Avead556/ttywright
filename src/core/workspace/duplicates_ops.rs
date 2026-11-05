use super::{Focus, ToolTab, Workspace};
use crate::core::command::{Command, Effect, Motion};
use crate::core::{DuplicatesEvent, DuplicatesPanel};

impl Workspace {
    pub const fn duplicates_panel(&self) -> &DuplicatesPanel {
        &self.duplicates
    }

    pub(super) fn duplicates_show(&mut self, fx: &mut Vec<Effect>) {
        self.tool_window_visible = true;
        self.tool_tab = ToolTab::Duplicates;
        self.focus = Focus::Duplicates;
        self.duplicates_scan(fx);
    }

    fn duplicates_scan(&mut self, fx: &mut Vec<Effect>) {
        self.autosave(fx);
        fx.push(Effect::Duplicates(self.duplicates.scan(self.root.clone())));
    }

    pub(super) fn duplicates_finished(&mut self, event: DuplicatesEvent) {
        self.duplicates.handle(event);
    }

    pub(super) fn duplicates_input(&mut self, input: &Command, fx: &mut Vec<Effect>) {
        let rows = |rows: usize| isize::try_from(rows).unwrap_or(isize::MAX);
        match input {
            Command::Move(Motion::Down) => self.duplicates.step(1),
            Command::Move(Motion::Up) => self.duplicates.step(-1),
            Command::Move(Motion::PageDown { rows: n }) => self.duplicates.step(rows(*n)),
            Command::Move(Motion::PageUp { rows: n }) => self.duplicates.step(-rows(*n)),
            Command::Newline => self.duplicates_open(fx),
            Command::Insert(letter) => match letter.to_lowercase().as_str() {
                "r" => self.duplicates_scan(fx),
                "s" => fx.extend(self.duplicates.stop().map(Effect::Duplicates)),
                "a" => {
                    self.duplicates.anonymize = !self.duplicates.anonymize;
                    self.duplicates_scan(fx);
                }
                _ => {}
            },
            _ => {}
        }
    }

    fn duplicates_open(&mut self, fx: &mut Vec<Effect>) {
        if let Some(place) = self.duplicates.selected_place() {
            self.focus = Focus::Editor;
            self.open_file(place.path, Some((place.first_line as usize, 0)), fx);
        }
    }

    pub(super) fn duplicate_row_clicked(&mut self, index: usize, open: bool, fx: &mut Vec<Effect>) {
        self.picker = None;
        self.focus = Focus::Duplicates;
        if self.duplicates.select(index) && open {
            self.duplicates_open(fx);
        }
    }
}
