use super::{
    Cursor, Document, DocumentId, Effect, Focus, Language, LspAnswer, LspCodeAction, LspFileEdit,
    LspQuery, LspState, LspTextEdit, LspUsage, Path, PathBuf, PendingQuery, PickerKind,
    PositionEncoding, RenameUndo, SearchMatch, ServerKey, ServerStatus, ToolTab, Workspace,
    char_at, is_word_char, lsp_wanted, merge_edits, position_of, text, word_start,
};
use crate::core::lsp_command::LspEffect;

impl Workspace {
    pub(super) fn ask_for_grammars(&mut self, fx: &mut Vec<Effect>) {
        for language in crate::lang::syntax::take_wanted() {
            fx.push(Effect::LoadGrammar { language });
        }
    }

    pub(super) fn grammar_loaded(
        &mut self,
        language: crate::lang::languages::Language,
        error: Option<String>,
    ) {
        if let Some(error) = error {
            return self.info(format!("No colours for {}: {error}", language.name()));
        }
        for d in self.documents.values_mut() {
            if d.language() == language && d.syntax().is_none() {
                d.grammar_arrived();
            }
        }
    }

    pub(super) fn sync_lsp(&mut self, fx: &mut Vec<Effect>) {
        if !self.trusted {
            return;
        }
        self.assign_servers(fx);
        for d in self.documents.values() {
            if let Some(server) = d
                .lsp_server()
                .filter(|s| !self.lsp.servers.contains_key(*s))
            {
                self.lsp
                    .servers
                    .insert(server.clone(), ServerStatus::default());
                fx.push(Effect::Lsp(LspEffect::Start {
                    server: server.clone(),
                }));
            }
        }
        for d in self.documents.values() {
            if let Some(items) = self.diagnostics.get_mut(d.path()) {
                for item in items.iter_mut().filter(|i| i.chars.is_none()) {
                    item.resolve(d.text());
                }
            }
        }
        for (server, status) in self
            .lsp
            .servers
            .iter_mut()
            .filter(|(_, s)| s.state == LspState::Ready)
        {
            let documents = &self.documents;
            let gone: Vec<PathBuf> = status
                .open
                .iter()
                .filter(|p| {
                    !documents
                        .values()
                        .any(|d| d.path() == *p && d.lsp_server() == Some(server))
                })
                .cloned()
                .collect();
            for path in gone {
                status.open.remove(&path);
                fx.push(Effect::Lsp(LspEffect::DidClose {
                    server: server.clone(),
                    path,
                }));
            }
        }
        for d in self.documents.values_mut().filter(|d| lsp_wanted(d)) {
            if d.lsp_version() == Some(d.version()) {
                continue;
            }
            let Some(server) = d.lsp_server().cloned() else {
                continue;
            };
            let Some(status) = self
                .lsp
                .servers
                .get_mut(&server)
                .filter(|s| s.state == LspState::Ready)
            else {
                continue;
            };
            let (path, version) = (d.path().to_path_buf(), d.version());
            let text = d.text().to_string();
            fx.push(if status.open.insert(path.clone()) {
                Effect::Lsp(LspEffect::DidOpen {
                    server,
                    language: d.language(),
                    path,
                    version,
                    text,
                })
            } else {
                Effect::Lsp(LspEffect::DidChange {
                    server,
                    path,
                    version,
                    text,
                })
            });
            d.set_lsp_version(Some(version));
        }
    }

    pub(super) fn assign_servers(&mut self, fx: &mut Vec<Effect>) {
        let unassigned: Vec<DocumentId> = self
            .documents
            .iter()
            .filter(|(_, d)| {
                lsp_wanted(d)
                    && d.lsp_server().is_none()
                    && !self.lsp.root_asked(d.path())
                    && !self
                        .scratch_dir
                        .as_ref()
                        .is_some_and(|dir| d.path().starts_with(dir))
            })
            .map(|(id, _)| *id)
            .collect();
        for id in unassigned {
            let Some(d) = self.documents.get(&id) else {
                continue;
            };
            let Some(server) = d.language().server().map(|s| s.id) else {
                continue;
            };
            if d.path().starts_with(&self.root) {
                let path = d.path().to_path_buf();
                self.lsp.ask_root(path.clone());
                fx.push(Effect::Lsp(LspEffect::FindRoot {
                    path,
                    server,
                    project: self.root.clone(),
                }));
            } else {
                let key = self.session_for_outside_file(server);
                if let Some(d) = self.documents.get_mut(&id) {
                    d.set_lsp_server(Some(key));
                }
            }
        }
    }

