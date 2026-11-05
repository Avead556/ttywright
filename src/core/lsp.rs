use std::collections::BTreeSet;
use std::ops::Range;
use std::path::PathBuf;

use ropey::Rope;
use serde::{Deserialize, Serialize};

use super::document::Language;
use super::picker::{PickerMatch, fuzzy_filter};
use super::text;
use super::workspace::ViewId;

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ServerKey {
    pub server: Language,
    pub root: PathBuf,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum LspState {
    #[default]
    Off,
    Installing,
    Starting,
    Ready,
    Crashed,
    Stopped,
    NotFound,
    Disabled,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspStatus {
    pub state: LspState,
    pub busy: bool,
    pub name: String,
    pub install: Option<&'static str>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LspPanel {
    pub status: LspStatus,
    pub language: &'static str,
    pub root: PathBuf,
    pub open_files: usize,
    pub failure: Option<String>,
    pub can_choose: bool,
}

#[derive(Debug, Clone)]
pub(super) struct ServerStatus {
    pub state: LspState,
    pub busy: bool,
    pub name: Option<String>,
    pub failure: Option<String>,
    pub encoding: PositionEncoding,
    pub completion_triggers: Vec<String>,
    pub open: BTreeSet<PathBuf>,
}

impl Default for ServerStatus {
    fn default() -> Self {
        Self {
            state: LspState::Starting,
            busy: false,
            name: None,
            failure: None,
            encoding: PositionEncoding::Utf16,
            completion_triggers: Vec::new(),
            open: BTreeSet::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub enum Severity {
    Error,
    Warning,
    Info,
    Hint,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PositionEncoding {
    Utf8,
    Utf16,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RawDiagnostic {
    pub severity: Severity,
    pub message: String,
    pub start: (u32, u32),
    pub end: (u32, u32),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Diagnostic {
    pub raw: RawDiagnostic,
    pub server: ServerKey,
    pub encoding: PositionEncoding,
    pub chars: Option<Range<usize>>,
}

impl Diagnostic {
    pub fn resolve(&mut self, text: &Rope) {
        let start = char_at(text, self.raw.start, self.encoding);
        let end = char_at(text, self.raw.end, self.encoding);
        self.chars = Some(start..end.max(start));
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ServerChoice {
    #[default]
    Automatic,
    FromPath,
    Bundled,
    Release(String),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspServerOption {
    pub label: String,
    pub choice: ServerChoice,
    pub current: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LspQuery {
    Definition { at: (u32, u32) },
    Hover { at: (u32, u32) },
    Completion { at: (u32, u32) },
    References { at: (u32, u32) },
    Formatting { tab_size: usize },
    CodeActions { at: (u32, u32), to: (u32, u32) },
    Rename { at: (u32, u32), new_name: String },
    Refactorings { at: (u32, u32), to: (u32, u32) },
    SafeDeleteUsages { at: (u32, u32) },
    Symbols,
    PrepareCalls { at: (u32, u32), outgoing: bool },
    Calls { item: String, outgoing: bool },
    ResolveCompletion { item: String },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspSymbol {
    pub name: String,
    pub kind: String,
    pub depth: usize,
    pub start: (u32, u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProjectSymbol {
    pub name: String,
    pub kind: String,
    pub path: PathBuf,
    pub start: (u32, u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspLocation {
    pub path: PathBuf,
    pub start: (u32, u32),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LspAnswer {
    Locations(Vec<LspLocation>),
    Hover(Option<String>),
    References(Vec<LspUsage>),
    Edits(Vec<LspTextEdit>),
    CodeActions(Vec<LspCodeAction>),
    Refactorings(Vec<RefactorOffer>),
    SafeDeleteUsages(Vec<LspUsage>),
    Rename {
        changes: Option<Vec<LspFileEdit>>,
        places: Vec<LspUsage>,
    },
    Symbols(Vec<LspSymbol>),
    CallRoot {
        root: Option<(String, String)>,
        outgoing: bool,
    },
    Calls(Vec<LspUsage>),
    Completion {
        items: Vec<LspCompletion>,
        incomplete: bool,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspUsage {
    pub location: LspLocation,
    pub disk_line: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspTextEdit {
    pub start: (u32, u32),
    pub end: (u32, u32),
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspCodeAction {
    pub title: String,
    pub changes: Vec<LspFileEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RefactorOffer {
    pub action: LspCodeAction,
    pub places: Vec<LspUsage>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspFileEdit {
    pub path: PathBuf,
    pub edits: Vec<LspTextEdit>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergedEdit {
    pub from: usize,
    pub to: usize,
    pub text: String,
    pub cursor: usize,
}

pub fn merge_edits(
    text: &Rope,
    edits: &[LspTextEdit],
    encoding: PositionEncoding,
    cursor: usize,
) -> Result<Option<MergedEdit>, String> {
    let mut spans: Vec<(usize, usize, &str)> = edits
        .iter()
        .map(|e| {
            let start = char_at(text, e.start, encoding);
            let end = char_at(text, e.end, encoding).max(start);
            (start, end, e.text.as_str())
        })
        .collect();
    spans.sort_by_key(|s| (s.0, s.1));
    if spans.windows(2).any(|w| w[0].1 > w[1].0) {
        return Err("the server sent overlapping edits".to_owned());
    }
    let old: Vec<char> = text.chars().collect();
    let mut new: Vec<char> = Vec::with_capacity(old.len());
    let mut copied = 0;
    let mut new_cursor = None;
    for (start, end, replacement) in &spans {
        if new_cursor.is_none() && cursor < *start {
            new_cursor = Some(new.len() + (cursor - copied));
        }
        new.extend(&old[copied..*start]);
        new.extend(replacement.chars());
        if new_cursor.is_none() && cursor < *end {
            new_cursor = Some(new.len());
        }
        copied = *end;
    }
    let cursor = new_cursor.unwrap_or(new.len() + cursor.saturating_sub(copied));
    new.extend(&old[copied..]);
    if new == old {
        return Ok(None);
    }
    let prefix = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let room = old.len().min(new.len()) - prefix;
    let suffix = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    Ok(Some(MergedEdit {
        from: prefix,
        to: old.len() - suffix,
        text: new[prefix..new.len() - suffix].iter().collect(),
        cursor,
    }))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LspCompletion {
    pub label: String,
    pub detail: Option<String>,
    pub filter: String,
    pub sort: String,
    pub insert: String,
    pub start: Option<(u32, u32)>,
    pub additional: Vec<LspTextEdit>,
    pub resolve: Option<String>,
}

#[derive(Debug, Clone)]
pub struct Completion {
    pub items: Vec<LspCompletion>,
    filters: Vec<String>,
    pub matches: Vec<PickerMatch>,
    pub selected: usize,
    pub anchor: usize,
    pub incomplete: bool,
    pub view: ViewId,
    pub version: u64,
}

impl Completion {
    pub fn new(
        mut items: Vec<LspCompletion>,
        incomplete: bool,
        anchor: usize,
        view: ViewId,
        version: u64,
    ) -> Self {
        items.sort_by(|a, b| a.sort.cmp(&b.sort));
        let filters = items.iter().map(|i| i.filter.clone()).collect();
        Self {
            items,
            filters,
            matches: Vec::new(),
            selected: 0,
            anchor,
            incomplete,
            view,
            version,
        }
    }

    pub fn refilter(&mut self, typed: &str) {
        self.matches = fuzzy_filter(typed, &self.filters, false);
        self.selected = 0;
    }

    pub fn select_by(&mut self, delta: isize) {
        let last = self.matches.len().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub fn selected_item(&self) -> Option<&LspCompletion> {
        let m = self.matches.get(self.selected)?;
        self.items.get(m.index)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HoverLine {
    pub text: String,
    pub code: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Hover {
    pub lines: Vec<HoverLine>,
    pub scroll: usize,
}

pub fn hover_lines(markdown: &str) -> Vec<HoverLine> {
    let mut lines: Vec<HoverLine> = Vec::new();
    let mut in_code = false;
    for raw in markdown.lines() {
        if raw.trim_start().starts_with("```") {
            in_code = !in_code;
            continue;
        }
        let text = if !in_code && raw.trim() == "---" {
            ""
        } else {
            raw
        };
        let blank = text.trim().is_empty();
        if blank && lines.last().is_none_or(|l| l.text.is_empty()) {
            continue;
        }
        lines.push(HoverLine {
            text: if blank {
                String::new()
            } else {
                text.to_owned()
            },
            code: in_code,
        });
    }
    if lines.last().is_some_and(|l| l.text.is_empty()) {
        lines.pop();
    }
    lines
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Problem {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub severity: Severity,
    pub message: String,
    pub source: &'static str,
}

pub fn char_at(text: &Rope, (line, col): (u32, u32), encoding: PositionEncoding) -> usize {
    let line = line as usize;
    if line >= text.len_lines() {
        return text.len_chars();
    }
    let start = text.line_to_char(line);
    let mut units = 0;
    let mut chars = 0;
    for c in text::line_content(text, line).chars() {
        if units >= col as usize {
            break;
        }
        units += match encoding {
            PositionEncoding::Utf8 => c.len_utf8(),
            PositionEncoding::Utf16 => c.len_utf16(),
        };
        chars += 1;
    }
    start + chars
}

pub fn position_of(text: &Rope, idx: usize, encoding: PositionEncoding) -> (u32, u32) {
    let idx = idx.min(text.len_chars());
    let line = text.char_to_line(idx);
    let units: usize = text
        .slice(text.line_to_char(line)..idx)
        .chars()
        .map(|c| match encoding {
            PositionEncoding::Utf8 => c.len_utf8(),
            PositionEncoding::Utf16 => c.len_utf16(),
        })
        .sum();
    (
        u32::try_from(line).unwrap_or(u32::MAX),
        u32::try_from(units).unwrap_or(u32::MAX),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn columns_are_counted_in_the_servers_units() {
        let t = Rope::from_str("ы𝄞x\nnext");
        assert_eq!(char_at(&t, (0, 6), PositionEncoding::Utf8), 2);
        assert_eq!(char_at(&t, (0, 3), PositionEncoding::Utf16), 2);
        assert_eq!(char_at(&t, (1, 2), PositionEncoding::Utf8), 6);
    }

    #[test]
    fn only_the_protocols_line_ends_count_as_lines() {
        let t = Rope::from_str("a\u{c}b\u{b}c\u{85}d\u{2028}e\u{2029}f\nx\r\ny\rz");
        assert_eq!(t.len_lines(), 4);
        assert_eq!(char_at(&t, (1, 0), PositionEncoding::Utf16), 12);
        assert_eq!(position_of(&t, 12, PositionEncoding::Utf16), (1, 0));
        assert_eq!(char_at(&t, (2, 0), PositionEncoding::Utf16), 15);
        assert_eq!(char_at(&t, (3, 0), PositionEncoding::Utf16), 17);
    }

    #[test]
    fn positions_of_chars_round_trip() {
        let t = Rope::from_str("ы𝄞x\nnext");
        for encoding in [PositionEncoding::Utf8, PositionEncoding::Utf16] {
            for idx in 0..=t.len_chars() {
                assert_eq!(char_at(&t, position_of(&t, idx, encoding), encoding), idx);
            }
        }
    }

    #[test]
    fn hover_markdown_becomes_code_and_text_lines() {
        let lines = hover_lines("```rust\nfn a()\n```\n\n---\n\n\nDocs *here*\n\n");
        let shown: Vec<_> = lines.iter().map(|l| (l.text.as_str(), l.code)).collect();
        assert_eq!(
            shown,
            [("fn a()", true), ("", false), ("Docs *here*", false)]
        );
    }

    fn edit(start: (u32, u32), end: (u32, u32), text: &str) -> LspTextEdit {
        LspTextEdit {
            start,
            end,
            text: text.to_owned(),
        }
    }

    fn merged(text: &str, edits: &[LspTextEdit], cursor: usize) -> (String, usize) {
        let mut rope = Rope::from_str(text);
        let m = merge_edits(&rope, edits, PositionEncoding::Utf8, cursor)
            .expect("no overlap")
            .expect("a change");
        rope.remove(m.from..m.to);
        rope.insert(m.from, &m.text);
        (rope.to_string(), m.cursor)
    }

    #[test]
    fn edits_merge_in_any_order_and_the_cursor_follows_its_text() {
        let edits = [edit((1, 0), (1, 0), "  "), edit((0, 1), (0, 1), "  ")];
        assert_eq!(merged("a\ny\n", &edits, 2), ("a  \n  y\n".to_owned(), 6));
    }

    #[test]
    fn a_cursor_inside_replaced_text_lands_after_the_replacement() {
        let edits = [edit((0, 1), (0, 3), "XYZ")];
        assert_eq!(merged("abcd", &edits, 2), ("aXYZd".to_owned(), 4));
    }

    #[test]
    fn a_cursor_before_every_edit_stays_put() {
        let edits = [edit((0, 3), (0, 4), "")];
        assert_eq!(merged("abcd", &edits, 1), ("abc".to_owned(), 1));
    }

    #[test]
    fn a_cursor_after_every_edit_shifts_by_them() {
        let edits = [edit((0, 0), (0, 2), "")];
        assert_eq!(merged("abcd", &edits, 3), ("cd".to_owned(), 1));
    }

    #[test]
    fn edit_columns_are_in_server_units() {
        let edits = [edit((0, 2), (0, 2), "x")];
        assert_eq!(merged("ыы", &edits, 2), ("ыxы".to_owned(), 3));
    }

    #[test]
    fn edits_that_change_nothing_or_overlap_are_told_apart() {
        let t = Rope::from_str("abc");
        let same = [edit((0, 0), (0, 1), "a")];
        assert_eq!(merge_edits(&t, &same, PositionEncoding::Utf8, 0), Ok(None));
        let overlap = [edit((0, 0), (0, 2), "x"), edit((0, 1), (0, 3), "y")];
        assert!(merge_edits(&t, &overlap, PositionEncoding::Utf8, 0).is_err());
        assert_eq!(merge_edits(&t, &[], PositionEncoding::Utf8, 0), Ok(None));
    }

    #[test]
    fn positions_past_the_end_are_clamped() {
        let t = Rope::from_str("ab\ncd");
        assert_eq!(char_at(&t, (0, 99), PositionEncoding::Utf8), 2);
        assert_eq!(char_at(&t, (99, 0), PositionEncoding::Utf8), 5);
    }

    #[test]
    fn resolving_gives_a_range_even_for_reversed_positions() {
        let mut d = Diagnostic {
            server: ServerKey {
                server: Language::Rust,
                root: PathBuf::from("/"),
            },
            raw: RawDiagnostic {
                severity: Severity::Error,
                message: String::new(),
                start: (0, 3),
                end: (0, 1),
            },
            encoding: PositionEncoding::Utf8,
            chars: None,
        };
        d.resolve(&Rope::from_str("abcd"));
        assert_eq!(d.chars, Some(3..3));
    }
}
