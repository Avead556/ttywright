use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear};

use super::mouse::Hit;
use super::render::Ctx;
use super::text::ellipsize;
use crate::core::notify::{Notification, NotifyLevel};

const WIDTH: u16 = 46;
const MIN_SCREEN: (u16, u16) = (60, 16);

fn height_of(n: &Notification) -> u16 {
    3 + u16::from(n.action.is_some()) + u16::from(!n.text.is_empty())
}

pub fn render_notifications(frame: &mut Frame, screen: Rect, cx: &mut Ctx<'_>) {
    if screen.width < MIN_SCREEN.0 || screen.height < MIN_SCREEN.1 {
        return;
    }
    let theme = cx.theme;
    let mut bottom = screen.bottom().saturating_sub(1);
    let top = screen.y + 2;
    let width = WIDTH.min(screen.width / 2);
    let x = screen.right().saturating_sub(width + 1);
    let shown = cx.ws.notifications().shown().to_vec();
    for n in shown.iter().rev() {
        let height = height_of(n);
        if bottom < top + height {
            break;
        }
        bottom -= height;
        let area = Rect::new(x, bottom, width, height);
        let color = match n.level {
            NotifyLevel::Success => theme.success,
            NotifyLevel::Info => theme.accent,
            NotifyLevel::Warning => theme.warning,
            NotifyLevel::Error => theme.error,
        };
        frame.render_widget(Clear, area);
        let block = Block::bordered()
            .border_type(BorderType::Rounded)
            .border_style(Style::new().fg(color))
            .style(Style::new().bg(theme.bg_popup));
        let inner = block.inner(area);
        frame.render_widget(block, area);
        let room = usize::from(inner.width.saturating_sub(2));
        let mut y = inner.y;
        let mut row = |frame: &mut Frame, line: Line<'_>| {
            frame.render_widget(line, Rect::new(inner.x, y, inner.width, 1));
            y += 1;
        };
        row(
            frame,
            Line::from(Span::styled(
                format!(" {}", ellipsize(&n.title, room.saturating_sub(2))),
                Style::new().fg(color).add_modifier(Modifier::BOLD),
            )),
        );
        if !n.text.is_empty() {
            row(
                frame,
                Line::from(format!(" {}", ellipsize(&n.text, room))).fg(theme.fg_primary),
            );
        }
        if let Some(action) = n.action {
            row(
                frame,
                Line::from(format!(" {}", ellipsize(action.spec().title, room))).fg(theme.accent),
            );
        }
        cx.ui.hits.push(area, Hit::Notification { id: n.id });
        let cross = Rect::new(area.right().saturating_sub(3), area.y + 1, 2, 1);
        frame.render_widget(Line::from("×").fg(theme.fg_muted), cross);
        cx.ui.hits.push(cross, Hit::NotificationClose { id: n.id });
    }
}