    pub(super) fn session_for_outside_file(&self, server: Language) -> ServerKey {
        self.lsp.session_for_outside_file(server, &self.root)
    }

    pub(super) fn lsp_root_found(&mut self, path: &Path, server: ServerKey) {
        self.lsp.root_answered(path);
        for d in self.documents.values_mut().filter(|d| d.path() == path) {
            if d.language().server().map(|s| s.id) == Some(server.server) {
                d.set_lsp_server(Some(server.clone()));
            }
        }
    }

    pub(super) fn lsp_ready(
        &mut self,
        server: ServerKey,
        encoding: PositionEncoding,
        completion_triggers: Vec<String>,
    ) {
        self.drop_query_of(&server);
        self.diagnostics
            .retain(|_, items| items.first().is_none_or(|item| item.server != server));
        for d in self.documents.values_mut() {
            if d.lsp_server() == Some(&server) {
                d.set_lsp_version(None);
            }
        }
        self.lsp.ready(server, encoding, completion_triggers);
    }

    pub(super) fn lsp_state_changed(&mut self, server: &ServerKey, state: LspState) {
        self.lsp.state_changed(server, state);
    }

    pub(super) fn drop_query_of(&mut self, server: &ServerKey) {
        self.lsp.drop_query_of(server);
    }

    pub(super) fn lsp_query(
        &mut self,
        query: impl FnOnce((u32, u32)) -> LspQuery,
        fx: &mut Vec<Effect>,
    ) {
        self.lsp_ask(
            |text, cursor, encoding| query(position_of(text, cursor.idx, encoding)),
            fx,
        );
    }

    pub(super) fn lsp_ask(
        &mut self,
        query: impl FnOnce(&ropey::Rope, Cursor, PositionEncoding) -> LspQuery,
        fx: &mut Vec<Effect>,
    ) {
        if self.focus != Focus::Editor {
            return;
        }
        if !self.trusted {
            return self.info(
                "No language server: this project is not trusted (`Trust This Project`)".to_owned(),
            );
        }
        let Some(view_id) = self.active_view() else {
            return;
        };
        let Some((view, d)) = self.active() else {
            return;
        };
        if !lsp_wanted(d) || self.lsp_state() == LspState::Disabled {
            return self.info("No language server for this file".to_owned());
        }
        let session = d
            .lsp_server()
            .and_then(|key| self.lsp.servers.get_key_value(key))
            .filter(|(_, s)| s.state == LspState::Ready && s.open.contains(d.path()));
        let Some((server, status)) = session else {
            return self.info("Language server is not ready".to_owned());
        };
        let server = server.clone();
        let cursor = view.cursor;
        let (path, version) = (d.path().to_path_buf(), d.version());
        let query = query(d.text(), cursor, status.encoding);
        if let Some(old) = self.lsp.pending.take() {
            fx.push(Effect::Lsp(LspEffect::Cancel {
                server: old.server,
                id: old.id,
            }));
        }
        let id = self.lsp.next_id();
        self.lsp.asked_by(&server);
        self.lsp.pending = Some(PendingQuery {
            server: server.clone(),
            id,
            view: view_id,
            version,
            cursor,
            completion: matches!(query, LspQuery::Completion { .. }),
            quiet: false,
        });
        fx.push(Effect::Lsp(LspEffect::Query {
            server,
            id,
            path,
            query,
        }));
    }

