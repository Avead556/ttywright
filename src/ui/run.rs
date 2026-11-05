use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::Color;
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear};
use unicode_width::UnicodeWidthChar;
use unicode_width::UnicodeWidthStr;

use super::glyphs;
use super::mouse::Hit;
use super::render::Ctx;
use super::text::{ellipsize, styled_text};
use crate::core::run::visible;
use crate::core::run_output::{RunColor, RunStyle};
use crate::core::{Action, Focus, RunLine, RunOutcome, RunPanel, RunStatus};

const MAX_TAB_NAME: usize = 24;
const HEADER_ROWS: u16 = 2;
const MARK_COLS: u16 = 2;

fn row(area: Rect, y: u16) -> Rect {
    Rect::new(area.x, area.y + y, area.width, 1).intersection(area)
}

fn duration(ms: u64) -> String {
    if ms < 1000 {
        return format!("{ms} ms");
    }
    if ms < 60_000 {
        return format!("{:.1} s", ms as f64 / 1000.0);
    }
    format!("{} min {} s", ms / 60_000, ms / 1000 % 60)
}

fn status_span(panel: &RunPanel, cx: &Ctx<'_>) -> Span<'static> {
    let theme = cx.theme;
    match &panel.status {
        RunStatus::Running => Span::raw(format!("{} Running…", glyphs::SPINNER)).fg(theme.success),
        RunStatus::Stopping => {
            Span::raw(format!("{} Stopping…", glyphs::SPINNER)).fg(theme.fg_muted)
        }
        RunStatus::Finished {
            outcome,
            duration_ms,
        } => {
            let time = duration(*duration_ms);
            match outcome {
                RunOutcome::FailedToStart(_) => {
                    Span::raw(format!("{} {outcome}", glyphs::ERROR)).fg(theme.error)
                }
                RunOutcome::Stopped => {
                    Span::raw(format!("■ {outcome} · {time}")).fg(theme.fg_muted)
                }
                _ if outcome.is_success() => {
                    Span::raw(format!("{} {outcome} · {time}", glyphs::SUCCESS)).fg(theme.success)
                }
                _ => Span::raw(format!("{} {outcome} · {time}", glyphs::ERROR)).fg(theme.error),
            }
        }
    }
}

pub fn render_run(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let ws = cx.ws;
    let theme = cx.theme;
    let focused = ws.focus() == Focus::Run && ws.picker().is_none();
    frame
        .buffer_mut()
        .set_style(area, Style::new().bg(theme.bg_panel));
    cx.ui.hits.push(area, Hit::RunPanel);
    let title_style = if focused {
        Style::new()
            .fg(theme.fg_primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.fg_muted)
    };
    let Some(panel) = ws.run_panel() else {
        let hint = if !ws.is_run_window() {
            format!(
                "   Nothing has run yet. {}: choose what to run",
                cx.keymap.label(Action::RunChoose).unwrap_or("Run...")
            )
        } else if ws.run_connected() {
            "   Waiting for a run in the main window".to_owned()
        } else {
            "   The main window of this project is not running".to_owned()
        };
        let line = Line::from(vec![
            Span::styled(" Run", title_style),
            Span::raw(hint).fg(theme.fg_muted),
        ]);
        frame.render_widget(line, row(area, 0));
        return;
    };

    let mut title = vec![Span::styled(" Run", title_style), Span::raw("  ")];
    let mut x = area.x + 6;
    let tabs = ws.run_tabs();
    if tabs.len() > 1 {
        for (index, tab) in tabs.iter().enumerate() {
            let mark = if tab.is_active() { "▶ " } else { "" };
            let name = format!(
                " {mark}{} ",
                ellipsize(&visible(&tab.target.name), MAX_TAB_NAME)
            );
            let width = u16::try_from(name.width()).unwrap_or(u16::MAX);
            let style = if index == ws.run_active() {
                theme.selected
            } else {
                Style::new().fg(theme.fg_muted)
            };
            let cell = Rect::new(x, area.y, width, 1).intersection(area);
            cx.ui.hits.push(cell, Hit::RunTab { index });
            title.push(Span::styled(name, style));
            title.push(Span::raw(" "));
            x = x.saturating_add(width + 1);
        }
    }
    title.push(status_span(panel, cx));
    if ws.is_run_window() && !ws.run_connected() {
        title.push(
            Span::raw("  (the main window is gone: this is what it last said)").fg(theme.warning),
        );
    }
    if panel.trimmed() > 0 {
        title.push(
            Span::raw(format!("  ({} earlier lines trimmed)", panel.trimmed())).fg(theme.warning),
        );
    }
    frame.render_widget(Line::from(title), row(area, 0));
    if focused {
        let hints = if ws.is_run_window() {
            "n/p error  ⏎ open in the main window "
        } else if panel.is_active() {
            "s stop  x close  Tab next  n/p error  ⏎ open  w window "
        } else {
            "r rerun  x close  Tab next  n/p error  ⏎ open  w window "
        };
        frame.render_widget(
            Line::from(hints).fg(theme.fg_subtle).right_aligned(),
            row(area, 0),
        );
    }
    let command = format!(" $ {}", visible(&panel.target.command_line()));
    frame.render_widget(
        Line::from(ellipsize(&command, usize::from(area.width))).fg(theme.fg_muted),
        row(area, 1),
    );
    render_output(frame, area, panel, focused, cx);
}

fn output_top(panel: &RunPanel, scroll: usize, height: usize) -> usize {
    let (first, end) = (panel.first_line(), panel.end_line());
    let total = end - first + usize::from(!panel.partial.is_empty());
    let last_top = first + total.saturating_sub(height);
    match panel.selected() {
        None => last_top,
        Some(selected) => {
            let top = scroll.clamp(first, last_top);
            if selected < top {
                selected
            } else if selected >= top + height {
                selected + 1 - height
            } else {
                top
            }
        }
    }
}

fn render_output(frame: &mut Frame, area: Rect, panel: &RunPanel, focused: bool, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let height = area.height.saturating_sub(HEADER_ROWS);
    if height == 0 {
        return;
    }
    if focused {
        cx.ui.page_rows = usize::from(height);
    }
    let top = output_top(panel, cx.ui.run_scroll, usize::from(height));
    cx.ui.run_scroll = top;
    let width = usize::from(area.width.saturating_sub(MARK_COLS));
    let tab_width = cx.ws.tab_width();
    let base = Style::new().fg(theme.fg_primary);
    for y in 0..height {
        let number = top + usize::from(y);
        let screen = row(area, HEADER_ROWS + y);
        let Some(line) = panel.line(number) else {
            if number == panel.end_line() && !panel.partial.is_empty() {
                let mut spans = vec![Span::raw("  ")];
                spans.extend(styled_text(
                    &panel.partial,
                    &[],
                    base,
                    panel.left,
                    width,
                    tab_width,
                ));
                frame.render_widget(Line::from(spans), screen);
            }
            continue;
        };
        cx.ui.hits.push(screen, Hit::RunLine { line: number });
        let selected = panel.selected() == Some(number);
        let row_style = match (selected, focused) {
            (true, true) => theme.selected,
            (true, false) => theme.selected_inactive,
            _ => Style::new().bg(theme.bg_panel),
        };
        frame.render_widget(
            output_line(line, panel.left, width, tab_width, cx).style(row_style),
            screen,
        );
    }
}

fn run_color(color: RunColor) -> Color {
    match color {
        RunColor::Indexed(index) => Color::Indexed(index),
        RunColor::Rgb(r, g, b) => Color::Rgb(r, g, b),
    }
}

fn program_style(style: RunStyle, base: Style) -> Style {
    let mut out = base;
    if let Some(fg) = style.fg {
        out = out.fg(run_color(fg));
    }
    if let Some(bg) = style.bg {
        out = out.bg(run_color(bg));
    }
    let mut modifiers = Modifier::empty();
    modifiers.set(Modifier::BOLD, style.bold);
    modifiers.set(Modifier::DIM, style.dim);
    modifiers.set(Modifier::ITALIC, style.italic);
    modifiers.set(Modifier::UNDERLINED, style.underline);
    modifiers.set(Modifier::REVERSED, style.reverse);
    out.add_modifier(modifiers)
}

fn program_styles(line: &RunLine, base: Style) -> Vec<Style> {
    let mut styles = vec![base; line.spans.last().map_or(0, |span| span.end)];
    for span in &line.spans {
        if let Some(chars) = styles.get_mut(span.start..span.end) {
            chars.fill(program_style(span.style, base));
        }
    }
    styles
}

fn output_line(
    line: &RunLine,
    left: usize,
    width: usize,
    tab_width: usize,
    cx: &Ctx<'_>,
) -> Line<'static> {
    let theme = cx.theme;
    let base = Style::new().fg(theme.fg_primary);
    let mut styles = program_styles(line, base);
    let Some(location) = &line.location else {
        let mut spans = vec![Span::raw("  ")];
        spans.extend(styled_text(
            &line.text, &styles, base, left, width, tab_width,
        ));
        return Line::from(spans);
    };
    let link = Style::new()
        .fg(theme.accent)
        .add_modifier(Modifier::UNDERLINED);
    if styles.len() < location.span.1 {
        styles.resize(location.span.1, base);
    }
    for style in styles
        .iter_mut()
        .take(location.span.1)
        .skip(location.span.0)
    {
        *style = link;
    }
    let mut spans = vec![Span::raw(format!("{} ", glyphs::BREADCRUMB)).fg(theme.accent)];
    spans.extend(styled_text(
        &line.text, &styles, base, left, width, tab_width,
    ));
    Line::from(spans)
}

