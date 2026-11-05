use ratatui::Frame;
use ratatui::crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::glyphs;
use super::mouse::Hit;
use super::render::Ctx;
use crate::core::{ClaudeState, Focus};

fn color(value: vt100::Color) -> Option<Color> {
    match value {
        vt100::Color::Default => None,
        vt100::Color::Idx(index) => Some(Color::Indexed(index)),
        vt100::Color::Rgb(r, g, b) => Some(Color::Rgb(r, g, b)),
    }
}

fn cell_style(cell: &vt100::Cell, base: Style) -> Style {
    let mut style = base;
    if let Some(fg) = color(cell.fgcolor()) {
        style = style.fg(fg);
    }
    if let Some(bg) = color(cell.bgcolor()) {
        style = style.bg(bg);
    }
    let mut modifiers = Modifier::empty();
    modifiers.set(Modifier::BOLD, cell.bold());
    modifiers.set(Modifier::ITALIC, cell.italic());
    modifiers.set(Modifier::UNDERLINED, cell.underline());
    modifiers.set(Modifier::REVERSED, cell.inverse());
    style.add_modifier(modifiers)
}

pub fn render_claude(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    cx.ui.hits.push(area, Hit::ClaudePanel);
    let theme = cx.theme;
    if area.height < 2 || area.width < 2 {
        return;
    }
    let head = Rect::new(area.x, area.y, area.width, 1);
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    cx.ui.claude_size = Some((body.height, body.width));
    let Some(panel) = cx.ws.claude() else {
        return;
    };
    let focused = cx.ws.focus() == Focus::Claude && !cx.ws.overlay_open();
    let link = if panel.connected {
        Span::raw("● connected to the editor").fg(theme.success)
    } else {
        Span::raw("○ not connected").fg(theme.fg_muted)
    };
    let hint = if matches!(panel.state, ClaudeState::Exited(_)) {
        "  has exited: Enter starts it again"
    } else if focused {
        "  F9 back to the editor"
    } else {
        "  F9 type here"
    };
    frame.render_widget(
        Line::from(vec![
            Span::raw(" Claude Code  ").fg(theme.accent).bold(),
            link,
            Span::raw(hint).fg(theme.fg_muted),
        ])
        .bg(theme.bg_panel),
        head,
    );
    match &panel.state {
        ClaudeState::Running | ClaudeState::Waiting => {}
        ClaudeState::Failed(message) => {
            let text = format!(" {message}  (Enter tries again)");
            frame.render_widget(Line::from(text).fg(theme.error), body);
            return;
        }
        ClaudeState::Exited(_) => {}
    }
    draw_screen(frame, body, panel.screen.screen(), focused, cx);
}

fn draw_screen(frame: &mut Frame, body: Rect, screen: &vt100::Screen, focused: bool, cx: &Ctx<'_>) {
    let base = Style::new().fg(cx.theme.fg_primary).bg(cx.theme.bg);
    let buffer = frame.buffer_mut();
    for row in 0..body.height {
        for column in 0..body.width {
            let Some(cell) = screen.cell(row, column) else {
                continue;
            };
            if cell.is_wide_continuation() {
                continue;
            }
            let target = &mut buffer[(body.x + column, body.y + row)];
            let contents = cell.contents();
            target.set_symbol(if contents.is_empty() { " " } else { contents });
            target.set_style(cell_style(cell, base));
        }
    }
    let (row, column) = screen.cursor_position();
    if focused
        && !screen.hide_cursor()
        && screen.scrollback() == 0
        && row < body.height
        && column < body.width
    {
        frame.set_cursor_position(Position::new(body.x + column, body.y + row));
    }
}

pub fn render_terminal(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    cx.ui.hits.push(area, Hit::TerminalPanel);
    if area.height < 2 || area.width < 2 {
        return;
    }
    let theme = cx.theme;
    let head = Rect::new(area.x, area.y, area.width, 1);
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 1);
    cx.ui.terminal_size = Some((body.height, body.width));
    frame.render_widget(Line::from("").bg(theme.bg_panel), head);
    let active = cx.ws.terminal_active();
    let mut x = head.x;
    for (index, tab) in cx.ws.terminal_tabs().iter().enumerate() {
        let name = format!(" {} ", tab.title);
        let width = u16::try_from(name.width()).unwrap_or(u16::MAX);
        let style = if index == active {
            theme.selected
        } else {
            Style::new().fg(theme.fg_muted)
        };
        let label = Rect::new(x, head.y, width, 1).intersection(head);
        frame.render_widget(Line::from(name).style(style), label);
        cx.ui.hits.push(label, Hit::TerminalTab { index });
        let close = Rect::new(x.saturating_add(width), head.y, 2, 1).intersection(head);
        frame.render_widget(
            Line::from(format!("{} ", glyphs::CLOSE)).style(style),
            close,
        );
        cx.ui.hits.push(close, Hit::TerminalCloseTab { index });
        x = x.saturating_add(width + 3);
    }
    let plus = Rect::new(x, head.y, 3, 1).intersection(head);
    frame.render_widget(Line::from(" + ").fg(theme.accent).bold(), plus);
    cx.ui.hits.push(plus, Hit::TerminalNewTab);
    let Some(tab) = cx.ws.terminal_tabs().get(active) else {
        return;
    };
    let focused = cx.ws.focus() == Focus::Terminal && !cx.ws.overlay_open();
    draw_screen(frame, body, tab.screen.screen(), focused, cx);
}