    pub(super) fn lsp_answered(
        &mut self,
        server: &ServerKey,
        id: u64,
        result: Result<LspAnswer, String>,
        fx: &mut Vec<Effect>,
    ) {
        if self
            .pending_accept
            .as_ref()
            .is_some_and(|p| p.id == id && p.server == *server)
        {
            return self.completion_resolved(server, result);
        }
        let Some(pending) = self
            .lsp
            .pending
            .take_if(|p| p.id == id && p.server == *server)
        else {
            return;
        };
        let unchanged = self.active().is_some_and(|(view, d)| {
            view.cursor == pending.cursor && d.version() == pending.version
        });
        let undisturbed = self.focus == Focus::Editor
            && !self.is_asking()
            && self.local_history.is_none()
            && self.active_view() == Some(pending.view);
        if !unchanged || !undisturbed {
            return;
        }
        if let Some(status) = self.lsp.servers.get(server) {
            self.answer_encoding = status.encoding;
        }
        match result {
            Err(_) if pending.quiet => {}
            Err(message) => self.error(format!("Language server: {message}")),
            Ok(LspAnswer::Locations(mut locations)) => match locations.len() {
                0 => self.info("No definition found".to_owned()),
                1 => {
                    if let Some(location) = locations.pop() {
                        self.jump_to(location, fx);
                    }
                }
                _ => {
                    self.locations = locations;
                    self.open_picker(PickerKind::Locations);
                }
            },
            Ok(LspAnswer::Hover(markdown)) => self.show_hover(markdown.as_deref()),
            Ok(LspAnswer::References(usages)) => self.show_usages(usages),
            Ok(LspAnswer::Edits(edits)) => self.apply_formatting(&edits),
            Ok(LspAnswer::CodeActions(actions)) if actions.is_empty() => {
                self.info("No actions available here".to_owned());
            }
            Ok(LspAnswer::CodeActions(actions)) => {
                self.code_actions = actions;
                self.code_actions_stamp = self.edit_stamp();
                self.open_picker(PickerKind::CodeActions);
            }
            Ok(LspAnswer::Completion { items, incomplete }) => {
                self.show_completion(items, incomplete, pending.quiet);
            }
            Ok(LspAnswer::Rename { changes, places }) => self.rename_preview(changes, places),
            Ok(LspAnswer::Refactorings(offers)) => self.refactor_show(offers),
            Ok(LspAnswer::SafeDeleteUsages(usages)) => self.safe_delete_answered(usages),
            Ok(LspAnswer::Symbols(symbols)) => self.structure_show(symbols),
            Ok(LspAnswer::CallRoot { root, outgoing }) => self.calls_root(root, outgoing, fx),
            Ok(LspAnswer::Calls(calls)) => self.calls_show(calls),
        }
    }

    pub(super) fn word_at_cursor(&self) -> String {
        self.active().map_or_else(String::new, |(view, d)| {
            let t = d.text();
            let start = word_start(t, view.cursor.idx);
            let word = t.chars_at(start).take_while(|c| is_word_char(*c));
            word.collect()
        })
    }

    pub(super) fn rename_ask(&mut self) {
        if self.focus != Focus::Editor || self.active().is_none() {
            return;
        }
        let old = self.word_at_cursor();
        if old.is_empty() {
            return self.info("Put the cursor on a name to rename it".to_owned());
        }
        self.file_prompt_open(
            crate::core::file_op::FilePromptKind::RenameSymbol { old: old.clone() },
            &old,
        );
    }

    pub(super) fn rename_named(&mut self, old: &str, new_name: String, fx: &mut Vec<Effect>) {
        let refusal = match new_name.as_str() {
            "" => Some("Type a name"),
            name if name == old => Some("That is its name already"),
            _ => None,
        };
        if let Some(prompt) = self.file_prompt.as_mut() {
            prompt.error = refusal.map(str::to_owned);
        }
        if refusal.is_some() {
            return;
        }
        self.file_prompt = None;
        self.lsp_query(|at| LspQuery::Rename { at, new_name }, fx);
    }

    pub(super) fn rename_preview(
        &mut self,
        changes: Option<Vec<LspFileEdit>>,
        places: Vec<LspUsage>,
    ) {
        let Some(changes) = changes.filter(|c| !c.is_empty()) else {
            return self.info("Cannot rename this here".to_owned());
        };
        let summary = format!(
            "{} places in {} files, listed in Find. Apply?",
            places.len(),
            changes.len()
        );
        self.pending_rename = changes;
        self.code_actions_stamp = self.edit_stamp();
        self.show_usages(places);
        self.file_prompt_open(
            crate::core::file_op::FilePromptKind::RenameConfirm { summary },
            "",
        );
    }

    pub(super) fn rename_confirmed(&mut self, fx: &mut Vec<Effect>) {
        self.focus = Focus::Editor;
        let changes = std::mem::take(&mut self.pending_rename);
        self.apply_as_one(changes, fx);
    }

    pub(super) fn apply_as_one(&mut self, changes: Vec<LspFileEdit>, fx: &mut Vec<Effect>) {
        if changes.len() > 1 {
            self.rename_undo = Some(RenameUndo {
                documents: Vec::new(),
                applying: true,
            });
        }
        self.run_code_action(
            LspCodeAction {
                title: String::new(),
                changes,
            },
            fx,
        );
        self.rename_applying_ends();
    }

