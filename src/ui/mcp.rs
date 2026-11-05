use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::Clear;

use super::glyphs;
use super::history::diff_line;
use super::mouse::Hit;
use super::render::Ctx;
use super::text::ellipsize;
use crate::core::mcp::Decision;

const LIST_WIDTH: u16 = 32;

pub fn render_mcp_review(frame: &mut Frame, screen: Rect, cx: &mut Ctx<'_>) {
    let Some((proposal, review)) = cx.ws.mcp_proposal() else {
        return;
    };
    let theme = cx.theme;
    frame.render_widget(Clear, screen);
    frame
        .buffer_mut()
        .set_style(screen, Style::new().bg(theme.bg));
    cx.ui.hits.push(screen, Hit::History);
    let [title, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(screen);
    let summary = if proposal.summary.is_empty() {
        "proposed edit"
    } else {
        proposal.summary.as_str()
    };
    let room = usize::from(screen.width).saturating_sub(16);
    frame.render_widget(
        Line::from(vec![
            Span::raw(" Claude: ").fg(theme.accent).bold(),
            Span::raw(ellipsize(summary, room)).fg(theme.fg_primary),
        ])
        .bg(theme.bg_panel),
        title,
    );
    frame.render_widget(
        Line::from(
            " A accept file  R reject file  Y accept all  D reject all  Tab next file  Esc later",
        )
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
    let files = proposal.files.iter().enumerate();
    for (y, (index, file)) in (list.y..list.bottom()).zip(files) {
        let mark = match file.decision {
            None => "[ ]",
            Some(Decision::Accepted | Decision::Applying) => "[✓]",
            Some(Decision::Rejected) => "[x]",
            Some(Decision::Stale) => "[!]",
        };
        let name = cx.ws.display_path(&file.path).display().to_string();
        let room = usize::from(list.width).saturating_sub(6);
        let line = Line::from(format!(" {mark} {}", ellipsize(&name, room)));
        let line = if index == review.file {
            line.style(theme.selected)
        } else {
            line.fg(theme.fg_primary)
        };
        frame.render_widget(line, Rect::new(list.x, y, list.width, 1));
    }
    for y in divider.y..divider.bottom() {
        frame.render_widget(
            Line::from(glyphs::V_LINE).fg(theme.border),
            Rect::new(divider.x, y, 1, 1),
        );
    }
    cx.ui.page_rows = usize::from(diff.height);
    let Some(file) = proposal.files.get(review.file) else {
        return;
    };
    let last = file.rows.len().saturating_sub(usize::from(diff.height));
    let rows = file.rows.iter().skip(review.scroll.min(last));
    for (y, row) in (diff.y..diff.bottom()).zip(rows) {
        frame.render_widget(diff_line(row, cx), Rect::new(diff.x, y, diff.width, 1));
    }
}
