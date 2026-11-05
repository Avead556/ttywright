use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::Line;
use ratatui::widgets::{Block, BorderType, Clear};
use unicode_width::UnicodeWidthStr;

use super::glyphs;
use super::mouse::Hit;
use super::render::Ctx;
use super::text::ellipsize;
use crate::core::Workspace;
use crate::core::file_op::FilePromptKind;
use crate::core::menu::{MENUS, MenuEntry};
use crate::core::templates::{FILE_TEMPLATES, PROJECT_TEMPLATES};

pub fn title_span(menu: usize) -> (u16, u16) {
    let before: usize = MENUS[..menu].iter().map(|m| m.title.len() + 2).sum();
    let width = MENUS.get(menu).map_or(0, |m| m.title.len() + 2);
    (
        u16::try_from(before).unwrap_or(0),
        u16::try_from(width).unwrap_or(0),
    )
}

pub fn titles_width() -> u16 {
    let (x, width) = title_span(MENUS.len() - 1);
    x + width
}

pub fn render_menu(frame: &mut Frame, screen: Rect, cx: &mut Ctx<'_>) {
    let Some(state) = cx.ws.menu() else {
        return;
    };
    let theme = cx.theme;
    let entries = MENUS[state.menu].entries;
    let label = |entry: &MenuEntry| match entry {
        MenuEntry::Item(action) => (action.spec().title, cx.keymap.label(*action).unwrap_or("")),
        MenuEntry::Separator => ("", ""),
    };
    let widest = entries
        .iter()
        .map(|e| {
            let (title, keys) = label(e);
            title.width() + keys.width()
        })
        .max()
        .unwrap_or(0);
    let width = u16::try_from(widest + 7)
        .unwrap_or(u16::MAX)
        .min(screen.width);
    let wanted = u16::try_from(entries.len() + 2).unwrap_or(u16::MAX);
    let top = screen.y + 1;
    let height = wanted.min(screen.height.saturating_sub(1));
    let x = title_span(state.menu)
        .0
        .min(screen.right().saturating_sub(width));
    let area = Rect::new(x, top, width, height);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.border_popup))
        .style(Style::new().bg(theme.bg_popup));
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    cx.ui.hits.push(area, Hit::Menu);
    let rows = usize::from(inner.height);
    let first = (state.entry + 1).saturating_sub(rows);
    let room = usize::from(inner.width);
    for (y, (index, entry)) in (inner.y..inner.bottom()).zip(entries.iter().enumerate().skip(first))
    {
        let row = Rect::new(inner.x, y, inner.width, 1);
        if *entry == MenuEntry::Separator {
            frame.render_widget(
                Line::from(glyphs::H_LINE.repeat(room)).fg(theme.border),
                row,
            );
            continue;
        }
        let (title, keys) = label(entry);
        let gap = room.saturating_sub(title.width() + keys.width() + 2);
        let text = format!(" {title}{} {keys}", " ".repeat(gap));
        let line = Line::from(text);
        let line = if index == state.entry {
            line.style(theme.selected.add_modifier(Modifier::BOLD))
        } else {
            line.fg(theme.fg_primary)
        };
        frame.render_widget(line, row);
        cx.ui.hits.push(row, Hit::MenuItem { entry: index });
    }
}

fn relative(ws: &Workspace, path: &std::path::Path) -> String {
    ws.display_path(path).display().to_string()
}

