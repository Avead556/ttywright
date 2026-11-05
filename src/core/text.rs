use ropey::Rope;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

pub fn next_grapheme_boundary(text: &Rope, idx: usize) -> usize {
    if idx >= text.len_chars() {
        return text.len_chars();
    }
    let line = text.char_to_line(idx);
    let start = text.line_to_char(line);
    let offset = idx - start;
    let mut boundary = 0;
    for g in text.line(line).to_string().graphemes(true) {
        boundary += g.chars().count();
        if boundary > offset {
            break;
        }
    }
    start + boundary
}

pub fn prev_grapheme_boundary(text: &Rope, idx: usize) -> usize {
    if idx == 0 {
        return 0;
    }
    let idx = idx.min(text.len_chars());
    let line = text.char_to_line(idx - 1);
    let start = text.line_to_char(line);
    let offset = idx - start;
    let mut boundary = 0;
    for g in text.line(line).to_string().graphemes(true) {
        let next = boundary + g.chars().count();
        if next >= offset {
            break;
        }
        boundary = next;
    }
    start + boundary
}

pub fn word_motion_len(chars: impl Iterator<Item = char>) -> usize {
    let mut len = 0;
    let mut in_word = false;
    for c in chars {
        let is_word = c.is_alphanumeric() || c == '_';
        if in_word && !is_word {
            break;
        }
        in_word |= is_word;
        len += 1;
    }
    len
}

pub fn line_content_len(text: &Rope, line: usize) -> usize {
    let slice = text.line(line);
    let len = slice.len_chars();
    let ending = match (
        len.checked_sub(2).map(|i| slice.char(i)),
        len.checked_sub(1).map(|i| slice.char(i)),
    ) {
        (Some('\r'), Some('\n')) => 2,
        (_, Some('\n' | '\r')) => 1,
        _ => 0,
    };
    len - ending
}

pub fn line_content(text: &Rope, line: usize) -> String {
    text.line(line)
        .slice(..line_content_len(text, line))
        .to_string()
}

pub fn grapheme_width(grapheme: &str, col: usize, tab_width: usize) -> usize {
    if grapheme == "\t" {
        tab_width - col % tab_width
    } else {
        grapheme.width()
    }
}

pub fn col_of(text: &Rope, idx: usize, tab_width: usize) -> usize {
    let line = text.char_to_line(idx);
    let offset = idx - text.line_to_char(line);
    let mut chars = 0;
    let mut col = 0;
    for g in line_content(text, line).graphemes(true) {
        if chars >= offset {
            break;
        }
        chars += g.chars().count();
        col += grapheme_width(g, col, tab_width);
    }
    col
}

pub fn idx_at_col(text: &Rope, line: usize, col: usize, tab_width: usize) -> usize {
    let start = text.line_to_char(line);
    let mut chars = 0;
    let mut cur = 0;
    for g in line_content(text, line).graphemes(true) {
        let w = grapheme_width(g, cur, tab_width);
        if cur + w > col {
            break;
        }
        cur += w;
        chars += g.chars().count();
    }
    start + chars
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn crlf_is_one_step() {
        let t = Rope::from_str("ab\r\ncd");
        assert_eq!(next_grapheme_boundary(&t, 2), 4);
        assert_eq!(prev_grapheme_boundary(&t, 4), 2);
    }

    #[test]
    fn steps_over_combining_and_zwj_clusters() {
        let t = Rope::from_str("e\u{301}👨\u{200D}👩\u{200D}👧x");
        assert_eq!(next_grapheme_boundary(&t, 0), 2);
        assert_eq!(next_grapheme_boundary(&t, 2), 7);
        assert_eq!(prev_grapheme_boundary(&t, 7), 2);
        assert_eq!(prev_grapheme_boundary(&t, 2), 0);
    }

    #[test]
    fn boundaries_clamp_at_document_edges() {
        let t = Rope::from_str("ab");
        assert_eq!(prev_grapheme_boundary(&t, 0), 0);
        assert_eq!(next_grapheme_boundary(&t, 2), 2);
    }

    #[test]
    fn line_content_strips_any_line_ending() {
        let t = Rope::from_str("one\r\ntwo\nthree");
        assert_eq!(line_content(&t, 0), "one");
        assert_eq!(line_content(&t, 1), "two");
        assert_eq!(line_content(&t, 2), "three");
    }

    #[test]
    fn columns_account_for_tabs_and_wide_chars() {
        let t = Rope::from_str("\tx界y");
        assert_eq!(col_of(&t, 1, 4), 4);
        assert_eq!(col_of(&t, 2, 4), 5);
        assert_eq!(col_of(&t, 3, 4), 7);
    }

    #[test]
    fn idx_at_col_snaps_to_grapheme_start_and_clamps() {
        let t = Rope::from_str("a界b\nshort");
        assert_eq!(
            idx_at_col(&t, 0, 2, 4),
            1,
            "column inside a wide char maps to its start"
        );
        assert_eq!(idx_at_col(&t, 0, 3, 4), 2);
        assert_eq!(
            idx_at_col(&t, 0, 99, 4),
            3,
            "clamped before the line ending"
        );
        assert_eq!(idx_at_col(&t, 1, 99, 4), 9);
    }
}
