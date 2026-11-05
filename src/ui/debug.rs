use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::glyphs;
use super::mouse::Hit;
use super::render::Ctx;
use super::text::ellipsize;
use crate::core::debug::{DebugPart, DebugPhase, DebugSession};
use crate::core::{Action, Focus};

const ALL_PARTS_MIN: u16 = 90;

fn row(area: Rect, y: u16) -> Rect {
    Rect::new(area.x, area.y + y, area.width, 1).intersection(area)
}

fn first_shown(selected: usize, height: usize) -> usize {
    (selected + 1).saturating_sub(height)
}

pub fn render_debug(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    for y in area.y..area.bottom() {
        frame.render_widget(Line::default().bg(theme.bg_panel), row(area, y - area.y));
    }
    cx.ui.hits.push(area, Hit::DebugPanel);
    let Some(session) = cx.ws.debug_session() else {
        return render_nothing_yet(frame, area, cx);
    };
    let title = header(session, cx);
    let used = u16::try_from(title.width()).unwrap_or(u16::MAX);
    frame.render_widget(title, row(area, 0));
    render_buttons(frame, row(area, 0), used, session, cx);
    if area.height < 3 {
        return;
    }
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    if cx.ws.focus() == Focus::Debug {
        cx.ui.page_rows = usize::from(body.height.saturating_sub(1));
    }
    if area.width < ALL_PARTS_MIN {
        return render_part(frame, body, session.part, session, cx);
    }
    let [frames, vars, console] = Layout::horizontal([
        Constraint::Percentage(30),
        Constraint::Percentage(40),
        Constraint::Percentage(30),
    ])
    .areas(body);
    render_part(frame, frames, DebugPart::Frames, session, cx);
    render_part(frame, vars, DebugPart::Variables, session, cx);
    render_part(frame, console, DebugPart::Console, session, cx);
}

fn render_nothing_yet(frame: &mut Frame, area: Rect, cx: &Ctx<'_>) {
    let theme = cx.theme;
    let key = |action| cx.keymap.label(action).unwrap_or("palette");
    let lines = [
        format!(
            " Debug: {} sets a breakpoint (or click a line number), {} chooses what to debug",
            key(Action::ToggleBreakpoint),
            key(Action::DebugChoose),
        ),
        format!(
            " Then {} over, {} into, {} out, {} resume",
            key(Action::DebugStepOver),
            key(Action::DebugStepInto),
            key(Action::DebugStepOut),
            key(Action::ResumeProgram),
        ),
    ];
    for (n, text) in lines.iter().enumerate() {
        if let Ok(n) = u16::try_from(n) {
            frame.render_widget(Line::from(text.as_str()).fg(theme.fg_muted), row(area, n));
        }
    }
}

fn header(session: &DebugSession, cx: &Ctx<'_>) -> Line<'static> {
    let theme = cx.theme;
    let state = match &session.phase {
        DebugPhase::Building => {
            Span::raw(format!("{} Building…", glyphs::SPINNER)).fg(theme.fg_muted)
        }
        DebugPhase::Starting => {
            Span::raw(format!("{} Starting…", glyphs::SPINNER)).fg(theme.fg_muted)
        }
        DebugPhase::Running => Span::raw(format!("{} Running", glyphs::SPINNER)).fg(theme.success),
        DebugPhase::Paused { reason } => {
            Span::raw(format!("{} Paused: {reason}", glyphs::EXECUTION)).fg(theme.warning)
        }
        DebugPhase::Ended { note } => Span::raw(format!("■ {note}")).fg(theme.fg_muted),
    };
    Line::from(vec![
        Span::raw(format!(" {} ", glyphs::DEBUG)).fg(theme.error),
        Span::raw(format!("{}  ", session.name))
            .fg(theme.fg_primary)
            .bold(),
        state,
    ])
    .bg(theme.bg_panel)
}

fn buttons(session: &DebugSession) -> &'static [(&'static str, Action)] {
    if session.is_paused() {
        &[
            ("▶ c resume", Action::ResumeProgram),
            ("↷ n over", Action::DebugStepOver),
            ("↓ i into", Action::DebugStepInto),
            ("↑ o out", Action::DebugStepOut),
            ("= e eval", Action::DebugEvaluate),
            ("+ w watch", Action::DebugAddWatch),
            ("■ s stop", Action::DebugStop),
        ]
    } else if session.is_live() {
        &[
            ("‖ p pause", Action::DebugPause),
            ("■ s stop", Action::DebugStop),
        ]
    } else {
        &[("▶ r again", Action::DebugLast)]
    }
}

fn render_buttons(
    frame: &mut Frame,
    area: Rect,
    from: u16,
    session: &DebugSession,
    cx: &mut Ctx<'_>,
) {
    let mut x = area.x + from;
    for (label, action) in buttons(session) {
        let width = u16::try_from(label.width()).unwrap_or(u16::MAX) + 2;
        if x + width > area.right() {
            return;
        }
        let cell = Rect::new(x, area.y, width, 1);
        frame.render_widget(
            Line::from(format!(" {label} "))
                .fg(cx.theme.fg_muted)
                .bg(cx.theme.bg_panel),
            cell,
        );
        cx.ui.hits.push(cell, Hit::Button(*action));
        x += width;
    }
}

