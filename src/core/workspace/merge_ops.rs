use super::{Cursor, Document, Workspace};
use crate::core::action::Action;
use crate::core::command::{Command, Effect, Motion};
use crate::core::conflict::{ConflictChoice, conflict_regions, resolution};
use crate::core::git::{GitChange, GitFile, GitNotice};
use crate::core::git_command::GitEffect;
use crate::core::merge::MergeView;

impl Workspace {
    pub fn merge_view(&self) -> Option<&MergeView> {
        self.merge.as_ref()
    }

    pub fn merge_set_page(&mut self, rows: usize) {
        if let Some(view) = self.merge.as_mut() {
            view.set_page(rows);
        }
    }

    pub(super) fn merge_open(&mut self, fx: &mut Vec<Effect>) {
        if self
            .active()
            .is_some_and(|(_, d)| !conflict_regions(d.text()).is_empty())
        {
            return self.merge_open_active();
        }
        let file = self
            .git_panel
            .as_ref()
            .and_then(|p| p.selected_file().cloned())
            .filter(|f| f.change == GitChange::Conflicted);
        match file {
            Some(file) => self.merge_open_file(&file, fx),
            None => self.info("No conflict markers in this file".to_owned()),
        }
    }

    pub(super) fn merge_open_file(&mut self, file: &GitFile, fx: &mut Vec<Effect>) {
        if file.code.index == 'D' || file.code.worktree == 'D' {
            return self.git_notice(
                "One side deleted it, so there are no markers: Space keeps the folder as it is",
                true,
            );
        }
        let top = self.repo.top_or(&self.root);
        let path = top.join(&file.path);
        if self.active().is_some_and(|(_, d)| d.path() == path) {
            return self.merge_open_active();
        }
        self.git_open_conflict(file, fx);
        if !self.window {
            self.merge_pending = Some(path);
        }
    }

    fn merge_open_active(&mut self) {
        let Some((view, d)) = self.active() else {
            return;
        };
        if conflict_regions(d.text()).is_empty() {
            let text = "No conflict markers in this file: Space marks it resolved";
            self.info(text.to_owned());
            return self.git_notice(text, false);
        }
        let file = self.display_path(d.path()).display().to_string();
        self.merge = Some(MergeView::new(view.doc, d.version(), file, d.text()));
    }

    pub(super) fn merge_catch_up(&mut self) {
        if let Some(path) = self.merge_pending.clone()
            && self.active().is_some_and(|(_, d)| d.path() == path)
        {
            self.merge_pending = None;
            self.merge_open_active();
        }
        let Some(view) = self.merge.as_ref() else {
            return;
        };
        let (doc, version, selected, page) = (view.doc, view.version, view.selected, view.page);
        let Some(d) = self.documents.get(&doc) else {
            self.merge = None;
            return;
        };
        if d.version() == version {
            return;
        }
        let file = view.file.clone();
        let mut fresh = MergeView::new(doc, d.version(), file, d.text());
        fresh.page = page;
        fresh.selected = selected.min(fresh.conflict_count().saturating_sub(1));
        fresh.show_selected();
        fresh.notice = Some(GitNotice {
            text: "The text changed, so this was laid out again".to_owned(),
            error: false,
        });
        self.merge = Some(fresh);
    }

    fn merge_notice(&mut self, text: impl Into<String>, error: bool) {
        if let Some(view) = self.merge.as_mut() {
            view.notice = Some(GitNotice {
                text: text.into(),
                error,
            });
        }
    }

