use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, BorderType, Clear};
use unicode_width::UnicodeWidthStr;

use super::glyphs;
use super::mouse::Hit;
use super::render::{Ctx, ensure_visible};
use super::text::highlighted;
use crate::core::{Action, PickerKind, Workspace};

const MAX_ROWS: u16 = 12;

pub fn render_picker(frame: &mut Frame, screen: Rect, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let Some(picker) = cx.ws.picker() else {
        return;
    };
    let items = cx.ws.picker_items(picker.kind);
    let width = (screen.width * 3 / 5)
        .clamp(40, 80)
        .min(screen.width.saturating_sub(2));
    let list_rows = (picker.matches.len() as u16).clamp(1, MAX_ROWS);
    let height = (list_rows + 4).min(screen.height.saturating_sub(2));
    let area = Rect::new(
        screen.x + (screen.width - width) / 2,
        screen.y + screen.height / 6,
        width,
        height,
    )
    .intersection(screen);

    let title = match picker.kind {
        PickerKind::Actions => " Actions ",
        PickerKind::Files => " Files ",
        PickerKind::RecentFiles => " Recent Files ",
        PickerKind::Everywhere => " Search Everywhere ",
        PickerKind::Locations => " Definitions ",
        PickerKind::Notifications => " Notifications ",
        PickerKind::Servers => " Language Server ",
        PickerKind::CodeActions => " Context Actions ",
        PickerKind::Refactorings => " Refactor This ",
        PickerKind::CompareBranches => " Compare with Branch ",
        PickerKind::Changelists => " Move to Changelist: choose or type a new name ",
        PickerKind::Branches => " Branches ",
        PickerKind::Run => " Run ",
        PickerKind::Symbols => " File Structure ",
        PickerKind::ProjectSymbols => " Go to Symbol ",
        PickerKind::Breakpoints => " Breakpoints ",
        PickerKind::DebugTargets => " Debug ",
        PickerKind::DebugExecutables => " Which executable? ",
        PickerKind::DebugThreads => " Threads ",
        PickerKind::FileTemplates => " New from Template ",
        PickerKind::ScratchKinds => " New Scratch File ",
        PickerKind::Scratches => " Scratches ",
        PickerKind::ProjectTemplates => " New Project ",
    };
    let footer = format!(
        " ↑↓ ⏎ Esc ── {}/{} ",
        if picker.matches.is_empty() {
            0
        } else {
            picker.selected + 1
        },
        picker.matches.len()
    );
    let block = Block::bordered()
        .border_type(BorderType::Rounded)
        .border_style(Style::new().fg(theme.border_popup))
        .style(Style::new().bg(theme.bg_popup))
        .title(Line::from(title).fg(theme.fg_primary).bold())
        .title_bottom(Line::from(footer).fg(theme.fg_muted).right_aligned());
    let inner = block.inner(area);
    frame.render_widget(Clear, area);
    frame.render_widget(block, area);
    cx.ui.hits.push(area, Hit::Picker);
    if inner.height < 3 {
        return;
    }

    let prompt = format!("{} ", glyphs::PROMPT);
    let query = picker.input.text();
    let placeholder = match picker.kind {
        PickerKind::Actions => "Type an action name…",
        PickerKind::Files => "Type a file name…",
        PickerKind::Everywhere => "Type a file, symbol or action name…",
        PickerKind::ProjectSymbols => "Type a name declared in this project…",
        PickerKind::RecentFiles
        | PickerKind::Notifications
        | PickerKind::Locations
        | PickerKind::Servers
        | PickerKind::CodeActions
        | PickerKind::Refactorings
        | PickerKind::CompareBranches
        | PickerKind::Changelists
        | PickerKind::Symbols
        | PickerKind::Breakpoints
        | PickerKind::DebugTargets
        | PickerKind::DebugExecutables
        | PickerKind::DebugThreads
        | PickerKind::FileTemplates
        | PickerKind::ScratchKinds
        | PickerKind::Scratches
        | PickerKind::ProjectTemplates
        | PickerKind::Run => "Type to filter…",
        PickerKind::Branches => "Filter, or a new name and Enter…",
    };
    let input = Line::from(vec![
        Span::raw(prompt.clone()).fg(theme.accent),
        if query.is_empty() {
            Span::raw(placeholder).fg(theme.fg_subtle).italic()
        } else {
            Span::raw(query.to_owned()).fg(theme.fg_primary)
        },
    ]);
    frame.render_widget(input, Rect::new(inner.x, inner.y, inner.width, 1));
    let before: String = query.chars().take(picker.input.cursor()).collect();
    frame.set_cursor_position(Position::new(
        (inner.x + (prompt.width() + before.width()) as u16).min(inner.right().saturating_sub(1)),
        inner.y,
    ));
    frame.render_widget(
        Line::from(glyphs::H_LINE.repeat(usize::from(inner.width))).fg(theme.border_popup),
        Rect::new(inner.x, inner.y + 1, inner.width, 1),
    );

    let list = Rect::new(inner.x, inner.y + 2, inner.width, inner.height - 2);
    let rows = usize::from(list.height);
    cx.ui.page_rows = rows;
    cx.ui.picker_scroll =
        ensure_visible((cx.ui.picker_scroll, 0), (picker.selected, 0), (rows, 1)).0;
    if picker.matches.is_empty() {
        frame.render_widget(Line::from("  Nothing found").fg(theme.fg_muted), list);
        return;
    }
    let base = Style::new().fg(theme.fg_primary);
    let hit = Style::new().fg(theme.accent).add_modifier(Modifier::BOLD);
    for (n, m) in picker
        .matches
        .iter()
        .enumerate()
        .skip(cx.ui.picker_scroll)
        .take(rows)
    {
        let y = list.y + (n - cx.ui.picker_scroll) as u16;
        let row = Rect::new(list.x, y, list.width, 1);
        cx.ui.hits.push(row, Hit::PickerItem { index: n });
        let row_style = if n == picker.selected {
            theme.selected
        } else {
            Style::new().bg(theme.bg_popup)
        };
        let everywhere_action = (picker.kind == PickerKind::Everywhere)
            .then(|| Workspace::everywhere_action(m.index, items.len()))
            .flatten();
        let is_file = matches!(picker.kind, PickerKind::Files | PickerKind::RecentFiles)
            || (picker.kind == PickerKind::Everywhere
                && everywhere_action.is_none()
                && cx.ws.everywhere_symbol(m.index, items.len()).is_none());
        let text = if is_file {
            base.fg(theme.file_color(std::path::Path::new(&items[m.index])))
        } else {
            base
        };
        let mut spans = vec![Span::raw("  ")];
        spans.extend(highlighted(
            &items[m.index],
            &m.indices,
            text,
            hit,
            cx.ws.tab_width(),
        ));
        frame.render_widget(Line::from(spans).style(row_style), row);
        if picker.kind == PickerKind::Everywhere {
            let right = match everywhere_action {
                Some(action) => Line::from(vec![
                    Span::raw("Action  ").fg(theme.syntax.function),
                    Span::raw(format!("{:>14}  ", cx.keymap.label(action).unwrap_or("")))
                        .fg(theme.accent),
                ]),
                None if cx.ws.everywhere_symbol(m.index, items.len()).is_some() => {
                    Line::from(Span::raw("Symbol  ").fg(theme.syntax.function))
                }
                None => Line::from(Span::raw("File  ").fg(theme.syntax.keyword)),
            };
            frame.render_widget(right.right_aligned(), row);
        }
        if picker.kind == PickerKind::Actions {
            let action = Action::ALL.get(m.index).copied();
            let category = action.map_or("Plugins", |a| a.spec().category);
            let keys = action.and_then(|a| cx.keymap.label(a)).unwrap_or("");
            let right = Line::from(vec![
                Span::raw(format!("{category:<14}")).fg(theme.category_color(category)),
                Span::raw(format!("{keys:>14}  ")).fg(theme.accent),
            ]);
            frame.render_widget(right.right_aligned(), row);
        }
    }
}
