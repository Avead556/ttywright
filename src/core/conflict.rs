use ropey::Rope;

use super::text;

const MARKER: usize = 7;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ConflictRegion {
    pub start: usize,
    pub base: Option<usize>,
    pub middle: usize,
    pub end: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConflictChoice {
    Ours,
    Theirs,
    Both,
    TheirsThenOurs,
}

fn is_marker(line: &str, c: char) -> bool {
    let run = line.chars().take_while(|l| *l == c).count();
    run == MARKER && line[MARKER..].chars().next().is_none_or(|n| n == ' ')
}

fn is_separator(line: &str) -> bool {
    line == "======="
}

pub fn is_conflict_marker(line: &str) -> bool {
    is_marker(line, '<') || is_marker(line, '>') || is_marker(line, '|') || is_separator(line)
}

pub fn conflict_regions(text_of: &Rope) -> Vec<ConflictRegion> {
    let mut regions = Vec::new();
    let mut open: Option<(usize, Option<usize>, Option<usize>)> = None;
    for number in 0..text_of.len_lines() {
        let first = text_of.line(number).chars().next();
        if !matches!(first, Some('<' | '=' | '|' | '>')) {
            continue;
        }
        let line = text::line_content(text_of, number);
        if is_marker(&line, '<') {
            open = Some((number, None, None));
        } else if let Some((start, base, middle)) = open {
            if middle.is_none() && base.is_none() && is_marker(&line, '|') {
                open = Some((start, Some(number), None));
            } else if middle.is_none() && is_separator(&line) {
                open = Some((start, base, Some(number)));
            } else if let (Some(middle), true) = (middle, is_marker(&line, '>')) {
                regions.push(ConflictRegion {
                    start,
                    base,
                    middle,
                    end: number,
                });
                open = None;
            }
        }
    }
    regions
}

pub fn conflict_at(regions: &[ConflictRegion], line: usize) -> Option<ConflictRegion> {
    regions.iter().find(|r| line <= r.end).copied()
}

pub fn step_conflict(regions: &[ConflictRegion], line: usize, forward: bool) -> Option<usize> {
    if forward {
        regions
            .iter()
            .find(|r| r.start > line)
            .or_else(|| regions.first())
    } else {
        regions
            .iter()
            .rev()
            .find(|r| r.start < line)
            .or_else(|| regions.last())
    }
    .map(|r| r.start)
}

pub fn resolution(
    text_of: &Rope,
    region: &ConflictRegion,
    choice: ConflictChoice,
) -> (usize, usize, String) {
    let at = |line: usize| text_of.line_to_char(line.min(text_of.len_lines()));
    let ours_end = region.base.unwrap_or(region.middle);
    let ours = text_of.slice(at(region.start + 1)..at(ours_end));
    let theirs = text_of.slice(at(region.middle + 1)..at(region.end));
    let kept = match choice {
        ConflictChoice::Ours => ours.to_string(),
        ConflictChoice::Theirs => theirs.to_string(),
        ConflictChoice::Both => format!("{ours}{theirs}"),
        ConflictChoice::TheirsThenOurs => format!("{theirs}{ours}"),
    };
    (at(region.start), at(region.end + 1), kept)
}

#[cfg(test)]
mod tests {
    use super::*;

    const ONE: &str = "top\n<<<<<<< HEAD\nmine\n=======\nyours\n>>>>>>> other\nbottom\n";

    fn apply(text_of: &str, choice: ConflictChoice) -> String {
        let mut rope = Rope::from_str(text_of);
        let region = conflict_regions(&rope)[0];
        let (from, to, kept) = resolution(&rope, &region, choice);
        rope.remove(from..to);
        rope.insert(from, &kept);
        rope.to_string()
    }

    #[test]
    fn each_choice_leaves_the_right_text_and_no_markers() {
        assert_eq!(apply(ONE, ConflictChoice::Ours), "top\nmine\nbottom\n");
        assert_eq!(apply(ONE, ConflictChoice::Theirs), "top\nyours\nbottom\n");
        assert_eq!(
            apply(ONE, ConflictChoice::Both),
            "top\nmine\nyours\nbottom\n"
        );
    }

    #[test]
    fn the_base_of_diff3_style_goes_with_every_choice() {
        let text = "<<<<<<< HEAD\nmine\n||||||| base\nold\n=======\nyours\n>>>>>>> o\n";
        assert_eq!(apply(text, ConflictChoice::Ours), "mine\n");
        assert_eq!(apply(text, ConflictChoice::Theirs), "yours\n");
        assert_eq!(apply(text, ConflictChoice::Both), "mine\nyours\n");
    }

    #[test]
    fn crlf_lines_keep_their_endings() {
        let text = "a\r\n<<<<<<< HEAD\r\nmine\r\n=======\r\nyours\r\n>>>>>>> o\r\nb\r\n";
        assert_eq!(apply(text, ConflictChoice::Ours), "a\r\nmine\r\nb\r\n");
    }

    #[test]
    fn a_last_line_without_a_newline_is_taken_whole() {
        let text = "<<<<<<< HEAD\nmine\n=======\nyours\n>>>>>>> o";
        assert_eq!(apply(text, ConflictChoice::Theirs), "yours\n");
    }

    #[test]
    fn an_empty_side_leaves_nothing() {
        let text = "<<<<<<< HEAD\n=======\nyours\n>>>>>>> o\n";
        assert_eq!(apply(text, ConflictChoice::Ours), "");
    }

    #[test]
    fn things_that_only_look_like_markers_are_not_conflicts() {
        for text in [
            "<<<<<<<< eight\n=======\n>>>>>>> o\n",
            "<<<<<<<x HEAD\n=======\n>>>>>>> o\n",
            "<<<<<<< no end\n=======\ntext\n",
            "======= alone\n>>>>>>> alone\n",
            "<<<<<<< HEAD\n======== long\n>>>>>>> o\n",
            "  <<<<<<< indented\n=======\n>>>>>>> o\n",
        ] {
            assert!(conflict_regions(&Rope::from_str(text)).is_empty(), "{text}");
        }
        let text = "<<<<<<< H\na\n=======\nb\n=======\nc\n>>>>>>> o\n";
        assert_eq!(apply(text, ConflictChoice::Theirs), "b\n=======\nc\n");
    }

    #[test]
    fn stepping_goes_round_and_the_cursor_picks_its_conflict() {
        let text = format!("x\n{ONE}y\n{ONE}");
        let regions = conflict_regions(&Rope::from_str(&text));
        assert_eq!(regions.len(), 2);
        let (a, b) = (regions[0], regions[1]);
        assert_eq!(step_conflict(&regions, 0, true), Some(a.start));
        assert_eq!(step_conflict(&regions, a.start, true), Some(b.start));
        assert_eq!(step_conflict(&regions, b.start, true), Some(a.start));
        assert_eq!(step_conflict(&regions, a.start, false), Some(b.start));
        assert_eq!(step_conflict(&[], 3, true), None);
        assert_eq!(conflict_at(&regions, a.middle), Some(a));
        assert_eq!(conflict_at(&regions, a.end + 1), Some(b));
        assert_eq!(conflict_at(&regions, b.end + 1), None);
    }

    #[test]
    fn marker_lines_are_recognized_for_coloring() {
        assert!(is_conflict_marker("<<<<<<< HEAD"));
        assert!(is_conflict_marker("======="));
        assert!(is_conflict_marker(">>>>>>> a"));
        assert!(!is_conflict_marker("========"));
        assert!(!is_conflict_marker("<<<<<<"));
    }
}
