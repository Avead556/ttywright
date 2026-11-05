use std::path::{Path, PathBuf};

use super::{Focus, Workspace};
use crate::core::action::Action;
use crate::core::command::Effect;
use crate::core::lsp::{LspState, PositionEncoding};
use crate::core::picker::{Picker, PickerKind};
use crate::core::session::SavedBookmark;
use crate::core::{LspLocation, LspQuery, LspSymbol, LspUsage, ProjectSymbol};
use crate::lang::symbols::MAX_SYMBOL_BYTES;
use ropey::Rope;

const SYMBOL_ROWS: usize = 200;
const EVERYWHERE_SYMBOL_ROWS: usize = 30;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Bookmark {
    pub path: PathBuf,
    pub line: usize,
    pub(super) at: Option<usize>,
}

fn line_start(text: &Rope, line: usize) -> usize {
    text.line_to_char(line.min(text.len_lines().saturating_sub(1)))
}

impl Workspace {
    pub fn bookmarks(&self) -> &[Bookmark] {
        &self.bookmarks
    }

    pub fn is_bookmarked(&self, path: &Path, line: usize) -> bool {
        self.bookmarks
            .iter()
            .any(|b| b.line == line && b.path == path)
    }

    pub(super) fn bookmark_toggle(&mut self) {
        let Some((view, d)) = self.active() else {
            return self.info("Open a file first".to_owned());
        };
        let text = d.text();
        let line = text.char_to_line(view.cursor.idx);
        let path = d.path().to_path_buf();
        let at = Some(text.line_to_char(line));
        if let Some(index) = self
            .bookmarks
            .iter()
            .position(|b| b.line == line && b.path == path)
        {
            self.bookmarks.remove(index);
            return self.info("Bookmark removed".to_owned());
        }
        self.bookmarks.push(Bookmark { path, line, at });
        self.bookmarks
            .sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
        self.info("Bookmark set".to_owned());
    }

    pub(super) fn bookmark_label(&self, bookmark: &Bookmark) -> String {
        let place = format!(
            "{}:{}",
            self.display_path(&bookmark.path).display(),
            bookmark.line + 1
        );
        let open = self.documents.values().find(|d| d.path() == bookmark.path);
        match open {
            Some(d) if bookmark.line < d.text().len_lines() => {
                let text = d.text().line(bookmark.line).to_string();
                format!("{place}  {}", text.trim())
            }
            _ => place,
        }
    }

    pub(super) fn bookmark_jump(&mut self, index: usize, fx: &mut Vec<Effect>) {
        if let Some(bookmark) = self.bookmarks.get(index) {
            let location = LspLocation {
                path: bookmark.path.clone(),
                start: (u32::try_from(bookmark.line).unwrap_or(u32::MAX), 0),
            };
            self.jump_to(location, fx);
        }
    }

    pub(super) fn bookmarks_changed(
        &mut self,
        path: &Path,
        text: &Rope,
        changes: &[crate::core::ChangeSet],
    ) {
        for bookmark in self.bookmarks.iter_mut().filter(|b| b.path == path) {
            let mut at = bookmark
                .at
                .unwrap_or_else(|| line_start(text, bookmark.line));
            for change in changes {
                at = change.map_pos(at, crate::core::Assoc::After);
            }
            let at = at.min(text.len_chars());
            bookmark.line = text.char_to_line(at);
            bookmark.at = Some(at);
        }
    }

    pub(super) fn bookmarks_sync(&mut self) {
        for bookmark in &mut self.bookmarks {
            let open = self.documents.values().find(|d| d.path() == bookmark.path);
            match (open, bookmark.at) {
                (Some(d), None) => bookmark.at = Some(line_start(d.text(), bookmark.line)),
                (None, Some(_)) => bookmark.at = None,
                _ => {}
            }
        }
    }

    pub(super) fn saved_bookmarks(&self) -> Vec<SavedBookmark> {
        self.bookmarks
            .iter()
            .map(|b| SavedBookmark {
                path: b.path.clone(),
                line: b.line,
            })
            .collect()
    }

    pub(super) fn restore_bookmarks(&mut self, saved: Vec<SavedBookmark>) {
        self.bookmarks = saved
            .into_iter()
            .map(|b| Bookmark {
                path: b.path,
                line: b.line,
                at: None,
            })
            .collect();
    }

    pub(super) fn structure_ask(&mut self, fx: &mut Vec<Effect>) {
        if self.focus != Focus::Editor {
            return;
        }
        let Some((_, d)) = self.active() else {
            return;
        };
        let served = d
            .lsp_server()
            .and_then(|key| self.lsp.servers.get(key))
            .is_some_and(|s| s.state == LspState::Ready && s.open.contains(d.path()));
        if served {
            return self.lsp_query(|_| LspQuery::Symbols, fx);
        }
        if d.text().len_bytes() > MAX_SYMBOL_BYTES {
            return self.info("This file is too big to list without a language server".to_owned());
        }
        fx.push(Effect::FileSymbols {
            path: d.path().to_path_buf(),
            version: d.version(),
            language: d.language(),
            text: d.text().to_string(),
        });
    }

