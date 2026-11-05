use super::{
    COMPLETION_PAGE, Command, Completion, Cursor, Effect, Focus, LspAnswer, LspCompletion,
    LspQuery, LspState, LspTextEdit, Motion, PendingAccept, ServerKey, Workspace, char_at,
    is_word_char, merge_edits, word_start,
};
use crate::core::lsp_command::LspEffect;

impl Workspace {
    pub(super) fn show_completion(
        &mut self,
        items: Vec<LspCompletion>,
        incomplete: bool,
        quiet: bool,
    ) {
        let Some((view_id, (view, d))) = self.active_view().zip(self.active()) else {
            return;
        };
        let anchor = items
            .iter()
            .find_map(|i| i.start)
            .map_or_else(
                || word_start(d.text(), view.cursor.idx),
                |start| char_at(d.text(), start, self.answer_encoding),
            )
            .min(view.cursor.idx);
        let version = d.version();
        self.completion = Some(Completion::new(items, incomplete, anchor, view_id, version));
        self.refilter_completion();
        if self.completion.is_none() && !quiet {
            self.info("No completions".to_owned());
        }
    }

    pub(super) fn refilter_completion(&mut self) {
        let Some((view, d)) = self.active() else {
            return;
        };
        let (cursor, version) = (view.cursor.idx, d.version());
        let typed = match &self.completion {
            Some(c) if c.anchor <= cursor => d.text().slice(c.anchor..cursor).to_string(),
            _ => {
                self.completion = None;
                return;
            }
        };
        if let Some(c) = self.completion.as_mut() {
            c.version = version;
            c.refilter(&typed);
            if c.matches.is_empty() {
                self.completion = None;
            }
        }
    }

    pub(super) fn completion_takes(&mut self, command: &Command) -> bool {
        if self.completion.is_none() {
            return false;
        }
        match command {
            Command::Move(
                Motion::Up | Motion::Down | Motion::PageUp { .. } | Motion::PageDown { .. },
            )
            | Command::Newline
            | Command::Escape => true,
            Command::Indent => true,
            Command::Insert(_) | Command::DeleteBackward => false,
            _ => {
                self.completion = None;
                false
            }
        }
    }

