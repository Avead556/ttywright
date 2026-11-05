use super::{
    Command, DocumentId, Effect, HistoryEntry, LocalHistoryPanel, Motion, SnapshotLabel, Workspace,
    diff_rows,
};

impl Workspace {
    pub(super) fn history_listed(
        &mut self,
        doc: DocumentId,
        entries: Vec<HistoryEntry>,
        now: u64,
        fx: &mut Vec<Effect>,
    ) {
        if entries.is_empty() {
            return self.info(format!("No Local History for {}", self.doc_name(doc)));
        }
        if self.documents.contains_key(&doc) {
            self.picker = None;
            self.local_history = Some(LocalHistoryPanel::new(doc, entries, now));
            self.read_selected_version(fx);
        }
    }

    pub(super) fn read_selected_version(&self, fx: &mut Vec<Effect>) {
        if let Some(entry) = self.local_history.as_ref().and_then(|p| p.selected_entry()) {
            fx.push(Effect::ReadHistoryVersion {
                hash: entry.hash.clone(),
            });
        }
    }

    pub(super) fn history_version(&mut self, hash: &str, result: Result<Vec<u8>, String>) {
        let Some(panel) = self.local_history.as_mut() else {
            return;
        };
        if panel.selected_entry().is_none_or(|e| e.hash != hash) {
            return;
        }
        match result {
            Ok(bytes) => {
                let current = self
                    .documents
                    .get(&panel.doc)
                    .map(|d| d.to_bytes())
                    .unwrap_or_default();
                panel.diff = diff_rows(
                    &String::from_utf8_lossy(&bytes),
                    &String::from_utf8_lossy(&current),
                );
                panel.version = Some(bytes);
            }
            Err(e) => panel.error = Some(format!("Could not read this version: {e}")),
        }
    }

    pub(super) fn history_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(panel) = self.local_history.as_mut() else {
            return;
        };
        let max_scroll = panel.diff.len().saturating_sub(1);
        let target = match input {
            Command::Escape => {
                self.local_history = None;
                return;
            }
            Command::Insert(s) if s.eq_ignore_ascii_case("r") => return self.revert_to_version(fx),
            Command::Move(Motion::Up) => panel.selected.saturating_sub(1),
            Command::Move(Motion::Down) => panel.selected + 1,
            Command::SelectHistoryEntry { index } => index,
            Command::Move(Motion::PageUp { rows }) => {
                panel.diff_scroll = panel.diff_scroll.saturating_sub(rows);
                return;
            }
            Command::Move(Motion::PageDown { rows }) => {
                panel.diff_scroll = (panel.diff_scroll + rows).min(max_scroll);
                return;
            }
            _ => return,
        };
        let target = target.min(panel.entries.len().saturating_sub(1));
        if target != panel.selected {
            panel.select(target);
            self.read_selected_version(fx);
        }
    }

    pub(super) fn revert_to_version(&mut self, fx: &mut Vec<Effect>) {
        let Some(panel) = self.local_history.as_ref() else {
            return;
        };
        let (doc, Some(version)) = (panel.doc, panel.version.clone()) else {
            return;
        };
        let Some(document) = self.documents.get_mut(&doc) else {
            return;
        };
        fx.push(Effect::Snapshot {
            path: document.path().to_path_buf(),
            bytes: document.to_bytes(),
            label: SnapshotLabel::BeforeRevert,
        });
        match document.replace_unsaved(&version) {
            Ok(applied) => {
                self.local_history = None;
                self.after_change(None, doc, &applied);
                self.info("Reverted; Ctrl+Z brings the previous text back".to_owned());
            }
            Err(e) => self.error(e.to_string()),
        }
    }
}
