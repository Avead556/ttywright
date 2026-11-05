use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Clear;

use super::glyphs;
use super::mouse::Hit;
use super::render::Ctx;
use super::text::ellipsize;
use crate::core::{DiffKind, DiffRow, LocalHistoryPanel};

const LIST_WIDTH: u16 = 34;

pub fn render_history(frame: &mut Frame, screen: Rect, cx: &mut Ctx<'_>) {
    let Some(panel) = cx.ws.local_history() else {
        return;
    };
    frame.render_widget(Clear, screen);
    cx.ui.hits.push(screen, Hit::History);
    let [title, rule, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(screen);
    let theme = cx.theme;
    let name = cx
        .ws
        .document(panel.doc)
        .map(|d| cx.ws.display_path(d.path()).display().to_string())
        .unwrap_or_default();
    frame.render_widget(
        Line::from(format!(" Local History — {name}"))
            .fg(theme.fg_primary)
            .bold()
            .bg(theme.bg_panel),
        title,
    );
    frame.render_widget(
        Line::from(glyphs::H_LINE.repeat(usize::from(rule.width)))
            .fg(theme.border)
            .bg(theme.bg),
        rule,
    );
    frame.render_widget(
        Line::from(" ↑↓ version   PgUp/PgDn scroll   R revert to this version   Esc close")
            .fg(theme.fg_muted)
            .bg(theme.bg_panel),
        footer,
    );
    let list_width = LIST_WIDTH.min(body.width / 2);
    let [list, divider, diff] = Layout::horizontal([
        Constraint::Length(list_width),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(body);
    render_list(frame, list, panel, cx);
    for y in divider.y..divider.bottom() {
        frame.render_widget(
            Line::from(glyphs::V_LINE).fg(theme.border).bg(theme.bg),
            Rect::new(divider.x, y, 1, 1),
        );
    }
    render_diff(frame, diff, panel, cx);
    cx.ui.page_rows = usize::from(diff.height);
}

fn render_list(frame: &mut Frame, area: Rect, panel: &LocalHistoryPanel, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let rows = usize::from(area.height);
    let first = (panel.selected + 1).saturating_sub(rows);
    for (n, entry) in panel.entries.iter().enumerate().skip(first).take(rows) {
        let row = Rect::new(area.x, area.y + (n - first) as u16, area.width, 1);
        cx.ui.hits.push(row, Hit::HistoryEntry { index: n });
        let selected = n == panel.selected;
        let style = if selected {
            theme.selected
        } else {
            Style::new().bg(theme.bg_panel)
        };
        let marker = if selected {
            Span::raw(glyphs::SELECTED).fg(theme.accent)
        } else {
            Span::raw(" ")
        };
        let age = format!("{:>9}  ", age(panel.now, entry.timestamp));
        let room = usize::from(area.width).saturating_sub(1 + age.chars().count());
        let line = Line::from(vec![
            marker,
            Span::raw(age).fg(theme.fg_muted),
            Span::raw(ellipsize(entry.label.title(), room)).fg(theme.fg_primary),
        ]);
        frame.render_widget(line.style(style), row);
    }
}

fn render_diff(frame: &mut Frame, area: Rect, panel: &LocalHistoryPanel, cx: &Ctx<'_>) {
    let theme = cx.theme;
    for y in area.y..area.bottom() {
        frame.render_widget(
            Line::default().bg(theme.bg),
            Rect::new(area.x, y, area.width, 1),
        );
    }
    let message = if let Some(error) = &panel.error {
        Some(error.as_str())
    } else if panel.version.is_none() {
        Some("Loading…")
    } else if panel.diff.is_empty() {
        Some("Same as the current text")
    } else {
        None
    };
    if let Some(message) = message {
        frame.render_widget(Line::from(format!(" {message}")).fg(theme.fg_muted), area);
        return;
    }
    let rows = panel.diff.iter().skip(panel.diff_scroll);
    for (y, row) in (area.y..area.bottom()).zip(rows) {
        frame.render_widget(diff_line(row, cx), Rect::new(area.x, y, area.width, 1));
    }
}

pub(super) fn diff_line(row: &DiffRow, cx: &Ctx<'_>) -> Line<'static> {
    let theme = cx.theme;
    let number = |n: Option<usize>| n.map_or_else(|| "     ".to_owned(), |n| format!("{n:>5}"));
    let (sign, style) = match row.kind {
        DiffKind::Same => (" ", Style::new().fg(theme.fg_primary)),
        DiffKind::Removed => ("-", theme.diff_removed),
        DiffKind::Added => ("+", theme.diff_added),
        DiffKind::Gap => {
            return Line::from("   ⋯").fg(theme.fg_subtle);
        }
        DiffKind::Section => return Line::from(format!(" {}", row.text)).fg(theme.accent).bold(),
        DiffKind::Hunk => return Line::from(format!(" {}", row.text)).fg(theme.fg_muted),
    };
    let numbers = format!("{} {} ", number(row.old_line), number(row.new_line));
    let text = row.text.replace('\t', "    ");
    Line::from(vec![
        Span::raw(numbers).fg(theme.fg_muted),
        Span::raw(format!("{sign} {text}")),
    ])
    .style(style)
}

pub(super) fn age(now: u64, then: u64) -> String {
    let secs = now.saturating_sub(then);
    match secs {
        0..60 => "just now".to_owned(),
        60..3600 => format!("{} min ago", secs / 60),
        3600..86_400 => format!("{} h ago", secs / 3600),
        _ => format!("{} d ago", secs / 86_400),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ages_read_in_the_largest_fitting_unit() {
        assert_eq!(age(100, 100), "just now");
        assert_eq!(
            age(100, 200),
            "just now",
            "a clock going back is not negative"
        );
        assert_eq!(age(120, 0), "2 min ago");
        assert_eq!(age(7200, 0), "2 h ago");
        assert_eq!(age(172_800, 0), "2 d ago");
    }
}
