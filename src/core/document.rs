use std::path::{Path, PathBuf};
use std::time::SystemTime;

use ropey::Rope;
use serde::{Deserialize, Serialize};

use super::change::{ChangeSet, Operation};
use super::history::History;
use super::lsp::ServerKey;
pub use crate::lang::languages::Language;
use crate::lang::syntax::{self, Syntax};

const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";
const MAX_SHEBANG_CHARS: usize = 256;

#[derive(Debug, thiserror::Error)]
pub enum DocumentError {
    #[error("{path} is not valid UTF-8")]
    NotUtf8 { path: PathBuf },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum LineEnding {
    Lf,
    Crlf,
}

impl LineEnding {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Lf => "\n",
            Self::Crlf => "\r\n",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Self::Lf => "LF",
            Self::Crlf => "CRLF",
        }
    }

    fn detect(text: &str) -> Self {
        match text.find('\n') {
            Some(i) if text[..i].ends_with('\r') => Self::Crlf,
            _ => Self::Lf,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiskState {
    pub modified: Option<SystemTime>,
    pub len: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Conflict {
    Modified,
    Deleted,
}

#[derive(Debug)]
pub struct Document {
    path: PathBuf,
    text: Rope,
    version: u64,
    saved_version: u64,
    line_ending: LineEnding,
    has_bom: bool,
    history: History,
    language: Language,
    syntax: Option<Syntax>,
    disk: Option<DiskState>,
    conflict: Option<Conflict>,
    write_failure_shown: bool,
    swapped_version: Option<u64>,
    lsp_server: Option<ServerKey>,
    lsp_version: Option<u64>,
    journal_base: Option<Vec<u8>>,
    journal_pending: Vec<ChangeSet>,
    journaling: bool,
    image: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Applied {
    pub changes: Vec<ChangeSet>,
    pub cursor: usize,
}

impl Document {
    pub fn new(path: PathBuf) -> Self {
        Self {
            path,
            text: Rope::new(),
            version: 0,
            saved_version: 0,
            line_ending: LineEnding::Lf,
            has_bom: false,
            history: History::default(),
            language: Language::PlainText,
            syntax: None,
            disk: None,
            conflict: None,
            write_failure_shown: false,
            swapped_version: None,
            lsp_server: None,
            lsp_version: None,
            journal_base: None,
            journal_pending: Vec::new(),
            journaling: false,
            image: false,
        }
        .with_syntax()
    }

    pub fn from_bytes(path: PathBuf, bytes: &[u8]) -> Result<Self, DocumentError> {
        if super::image_art::is_image(&path, bytes) {
            let mut picture = Self::new(path);
            picture.image = true;
            return Ok(picture);
        }
        let (has_bom, body) = match bytes.strip_prefix(UTF8_BOM) {
            Some(rest) => (true, rest),
            None => (false, bytes),
        };
        let Ok(text) = std::str::from_utf8(body) else {
            return Err(DocumentError::NotUtf8 { path });
        };
        Ok(Self {
            language: Language::PlainText,
            syntax: None,
            line_ending: LineEnding::detect(text),
            text: Rope::from_str(text),
            path,
            version: 0,
            saved_version: 0,
            has_bom,
            history: History::default(),
            disk: None,
            conflict: None,
            write_failure_shown: false,
            swapped_version: None,
            lsp_server: None,
            lsp_version: None,
            journal_base: None,
            journal_pending: Vec::new(),
            journaling: false,
            image: false,
        }
        .with_syntax())
    }

    pub fn to_bytes(&self) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(self.text.len_bytes() + UTF8_BOM.len());
        if self.has_bom {
            bytes.extend_from_slice(UTF8_BOM);
        }
        for chunk in self.text.chunks() {
            bytes.extend_from_slice(chunk.as_bytes());
        }
        bytes
    }

    pub fn is_image(&self) -> bool {
        self.image
    }

    pub fn path(&self) -> &Path {
        &self.path
    }

    pub fn text(&self) -> &Rope {
        &self.text
    }

    pub fn version(&self) -> u64 {
        self.version
    }

    pub fn line_ending(&self) -> LineEnding {
        self.line_ending
    }

    pub fn is_dirty(&self) -> bool {
        self.version != self.saved_version
    }

    pub fn file_name(&self) -> String {
        self.path.file_name().map_or_else(
            || self.path.display().to_string(),
            |n| n.to_string_lossy().into_owned(),
        )
    }

    pub fn language(&self) -> Language {
        self.language
    }

    pub fn edit(&mut self, from: usize, to: usize, text: &str, mergeable: bool) -> Applied {
        if self.image {
            return Applied {
                changes: Vec::new(),
                cursor: from,
            };
        }
        let normalized = normalize_newlines(text, self.line_ending);
        let cs = ChangeSet::replace(self.text.len_chars(), from, to, &normalized);
        let inverse = cs.invert(&self.text);
        self.apply(&cs);
        let cursor = from + normalized.chars().count();
        self.history
            .record(cs.clone(), inverse, (from, cursor), mergeable);
        Applied {
            changes: vec![cs],
            cursor,
        }
    }

    pub fn undo(&mut self) -> Option<Applied> {
        let travel = self.history.undo()?;
        Some(self.travel(travel.changes, travel.cursor))
    }

    pub fn redo(&mut self) -> Option<Applied> {
        let travel = self.history.redo()?;
        Some(self.travel(travel.changes, travel.cursor))
    }

    pub fn revision(&self) -> usize {
        self.history.revision()
    }

    pub fn close_undo_group(&mut self) {
        self.history.close_group();
    }

    fn travel(&mut self, changes: Vec<ChangeSet>, cursor: usize) -> Applied {
        for cs in &changes {
            self.apply(cs);
        }
        Applied { changes, cursor }
    }

    fn apply(&mut self, cs: &ChangeSet) {
        if !self.journaling {
            self.journal_base = Some(self.to_bytes());
            self.journaling = true;
        }
        self.journal_pending.push(cs.clone());
        self.apply_to_text(cs);
        self.version += 1;
        let language = self.detect_language();
        if language != self.language {
            self.language = language;
            self.syntax = Syntax::new(language, &self.text);
            self.lsp_server = None;
            self.lsp_version = None;
        }
    }

    fn apply_to_text(&mut self, cs: &ChangeSet) {
        let Some(tree) = self.syntax.as_mut() else {
            cs.apply(&mut self.text);
            return;
        };
        let mut pos = 0;
        for op in cs.ops() {
            match op {
                Operation::Retain(n) => pos += n,
                Operation::Delete(n) => {
                    let pending = syntax::begin_edit(&self.text, pos, pos + n);
                    self.text.remove(pos..pos + n);
                    tree.edit(&syntax::finish_edit(pending, &self.text, pos));
                }
                Operation::Insert(s) => {
                    let pending = syntax::begin_edit(&self.text, pos, pos);
                    self.text.insert(pos, s);
                    pos += s.chars().count();
                    tree.edit(&syntax::finish_edit(pending, &self.text, pos));
                }
            }
        }
        tree.reparse(&self.text);
    }

    fn detect_language(&self) -> Language {
        let first_line: String = self
            .text
            .line(0)
            .chars()
            .take(MAX_SHEBANG_CHARS)
            .take_while(|c| *c != '\n')
            .collect();
        Language::detect(&self.path, &first_line)
    }

    fn with_syntax(mut self) -> Self {
        self.language = self.detect_language();
        self.syntax = Syntax::new(self.language, &self.text);
        self
    }

    pub fn grammar_arrived(&mut self) {
        self.syntax = Syntax::new(self.language, &self.text);
    }

    pub fn syntax(&self) -> Option<&Syntax> {
        self.syntax.as_ref()
    }

    pub fn mark_saved(&mut self, version: u64) {
        self.saved_version = version;
    }

    pub fn lsp_server(&self) -> Option<&ServerKey> {
        self.lsp_server.as_ref()
    }

    pub fn set_lsp_server(&mut self, server: Option<ServerKey>) {
        self.lsp_server = server;
    }

    pub fn lsp_version(&self) -> Option<u64> {
        self.lsp_version
    }

    pub fn set_lsp_version(&mut self, version: Option<u64>) {
        self.lsp_version = version;
    }

    pub fn needs_swap(&self) -> bool {
        self.is_dirty() && self.swapped_version != Some(self.version)
    }

    pub fn mark_swapped(&mut self) {
        self.swapped_version = Some(self.version);
    }

    pub fn take_journal(&mut self) -> Option<(Option<Vec<u8>>, Vec<ChangeSet>)> {
        if self.journal_pending.is_empty() {
            return None;
        }
        Some((
            self.journal_base.take(),
            std::mem::take(&mut self.journal_pending),
        ))
    }

    pub fn take_stale_journal(&mut self) -> bool {
        let stale = self.journaling && !self.is_dirty();
        if stale {
            self.journaling = false;
        }
        stale
    }

    pub fn take_stale_swap(&mut self) -> bool {
        !self.is_dirty() && self.swapped_version.take().is_some()
    }

    pub fn recover_unsaved(&mut self, contents: &[u8]) -> Result<Applied, DocumentError> {
        let applied = self.replace_unsaved(contents)?;
        self.mark_swapped();
        Ok(applied)
    }

    pub fn replace_unsaved(&mut self, contents: &[u8]) -> Result<Applied, DocumentError> {
        let (saved, disk, conflict) = (self.saved_version, self.disk, self.conflict);
        let applied = self.reload(contents, disk)?;
        self.saved_version = saved;
        self.conflict = conflict;
        Ok(applied)
    }

    pub fn disk(&self) -> Option<DiskState> {
        self.disk
    }

    pub fn set_disk(&mut self, disk: Option<DiskState>) {
        self.disk = disk;
    }

    pub fn conflict(&self) -> Option<Conflict> {
        self.conflict
    }

    pub fn set_conflict(&mut self, conflict: Option<Conflict>) {
        self.conflict = conflict;
    }

    pub fn note_write_failure(&mut self) -> bool {
        !std::mem::replace(&mut self.write_failure_shown, true)
    }

    pub fn clear_write_failure(&mut self) {
        self.write_failure_shown = false;
    }

    pub fn reload(
        &mut self,
        bytes: &[u8],
        disk: Option<DiskState>,
    ) -> Result<Applied, DocumentError> {
        let (has_bom, body) = match bytes.strip_prefix(UTF8_BOM) {
            Some(rest) => (true, rest),
            None => (false, bytes),
        };
        let Ok(new) = std::str::from_utf8(body) else {
            return Err(DocumentError::NotUtf8 {
                path: self.path.clone(),
            });
        };
        let old_len = self.text.len_chars();
        let new_len = new.chars().count();
        let from = self
            .text
            .chars()
            .zip(new.chars())
            .take_while(|(a, b)| a == b)
            .count();
        let common_end = self
            .text
            .chars_at(old_len)
            .reversed()
            .zip(new.chars().rev())
            .take_while(|(a, b)| a == b)
            .count()
            .min(old_len.min(new_len) - from);
        let middle: String = new
            .chars()
            .skip(from)
            .take(new_len - from - common_end)
            .collect();
        let to = old_len - common_end;
        let mut changes = Vec::new();
        if from != to || !middle.is_empty() {
            let cs = ChangeSet::replace(old_len, from, to, &middle);
            let inverse = cs.invert(&self.text);
            self.apply(&cs);
            let cursor = from + middle.chars().count();
            self.history.close_group();
            self.history
                .record(cs.clone(), inverse, (from, cursor), false);
            self.history.close_group();
            changes.push(cs);
        }
        self.has_bom = has_bom;
        self.line_ending = LineEnding::detect(new);
        self.saved_version = self.version;
        self.disk = disk;
        self.conflict = None;
        Ok(Applied {
            changes,
            cursor: from,
        })
    }
}

fn normalize_newlines(s: &str, ending: LineEnding) -> String {
    let lf = s.replace("\r\n", "\n").replace('\r', "\n");
    match ending {
        LineEnding::Lf => lf,
        LineEnding::Crlf => lf.replace('\n', "\r\n"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn doc(bytes: &[u8]) -> Document {
        Document::from_bytes(PathBuf::from("a.rs"), bytes).unwrap()
    }

    #[test]
    fn round_trips_bytes_unchanged() {
        for input in [
            &b"a\r\nb\r\n"[..],
            b"a\nb",
            b"\xEF\xBB\xBFbom\n",
            b"mixed\r\nends\n",
            b"",
        ] {
            assert_eq!(doc(input).to_bytes(), input);
        }
    }

    #[test]
    fn rejects_non_utf8() {
        let err = Document::from_bytes(PathBuf::from("x.bin"), &[0xFF, 0xFE]).unwrap_err();
        assert!(matches!(err, DocumentError::NotUtf8 { .. }));
    }

    #[test]
    fn typed_newlines_follow_the_file_line_ending() {
        let mut d = doc(b"a\r\nb");
        d.edit(1, 1, "x\ny", false);
        assert_eq!(d.to_bytes(), b"ax\r\ny\r\nb");
        assert_eq!(d.line_ending().label(), "CRLF");
    }

    #[test]
    fn dirty_until_current_version_is_saved() {
        let mut d = doc(b"");
        assert!(!d.is_dirty());
        d.edit(0, 0, "a", false);
        let saving = d.version();
        d.edit(1, 1, "b", false);
        d.mark_saved(saving);
        assert!(d.is_dirty(), "a newer edit is still unsaved");
        d.mark_saved(d.version());
        assert!(!d.is_dirty());
    }

    #[test]
    fn undo_and_redo_restore_text_and_cursor() {
        let mut d = doc(b"ab");
        d.edit(1, 1, "X", true);
        d.edit(2, 2, "Y", true);
        let undone = d.undo().unwrap();
        assert_eq!((d.text().to_string().as_str(), undone.cursor), ("ab", 1));
        let redone = d.redo().unwrap();
        assert_eq!((d.text().to_string().as_str(), redone.cursor), ("aXYb", 3));
    }

    #[test]
    fn syntax_tree_follows_edits_and_undo() {
        let mut d = Document::from_bytes(PathBuf::from("a.rs"), b"fn a() {}\n").unwrap();
        d.edit(3, 4, "bcd", false);
        d.undo();
        d.edit(8, 8, "let x = \"s\";", false);
        let fresh = Syntax::new(Language::Rust, d.text()).unwrap();
        let highlights = d.syntax().unwrap().highlight(d.text(), 0..2);
        assert_eq!(highlights, fresh.highlight(d.text(), 0..2));
        assert!(highlights[0].iter().any(|s| s.capture == "string"));
    }
}