fn render_part(
    frame: &mut Frame,
    area: Rect,
    part: DebugPart,
    session: &DebugSession,
    cx: &mut Ctx<'_>,
) {
    let theme = cx.theme;
    let has_keys =
        session.part == part && cx.ws.focus() == Focus::Debug && cx.ws.picker().is_none();
    let title = match part {
        DebugPart::Frames => match session
            .thread
            .and_then(|id| session.threads.iter().find(|t| t.0 == id))
        {
            Some((_, name)) if session.threads.len() > 1 => format!("Frames · {name} (t)"),
            _ => "Frames".to_owned(),
        },
        DebugPart::Variables => "Variables".to_owned(),
        DebugPart::Console => "Console".to_owned(),
    };
    let title_style = if has_keys {
        Style::new()
            .fg(theme.fg_primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.fg_muted)
    };
    let width = usize::from(area.width).saturating_sub(1);
    frame.render_widget(
        Line::from(Span::styled(
            format!(" {}", ellipsize(&title, width)),
            title_style,
        ))
        .bg(theme.bg_panel),
        row(area, 0),
    );
    let rows = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height.saturating_sub(1),
    );
    match part {
        DebugPart::Frames => render_frames(frame, rows, session, has_keys, cx),
        DebugPart::Variables => render_vars(frame, rows, session, has_keys, cx),
        DebugPart::Console => render_console(frame, rows, session, cx),
    }
}

fn selected_style(cx: &Ctx<'_>, selected: bool, has_keys: bool) -> Style {
    match (selected, has_keys) {
        (true, true) => cx.theme.selected,
        (true, false) => cx.theme.selected_inactive,
        _ => Style::new().bg(cx.theme.bg_panel),
    }
}

fn render_frames(
    frame: &mut Frame,
    area: Rect,
    session: &DebugSession,
    has_keys: bool,
    cx: &mut Ctx<'_>,
) {
    let theme = cx.theme;
    let height = usize::from(area.height);
    let width = usize::from(area.width);
    let first = first_shown(session.frame, height);
    for (n, f) in session.frames.iter().enumerate().skip(first).take(height) {
        let Ok(y) = u16::try_from(n - first) else {
            break;
        };
        let place = f.path.as_deref().map_or_else(
            || "no source".to_owned(),
            |p| {
                let name = p.file_name().map_or_else(
                    || p.display().to_string(),
                    |n| n.to_string_lossy().into_owned(),
                );
                format!("{name}:{}", f.line + 1)
            },
        );
        let name_room = width.saturating_sub(place.chars().count() + 3);
        let color = if f.exists {
            theme.fg_primary
        } else {
            theme.fg_subtle
        };
        let line = Line::from(vec![
            Span::raw(format!(" {} ", ellipsize(&f.name, name_room))).fg(color),
            Span::raw(place).fg(theme.fg_muted),
        ])
        .style(selected_style(cx, n == session.frame, has_keys));
        let at = row(area, y);
        cx.ui.hits.push(
            at,
            Hit::DebugRow {
                part: DebugPart::Frames,
                index: n,
            },
        );
        frame.render_widget(line, at);
    }
}

fn render_vars(
    frame: &mut Frame,
    area: Rect,
    session: &DebugSession,
    has_keys: bool,
    cx: &mut Ctx<'_>,
) {
    let theme = cx.theme;
    let rows = cx.ws.debug_var_rows();
    let height = usize::from(area.height);
    let width = usize::from(area.width);
    let selected = session.var_selected.min(rows.len().saturating_sub(1));
    let first = first_shown(selected, height);
    for (n, var) in rows.iter().enumerate().skip(first).take(height) {
        let Ok(y) = u16::try_from(n - first) else {
            break;
        };
        let mark = match (var.expandable, var.expanded) {
            (true, true) => glyphs::EXPANDED,
            (true, false) => glyphs::COLLAPSED,
            _ => " ",
        };
        let indent = "  ".repeat(var.depth);
        let eye = if var.watch.is_some() { "◎ " } else { "" };
        let name = format!(" {indent}{mark} {eye}{}", var.name);
        let kind = var
            .kind
            .as_deref()
            .map(|k| format!(": {k}"))
            .unwrap_or_default();
        let left = format!("{name}{kind}");
        let left = ellipsize(&left, width.saturating_sub(4).max(1) * 3 / 5);
        let used = left.chars().count() + 3;
        let line = Line::from(vec![
            Span::raw(left).fg(theme.syntax.function),
            Span::raw(if var.value.is_empty() { "" } else { " = " }).fg(theme.fg_subtle),
            Span::raw(ellipsize(&var.value, width.saturating_sub(used))).fg(theme.fg_primary),
        ])
        .style(selected_style(cx, n == selected, has_keys));
        let at = row(area, y);
        cx.ui.hits.push(
            at,
            Hit::DebugRow {
                part: DebugPart::Variables,
                index: n,
            },
        );
        frame.render_widget(line, at);
    }
}

fn render_console(frame: &mut Frame, area: Rect, session: &DebugSession, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let height = usize::from(area.height);
    let width = usize::from(area.width).saturating_sub(1);
    let end = session.console.len().saturating_sub(session.console_back);
    let start = end.saturating_sub(height);
    for (y, text) in session.console[start..end].iter().enumerate() {
        if let Ok(y) = u16::try_from(y) {
            frame.render_widget(
                Line::from(format!(" {}", ellipsize(text, width))).fg(theme.fg_primary),
                row(area, y),
            );
        }
    }
    cx.ui.hits.push(
        area,
        Hit::DebugRow {
            part: DebugPart::Console,
            index: 0,
        },
    );
}