pub(super) fn hard_wrap(text: &str, cols: usize) -> Vec<String> {
    let mut rows = vec![String::new()];
    let mut used = 0;
    for c in text.chars() {
        let w = c.width().unwrap_or(0);
        if used + w > cols.max(1) && used > 0 {
            rows.push(String::new());
            used = 0;
        }
        if let Some(last) = rows.last_mut() {
            last.push(c);
        }
        used += w;
    }
    rows
}

pub fn render_run_confirm(frame: &mut Frame, screen: Rect, cx: &mut Ctx<'_>) {
    let Some(confirm) = cx.ws.run_confirm() else {
        return;
    };
    let theme = cx.theme;
    let width = (screen.width.saturating_sub(4)).min(90);
    let cols = usize::from(width.saturating_sub(4));
    let root = cx.ws.root();
    let folder = cx.ws.display_path(&confirm.target.cwd);
    let folder = if folder.starts_with(root) || !confirm.target.cwd.starts_with(root) {
        confirm.target.cwd.display().to_string()
    } else if folder.as_os_str().is_empty() {
        "the project folder".to_owned()
    } else {
        folder.display().to_string()
    };
    let rows = hard_wrap(&visible(&confirm.target.command_line()), cols);
    let height = (u16::try_from(rows.len())
        .unwrap_or(u16::MAX)
        .saturating_add(7))
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
        .title(
            Line::from(" Run a command from this project's config? ")
                .fg(theme.fg_primary)
                .bold(),
        );
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
    let name = ellipsize(&visible(&confirm.target.name), cols);
    frame.render_widget(Line::from(name).fg(theme.fg_primary).bold(), line(0));
    frame.render_widget(
        Line::from(ellipsize(
            &format!(".ttywright/config.toml, in {}", visible(&folder)),
            cols,
        ))
        .fg(theme.fg_muted),
        line(1),
    );

    let room = usize::from(inner.height.saturating_sub(4));
    let last_top = rows.len().saturating_sub(room);
    let top = confirm.scroll.min(last_top);
    for (i, text) in rows.iter().skip(top).take(room).enumerate() {
        frame.render_widget(
            Line::from(text.clone()).fg(theme.fg_primary),
            line(3 + u16::try_from(i).unwrap_or(0)),
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

pub fn render_analysis_confirm(frame: &mut Frame, screen: Rect, cx: &mut Ctx<'_>) {
    let Some(confirm) = cx.ws.analysis_confirm() else {
        return;
    };
    let theme = cx.theme;
    let width = (screen.width.saturating_sub(4)).min(90);
    let cols = usize::from(width.saturating_sub(4));
    let mut rows: Vec<String> = Vec::new();
    for command in &confirm.commands {
        rows.extend(hard_wrap(&visible(&command.command_line()), cols));
        if command.from_project {
            rows.push("  ^ this program is part of the project; asked every time".to_owned());
        }
    }
    let height = (u16::try_from(rows.len())
        .unwrap_or(u16::MAX)
        .saturating_add(6))
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
        .title(Line::from(" Analyze Project ").fg(theme.fg_primary).bold());
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
        Line::from(ellipsize("These can run code from this project:", cols)).fg(theme.warning),
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn wrapping_keeps_every_character() {
        for (text, cols) in [
            ("abcdef", 2),
            ("абвгд", 3),
            ("", 4),
            ("日本語", 3),
            ("a", 0),
        ] {
            let rows = hard_wrap(text, cols);
            assert_eq!(rows.concat(), text);
            if cols >= 2 {
                for row in &rows {
                    let used: usize = row.chars().map(|c| c.width().unwrap_or(0)).sum();
                    assert!(used <= cols, "{row:?}");
                }
            }
        }
        assert_eq!(hard_wrap("abcd", 2), ["ab", "cd"]);
    }

    #[test]
    fn durations_read_naturally() {
        assert_eq!(duration(5), "5 ms");
        assert_eq!(duration(1500), "1.5 s");
        assert_eq!(duration(125_000), "2 min 5 s");
    }
}
