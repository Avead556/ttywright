use super::{
    BTreeMap, Cursor, Document, DocumentError, DocumentId, Effect, Focus, Layout, Pane, PaneId,
    QuitState, RecoveredSwap, SavedLayout, SavedView, Session, SessionFile, View, ViewId,
    Workspace, WriteGuard, WriteOrigin, conflict_for, text,
};

impl Workspace {
    pub(super) fn write_effect(&self, doc: DocumentId, origin: WriteOrigin) -> Option<Effect> {
        let guard = WriteGuard::Unchanged(self.documents.get(&doc)?.disk());
        self.write_with(doc, guard, origin)
    }

    pub(super) fn overwrite_effect(&self, doc: DocumentId) -> Option<Effect> {
        self.write_with(doc, WriteGuard::Overwrite, WriteOrigin::User)
    }

    pub(super) fn write_with(
        &self,
        doc: DocumentId,
        guard: WriteGuard,
        origin: WriteOrigin,
    ) -> Option<Effect> {
        let d = self.documents.get(&doc).filter(|d| !d.is_image())?;
        Some(Effect::WriteFile {
            doc,
            version: d.version(),
            path: d.path().to_path_buf(),
            bytes: d.to_bytes(),
            guard,
            origin,
        })
    }

    pub(super) fn autosave(&self, fx: &mut Vec<Effect>) {
        let queued = |doc: DocumentId, fx: &[Effect]| {
            fx.iter()
                .any(|e| matches!(e, Effect::WriteFile { doc: d, .. } if *d == doc))
        };
        for doc in self.dirty_docs() {
            let in_conflict = self
                .documents
                .get(&doc)
                .is_some_and(|d| d.conflict().is_some());
            if !in_conflict && !queued(doc, fx) {
                fx.extend(self.write_effect(doc, WriteOrigin::Auto));
            }
        }
    }

    pub(super) fn autosave_on_switch(&self, active_before: Option<ViewId>, fx: &mut Vec<Effect>) {
        if self.autosave_on_switch && active_before.is_some() && self.active_view() != active_before
        {
            self.autosave(fx);
        }
    }

    pub(super) fn doc_name(&self, doc: DocumentId) -> String {
        self.documents
            .get(&doc)
            .map(Document::file_name)
            .unwrap_or_default()
    }

    pub(super) fn write_refused(&mut self, doc: DocumentId, mut text: String, tell: bool) {
        if !self.views.values().any(|v| v.doc == doc) {
            self.add_view(doc, Cursor::default());
        }
        let quitting = self.quit == QuitState::SavingBeforeQuit;
        if quitting {
            self.quit = QuitState::SaveFailed;
            text.push_str(" — quit again to discard changes");
        }
        if tell || quitting {
            self.error(text);
        }
    }

    pub(super) fn write_swaps(&mut self) -> Vec<Effect> {
        let mut fx = Vec::new();
        for d in self.documents.values_mut().filter(|d| d.needs_swap()) {
            fx.push(Effect::WriteSwap {
                path: d.path().to_path_buf(),
                base: d.disk(),
                contents: d.to_bytes(),
            });
            d.mark_swapped();
        }
        fx
    }

    pub(super) fn journal_changes(&mut self, fx: &mut Vec<Effect>) {
        let mut journal = Vec::new();
        for d in self.documents.values_mut() {
            if let Some((start, changes)) = d.take_journal() {
                journal.push(Effect::JournalAppend {
                    path: d.path().to_path_buf(),
                    disk: d.disk(),
                    start,
                    changes,
                });
            }
        }
        fx.splice(0..0, journal);
    }

    pub(super) fn sweep_swaps(&mut self, fx: &mut Vec<Effect>) {
        self.journal_changes(fx);
        for d in self.documents.values_mut() {
            if d.take_stale_journal() {
                fx.push(Effect::RemoveJournal {
                    path: d.path().to_path_buf(),
                });
            }
            if d.take_stale_swap() {
                fx.push(Effect::RemoveSwap {
                    path: d.path().to_path_buf(),
                });
            }
        }
    }

    pub(super) fn recover_swaps(&mut self, swaps: Vec<RecoveredSwap>) {
        if !self.trusted {
            if !swaps.is_empty() {
                self.info(format!(
                    "Unsaved changes in {} file(s) wait until this project is trusted",
                    swaps.len()
                ));
            }
            self.untrusted_swaps.extend(swaps);
            return;
        }
        let mut recovered = 0;
        for swap in swaps {
            match self.recover_swap(swap) {
                Ok(()) => recovered += 1,
                Err(e) => self.error(format!("Could not recover: {e}")),
            }
        }
        if recovered > 0 {
            self.info(format!("Recovered unsaved changes in {recovered} file(s)"));
        }
    }

    pub(super) fn recover_swap(&mut self, swap: RecoveredSwap) -> Result<(), DocumentError> {
        let open = self
            .documents
            .iter()
            .find(|(_, d)| d.path() == swap.path)
            .map(|(id, _)| *id);
        let id = match open {
            Some(id) => id,
            None => {
                let document = match &swap.on_disk {
                    Some(bytes) => Document::from_bytes(swap.path.clone(), bytes)?,
                    None => Document::new(swap.path.clone()),
                };
                let id = DocumentId(self.alloc_id());
                self.documents.insert(id, document);
                id
            }
        };
        let Some(document) = self.documents.get_mut(&id) else {
            return Ok(());
        };
        document.set_disk(swap.current);
        let applied = document.recover_unsaved(&swap.contents)?;
        if swap.current != swap.base {
            document.set_conflict(Some(conflict_for(swap.current)));
        }
        self.after_change(None, id, &applied);
        if !self.views.values().any(|v| v.doc == id) {
            self.show_document(id, None);
        }
        Ok(())
    }

