use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear};

use super::history::diff_line;
use super::mouse::Hit;
use super::render::Ctx;
use super::run::hard_wrap;
use super::text::ellipsize;
use crate::core::Focus;
use crate::core::cli::{CliTool, clean_block, clean_line, command_text};
use crate::core::github::{GhConfirm, GhView, GhWrite, GitHubPanel};

const FILE_LIST_WIDTH: u16 = 34;

fn row(area: Rect, y: u16) -> Rect {
    Rect::new(area.x, area.y + y, area.width, 1).intersection(area)
}

fn hints(panel: &GitHubPanel) -> &'static str {
    match panel.view {
        GhView::List => "⏎ open  Tab PRs/issues  n new PR  r refresh",
        GhView::Files => "↑↓ file  PgUp/PgDn diff  Esc back",
        GhView::Detail
            if panel
                .detail
                .as_ref()
                .is_some_and(|d| d.kind == crate::core::github::GhKind::Pr) =>
        {
            "f files  c comment  a approve  x changes  o checkout  m merge  d close  Esc back"
        }
        GhView::Detail => "c comment  d close  Esc back",
    }
}

pub fn render_github(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let ws = cx.ws;
    let panel = ws.github_panel();
    let focused = ws.focus() == Focus::GitHub && ws.picker().is_none();
    let title_style = if focused {
        Style::new()
            .fg(theme.fg_primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.fg_muted)
    };
    let mut title = vec![Span::styled(" GitHub", title_style)];
    if let Some(repo) = &panel.repo {
        title.push(Span::raw(format!("   {repo}")).fg(theme.accent));
    }
    if let Some(account) = &panel.account {
        title.push(Span::raw(format!("   as {account}")).fg(theme.fg_muted));
    }
    let tab = |label: String, on: bool| {
        if on {
            Span::raw(format!("  {label}")).fg(theme.accent).bold()
        } else {
            Span::raw(format!("  {label}")).fg(theme.fg_muted)
        }
    };
    title.push(tab(
        format!("PRs {}", panel.prs.len()),
        !panel.showing_issues,
    ));
    title.push(tab(
        format!("Issues {}", panel.issues.len()),
        panel.showing_issues,
    ));
    if panel.busy() {
        title.push(Span::raw("   working…").fg(theme.accent));
    }
    frame.render_widget(Line::from(title).bg(theme.bg_panel), row(area, 0));
    cx.ui.hits.push(area, Hit::GitHubPanel);
    if area.height < 3 {
        return;
    }
    let width = usize::from(area.width).saturating_sub(1);
    frame.render_widget(
        Line::from(format!(" {}", ellipsize(hints(panel), width))).fg(theme.fg_muted),
        row(area, 1),
    );
    let mut top = 2;
    if let Some(note) = &panel.note {
        frame.render_widget(
            Line::from(format!(" {}", ellipsize(note, width))).fg(theme.warning),
            row(area, 2),
        );
        top = 3;
    }
    if area.height <= top {
        return;
    }
    let body = Rect::new(area.x, area.y + top, area.width, area.height - top);
    if focused {
        cx.ui.page_rows = usize::from(body.height);
    }
    match panel.view {
        GhView::List => render_list(frame, body, panel, cx),
        GhView::Detail => render_detail(frame, body, panel, cx),
        GhView::Files => render_files(frame, body, panel, cx),
    }
}

fn render_list(frame: &mut Frame, area: Rect, panel: &GitHubPanel, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let items = panel.items();
    if items.is_empty() {
        let text = if panel.busy() {
            " Loading…"
        } else {
            " Nothing open"
        };
        frame.render_widget(Line::from(text).fg(theme.fg_muted), row(area, 0));
        return;
    }
    let width = usize::from(area.width).saturating_sub(1);
    let chosen = panel.selected();
    let first = (chosen + 1).saturating_sub(usize::from(area.height));
    for (y, (n, item)) in (0..area.height).zip(items.iter().enumerate().skip(first)) {
        let text = format!(
            "#{} {}   {}   {}",
            item.number, item.title, item.author, item.note
        );
        let line = Line::from(format!(" {}", ellipsize(&text, width)));
        let line = if n == chosen {
            line.style(theme.selected)
        } else {
            line.fg(theme.fg_primary)
        };
        let place = row(area, y);
        frame.render_widget(line, place);
        cx.ui.hits.push(place, Hit::GitHubRow { index: n });
    }
}

