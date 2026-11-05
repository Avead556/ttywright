use super::navigate_ops::Bookmark;
use super::{Focus, ToolTab, Workspace};
use crate::core::command::{Command, Effect, Motion};
use crate::core::{FindOptions, SearchMatch};

const TODO_PATTERN: &str = r"\b(TODO|FIXME|XXX|HACK)\b";

#[derive(Debug, Clone, Default)]
pub struct TodoPanel {
    pub results: Vec<SearchMatch>,
    pub selected: usize,
    pub running: Option<u64>,
    pub truncated: bool,
}

pub(super) fn moved(selected: usize, count: usize, motion: Motion) -> usize {
    let last = count.saturating_sub(1);
    match motion {
        Motion::Down => (selected + 1).min(last),
        Motion::Up => selected.saturating_sub(1),
        Motion::PageDown { rows } => (selected + rows).min(last),
        Motion::PageUp { rows } => selected.saturating_sub(rows),
        _ => selected,
    }
}

impl Workspace {
    pub fn todo_panel(&self) -> &TodoPanel {
        &self.todo
    }

    pub fn bookmark_selected(&self) -> usize {
        self.bookmark_selected
    }

    pub fn bookmark_row(&self, bookmark: &Bookmark) -> String {
        self.bookmark_label(bookmark)
    }

    pub(super) fn todos_show(&mut self, fx: &mut Vec<Effect>) {
        self.tool_window_visible = true;
        self.tool_tab = ToolTab::Todo;
        self.focus = Focus::Todo;
        self.todos_refresh(fx);
    }

    fn todos_refresh(&mut self, fx: &mut Vec<Effect>) {
        self.autosave(fx);
        let id = u64::from(self.alloc_id());
        self.todo.running = Some(id);
        fx.push(Effect::SearchProject {
            id,
            root: self.root.clone(),
            query: TODO_PATTERN.to_owned(),
            options: FindOptions {
                regex: true,
                case_sensitive: true,
                ..FindOptions::default()
            },
        });
    }

    pub(super) fn todos_finished(
        &mut self,
        id: u64,
        matches: &[SearchMatch],
        truncated: bool,
    ) -> bool {
        if self.todo.running != Some(id) {
            return false;
        }
        self.todo.running = None;
        self.todo.results = matches.to_vec();
        self.todo.truncated = truncated;
        self.todo.selected = self.todo.selected.min(matches.len().saturating_sub(1));
        true
    }

    pub(super) fn todo_input(&mut self, input: &Command, fx: &mut Vec<Effect>) {
        match input {
            Command::Move(motion) => {
                self.todo.selected = moved(self.todo.selected, self.todo.results.len(), *motion);
            }
            Command::Newline => self.todo_open(fx),
            Command::Insert(letter) if letter.eq_ignore_ascii_case("r") => self.todos_refresh(fx),
            _ => {}
        }
    }

    fn todo_open(&mut self, fx: &mut Vec<Effect>) {
        if let Some(hit) = self.todo.results.get(self.todo.selected) {
            let (path, at) = (hit.path.clone(), (hit.line, hit.col));
            self.focus = Focus::Editor;
            self.open_file(path, Some(at), fx);
        }
    }

    pub(super) fn todo_row_clicked(&mut self, index: usize, open: bool, fx: &mut Vec<Effect>) {
        self.picker = None;
        self.focus = Focus::Todo;
        if index < self.todo.results.len() {
            self.todo.selected = index;
            if open {
                self.todo_open(fx);
            }
        }
    }

    pub(super) fn bookmarks_show(&mut self) {
        self.tool_window_visible = true;
        self.tool_tab = ToolTab::Bookmarks;
        self.focus = Focus::Bookmarks;
        self.bookmark_selected = self
            .bookmark_selected
            .min(self.bookmarks.len().saturating_sub(1));
    }

    pub(super) fn bookmarks_input(&mut self, input: &Command, fx: &mut Vec<Effect>) {
        match input {
            Command::Move(motion) => {
                self.bookmark_selected =
                    moved(self.bookmark_selected, self.bookmarks.len(), *motion);
            }
            Command::Newline => self.bookmark_open(fx),
            Command::DeleteBackward | Command::DeleteForward => self.bookmark_remove_selected(),
            Command::Insert(letter) if letter.eq_ignore_ascii_case("d") => {
                self.bookmark_remove_selected();
            }
            _ => {}
        }
    }

    fn bookmark_open(&mut self, fx: &mut Vec<Effect>) {
        if self.bookmark_selected < self.bookmarks.len() {
            self.focus = Focus::Editor;
            self.bookmark_jump(self.bookmark_selected, fx);
        }
    }

    fn bookmark_remove_selected(&mut self) {
        if self.bookmark_selected < self.bookmarks.len() {
            self.bookmarks.remove(self.bookmark_selected);
            self.bookmark_selected = self
                .bookmark_selected
                .min(self.bookmarks.len().saturating_sub(1));
            self.info("Bookmark removed".to_owned());
        }
    }

    pub(super) fn bookmark_row_clicked(&mut self, index: usize, open: bool, fx: &mut Vec<Effect>) {
        self.picker = None;
        self.focus = Focus::Bookmarks;
        if index < self.bookmarks.len() {
            self.bookmark_selected = index;
            if open {
                self.bookmark_open(fx);
            }
        }
    }
}
