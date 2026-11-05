use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear};

use super::mouse::Hit;
use super::render::Ctx;
use super::run::hard_wrap;
use super::text::ellipsize;
use crate::core::Focus;
use crate::core::cli::{CliTool, command_text};
use crate::core::docker::DockerSection;

fn row(area: Rect, y: u16) -> Rect {
    Rect::new(area.x, area.y + y, area.width, 1).intersection(area)
}

fn hints(section: DockerSection) -> &'static str {
    match section {
        DockerSection::Services => {
            "u up  U up all  s start  x stop  t restart  l logs  e shell  d down  r refresh"
        }
        DockerSection::Containers => {
            "s start  x stop  t restart  l logs  e shell  d remove  r refresh"
        }
        DockerSection::Images => "d remove  r refresh",
    }
}

pub fn render_docker(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let ws = cx.ws;
    let panel = ws.docker_panel();
    let focused = ws.focus() == Focus::Docker && ws.picker().is_none();
    let title_style = if focused {
        Style::new()
            .fg(theme.fg_primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.fg_muted)
    };
    let tab = |label: String, section: DockerSection| {
        if panel.section == section {
            Span::raw(format!("  {label}")).fg(theme.accent).bold()
        } else {
            Span::raw(format!("  {label}")).fg(theme.fg_muted)
        }
    };
    let mut title = vec![
        Span::styled(" Docker", title_style),
        tab(
            format!("Services {}", panel.services.len()),
            DockerSection::Services,
        ),
        tab(
            format!("Containers {}", panel.containers.len()),
            DockerSection::Containers,
        ),
        tab(
            format!("Images {}", panel.images.len()),
            DockerSection::Images,
        ),
    ];
    if panel.busy() {
        title.push(Span::raw("   working…").fg(theme.accent));
    }
    frame.render_widget(Line::from(title).bg(theme.bg_panel), row(area, 0));
    cx.ui.hits.push(area, Hit::DockerPanel);
    if area.height < 3 {
        return;
    }
    let width = usize::from(area.width).saturating_sub(1);
    frame.render_widget(
        Line::from(format!(" {}", ellipsize(hints(panel.section), width))).fg(theme.fg_muted),
        row(area, 1),
    );
    let note = match panel.section {
        DockerSection::Services => panel.compose_note.as_deref(),
        DockerSection::Containers | DockerSection::Images => panel.daemon_note.as_deref(),
    };
    if let Some(note) = note {
        frame.render_widget(
            Line::from(format!(" {}", ellipsize(note, width))).fg(theme.warning),
            row(area, 2),
        );
        return;
    }
    let rows: Vec<String> = match panel.section {
        DockerSection::Services => panel
            .services
            .iter()
            .map(|s| format!("{}   {}", s.name, s.image))
            .collect(),
        DockerSection::Containers => panel
            .containers
            .iter()
            .map(|c| format!("{}   {}   {}   {}", c.name, c.state, c.image, c.status))
            .collect(),
        DockerSection::Images => panel
            .images
            .iter()
            .map(|i| format!("{}   {}   {}", i.name, i.size, i.id))
            .collect(),
    };
    if rows.is_empty() {
        let text = if panel.busy() {
            " Loading…"
        } else {
            " Nothing here"
        };
        frame.render_widget(Line::from(text).fg(theme.fg_muted), row(area, 2));
        return;
    }
    let height = usize::from(area.height.saturating_sub(2));
    if focused {
        cx.ui.page_rows = height;
    }
    let chosen = panel.chosen();
    let first = (chosen + 1).saturating_sub(height);
    for (y, (n, text)) in (2u16..area.height).zip(rows.iter().enumerate().skip(first)) {
        let place = row(area, y);
        let line = Line::from(format!(" {}", ellipsize(text, width)));
        let line = if n == chosen {
            line.style(theme.selected)
        } else {
            line.fg(theme.fg_primary)
        };
        frame.render_widget(line, place);
        cx.ui.hits.push(
            place,
            Hit::DockerRow {
                section: panel.section.index(),
                index: n,
            },
        );
    }
}

pub fn render_docker_confirm(frame: &mut Frame, screen: Rect, cx: &mut Ctx<'_>) {
    let Some(confirm) = &cx.ws.docker_panel().confirm else {
        return;
    };
    let theme = cx.theme;
    let width = screen.width.saturating_sub(4).min(80);
    let cols = usize::from(width.saturating_sub(4)).max(1);
    let rows = hard_wrap(&command_text(CliTool::Docker, &confirm.args), cols);
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
        .title(Line::from(" Docker ").fg(theme.fg_primary).bold());
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    cx.ui.hits.push(area, Hit::Picker);
    if inner.height < 3 {
        return;
    }
    let line = |y: u16| Rect::new(inner.x + 1, inner.y + y, inner.width.saturating_sub(2), 1);
    frame.render_widget(
        Line::from("This removes things:").fg(theme.warning),
        line(0),
    );
    for (y, text) in (2u16..inner.height.saturating_sub(1)).zip(rows.iter()) {
        frame.render_widget(Line::from(text.clone()).fg(theme.fg_primary), line(y));
    }
    frame.render_widget(
        Line::from("y run   any other key: cancel").fg(theme.fg_muted),
        line(inner.height - 1),
    );
}