fn render_detail(frame: &mut Frame, area: Rect, panel: &GitHubPanel, cx: &Ctx<'_>) {
    let theme = cx.theme;
    let Some(detail) = &panel.detail else {
        let text = if panel.busy() {
            " Loading…"
        } else {
            " Could not be read"
        };
        frame.render_widget(Line::from(text).fg(theme.fg_muted), row(area, 0));
        return;
    };
    let cols = usize::from(area.width).saturating_sub(2).max(1);
    let rows = detail
        .lines
        .iter()
        .skip(panel.scroll)
        .flat_map(|line| hard_wrap(line, cols));
    for (y, text) in (0..area.height).zip(rows) {
        frame.render_widget(
            Line::from(format!(" {text}")).fg(theme.fg_primary),
            row(area, y),
        );
    }
}

fn render_files(frame: &mut Frame, area: Rect, panel: &GitHubPanel, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    if panel.files.is_empty() {
        let text = if panel.busy() {
            " Loading…"
        } else {
            " No changes"
        };
        frame.render_widget(Line::from(text).fg(theme.fg_muted), row(area, 0));
        return;
    }
    let [list, diff] = Layout::horizontal([
        Constraint::Length(FILE_LIST_WIDTH.min(area.width / 2)),
        Constraint::Min(1),
    ])
    .areas(area);
    let width = usize::from(list.width).saturating_sub(1);
    let first = (panel.selected_file + 1).saturating_sub(usize::from(list.height));
    for (y, (n, file)) in (0..list.height).zip(panel.files.iter().enumerate().skip(first)) {
        let text = format!("+{} -{} {}", file.added, file.removed, file.path);
        let line = Line::from(format!(" {}", ellipsize(&text, width)));
        let line = if n == panel.selected_file {
            line.style(theme.selected)
        } else {
            line.fg(theme.fg_primary)
        };
        let place = row(list, y);
        frame.render_widget(line, place);
        cx.ui.hits.push(place, Hit::GitHubRow { index: n });
    }
    if let Some(file) = panel.files.get(panel.selected_file) {
        let rows = file.rows.iter().skip(panel.diff_scroll);
        for (y, one) in (0..diff.height).zip(rows) {
            frame.render_widget(diff_line(one, cx), row(diff, y));
        }
    }
}

fn centered(screen: Rect, width: u16, height: u16) -> Rect {
    let width = width.min(screen.width.saturating_sub(2));
    let height = height.min(screen.height.saturating_sub(2));
    Rect::new(
        screen.x + (screen.width - width) / 2,
        screen.y + screen.height.saturating_sub(height) / 2,
        width,
        height,
    )
}

fn window(frame: &mut Frame, area: Rect, title: String, cx: &mut Ctx<'_>) -> Rect {
    let theme = cx.theme;
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.warning))
        .style(Style::new().bg(theme.bg_popup))
        .title(Line::from(title).fg(theme.fg_primary).bold());
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    cx.ui.hits.push(area, Hit::Picker);
    Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    )
}

