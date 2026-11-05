use super::{Command, Effect, Focus, ToolTab, Workspace};
use crate::core::deps::{Dependency, outdated_command};
use crate::core::run::{RunCommand, RunSource, RunTarget};

#[derive(Debug, Default)]
pub struct DepsPanel {
    pub rows: Vec<Dependency>,
    pub problems: Vec<String>,
    pub selected: usize,
    pub loading: bool,
}

impl Workspace {
    pub fn deps_panel(&self) -> &DepsPanel {
        &self.deps
    }

    pub(super) fn deps_show(&mut self, fx: &mut Vec<Effect>) {
        self.tool_window_visible = true;
        self.tool_tab = ToolTab::Dependencies;
        self.focus = Focus::Dependencies;
        self.deps_reload(fx);
    }

    fn deps_reload(&mut self, fx: &mut Vec<Effect>) {
        self.deps.loading = true;
        fx.push(Effect::ReadDependencies {
            root: self.root.clone(),
        });
    }

    pub(super) fn deps_read(&mut self, rows: Vec<Dependency>, problems: Vec<String>) {
        self.deps.selected = self.deps.selected.min(rows.len().saturating_sub(1));
        self.deps.rows = rows;
        self.deps.problems = problems;
        self.deps.loading = false;
    }

    pub(super) fn deps_input(&mut self, input: &Command, fx: &mut Vec<Effect>) {
        match input {
            Command::Move(motion) => {
                self.deps.selected =
                    super::list_ops::moved(self.deps.selected, self.deps.rows.len(), *motion);
            }
            Command::Newline => self.dep_open(fx),
            Command::Insert(letter) if letter.eq_ignore_ascii_case("r") => self.deps_reload(fx),
            Command::Insert(letter) if letter.eq_ignore_ascii_case("o") => self.deps_outdated(fx),
            _ => {}
        }
    }

    pub(super) fn dep_row_clicked(&mut self, index: usize, open: bool, fx: &mut Vec<Effect>) {
        self.picker = None;
        self.focus = Focus::Dependencies;
        if index < self.deps.rows.len() {
            self.deps.selected = index;
            if open {
                self.dep_open(fx);
            }
        }
    }

    fn dep_open(&mut self, fx: &mut Vec<Effect>) {
        if let Some(dep) = self.deps.rows.get(self.deps.selected) {
            let (path, line) = (self.root.join(&dep.file), dep.line);
            self.focus = Focus::Editor;
            self.open_file(path, Some((line, 0)), fx);
        }
    }

    fn deps_outdated(&mut self, fx: &mut Vec<Effect>) {
        let Some(file) = self
            .deps
            .rows
            .get(self.deps.selected)
            .map(|d| d.file.clone())
        else {
            return self.info("No dependencies listed".to_owned());
        };
        if !self.trusted {
            return self.trust_ask(Some("Asking the package manager".to_owned()));
        }
        let Some(argv) = outdated_command(&file) else {
            return;
        };
        let target = RunTarget {
            name: format!("outdated: {file}"),
            command: RunCommand::Argv(argv.iter().map(|&a| a.to_owned()).collect()),
            cwd: self.root.clone(),
            source: RunSource::Detected,
        };
        self.start_run(target, fx);
    }
}
