use super::{
    Applied, Assoc, Command, Cursor, Document, DocumentId, Effect, Focus, Motion, PasteTarget,
    ViewId, Workspace, apply_motion, is_word_char, text, word_start,
};
use crate::core::image_art::{ImageState, MAX_ZOOM, PictureView};

impl Workspace {
    pub(super) fn no_file_hint(&mut self) {
        self.info("No file is open: Ctrl+P finds one, Alt+Ins makes one".to_owned());
    }

    pub(super) fn editor_input(&mut self, input: Command) {
        let Some((view, doc)) = self.active() else {
            if matches!(input, Command::Insert(_)) {
                self.no_file_hint();
            }
            return;
        };
        if doc.is_image() {
            let id = view.doc;
            return self.picture_input(id, input);
        }
        let cursor = view.cursor.idx;
        let selected = view.cursor.selection();
        let text = doc.text();
        match input {
            Command::Insert(s) => {
                let mergeable =
                    selected.is_none() && s.chars().count() == 1 && !s.contains(['\n', '\r']);
                let span = selected.unwrap_or(cursor..cursor);
                self.edit(span.start, span.end, &s, mergeable);
            }
            Command::Newline => {
                let ending = doc.line_ending().as_str();
                let span = selected.unwrap_or(cursor..cursor);
                self.edit(span.start, span.end, ending, false);
            }
            Command::DeleteBackward => {
                let span =
                    selected.unwrap_or_else(|| text::prev_grapheme_boundary(text, cursor)..cursor);
                self.edit(span.start, span.end, "", false);
            }
            Command::DeleteForward => {
                let span =
                    selected.unwrap_or_else(|| cursor..text::next_grapheme_boundary(text, cursor));
                self.edit(span.start, span.end, "", false);
            }
            Command::Indent if selected.is_none() => {
                self.edit(cursor, cursor, &" ".repeat(self.tab_width), false);
            }
            Command::Indent => self.shift_lines(true),
            Command::Outdent => self.shift_lines(false),
            Command::Move(Motion::Left) if selected.is_some() => {
                self.close_undo_group();
                self.set_active_cursor(Cursor::at(selected.map_or(cursor, |s| s.start)));
            }
            Command::Move(Motion::Right) if selected.is_some() => {
                self.close_undo_group();
                self.set_active_cursor(Cursor::at(selected.map_or(cursor, |s| s.end)));
            }
            Command::Move(motion) => {
                let moved = apply_motion(text, view.cursor, motion, self.metrics());
                self.close_undo_group();
                self.set_active_cursor(moved);
            }
            Command::Select(motion) => {
                let mut moved = apply_motion(text, view.cursor, motion, self.metrics());
                moved.anchor = Some(view.cursor.anchor.unwrap_or(cursor));
                self.close_undo_group();
                self.set_active_cursor(moved);
            }
            Command::SelectTo { line, col } => {
                let line = line.min(text.len_lines() - 1);
                let mut moved = Cursor::at(text::idx_at_col(text, line, col, self.tab_width));
                moved.anchor = Some(view.cursor.anchor.unwrap_or(cursor));
                self.close_undo_group();
                self.set_active_cursor(moved);
            }
            Command::SelectWord => {
                let start = word_start(text, cursor);
                let len = text
                    .chars_at(start)
                    .take_while(|c| is_word_char(*c))
                    .count();
                if len > 0 {
                    let mut moved = Cursor::at(start + len);
                    moved.anchor = Some(start);
                    self.set_active_cursor(moved);
                }
            }
            _ => {}
        }
    }

    pub(super) fn edit(&mut self, from: usize, to: usize, s: &str, mergeable: bool) {
        if from == to && s.is_empty() {
            return;
        }
        let Some(view_id) = self.active_view() else {
            return;
        };
        let Some(doc_id) = self.views.get(&view_id).map(|v| v.doc) else {
            return;
        };
        let Some(doc) = self.documents.get_mut(&doc_id) else {
            return;
        };
        let applied = doc.edit(from, to, s, mergeable);
        self.after_change(Some(view_id), doc_id, &applied);
    }