    pub(super) fn save_session(&mut self) -> Vec<Effect> {
        let session = Session {
            version: Session::VERSION,
            layout: self.save_layout(&self.layout),
            project_visible: self.project_visible,
            tool_window_visible: self.tool_window_visible,
            recent_files: self.recent.clone(),
            tool_height: self.tool_height,
            project_width: self.project_width,
            bookmarks: self.saved_bookmarks(),
            breakpoints: self.saved_breakpoints(),
            watches: self.debug.watches.clone(),
            changelists: self.changelists.saved().clone(),
            markdown_preview: self.markdown_choices.clone(),
        };
        if self.saved_session.as_ref() == Some(&session) {
            return Vec::new();
        }
        self.saved_session = Some(session.clone());
        vec![Effect::WriteSession(session)]
    }

    pub(super) fn save_layout(&self, node: &Layout) -> SavedLayout {
        match node {
            Layout::Pane(id) => {
                let pane = self.panes.get(id);
                SavedLayout::Pane {
                    views: pane
                        .into_iter()
                        .flat_map(|p| &p.views)
                        .filter_map(|v| self.save_view(*v))
                        .collect(),
                    selected: pane.map_or(0, |p| p.active),
                    active: *id == self.active_pane,
                }
            }
            Layout::Split { axis, children } => SavedLayout::Split {
                axis: *axis,
                children: children.iter().map(|c| self.save_layout(c)).collect(),
            },
        }
    }

    pub(super) fn save_view(&self, id: ViewId) -> Option<SavedView> {
        let view = self.views.get(&id)?;
        let doc = self.documents.get(&view.doc)?;
        let line = doc.text().char_to_line(view.cursor.idx);
        Some(SavedView {
            path: doc.path().to_path_buf(),
            line,
            col: view.cursor.idx - doc.text().line_to_char(line),
        })
    }

    pub(super) fn restore_session(&mut self, session: Session, files: Vec<SessionFile>) {
        for path in &session.recent_files {
            if !self.recent.contains(path) {
                self.recent.push(path.clone());
            }
        }
        self.tool_height = session.tool_height;
        self.project_width = session.project_width;
        self.markdown_choices.clone_from(&session.markdown_preview);
        self.restore_bookmarks(session.bookmarks.clone());
        self.restore_breakpoints(session.breakpoints.clone());
        self.debug.watches.clone_from(&session.watches);
        self.changelists = crate::core::git::Changelists::from_saved(session.changelists.clone());
        if !self.documents.is_empty() {
            return;
        }
        for file in files {
            if let Ok(Some(bytes)) = file.result
                && let Ok(mut document) = Document::from_bytes(file.path, &bytes)
            {
                document.set_disk(file.disk);
                let id = DocumentId(self.alloc_id());
                self.documents.insert(id, document);
                self.preview_if_wanted(id);
            }
        }
        self.project_visible = session.project_visible;
        self.tool_window_visible = session.tool_window_visible;
        let mut active = None;
        let mut panes = BTreeMap::new();
        let Some(layout) = self.restore_layout(session.layout, &mut panes, &mut active) else {
            return;
        };
        self.layout = layout;
        self.active_pane = active.unwrap_or_else(|| self.layout.panes()[0]);
        self.panes = panes;
        self.focus = Focus::Editor;
    }

    pub(super) fn restore_layout(
        &mut self,
        saved: SavedLayout,
        panes: &mut BTreeMap<PaneId, Pane>,
        active_pane: &mut Option<PaneId>,
    ) -> Option<Layout> {
        match saved {
            SavedLayout::Pane {
                views,
                selected,
                active,
            } => {
                let mut pane = Pane::default();
                for (i, saved) in views.into_iter().enumerate() {
                    if let Some(view) = self.restore_view(&saved) {
                        if i <= selected {
                            pane.active = pane.views.len();
                        }
                        pane.views.push(view);
                    }
                }
                if pane.views.is_empty() {
                    return None;
                }
                let id = PaneId(self.alloc_id());
                panes.insert(id, pane);
                if active {
                    *active_pane = Some(id);
                }
                Some(Layout::Pane(id))
            }
            SavedLayout::Split { axis, children } => {
                let mut kept: Vec<Layout> = children
                    .into_iter()
                    .filter_map(|c| self.restore_layout(c, panes, active_pane))
                    .collect();
                match kept.len() {
                    0 => None,
                    1 => kept.pop(),
                    _ => Some(Layout::Split {
                        axis,
                        children: kept,
                    }),
                }
            }
        }
    }

    pub(super) fn restore_view(&mut self, saved: &SavedView) -> Option<ViewId> {
        let (doc, cursor) = self.documents.iter().find_map(|(id, d)| {
            (d.path() == saved.path).then(|| {
                let t = d.text();
                let line = saved.line.min(t.len_lines() - 1);
                let col = saved.col.min(text::line_content_len(t, line));
                (*id, Cursor::at(t.line_to_char(line) + col))
            })
        })?;
        let id = ViewId(self.alloc_id());
        self.views.insert(id, View { doc, cursor });
        Some(id)
    }
}