pub fn key_bytes(key: KeyEvent, application_cursor: bool) -> Option<Vec<u8>> {
    if key.kind == KeyEventKind::Release {
        return None;
    }
    let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
    let alt = key.modifiers.contains(KeyModifiers::ALT);
    let shift = key.modifiers.contains(KeyModifiers::SHIFT);
    let arrow = |letter: char| {
        let modifier = 1 + u8::from(shift) + 2 * u8::from(alt) + 4 * u8::from(ctrl);
        if modifier > 1 {
            format!("\x1b[1;{modifier}{letter}")
        } else if application_cursor {
            format!("\x1bO{letter}")
        } else {
            format!("\x1b[{letter}")
        }
        .into_bytes()
    };
    let tilde = |n: u8| format!("\x1b[{n}~").into_bytes();
    let mut bytes = match key.code {
        KeyCode::Char(c) if ctrl => {
            let lower = c.to_ascii_lowercase();
            match lower {
                'a'..='z' => vec![lower as u8 - b'a' + 1],
                ' ' | '2' | '@' => vec![0],
                '[' | '3' => vec![0x1b],
                '\\' | '4' => vec![0x1c],
                ']' | '5' => vec![0x1d],
                '6' | '^' => vec![0x1e],
                '/' | '7' | '_' => vec![0x1f],
                _ => return None,
            }
        }
        KeyCode::Char(c) => c.to_string().into_bytes(),
        KeyCode::Enter if shift => return Some(b"\x1b\r".to_vec()),
        KeyCode::Enter => vec![b'\r'],
        KeyCode::Tab => vec![b'\t'],
        KeyCode::BackTab => return Some(b"\x1b[Z".to_vec()),
        KeyCode::Backspace => vec![0x7f],
        KeyCode::Esc => vec![0x1b],
        KeyCode::Up => return Some(arrow('A')),
        KeyCode::Down => return Some(arrow('B')),
        KeyCode::Right => return Some(arrow('C')),
        KeyCode::Left => return Some(arrow('D')),
        KeyCode::Home => return Some(arrow('H')),
        KeyCode::End => return Some(arrow('F')),
        KeyCode::Insert => tilde(2),
        KeyCode::Delete => tilde(3),
        KeyCode::PageUp => tilde(5),
        KeyCode::PageDown => tilde(6),
        KeyCode::F(n @ 1..=4) => format!("\x1bO{}", (b'P' + n - 1) as char).into_bytes(),
        KeyCode::F(n @ 5) => tilde(10 + n),
        KeyCode::F(n @ 6..=10) => tilde(11 + n),
        KeyCode::F(n @ 11..=12) => tilde(12 + n),
        _ => return None,
    };
    if alt {
        bytes.insert(0, 0x1b);
    }
    Some(bytes)
}

pub fn paste_bytes(text: &str, bracketed: bool) -> Vec<u8> {
    if bracketed {
        format!("\x1b[200~{text}\x1b[201~").into_bytes()
    } else {
        text.as_bytes().to_vec()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> KeyEvent {
        KeyEvent::new(code, modifiers)
    }

    fn bytes(code: KeyCode, modifiers: KeyModifiers) -> Vec<u8> {
        key_bytes(press(code, modifiers), false).unwrap()
    }

    #[test]
    fn keys_are_what_a_terminal_sends() {
        let none = KeyModifiers::NONE;
        assert_eq!(bytes(KeyCode::Char('я'), none), "я".as_bytes());
        assert_eq!(bytes(KeyCode::Char('c'), KeyModifiers::CONTROL), [3]);
        assert_eq!(bytes(KeyCode::Enter, none), b"\r");
        assert_eq!(bytes(KeyCode::Enter, KeyModifiers::SHIFT), b"\x1b\r");
        assert_eq!(bytes(KeyCode::Esc, none), [0x1b]);
        assert_eq!(bytes(KeyCode::Backspace, none), [0x7f]);
        assert_eq!(bytes(KeyCode::BackTab, KeyModifiers::SHIFT), b"\x1b[Z");
        assert_eq!(bytes(KeyCode::Char('b'), KeyModifiers::ALT), b"\x1bb");
        assert_eq!(bytes(KeyCode::Up, none), b"\x1b[A");
        assert_eq!(bytes(KeyCode::Up, KeyModifiers::SHIFT), b"\x1b[1;2A");
        assert_eq!(
            key_bytes(press(KeyCode::Up, none), true).unwrap(),
            b"\x1bOA"
        );
        assert_eq!(bytes(KeyCode::F(1), none), b"\x1bOP");
        assert_eq!(bytes(KeyCode::F(5), none), b"\x1b[15~");
        assert_eq!(bytes(KeyCode::F(10), none), b"\x1b[21~");
        assert_eq!(bytes(KeyCode::F(12), none), b"\x1b[24~");
    }

    #[test]
    fn a_key_released_sends_nothing() {
        let mut key = press(KeyCode::Char('a'), KeyModifiers::NONE);
        key.kind = KeyEventKind::Release;
        assert_eq!(key_bytes(key, false), None);
    }

    #[test]
    fn a_paste_arrives_in_one_piece_when_asked_for() {
        assert_eq!(paste_bytes("a\nb", true), b"\x1b[200~a\nb\x1b[201~");
        assert_eq!(paste_bytes("a", false), b"a");
    }
}