    pub(super) fn travel(&mut self, step: fn(&mut Document) -> Option<Applied>, nothing: &str) {
        let Some(view_id) = self.active_view() else {
            return;
        };
        let Some(doc_id) = self.views.get(&view_id).map(|v| v.doc) else {
            return;
        };
        let Some(doc) = self.documents.get_mut(&doc_id) else {
            return;
        };
        match step(doc) {
            Some(applied) => self.after_change(Some(view_id), doc_id, &applied),
            None => self.info(nothing.to_owned()),
        }
    }

    pub(super) fn after_change(
        &mut self,
        editing: Option<ViewId>,
        doc: DocumentId,
        applied: &Applied,
    ) {
        if !self.bookmarks.is_empty()
            && let Some(d) = self.documents.get(&doc)
        {
            let (path, text) = (d.path().to_path_buf(), d.text().clone());
            self.bookmarks_changed(&path, &text, &applied.changes);
        }
        if !self.debug.breakpoints.is_empty()
            && let Some(d) = self.documents.get(&doc)
        {
            let (path, text) = (d.path().to_path_buf(), d.text().clone());
            self.breakpoints_changed(&path, &text, &applied.changes);
        }
        if let Some(path) = self.documents.get(&doc).map(|d| d.path().to_path_buf())
            && let Some(items) = self.diagnostics.get_mut(&path)
        {
            for item in items {
                if let Some(chars) = &mut item.chars {
                    for cs in &applied.changes {
                        chars.start = cs.map_pos(chars.start, Assoc::Before);
                        chars.end = cs.map_pos(chars.end, Assoc::Before).max(chars.start);
                    }
                }
            }
        }
        for (id, view) in &mut self.views {
            if view.doc != doc {
                continue;
            }
            if Some(*id) == editing {
                view.cursor = Cursor::at(applied.cursor);
            } else {
                for cs in &applied.changes {
                    view.cursor = Cursor::at(cs.map_pos(view.cursor.idx, Assoc::Before));
                }
            }
        }
        self.refresh_find();
    }

    pub(super) fn set_active_cursor(&mut self, cursor: Cursor) {
        if let Some(view) = self.active_view().and_then(|v| self.views.get_mut(&v)) {
            view.cursor = cursor;
        }
    }

    pub(super) fn paste_target(&self) -> PasteTarget {
        PasteTarget {
            view: self.active_view(),
            cursor: self.active().map(|(view, _)| view.cursor),
            focus: self.focus,
            stamp: self.edit_stamp(),
        }
    }

    pub(super) fn shift_lines(&mut self, indent: bool) {
        let tab_width = self.tab_width;
        let Some((view, d)) = self.active() else {
            return;
        };
        let t = d.text();
        let cursor = view.cursor;
        let selection = cursor.selection();
        let span = selection.clone().unwrap_or(cursor.idx..cursor.idx);
        let first = t.char_to_line(span.start);
        let end_line = t.char_to_line(span.end);
        let last = if end_line > first && t.line_to_char(end_line) == span.end {
            end_line - 1
        } else {
            end_line
        };
        let from = t.line_to_char(first);
        let to = t.line_to_char(last) + text::line_content_len(t, last);
        let mut merged = String::new();
        let mut shifts: Vec<(usize, isize)> = Vec::new();
        for line in first..=last {
            let start = t.line_to_char(line);
            let content = text::line_content(t, line);
            let change = if indent {
                if content.is_empty() {
                    0
                } else {
                    tab_width as isize
                }
            } else if content.starts_with('\t') {
                -1
            } else {
                -(content
                    .chars()
                    .take(tab_width)
                    .take_while(|c| *c == ' ')
                    .count() as isize)
            };
            if change > 0 {
                merged.push_str(&" ".repeat(tab_width));
            }
            merged.extend(
                content
                    .chars()
                    .skip(change.unsigned_abs() * usize::from(change < 0)),
            );
            if line < last {
                let ending_from = start + content.chars().count();
                let ending_to = t.line_to_char(line + 1);
                merged.extend(t.slice(ending_from..ending_to).chars());
            }
            shifts.push((start, change));
        }
        let total: isize = shifts.iter().map(|s| s.1).sum();
        if total == 0 {
            return;
        }
        let selecting = selection.is_some();
        let map = |pos: usize| -> usize {
            if pos < from {
                return pos;
            }
            if pos > to {
                return pos.saturating_add_signed(total);
            }
            let mut moved = pos;
            let mut done = 0;
            for (start, change) in &shifts {
                if pos < *start {
                    break;
                }
                let col = pos - start;
                if pos == *start && selecting && *change > 0 {
                    continue;
                }
                let own = if *change < 0 {
                    -((col as isize).min(-*change))
                } else {
                    *change
                };
                done += own;
            }
            moved = moved.saturating_add_signed(done);
            moved
        };
        let mut after = Cursor::at(map(cursor.idx));
        after.anchor = cursor.anchor.filter(|_| selecting).map(map);
        self.close_undo_group();
        self.edit(from, to, &merged, false);
        self.close_undo_group();
        self.set_active_cursor(after);
    }

