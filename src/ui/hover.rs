use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear};
use unicode_segmentation::UnicodeSegmentation;
use unicode_width::UnicodeWidthStr;

use super::render::Ctx;

const MAX_WIDTH: usize = 80;
const MAX_ROWS: usize = 12;
const HEADER_ROWS: u16 = 4;

fn wrap(text: &str, width: usize) -> Vec<String> {
    let mut rows = Vec::new();
    let mut row = String::new();
    let mut row_width = 0;
    let mut last_space = None;
    for g in text.graphemes(true) {
        let w = g.width();
        if row_width + w > width && !row.is_empty() {
            let rest = last_space.map_or(String::new(), |at: usize| row.split_off(at + 1));
            rows.push(row.trim_end().to_owned());
            row_width = rest.width();
            row = rest;
            last_space = None;
        }
        if g == " " && !row.trim().is_empty() {
            last_space = Some(row.len());
        }
        row.push_str(g);
        row_width += w;
    }
    rows.push(row);
    rows
}

pub fn render_hover(frame: &mut Frame, screen: Rect, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let (Some(hover), Some(cursor)) = (cx.ws.hover(), cx.ui.editor_cursor) else {
        return;
    };
    let room = usize::from(screen.width.saturating_sub(4)).min(MAX_WIDTH);
    if room < 10 {
        return;
    }
    let widest = hover
        .lines
        .iter()
        .map(|l| l.text.width())
        .max()
        .unwrap_or(0);
    let text_width = widest.clamp(20, room);
    let rows: Vec<(String, bool)> = hover
        .lines
        .iter()
        .flat_map(|l| wrap(&l.text, text_width).into_iter().map(|r| (r, l.code)))
        .collect();

    let above = usize::from(cursor.y.saturating_sub(screen.y + HEADER_ROWS));
    let below = usize::from((screen.bottom().saturating_sub(cursor.y + 2)).saturating_sub(1));
    let wanted = rows.len().min(MAX_ROWS);
    let visible = if above >= wanted + 2 {
        wanted
    } else {
        wanted.min(above.max(below).saturating_sub(2))
    };
    if visible == 0 {
        return;
    }
    let height = visible as u16 + 2;
    let y = if above >= usize::from(height) || above >= below {
        cursor.y.saturating_sub(height)
    } else {
        cursor.y + 1
    };
    let width = text_width as u16 + 4;
    let x = cursor.x.min(screen.right().saturating_sub(width));
    let area = Rect::new(x, y, width, height).intersection(screen);

    let last_start = rows.len() - visible;
    let scroll = hover.scroll.min(last_start);
    let mut block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.border_popup))
        .style(Style::new().bg(theme.bg_popup));
    if rows.len() > visible {
        let footer = format!(" ↑↓ {}/{} ", scroll + visible, rows.len());
        block = block.title_bottom(Line::from(footer).fg(theme.fg_muted).right_aligned());
    }
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    for (n, (text, code)) in rows.iter().skip(scroll).take(visible).enumerate() {
        let color = if *code {
            theme.accent
        } else {
            theme.fg_primary
        };
        let row = Rect::new(
            inner.x + 1,
            inner.y + n as u16,
            inner.width.saturating_sub(2),
            1,
        );
        frame.render_widget(Line::from(text.as_str()).fg(color), row);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_prefers_spaces_and_breaks_long_words() {
        assert_eq!(wrap("one two three", 8), ["one two", "three"]);
        assert_eq!(wrap("abcdefgh", 3), ["abc", "def", "gh"]);
        assert_eq!(wrap("", 5), [""]);
        assert_eq!(wrap("    deep code", 9), ["    deep", "code"]);
        assert_eq!(wrap("ыыы", 2), ["ыы", "ы"]);
    }
}