    pub(super) fn merge_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(view) = self.merge.as_mut() else {
            return;
        };
        view.notice = None;
        match input {
            Command::Escape => self.merge = None,
            Command::Move(Motion::Up) => view.scroll_by(-1),
            Command::Move(Motion::Down) => view.scroll_by(1),
            Command::Move(Motion::PageUp { rows }) => {
                view.scroll_by(-isize::try_from(rows).unwrap_or(1));
            }
            Command::Move(Motion::PageDown { rows }) => {
                view.scroll_by(isize::try_from(rows).unwrap_or(1));
            }
            Command::Move(Motion::Left) => {
                view.column = view.column.saturating_sub(HORIZONTAL_STEP)
            }
            Command::Move(Motion::Right) => view.column += HORIZONTAL_STEP,
            Command::Move(Motion::DocStart) => view.scroll = 0,
            Command::Move(Motion::DocEnd) => view.scroll_by(isize::MAX),
            Command::Run(Action::Undo) => self.merge_undo(),
            Command::Newline => self.merge_finish(fx),
            Command::Insert(s) => match s.to_lowercase().as_str() {
                "o" => self.merge_choose(ConflictChoice::Ours),
                "t" => self.merge_choose(ConflictChoice::Theirs),
                "b" => self.merge_choose(ConflictChoice::Both),
                "r" => self.merge_choose(ConflictChoice::TheirsThenOurs),
                "n" => self.merge_step(true),
                "p" => self.merge_step(false),
                "u" => self.merge_undo(),
                "e" => self.merge_edit_by_hand(),
                " " => self.merge_finish(fx),
                _ => {}
            },
            _ => {}
        }
    }

    fn merge_step(&mut self, forward: bool) {
        let Some(view) = self.merge.as_mut() else {
            return;
        };
        let count = view.conflict_count();
        if count == 0 {
            return;
        }
        view.selected = if forward {
            (view.selected + 1) % count
        } else {
            (view.selected + count - 1) % count
        };
        view.show_selected();
    }

    fn merge_choose(&mut self, choice: ConflictChoice) {
        let Some(view) = self.merge.as_ref() else {
            return;
        };
        let index = view.selected;
        let doc = view.doc;
        if view.conflict(index).is_some_and(|c| c.resolved.is_some()) {
            return self.merge_notice("Already decided: U undoes it", true);
        }
        let marker = view.marker_index(index);
        if self.active().is_none_or(|(v, _)| v.doc != doc) {
            self.merge = None;
            return self.info("Another file came up: open the conflict view again".to_owned());
        }
        let Some(d) = self.documents.get(&doc) else {
            return;
        };
        let Some(region) = conflict_regions(d.text()).get(marker).copied() else {
            return self.merge_notice("Its markers are gone from the file: Esc and look", true);
        };
        let (from, to, kept) = resolution(d.text(), &region, choice);
        self.close_undo_group();
        self.edit(from, to, &kept, false);
        self.close_undo_group();
        self.set_active_cursor(Cursor::at(from));
        let version = self.documents.get(&doc).map_or(0, Document::version);
        if let Some(view) = self.merge.as_mut() {
            view.version = version;
            view.set_resolved(index, Some(choice));
            view.undo.push(index);
            if let Some(next) = view.next_open(index, true) {
                view.selected = next;
                view.show_selected();
            }
        }
    }

    fn merge_undo(&mut self) {
        let Some(view) = self.merge.as_mut() else {
            return;
        };
        let Some(index) = view.undo.pop() else {
            return self.merge_notice("Nothing decided here to undo", true);
        };
        let doc = view.doc;
        self.travel(Document::undo, "Nothing to undo");
        let version = self.documents.get(&doc).map_or(0, Document::version);
        if let Some(view) = self.merge.as_mut() {
            view.version = version;
            view.set_resolved(index, None);
            view.selected = index;
            view.show_selected();
        }
    }

    fn merge_edit_by_hand(&mut self) {
        let Some(view) = self.merge.take() else {
            return;
        };
        let marker = view.marker_index(view.selected);
        let open = view
            .conflict(view.selected)
            .is_some_and(|c| c.resolved.is_none());
        self.focus = super::Focus::Editor;
        if open
            && let Some((_, d)) = self.active()
            && let Some(region) = conflict_regions(d.text()).get(marker).copied()
        {
            self.git_put_cursor(region.start);
        }
    }

    fn merge_finish(&mut self, fx: &mut Vec<Effect>) {
        let Some(view) = self.merge.as_ref() else {
            return;
        };
        let open = view.open_count();
        if open > 0 {
            return self.merge_notice(format!("{open} conflicts are still open"), true);
        }
        let doc = view.doc;
        let Some(path) = self.documents.get(&doc).map(|d| d.path().to_path_buf()) else {
            return;
        };
        let top = self.repo.top_or(&self.root);
        let relative = path.strip_prefix(&top).unwrap_or(&path).to_path_buf();
        self.merge = None;
        self.git_show(crate::core::git::GitTab::Commit, fx);
        if self.git_begin(fx) {
            fx.push(Effect::Git(GitEffect::StageFiles {
                root: self.git_root(),
                files: vec![GitFile::new(relative, GitChange::Conflicted)],
                stage: true,
            }));
        }
    }
}

const HORIZONTAL_STEP: usize = 8;