pub fn render_github_windows(frame: &mut Frame, screen: Rect, cx: &mut Ctx<'_>) {
    let panel = cx.ws.github_panel();
    if let Some(confirm) = &panel.confirm {
        render_confirm(frame, screen, confirm, panel, cx);
    } else if let Some(compose) = &panel.compose {
        let theme = cx.theme;
        let area = centered(screen, 80, 14);
        let inner = window(
            frame,
            area,
            format!(" {} ", clean_line(&compose.purpose.title())),
            cx,
        );
        if inner.height < 3 {
            return;
        }
        let cols = usize::from(inner.width).max(1);
        let rows: Vec<String> = clean_block(compose.input.text())
            .iter()
            .flat_map(|l| hard_wrap(l, cols))
            .collect();
        let room = usize::from(inner.height.saturating_sub(1));
        let first = rows.len().saturating_sub(room);
        for (y, text) in (0..inner.height - 1).zip(rows.iter().skip(first)) {
            frame.render_widget(
                Line::from(text.clone()).fg(theme.fg_primary),
                Rect::new(inner.x, inner.y + y, inner.width, 1),
            );
        }
        let foot = match &compose.error {
            Some(error) => Line::from(error.clone()).fg(theme.error),
            None => Line::from("⏎ next  Alt+Enter new line  Esc cancel").fg(theme.fg_muted),
        };
        frame.render_widget(
            foot,
            Rect::new(inner.x, inner.y + inner.height - 1, inner.width, 1),
        );
    }
}

fn render_confirm(
    frame: &mut Frame,
    screen: Rect,
    confirm: &GhConfirm,
    panel: &GitHubPanel,
    cx: &mut Ctx<'_>,
) {
    let theme = cx.theme;
    let repo = panel.repo.as_deref().unwrap_or_default();
    let account = panel.account.as_deref().unwrap_or_default();
    let width = screen.width.saturating_sub(4).min(90);
    let cols = usize::from(width.saturating_sub(4)).max(1);
    let write = &confirm.write;
    let mut top = vec![
        (clean_line(&write.headline()), true),
        (format!("Repository: {repo}"), false),
        (format!("Account:    {account}"), false),
    ];
    if write.is_local() {
        top.push((
            "Stays on this computer, but changes the files here.".to_owned(),
            false,
        ));
    }
    if matches!(
        write,
        GhWrite::Checkout {
            cross_repository: true,
            ..
        }
    ) {
        top.push((
            "A stranger's code: servers and builds will run it here.".to_owned(),
            true,
        ));
    }
    let top: Vec<(String, bool)> = top
        .into_iter()
        .flat_map(|(t, warn)| hard_wrap(&t, cols).into_iter().map(move |l| (l, warn)))
        .collect();
    let command = hard_wrap(&command_text(CliTool::Gh, &write.args(repo)), cols);
    let text: Vec<String> = write
        .text()
        .iter()
        .flat_map(|l| hard_wrap(l, cols))
        .collect();
    let wanted = top.len() + command.len() + text.len() + 6;
    let area = centered(screen, width, u16::try_from(wanted).unwrap_or(u16::MAX));
    let title = if write.is_local() {
        " Check Out? "
    } else {
        " Send to GitHub? "
    };
    let inner = window(frame, area, title.to_owned(), cx);
    if inner.height < 2 {
        return;
    }
    let mut rows: Vec<Line<'_>> = Vec::new();
    for (line, warn) in top {
        rows.push(if warn {
            Line::from(line).fg(theme.warning)
        } else {
            Line::from(line).fg(theme.fg_primary)
        });
    }
    rows.push(Line::default());
    rows.extend(command.into_iter().map(|l| Line::from(l).fg(theme.accent)));
    if !text.is_empty() {
        rows.push(Line::default());
        rows.extend(
            text.into_iter()
                .skip(confirm.scroll)
                .map(|l| Line::from(l).fg(theme.fg_primary)),
        );
    }
    let room = inner.height.saturating_sub(1);
    for (y, line) in (0..room).zip(rows) {
        frame.render_widget(line, Rect::new(inner.x, inner.y + y, inner.width, 1));
    }
    let foot = match (&confirm.typed, write.second_answer()) {
        (Some(typed), Some(number)) => Line::from(format!(
            "Type {number} and press ⏎ to do it: {}",
            clean_line(typed.text())
        ))
        .fg(theme.warning)
        .bold(),
        _ if matches!(write, GhWrite::Merge { .. }) => {
            Line::from("y continue  Tab merge/squash/rebase  any other key: cancel")
                .fg(theme.fg_muted)
        }
        _ => Line::from("y send  ↑↓ scroll  any other key: cancel").fg(theme.fg_muted),
    };
    frame.render_widget(foot, Rect::new(inner.x, inner.y + room, inner.width, 1));
}
