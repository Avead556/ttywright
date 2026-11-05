use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear};
use unicode_width::UnicodeWidthStr;

use super::mouse::Hit;
use super::render::Ctx;
use super::run::hard_wrap;
use super::text::ellipsize;
use crate::core::Focus;
use crate::core::db::{DbGrid, DbOutcome, DbPanel, DbSection, clean_cell};

const LIST_WIDTH: u16 = 28;
const SIDE_BY_SIDE: u16 = 60;
const MAX_COLUMN: usize = 40;

fn row(area: Rect, y: u16) -> Rect {
    Rect::new(area.x, area.y + y, area.width, 1).intersection(area)
}

pub fn render_database(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let ws = cx.ws;
    let panel = ws.db_panel();
    let focused = ws.focus() == Focus::Database && ws.picker().is_none();
    let title_style = if focused {
        Style::new()
            .fg(theme.fg_primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.fg_muted)
    };
    let mut title = vec![Span::styled(" Database", title_style)];
    if let Some(connection) = panel.active.and_then(|i| panel.connections.get(i)) {
        title.push(Span::raw(format!("   {}", clean_cell(&connection.name))).fg(theme.accent));
    }
    if panel.running.is_some() {
        title.push(Span::raw("   running…  s stop").fg(theme.accent));
    }
    if !panel.title.is_empty() {
        title.push(Span::raw(format!("   {}", panel.title)).fg(theme.fg_muted));
    }
    frame.render_widget(Line::from(title).bg(theme.bg_panel), row(area, 0));
    cx.ui.hits.push(area, Hit::DatabasePanel);
    if area.height < 2 {
        return;
    }
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    if panel.connections.is_empty() {
        let hint = " No connections: add [[database]] to your config.toml";
        frame.render_widget(Line::from(hint).fg(theme.fg_muted), row(body, 0));
        return;
    }
    if area.width < SIDE_BY_SIDE {
        if panel.section == DbSection::Results {
            render_results(frame, body, cx);
        } else {
            render_lists(frame, body, cx);
        }
        return;
    }
    let list = Rect::new(body.x, body.y, LIST_WIDTH, body.height);
    render_lists(frame, list, cx);
    let right = Rect::new(
        body.x + LIST_WIDTH + 1,
        body.y,
        body.width - LIST_WIDTH - 1,
        body.height,
    );
    render_results(frame, right, cx);
}