    pub(super) fn file_symbols_read(&mut self, path: &Path, version: u64, symbols: Vec<LspSymbol>) {
        let current = self
            .active()
            .is_some_and(|(_, d)| d.path() == path && d.version() == version);
        if !current || self.focus != Focus::Editor || self.picker.is_some() {
            return;
        }
        if symbols.is_empty() {
            return self.info("Nothing to list in this file".to_owned());
        }
        self.answer_encoding = PositionEncoding::Utf8;
        self.structure_show(symbols);
    }

    pub fn index_progress(&self) -> Option<(usize, usize)> {
        self.index_progress
    }

    pub(super) fn project_symbol_rows(&self) -> Vec<String> {
        self.project_symbols
            .iter()
            .map(|s| {
                format!(
                    "{}  {}  {}:{}",
                    s.name,
                    s.kind,
                    self.display_path(&s.path).display(),
                    s.start.0 + 1
                )
            })
            .collect()
    }

    pub fn everywhere_symbol(&self, index: usize, len: usize) -> Option<&ProjectSymbol> {
        let first = len.checked_sub(Action::ALL.len() + self.project_symbols.len())?;
        self.project_symbols.get(index.checked_sub(first)?)
    }

    pub(super) fn symbols_typed(&mut self, picker: &mut Picker, fx: &mut Vec<Effect>) {
        let limit = match picker.kind {
            PickerKind::ProjectSymbols => SYMBOL_ROWS,
            PickerKind::Everywhere => EVERYWHERE_SYMBOL_ROWS,
            _ => return,
        };
        let query = picker.input.text().trim().to_owned();
        if query.is_empty() {
            self.symbols_asked = None;
            self.project_symbols.clear();
            picker.refilter(&self.picker_items(picker.kind));
            return;
        }
        let id = u64::from(self.alloc_id());
        self.symbols_asked = Some(id);
        fx.push(Effect::QuerySymbols { id, query, limit });
    }

    pub(super) fn symbols_found(&mut self, id: u64, symbols: Vec<ProjectSymbol>) {
        if self.symbols_asked != Some(id) {
            return;
        }
        self.symbols_asked = None;
        self.project_symbols = symbols;
        let Some(mut picker) = self.picker.take() else {
            return;
        };
        let selected = picker.selected;
        picker.refilter(&self.picker_items(picker.kind));
        picker.selected = selected.min(picker.matches.len().saturating_sub(1));
        self.picker = Some(picker);
    }

    pub(super) fn project_symbol_jump(&mut self, symbol: ProjectSymbol, fx: &mut Vec<Effect>) {
        self.answer_encoding = PositionEncoding::Utf8;
        let location = LspLocation {
            path: symbol.path,
            start: symbol.start,
        };
        self.jump_to(location, fx);
    }

    pub(super) fn structure_show(&mut self, symbols: Vec<LspSymbol>) {
        if symbols.is_empty() {
            return self.info("The server sees nothing to list in this file".to_owned());
        }
        let cursor_line = self
            .active()
            .map_or(0, |(view, d)| d.text().char_to_line(view.cursor.idx));
        let around = symbols
            .iter()
            .rposition(|s| s.start.0 as usize <= cursor_line)
            .unwrap_or(0);
        self.symbols = symbols;
        self.symbols_path = self.active().map(|(_, d)| d.path().to_path_buf());
        self.open_picker(PickerKind::Symbols);
        if let Some(picker) = self.picker.as_mut() {
            picker.selected = around;
        }
    }

    pub(super) fn symbol_jump(&mut self, index: usize, fx: &mut Vec<Effect>) {
        if let (Some(symbol), Some(path)) = (self.symbols.get(index), self.symbols_path.clone()) {
            let location = LspLocation {
                path,
                start: symbol.start,
            };
            self.jump_to(location, fx);
        }
    }

    pub(super) fn calls_ask(&mut self, outgoing: bool, fx: &mut Vec<Effect>) {
        self.lsp_query(|at| LspQuery::PrepareCalls { at, outgoing }, fx);
    }

    pub(super) fn calls_root(
        &mut self,
        root: Option<(String, String)>,
        outgoing: bool,
        fx: &mut Vec<Effect>,
    ) {
        let Some((name, item)) = root else {
            return self.info("No function here for the server to follow".to_owned());
        };
        self.calls_of = Some((name, outgoing));
        self.lsp_query(|_| LspQuery::Calls { item, outgoing }, fx);
    }

    pub(super) fn calls_show(&mut self, calls: Vec<LspUsage>) {
        let Some((name, outgoing)) = self.calls_of.take() else {
            return;
        };
        let word = if outgoing { "callee" } else { "caller" };
        if calls.is_empty() {
            return self.info(format!("No {word}s of {name} found"));
        }
        self.show_usages(calls);
        self.search.usages_of = Some(name);
        self.search.usages_word = word;
    }
}
