use super::{Command, Effect, Focus, ToolTab, Workspace};
use crate::core::test_results::{TestPanel, only_failed};

const LOCATION_WITHIN: usize = 40;

impl Workspace {
    #[cfg(test)]
    pub(crate) fn run_target_for_test(
        &mut self,
        target: crate::core::run::RunTarget,
    ) -> Vec<Effect> {
        let mut fx = Vec::new();
        self.start_run(target, &mut fx);
        fx
    }

    pub fn test_panel(&self) -> &TestPanel {
        &self.tests
    }

    pub(super) fn tests_show(&mut self) {
        self.tool_window_visible = true;
        self.tool_tab = ToolTab::Tests;
        self.focus = Focus::Tests;
    }

    pub(super) fn tests_input(&mut self, input: &Command, fx: &mut Vec<Effect>) {
        match input {
            Command::Move(motion) => {
                self.tests.selected =
                    super::list_ops::moved(self.tests.selected, self.tests.rows.len(), *motion);
            }
            Command::Newline => self.test_open(fx),
            Command::Insert(letter) if letter.eq_ignore_ascii_case("r") => self.tests_rerun(fx),
            Command::Insert(letter) if letter.eq_ignore_ascii_case("f") => {
                self.tests_rerun_failed(fx);
            }
            _ => {}
        }
    }

    pub(super) fn test_row_clicked(&mut self, index: usize, open: bool, fx: &mut Vec<Effect>) {
        self.picker = None;
        self.focus = Focus::Tests;
        if index < self.tests.rows.len() {
            self.tests.selected = index;
            if open {
                self.test_open(fx);
            }
        }
    }

    fn tests_run_index(&self) -> Option<usize> {
        let id = self.tests.run_id?;
        self.run.position(id)
    }

    fn test_open(&mut self, fx: &mut Vec<Effect>) {
        let Some(line) = self.tests.rows.get(self.tests.selected).map(|r| r.line) else {
            return;
        };
        let Some(index) = self.tests_run_index() else {
            return self.info("That run's output is gone".to_owned());
        };
        let panel = &self.run.tabs()[index];
        let place = (line..line + LOCATION_WITHIN)
            .filter_map(|n| panel.line(n))
            .find_map(|l| l.location.clone());
        if let Some(place) = place {
            self.focus = Focus::Editor;
            return self.open_file(place.path, Some((place.line, place.col)), fx);
        }
        self.run.show_line(index, line);
        self.tool_tab = ToolTab::Run;
        self.focus = Focus::Run;
    }

    fn tests_target(&self) -> Option<crate::core::run::RunTarget> {
        Some(self.run.tabs()[self.tests_run_index()?].target.clone())
    }

    fn tests_rerun(&mut self, fx: &mut Vec<Effect>) {
        match self.tests_target() {
            Some(target) => self.start_run(target, fx),
            None => self.info("Nothing to run again yet".to_owned()),
        }
    }

    fn tests_rerun_failed(&mut self, fx: &mut Vec<Effect>) {
        let failed = self.tests.failed();
        if failed.is_empty() {
            return self.info("No failed tests".to_owned());
        }
        let narrowed = self.tests_target().and_then(|t| only_failed(&t, &failed));
        match narrowed {
            Some(target) => self.start_run(target, fx),
            None => self.info("This command cannot run single tests: r runs it all".to_owned()),
        }
    }
}