#[derive(Clone)]
enum ListRow {
    Heading(&'static str),
    Connection(usize),
    Table(usize),
}

fn list_rows(panel: &DbPanel) -> Vec<ListRow> {
    let mut rows = vec![ListRow::Heading("Connections")];
    rows.extend((0..panel.connections.len()).map(ListRow::Connection));
    rows.push(ListRow::Heading("Tables"));
    rows.extend((0..panel.tables.len()).map(ListRow::Table));
    rows
}

fn render_lists(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let panel = cx.ws.db_panel();
    let rows = list_rows(panel);
    let chosen = rows.iter().position(|r| match r {
        ListRow::Connection(i) => {
            panel.section == DbSection::Connections && *i == panel.selected_connection
        }
        ListRow::Table(i) => panel.section == DbSection::Tables && *i == panel.selected_table,
        ListRow::Heading(_) => false,
    });
    let room = usize::from(area.height);
    let top = chosen.map_or(0, |c| (c + 1).saturating_sub(room));
    let cols = usize::from(area.width);
    for (n, list_row) in rows.iter().skip(top).take(room).enumerate() {
        let place = row(area, u16::try_from(n).unwrap_or(0));
        let selected = chosen == Some(top + n);
        let (text, style) = match list_row {
            ListRow::Heading(name) => ((*name).to_owned(), Style::new().fg(theme.fg_muted).bold()),
            ListRow::Connection(i) => {
                cx.ui
                    .hits
                    .push(place, Hit::DatabaseConnection { index: *i });
                let mark = if panel.active == Some(*i) {
                    "● "
                } else {
                    "  "
                };
                (
                    format!("{mark}{}", clean_cell(&panel.connections[*i].name)),
                    Style::new().fg(theme.fg_primary),
                )
            }
            ListRow::Table(i) => {
                cx.ui.hits.push(place, Hit::DatabaseTable { index: *i });
                (
                    format!("  {}", panel.tables[*i].label()),
                    Style::new().fg(theme.fg_primary),
                )
            }
        };
        let style = if selected {
            style.bg(theme.bg_current_line).bold()
        } else {
            style
        };
        frame.render_widget(Line::from(ellipsize(&text, cols)).style(style), place);
    }
}

fn pad(text: &str, width: usize) -> String {
    let cut = ellipsize(text, width);
    let fill = width.saturating_sub(cut.width());
    format!("{cut}{}", " ".repeat(fill))
}

fn render_results(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let panel = cx.ws.db_panel();
    match &panel.outcome {
        None if panel.running.is_none() => {
            let hint = "Enter opens a connection or table; Alt+E runs a statement from a .sql file";
            frame.render_widget(
                Line::from(ellipsize(hint, usize::from(area.width))).fg(theme.fg_muted),
                row(area, 0),
            );
        }
        None => {}
        Some(DbOutcome::Cancelled) => {
            frame.render_widget(Line::from("Stopped").fg(theme.fg_muted), row(area, 0));
        }
        Some(DbOutcome::Failed(message)) => {
            frame.render_widget(Line::from(message.clone()).fg(theme.error), row(area, 0));
        }
        Some(DbOutcome::Done(grid)) => render_grid(frame, area, grid, panel, cx),
    }
}

fn render_grid(frame: &mut Frame, area: Rect, grid: &DbGrid, panel: &DbPanel, cx: &Ctx<'_>) {
    let theme = cx.theme;
    if grid.columns.is_empty() {
        let text = format!("Done, no rows returned   {} ms", grid.elapsed_ms);
        frame.render_widget(Line::from(text).fg(theme.success), row(area, 0));
        return;
    }
    let room = usize::from(area.height.saturating_sub(2));
    let visible = || grid.rows.iter().skip(panel.row).take(room);
    let cols = usize::from(area.width);
    let width_of = |c: usize| {
        visible()
            .filter_map(|r| r.get(c))
            .map(|v| v.width())
            .chain(std::iter::once(grid.columns[c].width()))
            .max()
            .unwrap_or(0)
            .clamp(1, MAX_COLUMN)
    };
    let widths: Vec<usize> = (0..grid.columns.len()).map(width_of).collect();
    let line_of = |cells: &[String]| {
        let mut text = String::new();
        for (c, width) in widths.iter().enumerate().skip(panel.column) {
            if text.width() >= cols {
                break;
            }
            if c > panel.column {
                text.push_str(" │ ");
            }
            text.push_str(&pad(cells.get(c).map_or("", String::as_str), *width));
        }
        ellipsize(&text, cols)
    };
    frame.render_widget(
        Line::from(line_of(&grid.columns)).fg(theme.accent).bold(),
        row(area, 0),
    );
    for (n, cells) in visible().enumerate() {
        frame.render_widget(
            Line::from(line_of(cells)).fg(theme.fg_primary),
            row(area, 1 + u16::try_from(n).unwrap_or(0)),
        );
    }
    let cut = if grid.truncated {
        "  more exist, not kept"
    } else {
        ""
    };
    let foot = format!(
        "row {}/{}  column {}/{}  {} ms{cut}",
        (panel.row + 1).min(grid.rows.len()),
        grid.rows.len(),
        panel.column + 1,
        grid.columns.len(),
        grid.elapsed_ms
    );
    if area.height >= 2 {
        frame.render_widget(
            Line::from(ellipsize(&foot, cols)).fg(if grid.truncated {
                theme.warning
            } else {
                theme.fg_muted
            }),
            row(area, area.height - 1),
        );
    }
}

pub fn render_db_confirm(frame: &mut Frame, screen: Rect, cx: &mut Ctx<'_>) {
    let panel = cx.ws.db_panel();
    let Some(confirm) = &panel.confirm else {
        return;
    };
    let theme = cx.theme;
    let name = panel
        .connections
        .get(confirm.connection)
        .map_or_else(String::new, |c| clean_cell(&c.name));
    let width = screen.width.saturating_sub(4).min(90);
    let cols = usize::from(width.saturating_sub(4)).max(1);
    let rows: Vec<String> = confirm
        .statement
        .lines()
        .flat_map(|l| hard_wrap(&clean_cell(l), cols))
        .collect();
    let height = u16::try_from(rows.len())
        .unwrap_or(u16::MAX)
        .saturating_add(6)
        .min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + (screen.width - width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    );
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.warning))
        .style(Style::new().bg(theme.bg_popup))
        .title(Line::from(" Run Statement ").fg(theme.fg_primary).bold());
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    cx.ui.hits.push(area, Hit::Picker);
    if inner.height < 4 {
        return;
    }
    let inner = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    );
    let line = |y: u16| Rect::new(inner.x, inner.y + y, inner.width, 1);
    frame.render_widget(
        Line::from(ellipsize(&format!("This may change data in {name}:"), cols)).fg(theme.warning),
        line(0),
    );
    let room = usize::from(inner.height.saturating_sub(3));
    let last_top = rows.len().saturating_sub(room);
    let top = confirm.scroll.min(last_top);
    for (i, text) in rows.iter().skip(top).take(room).enumerate() {
        frame.render_widget(
            Line::from(text.clone()).fg(theme.fg_primary),
            line(2 + u16::try_from(i).unwrap_or(0)),
        );
    }
    let more = if top < last_top {
        Span::raw(format!("↓ {} more rows  ", last_top - top))
            .fg(theme.warning)
            .bold()
    } else {
        Span::raw("")
    };
    frame.render_widget(
        Line::from(vec![
            more,
            Span::raw("y").fg(theme.accent).bold(),
            Span::raw(" run  any other key: cancel").fg(theme.fg_muted),
        ]),
        line(inner.height - 1),
    );
}