    pub(super) fn completion_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(c) = self.completion.as_mut() else {
            return;
        };
        let page = |rows: usize| isize::try_from(rows.min(COMPLETION_PAGE)).unwrap_or(1);
        match input {
            Command::Move(Motion::Down) => c.select_by(1),
            Command::Move(Motion::Up) => c.select_by(-1),
            Command::Move(Motion::PageDown { rows }) => c.select_by(page(rows)),
            Command::Move(Motion::PageUp { rows }) => c.select_by(-page(rows)),
            Command::Escape => self.completion = None,
            _ => self.accept_completion(fx),
        }
    }

    pub(super) fn accept_completion(&mut self, fx: &mut Vec<Effect>) {
        let Some(c) = self.completion.take() else {
            return;
        };
        let Some(item) = c.selected_item().cloned() else {
            return;
        };
        let Some((view_id, (view, d))) = self.active_view().zip(self.active()) else {
            return;
        };
        let to = view.cursor.idx;
        let from = item
            .start
            .map_or(c.anchor, |start| {
                char_at(d.text(), start, self.answer_encoding)
            })
            .min(to);
        let version = d.version();
        let server = d.lsp_server().cloned();
        let path = d.path().to_path_buf();
        match (item.resolve, server) {
            (Some(raw), Some(server)) if item.additional.is_empty() => {
                let id = self.lsp.next_id();
                fx.push(Effect::Lsp(LspEffect::Query {
                    server: server.clone(),
                    id,
                    path,
                    query: LspQuery::ResolveCompletion { item: raw },
                }));
                self.pending_accept = Some(PendingAccept {
                    server,
                    id,
                    view: view_id,
                    version,
                    from,
                    to,
                    insert: item.insert,
                });
            }
            _ => self.completion_insert(from, to, &item.insert, &item.additional),
        }
    }

    pub(super) fn completion_stop_waiting(&mut self) {
        if let Some(p) = self.pending_accept.take()
            && self.completion_still_fits(&p)
        {
            self.completion_insert(p.from, p.to, &p.insert, &[]);
        }
    }

    pub(super) fn completion_still_fits(&self, p: &PendingAccept) -> bool {
        self.active_view() == Some(p.view)
            && self.active().is_some_and(|(_, d)| d.version() == p.version)
    }

    pub(super) fn completion_resolved(
        &mut self,
        server: &ServerKey,
        result: Result<LspAnswer, String>,
    ) {
        let Some(p) = self.pending_accept.take() else {
            return;
        };
        if !self.completion_still_fits(&p) {
            return;
        }
        if let Some(status) = self.lsp.servers.get(server) {
            self.answer_encoding = status.encoding;
        }
        let additional = match result {
            Ok(LspAnswer::Edits(edits)) => edits,
            _ => Vec::new(),
        };
        self.completion_insert(p.from, p.to, &p.insert, &additional);
    }

    pub(super) fn completion_insert(
        &mut self,
        from: usize,
        to: usize,
        insert: &str,
        additional: &[LspTextEdit],
    ) {
        let Some((_, d)) = self.active() else {
            return;
        };
        let text = d.text();
        let merged = merge_edits(text, additional, self.answer_encoding, to)
            .ok()
            .flatten();
        let between = |a: usize, b: usize| text.slice(a..b).to_string();
        let (start, end, joined, cursor) = match merged {
            Some(m) if m.to <= from => {
                let joined = format!("{}{}{insert}", m.text, between(m.to, from));
                let end_of_word = m.from + joined.chars().count();
                (m.from, to, joined, end_of_word)
            }
            Some(m) if m.from >= to => {
                let joined = format!("{insert}{}{}", between(to, m.from), m.text);
                (from, m.to, joined, from + insert.chars().count())
            }
            _ => (from, to, insert.to_owned(), from + insert.chars().count()),
        };
        self.close_undo_group();
        self.edit(start, end, &joined, false);
        self.close_undo_group();
        self.set_active_cursor(Cursor::at(cursor));
    }

    pub(super) fn completion_after_typing(&mut self, typed: &Command, fx: &mut Vec<Effect>) {
        if self.focus != Focus::Editor || self.picker.is_some() || self.local_history.is_some() {
            return;
        }
        let one_char = match typed {
            Command::Insert(s) => {
                let mut chars = s.chars();
                chars.next().filter(|_| chars.next().is_none())
            }
            _ => None,
        };
        self.forget_dismissal_of_finished_word();
        let waiting = self.lsp.pending.as_ref().is_some_and(|p| p.completion);
        match (typed, one_char) {
            (Command::DeleteBackward, _) => {
                self.refilter_completion();
                if waiting {
                    self.ask_completion_quietly(fx);
                }
            }
            (_, Some(c)) if is_word_char(c) => {
                let incomplete = self.completion.as_ref().is_some_and(|p| p.incomplete);
                self.refilter_completion();
                let closed = self.completion.is_none();
                if (incomplete || waiting || closed) && self.word_wants_completion() {
                    self.ask_completion_quietly(fx);
                }
            }
            (_, Some(c)) => {
                self.completion = None;
                let triggers = self.active_server().map(|(_, s)| &s.completion_triggers);
                if triggers.is_some_and(|t| t.iter().any(|t| t.starts_with(c))) {
                    self.ask_completion_quietly(fx);
                }
            }
            _ => self.completion = None,
        }
    }

    pub(super) fn forget_dismissal_of_finished_word(&mut self) {
        let Some((view_id, (view, d))) = self.active_view().zip(self.active()) else {
            return;
        };
        let start = word_start(d.text(), view.cursor.idx);
        let inside = view.cursor.idx > start;
        if self.completion_dismissed != Some((view_id, start)) || !inside {
            self.completion_dismissed = None;
        }
    }

    pub(super) fn word_wants_completion(&self) -> bool {
        let Some((view_id, (view, d))) = self.active_view().zip(self.active()) else {
            return false;
        };
        let start = word_start(d.text(), view.cursor.idx);
        let is_number = d.text().get_char(start).is_some_and(|c| c.is_ascii_digit());
        !is_number && self.completion_dismissed != Some((view_id, start))
    }

    pub(super) fn dismiss_completion(&mut self, fx: &mut Vec<Effect>) {
        let waiting = self.lsp.pending.take_if(|p| p.completion);
        if self.completion.is_none() && waiting.is_none() {
            return;
        }
        if let Some(pending) = waiting {
            fx.push(Effect::Lsp(LspEffect::Cancel {
                server: pending.server,
                id: pending.id,
            }));
        }
        if let Some((view_id, (view, d))) = self.active_view().zip(self.active()) {
            let start = word_start(d.text(), view.cursor.idx);
            self.completion_dismissed = Some((view_id, start));
        }
    }

    pub(super) fn ask_completion_quietly(&mut self, fx: &mut Vec<Effect>) {
        let ready = self.active().is_some_and(|(_, d)| {
            self.active_server()
                .is_some_and(|(_, s)| s.state == LspState::Ready && s.open.contains(d.path()))
        });
        if !ready {
            return;
        }
        self.lsp_query(|at| LspQuery::Completion { at }, fx);
        if let Some(pending) = self.lsp.pending.as_mut() {
            pending.quiet = true;
        }
    }

    pub(super) fn drop_stale_completion(&mut self) {
        let current = self.completion.as_ref().is_some_and(|c| {
            self.active_view() == Some(c.view)
                && self.active().is_some_and(|(_, d)| d.version() == c.version)
        });
        if !current {
            self.completion = None;
        }
    }
}
