use super::{
    Axis, Cursor, DiskState, Document, DocumentId, Effect, Focus, Pane, PaneId, PathBuf, ToolTab,
    View, ViewId, Workspace, WriteOrigin, text,
};

impl Workspace {
    pub(super) fn open_file(
        &mut self,
        path: PathBuf,
        at: Option<(usize, usize)>,
        fx: &mut Vec<Effect>,
    ) {
        let path = if path.is_absolute() {
            path
        } else {
            self.root.join(path)
        };
        let existing = self
            .documents
            .iter()
            .find(|(_, d)| d.path() == path)
            .map(|(id, _)| *id);
        match existing {
            Some(doc) => self.show_document(doc, at),
            None => fx.push(Effect::ReadFile { path, at }),
        }
    }

    pub(super) fn file_read(
        &mut self,
        path: PathBuf,
        at: Option<(usize, usize)>,
        disk: Option<DiskState>,
        result: Result<Option<Vec<u8>>, String>,
    ) {
        if let Some(doc) = self
            .documents
            .iter()
            .find(|(_, d)| d.path() == path)
            .map(|(id, _)| *id)
        {
            self.show_document(doc, at);
            return;
        }
        let mut document = match result {
            Ok(Some(bytes)) => match Document::from_bytes(path, &bytes) {
                Ok(d) => d,
                Err(e) => return self.error(e.to_string()),
            },
            Ok(None) => Document::new(path),
            Err(e) => return self.error(format!("Could not open {}: {e}", path.display())),
        };
        document.set_disk(disk);
        let id = DocumentId(self.alloc_id());
        self.documents.insert(id, document);
        self.preview_if_wanted(id);
        self.show_document(id, at);
    }

    pub(super) fn show_document(&mut self, doc: DocumentId, at: Option<(usize, usize)>) {
        let pane = self.panes.entry(self.active_pane).or_default();
        let found = pane
            .views
            .iter()
            .position(|v| self.views.get(v).is_some_and(|v| v.doc == doc));
        match found {
            Some(i) => pane.active = i,
            None => self.add_view(doc, Cursor::default()),
        }
        if let Some((line, col)) = at {
            self.set_active_cursor_line_col(line, col);
        }
        self.focus = Focus::Editor;
        self.refresh_find();
    }

    pub(super) fn focus_view(&mut self, view: ViewId, at: Option<(usize, usize)>) {
        let Some(pane_id) = self
            .panes
            .iter()
            .find(|(_, p)| p.views.contains(&view))
            .map(|(id, _)| *id)
        else {
            return;
        };
        self.picker = None;
        self.close_undo_group();
        self.active_pane = pane_id;
        if let Some(pane) = self.panes.get_mut(&pane_id) {
            pane.active = pane.views.iter().position(|v| *v == view).unwrap_or(0);
        }
        self.focus = Focus::Editor;
        let text = self
            .views
            .get(&view)
            .and_then(|v| self.documents.get(&v.doc))
            .map(|d| d.text().clone());
        if let (Some((line, col)), Some(text)) = (at, text) {
            let line = line.min(text.len_lines() - 1);
            let idx = text::idx_at_col(&text, line, col, self.tab_width);
            self.set_active_cursor(Cursor::at(idx));
        }
        self.refresh_find();
    }

    pub(super) fn set_focus(&mut self, focus: Focus) {
        self.picker = None;
        match focus {
            Focus::Project => self.project_visible = true,
            Focus::FindInFiles => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Find;
            }
            Focus::Problems => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Problems;
            }
            Focus::Git => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Git;
            }
            Focus::Run => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Run;
            }
            Focus::Debug => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Debug;
            }
            Focus::Language => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Language;
            }
            Focus::Duplicates => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Duplicates;
            }
            Focus::Dependencies => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Dependencies;
            }
            Focus::Tests => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Tests;
            }
            Focus::Todo => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Todo;
            }
            Focus::Bookmarks => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Bookmarks;
            }
            Focus::Http => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Http;
            }
            Focus::Database => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Database;
            }
            Focus::Docker => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Docker;
            }
            Focus::GitHub => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::GitHub;
            }
            Focus::Claude => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Claude;
            }
            Focus::Terminal => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Terminal;
            }
            Focus::FindBar if self.find_bar.is_none() => return,
            Focus::Editor | Focus::FindBar => {}
        }
        self.focus = focus;
    }

    pub(super) fn add_view(&mut self, doc: DocumentId, cursor: Cursor) {
        let id = ViewId(self.alloc_id());
        self.views.insert(id, View { doc, cursor });
        let pane = self.panes.entry(self.active_pane).or_default();
        let at = (pane.active + 1).min(pane.views.len());
        pane.views.insert(at, id);
        pane.active = at;
    }

    pub(super) fn cycle_tab(&mut self, delta: isize) {
        if let Some(pane) = self.panes.get_mut(&self.active_pane)
            && !pane.views.is_empty()
        {
            let n = pane.views.len() as isize;
            pane.active = (pane.active as isize + delta).rem_euclid(n) as usize;
        }
        self.refresh_find();
    }

    pub(super) fn close_view(&mut self, id: ViewId, fx: &mut Vec<Effect>) {
        let Some(view) = self.views.remove(&id) else {
            return;
        };
        let pane_id = self
            .panes
            .iter()
            .find(|(_, p)| p.views.contains(&id))
            .map(|(pid, _)| *pid);
        if let Some(pane_id) = pane_id
            && let Some(pane) = self.panes.get_mut(&pane_id)
        {
            pane.views.retain(|v| *v != id);
            pane.active = pane.active.min(pane.views.len().saturating_sub(1));
            if pane.views.is_empty() && self.panes.len() > 1 {
                self.remove_pane(pane_id);
            }
        }
        if !self.views.values().any(|v| v.doc == view.doc) {
            match self.documents.get(&view.doc) {
                Some(d) if d.is_dirty() => {
                    fx.extend(self.write_effect(view.doc, WriteOrigin::User));
                }
                _ => {
                    self.documents.remove(&view.doc);
                }
            }
        }
        self.refresh_find();
    }

    pub(super) fn drop_if_orphan(&mut self, doc: DocumentId) {
        let clean = self.documents.get(&doc).is_some_and(|d| !d.is_dirty());
        if clean && !self.views.values().any(|v| v.doc == doc) {
            self.documents.remove(&doc);
        }
    }

    pub(super) fn split(&mut self, axis: Axis) {
        let Some((view, _)) = self.active() else {
            return;
        };
        let (doc, cursor) = (view.doc, view.cursor);
        let new = PaneId(self.alloc_id());
        if self.layout.split(self.active_pane, new, axis) {
            self.panes.insert(new, Pane::default());
            self.active_pane = new;
            self.add_view(doc, cursor);
            self.focus = Focus::Editor;
        }
    }

    pub(super) fn unsplit(&mut self, fx: &mut Vec<Effect>) {
        if self.panes.len() < 2 {
            return;
        }
        let pane = self.active_pane;
        let views = self
            .panes
            .get(&pane)
            .map(|p| p.views.clone())
            .unwrap_or_default();
        for v in views {
            self.close_view(v, fx);
        }
        if self.panes.contains_key(&pane) {
            self.remove_pane(pane);
        }
    }

    pub(super) fn remove_pane(&mut self, pane: PaneId) {
        self.layout.remove(pane);
        self.panes.remove(&pane);
        if self.active_pane == pane {
            self.active_pane = self.layout.panes()[0];
        }
    }

    pub(super) fn alloc_id(&mut self) -> u32 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }
}
