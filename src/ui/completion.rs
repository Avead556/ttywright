use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear};
use unicode_width::UnicodeWidthStr;

use super::render::{Ctx, ensure_visible};
use super::text::highlighted;

const MAX_ROWS: usize = 10;
const MIN_WIDTH: usize = 20;
const MAX_WIDTH: usize = 64;

pub fn render_completion(frame: &mut Frame, screen: Rect, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let (Some(popup), Some(cursor)) = (cx.ws.completion(), cx.ui.editor_cursor) else {
        return;
    };
    let widest = popup
        .matches
        .iter()
        .filter_map(|m| popup.items.get(m.index))
        .map(|i| i.label.width() + i.detail.as_ref().map_or(0, |d| d.width() + 2))
        .max()
        .unwrap_or(0);
    let room = usize::from(screen.width.saturating_sub(4));
    let text_width = widest.clamp(MIN_WIDTH, MAX_WIDTH).min(room);

    let wanted = popup.matches.len().min(MAX_ROWS);
    let below = usize::from(screen.bottom().saturating_sub(cursor.y + 2));
    let above = usize::from(cursor.y.saturating_sub(screen.y));
    let (visible, y) = if below >= wanted + 2 || below >= above {
        (wanted.min(below.saturating_sub(2)), cursor.y + 1)
    } else {
        let visible = wanted.min(above.saturating_sub(2));
        (visible, cursor.y.saturating_sub(visible as u16 + 2))
    };
    if visible == 0 || text_width == 0 {
        return;
    }
    let width = text_width as u16 + 4;
    let x = cursor.x.min(screen.right().saturating_sub(width));
    let area = Rect::new(x, y, width, visible as u16 + 2).intersection(screen);

    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.border_popup))
        .style(Style::new().bg(theme.bg_popup));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);

    let scroll = ensure_visible(
        (cx.ui.completion_scroll, 0),
        (popup.selected, 0),
        (visible, 1),
    )
    .0;
    cx.ui.completion_scroll = scroll;
    let base = Style::new().fg(theme.fg_primary);
    let hit = Style::new().fg(theme.accent).add_modifier(Modifier::BOLD);
    for (n, m) in popup.matches.iter().enumerate().skip(scroll).take(visible) {
        let Some(item) = popup.items.get(m.index) else {
            continue;
        };
        let row = Rect::new(inner.x, inner.y + (n - scroll) as u16, inner.width, 1);
        let style = if n == popup.selected {
            theme.selected
        } else {
            Style::new().bg(theme.bg_popup)
        };
        let indices: &[u32] = if item.filter == item.label {
            &m.indices
        } else {
            &[]
        };
        let mut spans = vec![Span::raw(" ")];
        spans.extend(highlighted(
            &item.label,
            indices,
            base,
            hit,
            cx.ws.tab_width(),
        ));
        frame.render_widget(Line::from(spans).style(style), row);
        if let Some(detail) = &item.detail {
            let free = text_width.saturating_sub(item.label.width() + 2);
            if free > 0 && detail.width() <= free {
                let right = Line::from(format!("{detail} ")).fg(theme.fg_muted);
                frame.render_widget(right.right_aligned(), row);
            }
        }
    }
}
