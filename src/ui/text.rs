use ratatui::style::Style;
use ratatui::text::Span;
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use crate::core::text::grapheme_width;

pub fn styled_text(
    content: &str,
    styles: &[Style],
    base: Style,
    from: usize,
    width: usize,
    tab_width: usize,
) -> Vec<Span<'static>> {
    let to = from + width;
    let mut spans: Vec<Span<'static>> = Vec::new();
    let mut col = 0;
    let mut char_idx = 0;
    for g in content.graphemes(true) {
        let w = grapheme_width(g, col, tab_width);
        let (start, end) = (col, col + w);
        let style = styles.get(char_idx).copied().unwrap_or(base);
        col = end;
        char_idx += g.chars().count();
        let invisible = g
            .chars()
            .all(|c| c.is_control() || matches!(c, '\u{2028}' | '\u{2029}'));
        if invisible && w == 0 {
            continue;
        }
        if end <= from {
            continue;
        }
        if start >= to {
            break;
        }
        let piece = if g == "\t" || invisible || start < from || end > to {
            " ".repeat(end.min(to) - start.max(from))
        } else {
            g.to_owned()
        };
        match spans.last_mut() {
            Some(last) if last.style == style => last.content.to_mut().push_str(&piece),
            _ => spans.push(Span::styled(piece, style)),
        }
    }
    spans
}

pub fn highlighted(
    s: &str,
    indices: &[u32],
    base: Style,
    hit: Style,
    tab_width: usize,
) -> Vec<Span<'static>> {
    let styles: Vec<Style> = (0..s.chars().count())
        .map(|i| {
            if indices.binary_search(&(i as u32)).is_ok() {
                hit
            } else {
                base
            }
        })
        .collect();
    styled_text(s, &styles, base, 0, s.width(), tab_width)
}

pub fn ellipsize(s: &str, width: usize) -> String {
    if s.width() <= width {
        return s.to_owned();
    }
    let mut out = String::new();
    let mut used = 0;
    for g in s.graphemes(true) {
        let w = g.width();
        if used + w + 1 > width {
            break;
        }
        out.push_str(g);
        used += w;
    }
    out.push('…');
    out
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::*;

    fn texts(spans: &[Span<'_>]) -> Vec<String> {
        spans.iter().map(|s| s.content.to_string()).collect()
    }

    #[test]
    fn groups_runs_of_equal_style() {
        let red = Style::new().red();
        let styles = [red, red, Style::new()];
        assert_eq!(
            texts(&styled_text("abc", &styles, Style::new(), 0, 10, 4)),
            ["ab", "c"]
        );
    }

    #[test]
    fn expands_tabs_and_clips_wide_chars_at_edges() {
        let base = Style::new();
        assert_eq!(texts(&styled_text("\tx", &[], base, 0, 10, 4)), ["    x"]);
        assert_eq!(texts(&styled_text("\tx", &[], base, 0, 10, 2)), ["  x"]);
        assert_eq!(texts(&styled_text("a界b", &[], base, 2, 10, 4)), [" b"]);
        assert_eq!(texts(&styled_text("a界b", &[], base, 0, 2, 4)), ["a "]);
    }

    #[test]
    fn highlights_matched_chars() {
        let hit = Style::new().fg(Color::Blue);
        let spans = highlighted("main.rs", &[0, 1], Style::new(), hit, 4);
        assert_eq!(texts(&spans), ["ma", "in.rs"]);
        assert_eq!(spans[0].style, hit);
    }

    #[test]
    fn ellipsize_keeps_short_text() {
        assert_eq!(ellipsize("short", 10), "short");
        assert_eq!(ellipsize("a long name", 6), "a lon…");
    }
}