pub fn render_file_prompt(frame: &mut Frame, screen: Rect, cx: &mut Ctx<'_>) {
    let Some(prompt) = cx.ws.file_prompt() else {
        return;
    };
    let theme = cx.theme;
    let (title, place, asks) = match &prompt.kind {
        FilePromptKind::NewFile { dir } => (" New File ", relative(cx.ws, dir), false),
        FilePromptKind::NewFolder { dir } => (" New Folder ", relative(cx.ws, dir), false),
        FilePromptKind::FromTemplate { dir, index } => (
            " New from Template ",
            format!(
                "{} in {}",
                FILE_TEMPLATES.get(*index).map_or("", |t| t.label),
                relative(cx.ws, dir)
            ),
            false,
        ),
        FilePromptKind::NewProject { index } => (
            " New Project ",
            format!(
                "{}: where to create it",
                PROJECT_TEMPLATES.get(*index).map_or("", |t| t.label)
            ),
            false,
        ),
        FilePromptKind::Rename { path } => (" Rename ", relative(cx.ws, path), false),
        FilePromptKind::Delete { path } => (" Delete ", relative(cx.ws, path), true),
        FilePromptKind::BreakpointCondition { path, line } => (
            " Breakpoint Condition ",
            format!(
                "{}:{}, stops only if true; empty: always",
                relative(cx.ws, path),
                line + 1
            ),
            false,
        ),
        FilePromptKind::BreakpointLog { path, line } => (
            " Logpoint ",
            format!(
                "{}:{}, prints instead of stopping, {{x}} for values",
                relative(cx.ws, path),
                line + 1
            ),
            false,
        ),
        FilePromptKind::DebugEvaluate => (
            " Evaluate Expression ",
            "In the selected frame; the answer goes to the console".to_owned(),
            false,
        ),
        FilePromptKind::DebugWatch => (
            " Add Watch ",
            "Shown above the variables at every stop".to_owned(),
            false,
        ),
        FilePromptKind::RunAnything { .. } => (
            " Run Anything ",
            format!("In your shell, in {}; ↑↓ earlier", cx.ws.root().display()),
            false,
        ),
        FilePromptKind::TrustProject { why } => (
            " Trust This Project? ",
            why.clone().unwrap_or_else(|| {
                "Trusting lets it run: servers, git, builds, requests".to_owned()
            }),
            true,
        ),
        FilePromptKind::RenameSymbol { old } => (
            " Rename Symbol ",
            format!("{old}: the language server finds every use"),
            false,
        ),
        FilePromptKind::RenameConfirm { summary } => (" Apply Refactoring ", summary.clone(), true),
        FilePromptKind::CoverageReport => (
            " Load Coverage Report ",
            "lcov or Cobertura, from the project; empty: look where tools write it".to_owned(),
            false,
        ),
        FilePromptKind::SafeDeleteConfirm { summary } => (" Safe Delete ", summary.clone(), true),
    };
    let width = (screen.width * 3 / 5).clamp(30, 70).min(screen.width);
    let height = 5.min(screen.height);
    let area = Rect::new(
        screen.x + (screen.width - width) / 2,
        screen.y + screen.height / 4,
        width,
        height,
    )
    .intersection(screen);
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.border_popup))
        .style(Style::new().bg(theme.bg_popup))
        .title(Line::from(title).fg(theme.fg_primary).bold());
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    cx.ui.hits.push(screen, Hit::Picker);
    let room = usize::from(inner.width).saturating_sub(1);
    let row = |n: u16| Rect::new(inner.x, inner.y + n, inner.width, 1);
    if inner.height < 3 {
        return;
    }
    frame.render_widget(
        Line::from(format!(" {}", ellipsize(&place, room))).fg(theme.fg_muted),
        row(0),
    );
    let foot = match (&prompt.error, prompt.busy, asks) {
        (Some(error), _, _) => Line::from(format!(" {}", ellipsize(error, room))).fg(theme.error),
        (None, true, _) => Line::from(" Working…").fg(theme.fg_muted),
        (None, false, true) => Line::from(match prompt.kind {
            FilePromptKind::Delete { .. } => " ⏎ or Y moves it to the Trash  Esc keeps it",
            FilePromptKind::TrustProject { .. } => " Y trust  N do not  Esc ask again later",
            FilePromptKind::SafeDeleteConfirm { .. } => " Y delete  Esc keep",
            _ => " ⏎ or Y apply  Esc cancel",
        })
        .fg(theme.fg_muted),
        (None, false, false) => Line::from(" ⏎ confirm  Esc cancel").fg(theme.fg_muted),
    };
    frame.render_widget(foot, row(2));
    if asks {
        frame.render_widget(
            Line::from(match prompt.kind {
                FilePromptKind::Delete { .. } => " Move to the Trash?",
                _ => " Are you sure?",
            })
            .fg(theme.fg_primary)
            .bold(),
            row(1),
        );
        return;
    }
    let text = prompt.input.text();
    let before: String = text.chars().take(prompt.input.cursor()).collect();
    let skip = (before.width() + 2).saturating_sub(room);
    let shown: String = text.chars().skip(skip).collect();
    frame.render_widget(
        Line::from(format!(
            "{} {}",
            glyphs::PROMPT,
            ellipsize(&shown, room.saturating_sub(2))
        ))
        .fg(theme.fg_primary),
        row(1),
    );
    let column = u16::try_from((before.width() + 2).min(room)).unwrap_or(0);
    frame.set_cursor_position(Position::new(inner.x + column, row(1).y));
}