    fn rename_applying_ends(&mut self) {
        if self.pending_edits.is_empty()
            && let Some(rename) = self.rename_undo.as_mut()
        {
            rename.applying = false;
        }
    }

    fn apply_file_edit(&mut self, doc: DocumentId, edits: &[LspTextEdit]) {
        match self.apply_edits(doc, edits) {
            Ok(changed) => {
                let revision = self.documents.get(&doc).map(Document::revision);
                if let (true, Some(revision), Some(rename)) =
                    (changed, revision, self.rename_undo.as_mut())
                    && rename.applying
                {
                    rename.documents.push((doc, revision));
                }
            }
            Err(message) => self.error(format!("Language server: {message}")),
        }
    }

    pub(super) fn undo_rename(&mut self) -> bool {
        let Some((view, doc)) = self
            .active_view()
            .and_then(|v| Some((v, self.views.get(&v)?.doc)))
        else {
            return false;
        };
        let Some(rename) = self.rename_undo.as_ref() else {
            return false;
        };
        let at_rename = rename.documents.iter().any(|(id, revision)| {
            *id == doc && self.documents.get(id).map(Document::revision) == Some(*revision)
        });
        if !at_rename {
            return false;
        }
        if rename.applying {
            self.info("The refactoring is still being applied".to_owned());
            return true;
        }
        let documents = rename.documents.clone();
        if documents
            .iter()
            .any(|(id, _)| !self.documents.contains_key(id))
        {
            self.rename_undo = None;
            self.error(
                "A file of this refactoring was closed: undo again to take it back here only"
                    .to_owned(),
            );
            return true;
        }
        let edited = documents.iter().find(|(id, revision)| {
            self.documents.get(id).map(Document::revision) != Some(*revision)
        });
        if let Some((id, _)) = edited {
            let name = self.doc_name(*id);
            self.error(format!(
                "Not undone: {name} was edited after the refactoring; undo that edit first"
            ));
            return true;
        }
        self.rename_undo = None;
        for (id, _) in &documents {
            let editing = (*id == doc).then_some(view);
            if let Some(applied) = self.documents.get_mut(id).and_then(Document::undo) {
                self.after_change(editing, *id, &applied);
            }
        }
        self.info(format!("Undone in {} files", documents.len()));
        true
    }

    pub(super) fn rename_declined(&mut self) {
        self.pending_rename.clear();
        self.focus = Focus::Editor;
    }

    pub(super) fn show_usages(&mut self, usages: Vec<LspUsage>) {
        if usages.is_empty() {
            return self.info("No usages found".to_owned());
        }
        let name = self.word_at_cursor();
        let mut results: Vec<SearchMatch> = usages.iter().map(|u| self.usage_row(u)).collect();
        results.sort_by(|a, b| (&a.path, a.line, a.col).cmp(&(&b.path, b.line, b.col)));
        let s = &mut self.search;
        s.results = results;
        s.truncated = false;
        s.running = None;
        s.selected = Some(0);
        s.usages_of = Some(name);
        s.usages_word = "";
        self.tool_window_visible = true;
        self.tool_tab = ToolTab::Find;
        self.focus = Focus::FindInFiles;
    }

    pub(super) fn usage_row(&self, usage: &LspUsage) -> SearchMatch {
        let location = &usage.location;
        let line = location.start.0 as usize;
        let open = self.documents.values().find(|d| d.path() == location.path);
        let (text, col) = match open {
            Some(d) => {
                let t = d.text();
                let at = char_at(t, location.start, self.answer_encoding);
                let on = t.char_to_line(at);
                (
                    text::line_content(t, on).to_string(),
                    at - t.line_to_char(on),
                )
            }
            None => {
                let t = ropey::Rope::from_str(&usage.disk_line);
                let at = char_at(&t, (0, location.start.1), self.answer_encoding);
                (usage.disk_line.clone(), at)
            }
        };
        let len = self
            .search
            .usages_of
            .as_deref()
            .unwrap_or_default()
            .chars()
            .count();
        SearchMatch {
            path: location.path.clone(),
            line,
            col,
            len,
            text,
        }
    }

