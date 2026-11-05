use std::ops::Range;

use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;

use super::text;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Metrics {
    pub tab_width: usize,
    pub wrap: Option<usize>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Row {
    pub chars: Range<usize>,
    pub cols: Range<usize>,
}

pub fn wrap_line(content: &str, cols: usize, tab_width: usize) -> Vec<Row> {
    let mut rows = Vec::new();
    let (mut row_char, mut row_col) = (0, 0);
    let (mut char_at, mut col) = (0, 0);
    let mut break_at = None;
    let mut row_has_text = false;
    for g in content.graphemes(true) {
        let width = text::grapheme_width(g, col, tab_width);
        if col + width - row_col > cols && char_at > row_char {
            let (to_char, to_col) = break_at.take().unwrap_or((char_at, col));
            rows.push(Row {
                chars: row_char..to_char,
                cols: row_col..to_col,
            });
            (row_char, row_col) = (to_char, to_col);
            row_has_text = to_char < char_at;
            if col + width - row_col > cols && char_at > row_char {
                rows.push(Row {
                    chars: row_char..char_at,
                    cols: row_col..col,
                });
                (row_char, row_col) = (char_at, col);
                row_has_text = false;
            }
        }
        char_at += g.chars().count();
        col += width;
        if !g.chars().all(char::is_whitespace) {
            row_has_text = true;
        } else if row_has_text {
            break_at = Some((char_at, col));
        }
    }
    rows.push(Row {
        chars: row_char..char_at,
        cols: row_col..col,
    });
    if col - row_col >= cols {
        rows.push(Row {
            chars: char_at..char_at,
            cols: col..col,
        });
    }
    rows
}

impl Metrics {
    pub fn rows(&self, t: &Rope, line: usize) -> Vec<Row> {
        let content = text::line_content(t, line);
        match self.wrap {
            Some(cols) => wrap_line(&content, cols, self.tab_width),
            None => {
                let end = content.chars().count();
                let col = text::col_of(t, t.line_to_char(line) + end, self.tab_width);
                vec![Row {
                    chars: 0..end,
                    cols: 0..col,
                }]
            }
        }
    }

    pub fn position(&self, t: &Rope, idx: usize) -> (usize, usize, usize) {
        let line = t.char_to_line(idx);
        let offset = idx - t.line_to_char(line);
        let rows = self.rows(t, line);
        let row = row_at(&rows, offset);
        let col = text::col_of(t, idx, self.tab_width);
        (line, row, col.saturating_sub(rows[row].cols.start))
    }

    pub fn idx_at(&self, t: &Rope, line: usize, row: usize, col: usize) -> usize {
        let rows = self.rows(t, line);
        let row_idx = row.min(rows.len() - 1);
        let target = &rows[row_idx];
        let idx = text::idx_at_col(t, line, target.cols.start + col, self.tab_width);
        let start = t.line_to_char(line);
        if row_idx + 1 < rows.len() && idx >= start + target.chars.end {
            return text::prev_grapheme_boundary(t, start + target.chars.end);
        }
        idx
    }

    pub fn step(&self, t: &Rope, (line, row): (usize, usize), delta: isize) -> (usize, usize) {
        if self.wrap.is_none() {
            let last = (t.len_lines() - 1) as isize;
            let line = (line.min(t.len_lines() - 1) as isize)
                .saturating_add(delta)
                .clamp(0, last);
            return (line as usize, 0);
        }
        let mut line = line.min(t.len_lines() - 1);
        let mut in_line = self.rows(t, line).len();
        let mut row = row.min(in_line - 1);
        if delta >= 0 {
            for _ in 0..delta {
                if row + 1 < in_line {
                    row += 1;
                } else if line + 1 < t.len_lines() {
                    line += 1;
                    row = 0;
                    in_line = self.rows(t, line).len();
                } else {
                    break;
                }
            }
        } else {
            for _ in 0..delta.unsigned_abs() {
                if row > 0 {
                    row -= 1;
                } else if line > 0 {
                    line -= 1;
                    row = self.rows(t, line).len() - 1;
                } else {
                    break;
                }
            }
        }
        (line, row)
    }

    pub fn follow(
        &self,
        t: &Rope,
        top: (usize, usize),
        cursor: (usize, usize),
        height: usize,
    ) -> (usize, usize) {
        let top = self.step(t, top, 0);
        if cursor < top || height == 0 {
            return cursor.min(top);
        }
        if cursor.0 < top.0 + height {
            let mut at = top;
            for _ in 0..height {
                if at == cursor {
                    return top;
                }
                at = self.step(t, at, 1);
            }
        }
        self.step(t, cursor, -((height - 1) as isize))
    }
}

pub fn row_at(rows: &[Row], offset: usize) -> usize {
    rows.iter()
        .position(|r| offset < r.chars.end)
        .unwrap_or(rows.len() - 1)
}

#[cfg(test)]
mod tests {
    use unicode_width::UnicodeWidthStr;

    use super::*;

    fn cut(content: &str, cols: usize) -> Vec<String> {
        wrap_line(content, cols, 4)
            .iter()
            .map(|r| {
                content
                    .chars()
                    .skip(r.chars.start)
                    .take(r.chars.len())
                    .collect()
            })
            .collect()
    }

    #[test]
    fn a_short_line_is_one_row_and_an_empty_line_too() {
        assert_eq!(cut("abc", 10), ["abc"]);
        assert_eq!(cut("", 10), [""]);
    }

    #[test]
    fn breaks_after_whitespace_when_it_can() {
        assert_eq!(cut("one two three", 8), ["one two ", "three"]);
        assert_eq!(cut("abcdefghij", 4), ["abcd", "efgh", "ij"]);
        assert_eq!(cut("a bcdefghij", 4), ["a ", "bcde", "fghi", "j"]);
    }

    #[test]
    fn indentation_alone_is_not_a_place_to_break() {
        assert_eq!(cut("    abcdefgh", 6), ["    ab", "cdefgh", ""]);
    }

    #[test]
    fn a_full_row_at_the_end_leaves_a_cell_for_the_cursor() {
        assert_eq!(cut("abcd", 4), ["abcd", ""]);
        let rows = wrap_line("abcd", 4, 4);
        assert_eq!(rows[1].cols, 4..4);
    }

    #[test]
    fn wide_chars_move_whole_to_the_next_row() {
        assert_eq!(cut("a界b", 2), ["a", "界", "b"]);
        let rows = wrap_line("a界b", 2, 4);
        assert_eq!(rows[1].cols, 1..3);
    }

    #[test]
    fn tabs_keep_their_width_from_the_start_of_the_line() {
        let rows = wrap_line("abc\tdef", 4, 4);
        assert_eq!(rows[0].cols, 0..4);
        assert_eq!(rows[0].chars, 0..4);
        assert_eq!(rows[1].cols, 4..7);
        let rows = wrap_line("ab\tc", 3, 4);
        assert_eq!(rows[0].cols, 0..2);
        assert_eq!(rows[1].cols, 2..5, "the tab is 2 wide, then c");
        assert_eq!(rows[1].chars, 2..4);
    }

    #[test]
    fn a_grapheme_wider_than_a_row_gets_one_rather_than_hanging() {
        assert_eq!(cut("界界", 1), ["界", "界", ""]);
        assert_eq!(cut("\tx", 2), ["\t", "x"]);
    }

    #[test]
    fn combining_clusters_are_never_cut() {
        let s = "e\u{301}e\u{301}e\u{301}";
        assert_eq!(cut(s, 2), ["e\u{301}e\u{301}", "e\u{301}"]);
    }

    const TRICKY: &[&str] = &[
        "",
        "a",
        "plain words that are long enough to wrap around several times",
        "\t\tindented\twith\ttabs",
        "ab\tcd\tef\tgh\tij",
        "汉字汉字汉字汉字汉字",
        "a汉b字c汉d字e汉f",
        "  mixed 汉字\tand e\u{301}e\u{301} combining   ",
        "    ",
        "\t",
        "x\ty 👨\u{200D}👩\u{200D}👧 z",
    ];

    fn each_layout(mut check: impl FnMut(&str, Metrics, usize)) {
        for s in TRICKY {
            for cols in 1..=14 {
                for tab_width in [1, 2, 3, 4, 8] {
                    let m = Metrics {
                        tab_width,
                        wrap: Some(cols),
                    };
                    check(s, m, cols);
                }
            }
        }
    }

    #[test]
    fn rows_cover_the_line_exactly_and_fit_unless_one_grapheme_cannot() {
        each_layout(|s, m, cols| {
            let rows = wrap_line(s, cols, m.tab_width);
            assert_eq!(rows[0].chars.start, 0);
            assert_eq!(rows[0].cols.start, 0);
            for pair in rows.windows(2) {
                assert_eq!(pair[0].chars.end, pair[1].chars.start, "{s:?} at {cols}");
                assert_eq!(pair[0].cols.end, pair[1].cols.start, "{s:?} at {cols}");
            }
            let last = rows.last().expect("at least one row");
            assert_eq!(last.chars.end, s.chars().count());
            let chars: Vec<char> = s.chars().collect();
            for row in &rows {
                let piece: String = chars[row.chars.clone()].iter().collect();
                let graphemes = piece.graphemes(true).count();
                assert!(
                    row.cols.len() <= cols || graphemes <= 1,
                    "{s:?} at {cols}: {row:?}"
                );
                let boundaries: Vec<usize> = s
                    .grapheme_indices(true)
                    .map(|(b, _)| s[..b].chars().count())
                    .chain([chars.len()])
                    .collect();
                assert!(boundaries.contains(&row.chars.start), "{s:?} {row:?}");
                assert!(boundaries.contains(&row.chars.end), "{s:?} {row:?}");
            }
        });
    }

    #[test]
    fn every_cursor_position_survives_the_trip_to_the_screen_and_back() {
        each_layout(|s, m, cols| {
            let t = Rope::from_str(s);
            let mut offset = 0;
            let mut graphemes: Vec<usize> = s
                .graphemes(true)
                .map(|g| {
                    offset += g.chars().count();
                    offset
                })
                .collect();
            graphemes.insert(0, 0);
            for idx in graphemes {
                let (line, row, col) = m.position(&t, idx);
                assert_eq!(
                    m.idx_at(&t, line, row, col),
                    idx,
                    "{s:?} cols {cols} tab {} ",
                    m.tab_width
                );
                let width: usize = s
                    .graphemes(true)
                    .map(UnicodeWidthStr::width)
                    .max()
                    .unwrap_or(0);
                if width <= cols && m.tab_width <= cols {
                    assert!(col < cols, "{s:?} cols {cols}: cursor at column {col}");
                }
            }
        });
    }

    #[test]
    fn stepping_down_visits_every_row_once_and_up_returns() {
        let text = "first line\n\nsecond\tline that wraps\n汉字汉字汉字汉字\nlast";
        let t = Rope::from_str(text);
        for cols in 1..=14 {
            let m = Metrics {
                tab_width: 4,
                wrap: Some(cols),
            };
            let total: usize = (0..t.len_lines()).map(|l| m.rows(&t, l).len()).sum();
            let mut at = (0, 0);
            let mut seen = vec![at];
            while m.step(&t, at, 1) != at {
                at = m.step(&t, at, 1);
                seen.push(at);
            }
            assert_eq!(seen.len(), total, "cols {cols}");
            let mut sorted = seen.clone();
            sorted.dedup();
            assert_eq!(sorted, seen, "no row twice, in order");
            assert_eq!(m.step(&t, at, -(total as isize)), (0, 0));
            for (i, row) in seen.iter().enumerate() {
                assert_eq!(m.step(&t, (0, 0), i as isize), *row);
            }
        }
    }

    #[test]
    fn a_column_past_the_end_of_a_row_stays_on_that_row() {
        let t = Rope::from_str("one two three");
        let m = Metrics {
            tab_width: 4,
            wrap: Some(8),
        };
        assert_eq!(m.rows(&t, 0).len(), 2);
        assert_eq!(m.idx_at(&t, 0, 0, 50), 7);
        assert_eq!(m.idx_at(&t, 0, 1, 50), 13, "the last row reaches the end");
    }

    #[test]
    fn follow_scrolls_as_little_as_possible() {
        let t = Rope::from_str("abcdefgh\nabcdefgh\nabcdefgh\nabcdefgh\n");
        let m = Metrics {
            tab_width: 4,
            wrap: Some(4),
        };
        assert_eq!(m.follow(&t, (1, 1), (1, 0), 3), (1, 0), "up, to the cursor");
        assert_eq!(m.follow(&t, (0, 0), (0, 2), 3), (0, 0), "already visible");
        assert_eq!(m.follow(&t, (0, 0), (1, 0), 3), (0, 1), "one row down");
        assert_eq!(m.follow(&t, (0, 0), (3, 2), 3), (3, 0), "far below");
    }

    #[test]
    fn follow_copes_with_a_scroll_position_past_a_line_that_got_shorter() {
        let t = Rope::from_str("ab\ncd\n");
        let m = Metrics {
            tab_width: 4,
            wrap: Some(4),
        };
        assert_eq!(m.follow(&t, (0, 5), (1, 0), 5), (0, 0));
    }
}
