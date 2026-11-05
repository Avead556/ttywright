use ropey::Rope;

use super::conflict::{ConflictChoice, ConflictRegion, conflict_regions};
use super::git::GitNotice;
use super::text;
use super::workspace::DocumentId;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeConflict {
    pub ours: Vec<String>,
    pub base: Option<Vec<String>>,
    pub theirs: Vec<String>,
    pub resolved: Option<ConflictChoice>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MergeChunk {
    Common(Vec<String>),
    Conflict(MergeConflict),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MergeCell {
    Plain,
    Open,
    Taken,
    Dropped,
    Base,
    Undecided,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MergeRow {
    pub ours: Option<(usize, String)>,
    pub result: Option<(Option<usize>, String)>,
    pub theirs: Option<(usize, String)>,
    pub cells: [MergeCell; 3],
    pub conflict: Option<usize>,
}

fn lines_between(text_of: &Rope, from: usize, to: usize) -> Vec<String> {
    (from..to).map(|n| text::line_content(text_of, n)).collect()
}

pub fn merge_chunks(text_of: &Rope) -> Vec<MergeChunk> {
    let mut lines = text_of.len_lines();
    if lines > 0 && text::line_content(text_of, lines - 1).is_empty() {
        lines -= 1;
    }
    let mut chunks = Vec::new();
    let mut at = 0;
    for ConflictRegion {
        start,
        base,
        middle,
        end,
    } in conflict_regions(text_of)
    {
        if start > at {
            chunks.push(MergeChunk::Common(lines_between(text_of, at, start)));
        }
        chunks.push(MergeChunk::Conflict(MergeConflict {
            ours: lines_between(text_of, start + 1, base.unwrap_or(middle)),
            base: base.map(|base| lines_between(text_of, base + 1, middle)),
            theirs: lines_between(text_of, middle + 1, end),
            resolved: None,
        }));
        at = end + 1;
    }
    if lines > at {
        chunks.push(MergeChunk::Common(lines_between(text_of, at, lines)));
    }
    chunks
}

fn side_names(text_of: &Rope) -> (String, String) {
    let name = |line: usize| {
        let content = text::line_content(text_of, line);
        content.chars().skip(8).collect::<String>()
    };
    conflict_regions(text_of)
        .first()
        .map_or((String::new(), String::new()), |r| {
            (name(r.start), name(r.end))
        })
}

fn result_of(conflict: &MergeConflict, choice: ConflictChoice) -> Vec<String> {
    let (ours, theirs) = (conflict.ours.iter(), conflict.theirs.iter());
    match choice {
        ConflictChoice::Ours => ours.cloned().collect(),
        ConflictChoice::Theirs => theirs.cloned().collect(),
        ConflictChoice::Both => ours.chain(theirs).cloned().collect(),
        ConflictChoice::TheirsThenOurs => theirs.chain(ours).cloned().collect(),
    }
}

pub fn merge_rows(chunks: &[MergeChunk]) -> Vec<MergeRow> {
    let mut rows = Vec::new();
    let (mut ours_no, mut result_no, mut theirs_no) = (1, 1, 1);
    let mut conflict_no = 0;
    for chunk in chunks {
        match chunk {
            MergeChunk::Common(lines) => {
                for line in lines {
                    rows.push(MergeRow {
                        ours: Some((ours_no, line.clone())),
                        result: Some((Some(result_no), line.clone())),
                        theirs: Some((theirs_no, line.clone())),
                        cells: [MergeCell::Plain; 3],
                        conflict: None,
                    });
                    ours_no += 1;
                    result_no += 1;
                    theirs_no += 1;
                }
            }
            MergeChunk::Conflict(conflict) => {
                let (result, result_cell): (Vec<String>, MergeCell) = match conflict.resolved {
                    Some(choice) => (result_of(conflict, choice), MergeCell::Taken),
                    None => match &conflict.base {
                        Some(base) => (base.clone(), MergeCell::Base),
                        None => (vec!["· undecided ·".to_owned()], MergeCell::Undecided),
                    },
                };
                let side = |taken: bool| match conflict.resolved {
                    None => MergeCell::Open,
                    Some(_) if taken => MergeCell::Taken,
                    Some(_) => MergeCell::Dropped,
                };
                let ours_cell = side(conflict.resolved != Some(ConflictChoice::Theirs));
                let theirs_cell = side(conflict.resolved != Some(ConflictChoice::Ours));
                let height = conflict
                    .ours
                    .len()
                    .max(conflict.theirs.len())
                    .max(result.len())
                    .max(1);
                for n in 0..height {
                    rows.push(MergeRow {
                        ours: conflict.ours.get(n).map(|l| (ours_no + n, l.clone())),
                        result: result.get(n).map(|l| {
                            let number = conflict.resolved.map(|_| result_no + n);
                            (number, l.clone())
                        }),
                        theirs: conflict.theirs.get(n).map(|l| (theirs_no + n, l.clone())),
                        cells: [ours_cell, result_cell, theirs_cell],
                        conflict: Some(conflict_no),
                    });
                }
                ours_no += conflict.ours.len();
                theirs_no += conflict.theirs.len();
                if conflict.resolved.is_some() {
                    result_no += result.len();
                }
                conflict_no += 1;
            }
        }
    }
    rows
}

#[derive(Debug, Clone)]
pub struct MergeView {
    pub doc: DocumentId,
    pub version: u64,
    pub file: String,
    pub ours_name: String,
    pub theirs_name: String,
    pub chunks: Vec<MergeChunk>,
    pub rows: Vec<MergeRow>,
    pub selected: usize,
    pub scroll: usize,
    pub page: usize,
    pub column: usize,
    pub undo: Vec<usize>,
    pub notice: Option<GitNotice>,
}

impl MergeView {
    pub fn new(doc: DocumentId, version: u64, file: String, text_of: &Rope) -> Self {
        let (ours_name, theirs_name) = side_names(text_of);
        let chunks = merge_chunks(text_of);
        let mut view = Self {
            doc,
            version,
            file,
            ours_name,
            theirs_name,
            rows: merge_rows(&chunks),
            chunks,
            selected: 0,
            scroll: 0,
            page: 0,
            column: 0,
            undo: Vec::new(),
            notice: None,
        };
        view.show_selected();
        view
    }

    pub fn conflict(&self, index: usize) -> Option<&MergeConflict> {
        self.chunks
            .iter()
            .filter_map(|c| match c {
                MergeChunk::Conflict(conflict) => Some(conflict),
                MergeChunk::Common(_) => None,
            })
            .nth(index)
    }

    fn conflict_mut(&mut self, index: usize) -> Option<&mut MergeConflict> {
        self.chunks
            .iter_mut()
            .filter_map(|c| match c {
                MergeChunk::Conflict(conflict) => Some(conflict),
                MergeChunk::Common(_) => None,
            })
            .nth(index)
    }

    pub fn conflict_count(&self) -> usize {
        (0..).take_while(|i| self.conflict(*i).is_some()).count()
    }

    pub fn open_count(&self) -> usize {
        (0..self.conflict_count())
            .filter(|i| self.conflict(*i).is_some_and(|c| c.resolved.is_none()))
            .count()
    }

    pub fn marker_index(&self, index: usize) -> usize {
        (0..index)
            .filter(|i| self.conflict(*i).is_some_and(|c| c.resolved.is_none()))
            .count()
    }

    pub fn set_resolved(&mut self, index: usize, choice: Option<ConflictChoice>) {
        if let Some(conflict) = self.conflict_mut(index) {
            conflict.resolved = choice;
        }
        self.rows = merge_rows(&self.chunks);
    }

    pub fn next_open(&self, from: usize, forward: bool) -> Option<usize> {
        let count = self.conflict_count();
        (1..=count)
            .map(|step| {
                if forward {
                    (from + step) % count
                } else {
                    (from + count - step % count) % count
                }
            })
            .find(|i| self.conflict(*i).is_some_and(|c| c.resolved.is_none()))
    }

    pub fn show_selected(&mut self) {
        let is_selected = |r: &MergeRow| r.conflict == Some(self.selected);
        let Some(first) = self.rows.iter().position(is_selected) else {
            return;
        };
        let last = self.rows.iter().rposition(is_selected).unwrap_or(first);
        if self.page == 0 || first < self.scroll || last >= self.scroll + self.page {
            self.scroll = first.saturating_sub(SHOWN_ABOVE);
        }
        self.scroll = self.scroll.min(self.last_scroll());
    }

    pub fn scroll_by(&mut self, delta: isize) {
        self.scroll = self
            .scroll
            .saturating_add_signed(delta)
            .min(self.last_scroll());
    }

    pub fn set_page(&mut self, page: usize) {
        self.page = page;
        self.scroll = self.scroll.min(self.last_scroll());
    }

    pub fn last_scroll(&self) -> usize {
        self.rows.len().saturating_sub(self.page.max(1))
    }
}

const SHOWN_ABOVE: usize = 3;

#[cfg(test)]
mod tests {
    use super::*;

    const FILE: &str = "top\n<<<<<<< HEAD\nmine 1\nmine 2\n=======\nyours\n>>>>>>> other\nmid\n<<<<<<< HEAD\nA\n=======\nB\n>>>>>>> other\nend\n";

    fn view() -> MergeView {
        MergeView::new(DocumentId(1), 1, "f".into(), &Rope::from_str(FILE))
    }

    #[test]
    fn a_file_is_cut_into_what_is_common_and_what_is_not() {
        let chunks = merge_chunks(&Rope::from_str(FILE));
        assert_eq!(chunks.len(), 5);
        assert_eq!(chunks[0], MergeChunk::Common(vec!["top".to_owned()]));
        let MergeChunk::Conflict(first) = &chunks[1] else {
            panic!("{chunks:?}");
        };
        assert_eq!(first.ours, ["mine 1", "mine 2"]);
        assert_eq!(first.theirs, ["yours"]);
        assert_eq!(first.base, None);
        assert_eq!(chunks[4], MergeChunk::Common(vec!["end".to_owned()]));
    }

    #[test]
    fn side_names_come_from_the_markers() {
        let v = view();
        assert_eq!(
            (v.ours_name.as_str(), v.theirs_name.as_str()),
            ("HEAD", "other")
        );
    }

    #[test]
    fn the_panes_line_up_row_by_row_with_their_own_numbers() {
        let rows = view().rows;
        type Side<'a> = Option<(usize, &'a str)>;
        fn shape(r: &MergeRow) -> (Side<'_>, Side<'_>, Option<usize>) {
            (
                r.ours.as_ref().map(|(n, t)| (*n, t.as_str())),
                r.theirs.as_ref().map(|(n, t)| (*n, t.as_str())),
                r.conflict,
            )
        }
        assert_eq!(shape(&rows[0]), (Some((1, "top")), Some((1, "top")), None));
        assert_eq!(
            shape(&rows[1]),
            (Some((2, "mine 1")), Some((2, "yours")), Some(0))
        );
        assert_eq!(shape(&rows[2]), (Some((3, "mine 2")), None, Some(0)));
        assert_eq!(shape(&rows[3]), (Some((4, "mid")), Some((3, "mid")), None));
        assert_eq!(rows[1].result, Some((None, "· undecided ·".to_owned())));
        assert_eq!(
            rows[3].result,
            Some((Some(2), "mid".to_owned())),
            "one line so far"
        );
    }

    #[test]
    fn a_choice_shows_in_the_result_and_moves_its_numbers() {
        let mut v = view();
        v.set_resolved(0, Some(ConflictChoice::Theirs));
        assert_eq!(v.rows[1].result, Some((Some(2), "yours".to_owned())));
        assert_eq!(
            v.rows[1].cells,
            [MergeCell::Dropped, MergeCell::Taken, MergeCell::Taken]
        );
        let mid = v
            .rows
            .iter()
            .find(|r| r.ours.as_ref().is_some_and(|o| o.1 == "mid"));
        assert_eq!(
            mid.and_then(|r| r.result.clone()),
            Some((Some(3), "mid".to_owned()))
        );
    }

    #[test]
    fn both_orders_stack_up_in_the_result() {
        let mut v = view();
        v.set_resolved(0, Some(ConflictChoice::TheirsThenOurs));
        let result: Vec<_> = v
            .rows
            .iter()
            .filter(|r| r.conflict == Some(0))
            .filter_map(|r| r.result.as_ref().map(|x| x.1.as_str()))
            .collect();
        assert_eq!(result, ["yours", "mine 1", "mine 2"]);
        assert_eq!(
            v.rows.iter().filter(|r| r.conflict == Some(0)).count(),
            3,
            "taller than a side"
        );
    }

    #[test]
    fn with_diff3_the_result_shows_the_base_until_a_choice() {
        let text = "<<<<<<< a\nm\n||||||| b\nold\n=======\nt\n>>>>>>> c\n";
        let mut v = MergeView::new(DocumentId(1), 1, "f".into(), &Rope::from_str(text));
        assert_eq!(v.rows[0].result, Some((None, "old".to_owned())));
        assert_eq!(v.rows[0].cells[1], MergeCell::Base);
        v.set_resolved(0, Some(ConflictChoice::Ours));
        assert_eq!(v.rows[0].result, Some((Some(1), "m".to_owned())));
    }

    #[test]
    fn a_marker_index_skips_what_has_no_markers_any_more() {
        let mut v = view();
        assert_eq!(v.marker_index(1), 1);
        v.set_resolved(0, Some(ConflictChoice::Ours));
        assert_eq!(v.marker_index(1), 0);
        assert_eq!(v.open_count(), 1);
    }

    #[test]
    fn going_to_the_next_open_conflict_goes_round_and_skips_decided_ones() {
        let mut v = view();
        assert_eq!(v.next_open(0, true), Some(1));
        assert_eq!(v.next_open(1, true), Some(0));
        assert_eq!(v.next_open(0, false), Some(1));
        v.set_resolved(1, Some(ConflictChoice::Ours));
        assert_eq!(v.next_open(0, true), Some(0), "only itself is left");
        v.set_resolved(0, Some(ConflictChoice::Ours));
        assert_eq!(v.next_open(0, true), None);
    }

    #[test]
    fn an_empty_side_still_has_a_row_to_stand_on() {
        let text = "<<<<<<< a\n=======\n>>>>>>> c\n";
        let v = MergeView::new(DocumentId(1), 1, "f".into(), &Rope::from_str(text));
        assert_eq!(v.rows.len(), 1);
        assert_eq!(v.rows[0].conflict, Some(0));
    }

    #[test]
    fn choosing_keeps_what_fits_in_place_and_scrolls_only_for_what_does_not() {
        let mut text = String::from("<<<<<<< a\n1\n=======\n2\n>>>>>>> b\n");
        text.push_str(&"common\n".repeat(40));
        text.push_str("<<<<<<< a\n3\n=======\n4\n>>>>>>> b\n");
        let mut v = MergeView::new(DocumentId(1), 1, "f".into(), &Rope::from_str(&text));
        v.set_page(100);
        v.selected = 1;
        v.show_selected();
        assert_eq!(v.scroll, 0, "all of it is on screen");
        v.set_page(10);
        v.show_selected();
        assert_eq!(v.scroll, 32, "stops with the last row at the bottom");
        v.scroll_by(99);
        assert_eq!(v.scroll, 32);
        v.selected = 0;
        v.show_selected();
        assert_eq!(v.scroll, 0);
    }
}