    pub(super) fn reformat(&mut self, fx: &mut Vec<Effect>) {
        if self.focus != Focus::Editor {
            return;
        }
        let Some((view, d)) = self.active() else {
            return;
        };
        let can_format = !matches!(d.language(), Language::Python | Language::Bash);
        let ready = d
            .lsp_server()
            .and_then(|key| self.lsp.servers.get(key))
            .is_some_and(|s| s.state == LspState::Ready && s.open.contains(d.path()));
        if can_format && ready {
            let tab_size = self.tab_width;
            return self.lsp_query(|_| LspQuery::Formatting { tab_size }, fx);
        }
        fx.push(Effect::FormatWithTool {
            doc: view.doc,
            version: d.version(),
            path: d.path().to_path_buf(),
            root: self.root.clone(),
            text: d.text().to_string(),
        });
        self.info("Formatting…".to_owned());
    }

    pub(super) fn formatted(
        &mut self,
        doc: DocumentId,
        version: u64,
        result: Result<String, String>,
    ) {
        let new = match result {
            Ok(new) => new,
            Err(message) => return self.error(message),
        };
        let Some(d) = self.documents.get_mut(&doc) else {
            return;
        };
        if d.version() != version {
            return self.info("The text changed: format again".to_owned());
        }
        let old = d.text().to_string();
        if old == new {
            return self.info("Already formatted".to_owned());
        }
        self.replace_text_keeping_cursor(doc, &old, &new);
        self.info("Formatted".to_owned());
    }

    pub(super) fn apply_formatting(&mut self, edits: &[LspTextEdit]) {
        let Some(doc) = self.active().map(|(view, _)| view.doc) else {
            return;
        };
        match self.apply_edits(doc, edits) {
            Err(message) => self.error(format!("Language server: {message}")),
            Ok(false) => self.info("Already formatted".to_owned()),
            Ok(true) => self.info("Formatted".to_owned()),
        }
    }

    pub(super) fn apply_edits(
        &mut self,
        doc: DocumentId,
        edits: &[LspTextEdit],
    ) -> Result<bool, String> {
        let active = self
            .active_view()
            .filter(|v| self.views.get(v).is_some_and(|view| view.doc == doc));
        let cursor = active
            .and_then(|v| self.views.get(&v))
            .map_or(0, |v| v.cursor.idx);
        let Some(d) = self.documents.get_mut(&doc) else {
            return Ok(false);
        };
        let Some(merged) = merge_edits(d.text(), edits, self.answer_encoding, cursor)? else {
            return Ok(false);
        };
        d.close_undo_group();
        let applied = d.edit(merged.from, merged.to, &merged.text, false);
        d.close_undo_group();
        self.after_change(active, doc, &applied);
        if active.is_some() {
            self.set_active_cursor(Cursor::at(merged.cursor));
        }
        Ok(true)
    }

    pub(super) fn run_code_action(&mut self, action: LspCodeAction, fx: &mut Vec<Effect>) {
        if self.edit_stamp() != self.code_actions_stamp {
            return self.info("The text changed; ask for actions again".to_owned());
        }
        self.return_to = self.active_view();
        for change in action.changes {
            let open = self
                .documents
                .iter()
                .find(|(_, d)| d.path() == change.path)
                .map(|(id, _)| *id);
            match open {
                Some(doc) => self.apply_file_edit(doc, &change.edits),
                None => {
                    fx.push(Effect::ReadFile {
                        path: change.path.clone(),
                        at: None,
                    });
                    self.pending_edits.push(change);
                }
            }
        }
    }

    pub(super) fn take_pending_edits(&mut self, path: &Path) -> Option<LspFileEdit> {
        let i = self.pending_edits.iter().position(|e| e.path == path)?;
        Some(self.pending_edits.remove(i))
    }

    pub(super) fn finish_pending_edits(
        &mut self,
        path: &Path,
        waiting: Option<LspFileEdit>,
        failed: bool,
    ) {
        let Some(change) = waiting else {
            return;
        };
        let doc = self
            .documents
            .iter()
            .find(|(_, d)| d.path() == path)
            .map(|(id, _)| *id);
        if let Some(doc) = doc.filter(|_| !failed) {
            self.apply_file_edit(doc, &change.edits);
        } else {
            let name = self.display_path(path).display().to_string();
            self.error(format!("Not applied to {name}: the file could not be read"));
        }
        self.rename_applying_ends();
        if self.pending_edits.is_empty()
            && let Some(view) = self.return_to.take()
        {
            self.focus_view(view, None);
        }
    }
}