    pub(super) fn copy(&mut self, cut: bool, fx: &mut Vec<Effect>) {
        if self.focus != Focus::Editor || self.picker.is_some() {
            return;
        }
        let Some((view, d)) = self.active() else {
            return;
        };
        let t = d.text();
        let span = view.cursor.selection().unwrap_or_else(|| {
            let line = t.char_to_line(view.cursor.idx);
            let next = (line + 1 < t.len_lines()).then(|| t.line_to_char(line + 1));
            t.line_to_char(line)..next.unwrap_or_else(|| t.len_chars())
        });
        let text = t.slice(span.clone()).to_string();
        self.clipboard.clone_from(&text);
        fx.push(Effect::WriteClipboard { text });
        if cut {
            self.close_undo_group();
            self.edit(span.start, span.end, "", false);
        }
    }

    pub(super) fn close_undo_group(&mut self) {
        if let Some(doc) = self.active().map(|(v, _)| v.doc)
            && let Some(d) = self.documents.get_mut(&doc)
        {
            d.close_undo_group();
        }
    }
}

const PICTURE_STEP: (u16, u16) = (8, 4);

impl Workspace {
    fn picture_input(&mut self, doc: DocumentId, input: Command) {
        let mut view = self.picture_view(doc);
        match input {
            Command::Insert(s) => match s.as_str() {
                "+" | "=" if view.zoom < MAX_ZOOM => {
                    view.zoom += 1;
                    view.x = view.x.saturating_mul(2);
                    view.y = view.y.saturating_mul(2);
                }
                "-" if view.zoom > 0 => {
                    view.zoom -= 1;
                    view.x /= 2;
                    view.y /= 2;
                }
                "0" => view = PictureView::default(),
                "+" | "=" | "-" => {}
                _ => return self.info("A picture cannot be edited: + - 0 zoom".to_owned()),
            },
            Command::Newline => return self.info("A picture cannot be edited here".to_owned()),
            Command::Move(Motion::Left) => view.x = view.x.saturating_sub(PICTURE_STEP.0),
            Command::Move(Motion::Right) => view.x = view.x.saturating_add(PICTURE_STEP.0),
            Command::Move(Motion::Up) => view.y = view.y.saturating_sub(PICTURE_STEP.1),
            Command::Move(Motion::Down) => view.y = view.y.saturating_add(PICTURE_STEP.1),
            _ => return,
        }
        let (x, y) = self.picture_extent(doc);
        view.x = view.x.min(x);
        view.y = view.y.min(y);
        self.picture_views.insert(doc, view);
    }

    fn picture_extent(&self, doc: DocumentId) -> (u16, u16) {
        let Some(path) = self
            .documents
            .get(&doc)
            .map(|d| d.path().display().to_string())
        else {
            return (0, 0);
        };
        self.images
            .iter()
            .filter(|(request, _)| request.source == path)
            .filter_map(|(_, state)| match state {
                ImageState::Ready(image) => Some((image.cols, image.rows)),
                _ => None,
            })
            .fold((0, 0), |(x, y), (c, r)| {
                (x.max(c.saturating_sub(1)), y.max(r.saturating_sub(1)))
            })
    }
}
