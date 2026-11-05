use std::path::PathBuf;

use serde::{Deserialize, Serialize};
use similar::{ChangeTag, TextDiff};

use super::workspace::DocumentId;

const CONTEXT_LINES: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SnapshotLabel {
    Opened,
    Save,
    Autosave,
    ExternalChange,
    BeforeReload,
    BeforeOverwrite,
    BeforeRevert,
    BeforeReplaceInFiles,
    BeforeAgent,
}

impl SnapshotLabel {
    pub fn title(self) -> &'static str {
        match self {
            Self::Opened => "Opened",
            Self::Save => "Save",
            Self::Autosave => "Autosave",
            Self::ExternalChange => "Changed on disk",
            Self::BeforeReload => "Before: Reload from Disk",
            Self::BeforeOverwrite => "Before: Keep My Version",
            Self::BeforeRevert => "Before: Revert",
            Self::BeforeReplaceInFiles => "Before: Replace",
            Self::BeforeAgent => "Before: Claude Code",
        }
    }

    pub fn is_routine_save(self) -> bool {
        matches!(self, Self::Save | Self::Autosave)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HistoryEntry {
    pub timestamp: u64,
    pub path: PathBuf,
    pub hash: String,
    pub label: SnapshotLabel,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum DiffKind {
    Same,
    Removed,
    Added,
    Gap,
    Section,
    Hunk,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DiffRow {
    pub kind: DiffKind,
    pub old_line: Option<usize>,
    pub new_line: Option<usize>,
    pub text: String,
}

pub fn diff_rows(old: &str, new: &str) -> Vec<DiffRow> {
    let diff = TextDiff::from_lines(old, new);
    let mut rows = Vec::new();
    for (i, group) in diff.grouped_ops(CONTEXT_LINES).iter().enumerate() {
        if i > 0 {
            rows.push(DiffRow {
                kind: DiffKind::Gap,
                old_line: None,
                new_line: None,
                text: String::new(),
            });
        }
        for op in group {
            for change in diff.iter_changes(op) {
                rows.push(DiffRow {
                    kind: match change.tag() {
                        ChangeTag::Equal => DiffKind::Same,
                        ChangeTag::Delete => DiffKind::Removed,
                        ChangeTag::Insert => DiffKind::Added,
                    },
                    old_line: change.old_index().map(|n| n + 1),
                    new_line: change.new_index().map(|n| n + 1),
                    text: change.value().trim_end_matches(['\n', '\r']).to_owned(),
                });
            }
        }
    }
    rows
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LocalHistoryPanel {
    pub doc: DocumentId,
    pub entries: Vec<HistoryEntry>,
    pub selected: usize,
    pub now: u64,
    pub version: Option<Vec<u8>>,
    pub diff: Vec<DiffRow>,
    pub diff_scroll: usize,
    pub error: Option<String>,
}

impl LocalHistoryPanel {
    pub fn new(doc: DocumentId, entries: Vec<HistoryEntry>, now: u64) -> Self {
        Self {
            doc,
            entries,
            selected: 0,
            now,
            version: None,
            diff: Vec::new(),
            diff_scroll: 0,
            error: None,
        }
    }

    pub fn selected_entry(&self) -> Option<&HistoryEntry> {
        self.entries.get(self.selected)
    }

    pub fn select(&mut self, index: usize) {
        self.selected = index.min(self.entries.len().saturating_sub(1));
        self.version = None;
        self.diff.clear();
        self.diff_scroll = 0;
        self.error = None;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn kinds(rows: &[DiffRow]) -> Vec<DiffKind> {
        rows.iter().map(|r| r.kind).collect()
    }

    #[test]
    fn identical_texts_have_no_rows() {
        assert!(diff_rows("a\nb\n", "a\nb\n").is_empty());
    }

    #[test]
    fn a_changed_line_is_removed_and_added_with_numbers_from_each_side() {
        let rows = diff_rows("a\nb\nc\n", "a\nX\nY\nc\n");
        assert_eq!(
            kinds(&rows),
            [
                DiffKind::Same,
                DiffKind::Removed,
                DiffKind::Added,
                DiffKind::Added,
                DiffKind::Same
            ]
        );
        assert_eq!(rows[1].text, "b");
        assert_eq!((rows[1].old_line, rows[1].new_line), (Some(2), None));
        assert_eq!((rows[3].old_line, rows[3].new_line), (None, Some(3)));
        assert_eq!((rows[4].old_line, rows[4].new_line), (Some(3), Some(4)));
    }

    #[test]
    fn distant_changes_are_separated_by_a_gap() {
        let old: String = (0..30).map(|n| format!("{n}\n")).collect();
        let new = old.replace("\n2\n", "\ntwo\n").replace("\n25\n", "\nx\n");
        let rows = diff_rows(&old, &new);
        assert_eq!(
            kinds(&rows).iter().filter(|k| **k == DiffKind::Gap).count(),
            1
        );
        assert!(rows.len() < 20, "unchanged middle is left out");
    }

    #[test]
    fn line_endings_do_not_show_in_the_text() {
        let rows = diff_rows("a\r\n", "b\r\n");
        assert_eq!(rows[0].text, "a");
        assert_eq!(rows[1].text, "b");
    }
}
