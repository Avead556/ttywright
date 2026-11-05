use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Clear;

use super::glyphs;
use super::mouse::Hit;
use super::render::Ctx;
use super::text::ellipsize;
use crate::core::merge::{MergeCell, MergeView};

const NUMBER_WIDTH: usize = 5;
const MIN_WIDTH: u16 = 60;

pub fn render_merge(frame: &mut Frame, screen: Rect, cx: &mut Ctx<'_>) {
    let Some(view) = cx.ws.merge_view() else {
        return;
    };
    let theme = cx.theme;
    frame.render_widget(Clear, screen);
    frame
        .buffer_mut()
        .set_style(screen, Style::new().bg(theme.bg));
    cx.ui.hits.push(screen, Hit::History);
    let [title, heads, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(screen);
    let count = view.conflict_count();
    let open = view.open_count();
    let progress = if open == 0 {
        "all decided: ⏎ marks it resolved".to_owned()
    } else {
        format!("conflict {} of {count}, {open} open", view.selected + 1)
    };
    frame.render_widget(
        Line::from(vec![
            Span::raw(format!(" {}  ", view.file))
                .fg(theme.accent)
                .bold(),
            Span::raw(progress).fg(theme.fg_muted),
        ])
        .bg(theme.bg_panel),
        title,
    );
    let foot = match &view.notice {
        Some(n) => Line::from(format!(" {}", n.text)).fg(if n.error { theme.error } else { theme.success }),
        None => Line::from(
            " O ours  T theirs  B both  R both, theirs first  U undo  N/P conflict  E edit by hand  ⏎ resolved  Esc close",
        )
        .fg(theme.fg_muted),
    };
    frame.render_widget(foot.bg(theme.bg_panel), footer);
    if screen.width < MIN_WIDTH {
        frame.render_widget(
            Line::from(" Too narrow for three panes: E edits by hand").fg(theme.fg_muted),
            body,
        );
        return;
    }
    let [left, line_a, middle, line_b, right] = Layout::horizontal([
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(body);
    render_heads(frame, heads, [left, middle, right], view, cx);
    for line in [line_a, line_b] {
        for y in line.y..line.bottom() {
            frame.render_widget(
                Line::from(glyphs::V_LINE).fg(theme.border),
                Rect::new(line.x, y, 1, 1),
            );
        }
    }
    cx.ui.page_rows = usize::from(body.height);
    let last = view.rows.len().saturating_sub(usize::from(body.height));
    let rows = view.rows.iter().skip(view.scroll.min(last));
    for (y, row) in (body.y..body.bottom()).zip(rows) {
        let chosen = row.conflict == Some(view.selected);
        let ours = row.ours.as_ref().map(|(n, text)| (Some(*n), text.as_str()));
        let result = row.result.as_ref().map(|(n, text)| (*n, text.as_str()));
        let theirs = row
            .theirs
            .as_ref()
            .map(|(n, text)| (Some(*n), text.as_str()));
        for (pane, cell, kind) in [
            (left, ours, row.cells[0]),
            (middle, result, row.cells[1]),
            (right, theirs, row.cells[2]),
        ] {
            let area = Rect::new(pane.x, y, pane.width, 1);
            render_cell(frame, area, cell, kind, chosen, view, cx);
        }
    }
}

fn render_heads(frame: &mut Frame, area: Rect, panes: [Rect; 3], view: &MergeView, cx: &Ctx<'_>) {
    let theme = cx.theme;
    let name = |side: &str, who: &str| {
        if who.is_empty() {
            side.to_owned()
        } else {
            format!("{side}: {who}")
        }
    };
    let titles = [
        name("Ours", &view.ours_name),
        "Result".to_owned(),
        name("Theirs", &view.theirs_name),
    ];
    for (pane, title) in panes.iter().zip(titles) {
        let cell = Rect::new(pane.x, area.y, pane.width, 1);
        let shown = ellipsize(&title, usize::from(pane.width).saturating_sub(1));
        frame.render_widget(
            Line::from(format!(" {shown}")).fg(theme.fg_primary).bold(),
            cell,
        );
    }
}

fn render_cell(
    frame: &mut Frame,
    area: Rect,
    cell: Option<(Option<usize>, &str)>,
    kind: MergeCell,
    chosen: bool,
    view: &MergeView,
    cx: &Ctx<'_>,
) {
    let theme = cx.theme;
    let style = match kind {
        MergeCell::Plain => Style::new().fg(theme.fg_primary),
        MergeCell::Open => theme.diff_removed,
        MergeCell::Taken => theme.diff_added,
        MergeCell::Dropped => Style::new().fg(theme.fg_subtle),
        MergeCell::Base => Style::new().fg(theme.fg_muted).italic(),
        MergeCell::Undecided => Style::new().fg(theme.warning),
    };
    let bar = if chosen { glyphs::SELECTED } else { " " };
    let Some((number, text)) = cell else {
        let empty = if kind == MergeCell::Plain {
            Style::new()
        } else {
            style
        };
        frame.render_widget(
            Line::from(vec![Span::raw(bar).fg(theme.accent)]).style(empty),
            area,
        );
        return;
    };
    let number = number.map_or_else(
        || " ".repeat(NUMBER_WIDTH - 1),
        |n| format!("{n:>width$}", width = NUMBER_WIDTH - 1),
    );
    let text = text.replace('\t', "    ");
    let room = usize::from(area.width).saturating_sub(NUMBER_WIDTH + 1);
    let shown: String = text.chars().skip(view.column).collect();
    frame.render_widget(
        Line::from(vec![
            Span::raw(bar).fg(theme.accent),
            Span::raw(format!("{number} ")).fg(theme.fg_muted),
            Span::raw(ellipsize(&shown, room)),
        ])
        .style(style),
        area,
    );
}
