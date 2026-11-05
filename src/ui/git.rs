use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::glyphs;
use super::history::{age, diff_line};
use super::mouse::Hit;
use super::render::Ctx;
use super::text::ellipsize;
use crate::core::{
    CommitMode, DiffKind, DiffRow, Focus, GitChange, GitField, GitFile, GitOperation, GitPanel,
    GitRefKind, GitTab, GraphCell, GraphRow, SideRow, side_by_side,
};

const FILES_WIDTH: u16 = 38;
const SIDE_BY_SIDE_WIDTH: u16 = 120;
const SIDE_BY_SIDE_LOG_WIDTH: u16 = 170;
const MAX_MESSAGE_ROWS: usize = 6;
const WIDE_LOG_WIDTH: u16 = 100;
const COMMIT_DETAILS_WIDTH: u16 = 60;
const SHORT_HASH: usize = 8;
const AUTHOR_WIDTH: usize = 10;
const AGE_WIDTH: usize = 10;

pub fn render_git(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let Some(panel) = cx.ws.git_panel() else {
        return;
    };
    cx.ui.git_panel_width = area.width;
    let theme = cx.theme;
    frame
        .buffer_mut()
        .set_style(area, Style::new().bg(theme.bg_panel));
    cx.ui.hits.push(area, Hit::GitPanel);
    let [title, body, footer] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);
    render_title(frame, title, panel, cx);
    let line = match &panel.notice {
        Some(n) => {
            let color = if n.error { theme.error } else { theme.success };
            Line::from(format!(" {}", n.text)).fg(color)
        }
        None => hint_line(hints(panel), cx),
    };
    frame.render_widget(line.bg(theme.bg_panel), footer);
    if let Some(reason) = &panel.unavailable {
        frame.render_widget(Line::from(format!(" {reason}")).fg(theme.fg_muted), body);
        return;
    }
    match panel.tab {
        GitTab::Commit => render_commit_view(frame, body, panel, cx),
        GitTab::Log => render_log_view(frame, body, panel, cx),
        GitTab::History => render_history_view(frame, body, panel, cx),
        GitTab::Stash => render_stash_view(frame, body, panel, cx),
        GitTab::Branches => render_branches_view(frame, body, panel, cx),
        GitTab::Compare => render_compare_view(frame, body, panel, cx),
    }
}

fn hint_line(hints: &str, cx: &Ctx<'_>) -> Line<'static> {
    let theme = cx.theme;
    let mut spans = Vec::new();
    for hint in hints.split("  ").map(str::trim).filter(|h| !h.is_empty()) {
        let (key, what) = hint.split_once(' ').unwrap_or((hint, ""));
        spans.push(Span::raw(format!(" {key}")).fg(theme.accent));
        spans.push(Span::raw(format!(" {what} ")).fg(theme.fg_muted));
    }
    Line::from(spans)
}

fn author_color(author: &str, cx: &Ctx<'_>) -> Color {
    let palette = lane_palette(cx);
    let hash = author.bytes().fold(0usize, |h, b| {
        h.wrapping_mul(31).wrapping_add(usize::from(b))
    });
    palette[hash % palette.len()]
}

fn lane_palette(cx: &Ctx<'_>) -> [Color; 5] {
    let theme = cx.theme;
    [
        theme.accent,
        theme.success,
        theme.syntax.keyword,
        theme.syntax.property,
        theme.syntax.number,
    ]
}

fn ref_color(kind: GitRefKind, cx: &Ctx<'_>) -> Color {
    let theme = cx.theme;
    match kind {
        GitRefKind::Head | GitRefKind::Local => theme.success,
        GitRefKind::Remote => theme.syntax.function,
        GitRefKind::Tag => theme.warning,
    }
}

fn active(field: GitField, panel: &GitPanel, cx: &Ctx<'_>) -> bool {
    cx.ws.focus() == Focus::Git && cx.ws.picker().is_none() && panel.field == field
}

pub(super) fn operation_label(operation: GitOperation) -> &'static str {
    match operation {
        GitOperation::Merging => "MERGING",
        GitOperation::Rebasing => "REBASING",
        GitOperation::CherryPicking => "CHERRY-PICKING",
        GitOperation::Reverting => "REVERTING",
    }
}

fn hints(panel: &GitPanel) -> &'static str {
    let in_conflict = panel
        .selected_file()
        .is_some_and(|f| f.change == GitChange::Conflicted);
    match (panel.tab, panel.field) {
        (GitTab::Commit, GitField::List) if in_conflict => {
            " ↑↓ file  N/P conflict  O ours  T theirs  B both  Space resolved  ⏎ open  C continue  X abort  Esc editor"
        }
        (GitTab::Commit, GitField::List) if panel.repo.operation.is_some() => {
            " ↑↓ file  C continue  X abort  Space stage  Tab hunks  ⏎ open  L log  Esc editor"
        }
        (GitTab::Stash, GitField::StashMessage) => " ⏎ stash  Tab untracked files too  Esc cancel",
        (GitTab::Stash, _) => {
            " ↑↓ stash  N new  A apply  O pop  D drop  PgUp/PgDn scroll  C commit  L log  G branches  Esc editor"
        }
        (GitTab::Branches, _) => {
            " ↑↓ branch  ⏎ switch  N new  M merge into current  D delete  C commit  L log  Z stash  Esc editor"
        }
        (GitTab::Commit, GitField::Message) => " ⏎ commit   Alt+⏎ new line   Tab files   Esc back",
        (GitTab::Commit, GitField::Hunks) => {
            " ↑↓ hunk  Space stage/unstage  ⏎ pick lines  PgUp/PgDn scroll  Tab message  Esc files"
        }
        (GitTab::Commit, GitField::HunkLines) => {
            " ↑↓ line  Space tick  A all  ⏎ stage/unstage ticked (or this line)  Esc back"
        }
        (GitTab::Commit, _) if panel.commit_mode() == CommitMode::Staged => {
            " ↑↓ file  Space stage file  A all  Tab hunks  S ticked  ⏎ open  M amend  U rollback  B branches  L log  H history  Z stash  G branches  Esc editor"
        }
        (GitTab::Commit, _) if over_staged(panel) => {
            " ~ partly staged, ticked: the commit takes all of it  S staged only  Space tick  Tab hunks  Esc editor"
        }
        (GitTab::Commit, _) => {
            " ↑↓ file  Space tick  A all  Tab hunks  S staged  ⏎ open  M amend  U rollback  B branch  L log  H history  Z stash  G branches  Esc editor"
        }
        (GitTab::Log, GitField::CommitFiles) => {
            " ↑↓ file  PgUp/PgDn scroll  ⏎ open  Tab commits  Esc editor"
        }
        (GitTab::Log, GitField::Filter) => " type to filter   ⏎ done   Esc clear",
        (GitTab::Compare, _) => " ↑↓ commit or file  ⏎ open file  L log  C commit  Esc back",
        (GitTab::History, _) => {
            " ↑↓ commit  PgUp/PgDn scroll  ⏎ open current file  L log  C commit  Z stash  G branches  Esc editor"
        }
        (GitTab::Log, _) => {
            " ↑↓ commit  Tab files  / filter  Y hash  C commit  H file history  Z stash  G branches  W window  R refresh  Esc editor"
        }
    }
}

fn render_title(frame: &mut Frame, area: Rect, panel: &GitPanel, cx: &Ctx<'_>) {
    let theme = cx.theme;
    let tab = |name: &'static str, on: bool| {
        if on {
            Span::raw(format!(" {name} "))
                .fg(theme.accent)
                .bold()
                .underlined()
        } else {
            Span::raw(format!(" {name} ")).fg(theme.fg_muted)
        }
    };
    let mut spans = vec![
        tab("Commit", panel.tab == GitTab::Commit),
        tab("Log", panel.tab == GitTab::Log),
        tab("History", panel.tab == GitTab::History),
        tab("Stash", panel.tab == GitTab::Stash),
        tab("Branches", panel.tab == GitTab::Branches),
    ];
    if panel.compare.is_some() {
        spans.push(tab("Compare", panel.tab == GitTab::Compare));
    }
    if let Some(branch) = cx.ws.git_branch() {
        spans.push(Span::raw(format!("   ⎇ {branch}")).fg(theme.accent));
    }
    let conflicts = panel
        .files
        .iter()
        .filter(|f| f.change == GitChange::Conflicted)
        .count();
    if let Some(operation) = panel.repo.operation {
        spans.push(
            Span::raw(format!("   {}", operation_label(operation)))
                .fg(theme.warning)
                .bold(),
        );
    }
    if conflicts > 0 {
        spans.push(Span::raw(format!("   {conflicts} conflicted")).fg(theme.error));
    }
    if panel.busy {
        spans.push(Span::raw(format!("   {} working…", glyphs::SPINNER)).fg(theme.fg_muted));
    }
    frame.render_widget(Line::from(spans).bg(theme.bg_panel), area);
}

fn divider(frame: &mut Frame, area: Rect, cx: &Ctx<'_>) {
    for y in area.y..area.bottom() {
        frame.render_widget(
            Line::from(glyphs::V_LINE)
                .fg(cx.theme.border)
                .bg(cx.theme.bg),
            Rect::new(area.x, y, 1, 1),
        );
    }
}

pub(super) fn change_color(change: GitChange, cx: &Ctx<'_>) -> Color {
    let theme = cx.theme;
    match change {
        GitChange::Added | GitChange::Untracked => theme.success,
        GitChange::Modified | GitChange::Renamed => theme.accent,
        GitChange::Deleted => theme.fg_muted,
        GitChange::Conflicted => theme.error,
    }
}

fn file_label(file: &GitFile) -> String {
    match &file.old_path {
        Some(old) => format!("{} ← {}", file.path.display(), old.display()),
        None => file.path.display().to_string(),
    }
}

fn stage_cells(panel: &GitPanel, file: &GitFile) -> (&'static str, &'static str) {
    let code = file.code;
    if code.conflicted() {
        return ("[!]", " ");
    }
    if panel.commit_mode() == CommitMode::Staged {
        let state = match (code.staged(), code.unstaged()) {
            (true, false) => "[s]",
            (true, true) => "[~]",
            _ => "[ ]",
        };
        return (state, " ");
    }
    let tick = if panel.checked.contains(&file.path) {
        "[x]"
    } else {
        "[ ]"
    };
    (tick, if code.staged() { "~" } else { " " })
}

fn commit_heading(panel: &GitPanel) -> (String, bool) {
    let staged = panel.files.iter().filter(|f| f.code.staged()).count();
    let verb = match panel.repo.operation {
        Some(GitOperation::Merging) => "Finish merge:",
        Some(_) => "Commit:",
        None if panel.amend => "Amend:",
        None => "Commit",
    };
    match panel.commit_mode() {
        CommitMode::Staged => (format!(" {verb} STAGED only ({staged} files)"), false),
        CommitMode::Ticked => {
            let count = panel.checked.len();
            if panel.amend && count == 0 {
                return (format!(" {verb} message only"), false);
            }
            (
                format!(" {verb} TICKED, whole files ({count})"),
                over_staged(panel),
            )
        }
    }
}

fn over_staged(panel: &GitPanel) -> bool {
    panel
        .files
        .iter()
        .any(|f| f.code.staged() && panel.checked.contains(&f.path))
}

fn render_commit_view(frame: &mut Frame, area: Rect, panel: &GitPanel, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let width = FILES_WIDTH.max(area.width / 3).min(area.width / 2);
    let [left, line, right] = Layout::horizontal([
        Constraint::Length(width),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    divider(frame, line, cx);
    let message_rows = panel
        .message
        .text()
        .split('\n')
        .count()
        .clamp(1, MAX_MESSAGE_ROWS.min(usize::from(area.height) / 3).max(1));
    let [list, message] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(message_rows as u16 + 1),
    ])
    .areas(left);
    if panel.files.is_empty() {
        frame.render_widget(Line::from(" No changes").fg(theme.fg_muted), list);
    }
    let rows = usize::from(list.height);
    let first = (panel.file_selected + 1).saturating_sub(rows);
    for (n, file) in panel.files.iter().enumerate().skip(first).take(rows) {
        let row = Rect::new(list.x, list.y + (n - first) as u16, list.width, 1);
        cx.ui.hits.push(row, Hit::GitRow { index: n });
        let (tick, mark) = stage_cells(panel, file);
        let room = usize::from(row.width).saturating_sub(8);
        let selected = n == panel.file_selected;
        let style = match (selected, active(GitField::List, panel, cx)) {
            (true, true) => theme.selected,
            (true, false) => theme.selected_inactive,
            _ => Style::new(),
        };
        let line = Line::from(vec![
            Span::raw(format!(" {tick} ")).fg(theme.fg_muted),
            Span::raw(mark).fg(theme.warning),
            Span::raw(format!("{} ", file.change.letter())).fg(change_color(file.change, cx)),
            Span::raw(ellipsize(&file_label(file), room)).fg(theme.fg_primary),
        ]);
        frame.render_widget(line.style(style), row);
        if let Some(list_name) = cx.ws.changelists().name_of(&file.path) {
            frame.render_widget(
                Line::from(format!("{list_name} "))
                    .fg(theme.fg_muted)
                    .right_aligned(),
                row,
            );
        }
    }
    let (heading, warn) = commit_heading(panel);
    frame.render_widget(
        Line::from(ellipsize(&heading, usize::from(message.width))).fg(if panel.amend || warn {
            theme.warning
        } else {
            theme.fg_muted
        }),
        Rect::new(message.x, message.y, message.width, 1),
    );
    let prompt = format!(" {} ", glyphs::PROMPT);
    let text = panel.message.text();
    let before: String = text.chars().take(panel.message.cursor()).collect();
    let cursor_line = before.matches('\n').count();
    let cursor_col = before.rsplit('\n').next().unwrap_or("").width();
    let first = (cursor_line + 1).saturating_sub(message_rows);
    for (n, line) in text.split('\n').enumerate().skip(first).take(message_rows) {
        let row = Rect::new(
            message.x,
            message.y + 1 + (n - first) as u16,
            message.width,
            1,
        );
        let lead = if n == 0 {
            Span::raw(prompt.clone()).fg(theme.accent)
        } else {
            Span::raw(" ".repeat(prompt.width()))
        };
        let shown = if text.is_empty() && panel.field != GitField::Message {
            Span::raw("Tab to write").fg(theme.fg_subtle).italic()
        } else {
            Span::raw(line.to_owned()).fg(theme.fg_primary)
        };
        frame.render_widget(Line::from(vec![lead, shown]), row);
    }
    if active(GitField::Message, panel, cx) {
        let x = message.x + (prompt.width() + cursor_col) as u16;
        let y = message.y + 1 + (cursor_line - first) as u16;
        frame.set_cursor_position(Position::new(x.min(message.right().saturating_sub(1)), y));
    }
    render_diff(frame, right, panel, cx);
}

fn render_log_view(frame: &mut Frame, area: Rect, panel: &GitPanel, cx: &mut Ctx<'_>) {
    let show_diff = area.width >= WIDE_LOG_WIDTH;
    let two_columns = panel.side_by_side && area.width >= SIDE_BY_SIDE_LOG_WIDTH;
    let files_width = if two_columns {
        area.width / 7
    } else if show_diff {
        area.width / 5
    } else {
        area.width / 3
    };
    let list_share = if two_columns {
        40
    } else if show_diff {
        55
    } else {
        62
    };
    let [commits, line, rest] = Layout::horizontal([
        Constraint::Percentage(list_share),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    divider(frame, line, cx);
    render_commits(frame, commits, panel, cx);
    if !show_diff {
        render_commit_files(frame, rest, panel, cx);
        return;
    }
    let [files, line, diff] = Layout::horizontal([
        Constraint::Length(files_width.min(rest.width / 2)),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(rest);
    divider(frame, line, cx);
    render_commit_files(frame, files, panel, cx);
    render_diff(frame, diff, panel, cx);
}

fn render_history_view(frame: &mut Frame, area: Rect, panel: &GitPanel, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let width = (area.width * 2 / 5).max(FILES_WIDTH.min(area.width));
    let [list, line, diff] = Layout::horizontal([
        Constraint::Length(width),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    divider(frame, line, cx);
    let name = panel
        .history_path
        .as_ref()
        .map_or_else(String::new, |p| cx.ws.display_path(p).display().to_string());
    let [head, rows] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(list);
    frame.render_widget(
        Line::from(ellipsize(&format!(" {name}"), usize::from(head.width)))
            .fg(theme.accent)
            .bold(),
        head,
    );
    if panel.history.is_empty() {
        let text = if panel.history_loading {
            " Loading…"
        } else {
            " No commits have this file"
        };
        frame.render_widget(Line::from(text).fg(theme.fg_muted), rows);
    }
    let height = usize::from(rows.height);
    let first = (panel.history_selected + 1).saturating_sub(height);
    for (n, entry) in panel.history.iter().enumerate().skip(first).take(height) {
        let row = Rect::new(rows.x, rows.y + (n - first) as u16, rows.width, 1);
        cx.ui.hits.push(row, Hit::GitRow { index: n });
        let style = match (
            n == panel.history_selected,
            active(GitField::List, panel, cx),
        ) {
            (true, true) => theme.selected,
            (true, false) => theme.selected_inactive,
            _ => Style::new(),
        };
        let commit = &entry.commit;
        let lead = format!(" {:.SHORT_HASH$} ", commit.hash);
        let when = format!(" {:>AGE_WIDTH$} ", age(panel.now, commit.time));
        let room = usize::from(row.width).saturating_sub(lead.width() + when.width());
        let line = Line::from(vec![
            Span::raw(lead).fg(theme.syntax.attribute),
            Span::raw(ellipsize(&commit.subject, room)).fg(theme.fg_primary),
        ]);
        frame.render_widget(line.style(style), row);
        frame.render_widget(Line::from(when).fg(theme.fg_muted).right_aligned(), row);
    }
    render_diff(frame, diff, panel, cx);
}

fn render_compare_view(frame: &mut Frame, area: Rect, panel: &GitPanel, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let Some(compare) = panel.compare.as_ref() else {
        return;
    };
    let width = (area.width * 2 / 5).max(FILES_WIDTH.min(area.width));
    let [list, line, diff] = Layout::horizontal([
        Constraint::Length(width),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    divider(frame, line, cx);
    let title = match &compare.path {
        Some(path) => format!(
            " {} now against {}",
            cx.ws.display_path(path).display(),
            compare.base
        ),
        None => format!(" This branch against {}", compare.base),
    };
    let [head, rows] = Layout::vertical([Constraint::Length(1), Constraint::Fill(1)]).areas(list);
    frame.render_widget(
        Line::from(ellipsize(&title, usize::from(head.width)))
            .fg(theme.accent)
            .bold(),
        head,
    );
    if compare.row_count() == 0 {
        let text = if compare.loading {
            " Loading…"
        } else {
            " No difference"
        };
        frame.render_widget(Line::from(text).fg(theme.fg_muted), rows);
    }
    let height = usize::from(rows.height);
    let first = (compare.selected + 1).saturating_sub(height);
    for n in (first..compare.row_count()).take(height) {
        let row = Rect::new(rows.x, rows.y + (n - first) as u16, rows.width, 1);
        cx.ui.hits.push(row, Hit::GitRow { index: n });
        let style = match (n == compare.selected, active(GitField::List, panel, cx)) {
            (true, true) => theme.selected,
            (true, false) => theme.selected_inactive,
            _ => Style::new(),
        };
        let room = usize::from(row.width);
        let text = if let Some(entry) = compare.found.commits.get(n) {
            let side = if entry.ours { "↑" } else { "↓" };
            Line::from(vec![
                Span::raw(format!(" {side} {:.SHORT_HASH$} ", entry.commit.hash))
                    .fg(theme.syntax.attribute),
                Span::raw(ellipsize(&entry.commit.subject, room.saturating_sub(12)))
                    .fg(theme.fg_primary),
            ])
        } else if let Some(file) = compare.found.files.get(n - compare.found.commits.len()) {
            Line::from(vec![
                Span::raw(format!(" {} ", file.change.letter())).fg(change_color(file.change, cx)),
                Span::raw(ellipsize(&file_label(file), room.saturating_sub(3)))
                    .fg(theme.fg_primary),
            ])
        } else {
            Line::default()
        };
        frame.render_widget(text.style(style), row);
    }
    render_diff(frame, diff, panel, cx);
}

fn render_stash_view(frame: &mut Frame, area: Rect, panel: &GitPanel, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let width = (area.width * 2 / 5).max(FILES_WIDTH.min(area.width));
    let [left, line, diff] = Layout::horizontal([
        Constraint::Length(width),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(area);
    divider(frame, line, cx);
    let naming = panel.field == GitField::StashMessage;
    let [input, rows] = Layout::vertical([
        Constraint::Length(u16::from(naming) * 2),
        Constraint::Fill(1),
    ])
    .areas(left);
    if naming {
        let scope = if panel.stash_untracked {
            " Stash tracked and untracked files as:"
        } else {
            " Stash tracked files as (untracked stay):"
        };
        frame.render_widget(
            Line::from(scope).fg(theme.fg_muted),
            Rect::new(input.x, input.y, input.width, 1),
        );
        let prompt = format!(" {} ", glyphs::PROMPT);
        let text = panel.stash_message.text();
        let row = Rect::new(input.x, input.y + 1, input.width, 1);
        frame.render_widget(
            Line::from(vec![
                Span::raw(prompt.clone()).fg(theme.accent),
                Span::raw(text.to_owned()).fg(theme.fg_primary),
            ]),
            row,
        );
        if active(GitField::StashMessage, panel, cx) {
            let before: String = text.chars().take(panel.stash_message.cursor()).collect();
            let x = row.x + (prompt.width() + before.width()) as u16;
            frame.set_cursor_position(Position::new(x.min(row.right().saturating_sub(1)), row.y));
        }
    }
    if panel.stashes.is_empty() {
        frame.render_widget(
            Line::from(" No stashes. N makes one").fg(theme.fg_muted),
            rows,
        );
    }
    let height = usize::from(rows.height);
    let first = (panel.stash_selected + 1).saturating_sub(height);
    for (n, entry) in panel.stashes.iter().enumerate().skip(first).take(height) {
        let row = Rect::new(rows.x, rows.y + (n - first) as u16, rows.width, 1);
        cx.ui.hits.push(row, Hit::GitRow { index: n });
        let style = match (n == panel.stash_selected, active(GitField::List, panel, cx)) {
            (true, true) => theme.selected,
            (true, false) => theme.selected_inactive,
            _ => Style::new(),
        };
        let lead = format!(" {{{}}} ", entry.index);
        let when = format!(" {:>AGE_WIDTH$} ", age(panel.now, entry.time));
        let room = usize::from(row.width).saturating_sub(lead.width() + when.width());
        let line = Line::from(vec![
            Span::raw(lead).fg(theme.syntax.attribute),
            Span::raw(ellipsize(&entry.subject, room)).fg(theme.fg_primary),
        ]);
        frame.render_widget(line.style(style), row);
        frame.render_widget(Line::from(when).fg(theme.fg_muted).right_aligned(), row);
    }
    render_diff(frame, diff, panel, cx);
}

fn render_branches_view(frame: &mut Frame, area: Rect, panel: &GitPanel, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let branches = cx.ws.git_branches();
    if branches.is_empty() {
        frame.render_widget(Line::from(" Reading branches…").fg(theme.fg_muted), area);
        return;
    }
    let current = cx.ws.git_branch();
    let height = usize::from(area.height);
    let first = (panel.branch_selected + 1).saturating_sub(height);
    for (n, branch) in branches.iter().enumerate().skip(first).take(height) {
        let row = Rect::new(area.x, area.y + (n - first) as u16, area.width, 1);
        cx.ui.hits.push(row, Hit::GitRow { index: n });
        let style = match (
            n == panel.branch_selected,
            active(GitField::List, panel, cx),
        ) {
            (true, true) => theme.selected,
            (true, false) => theme.selected_inactive,
            _ => Style::new(),
        };
        let here = current == Some(branch.name.as_str());
        let color = if branch.remote {
            theme.syntax.function
        } else {
            theme.success
        };
        let name = Span::raw(branch.name.clone()).fg(color);
        let line = Line::from(vec![
            Span::raw(if here { " * " } else { "   " }).fg(theme.accent),
            if here { name.bold() } else { name },
        ]);
        frame.render_widget(line.style(style), row);
    }
}

fn graph_spans(row: Option<&GraphRow>, cx: &Ctx<'_>) -> Vec<Span<'static>> {
    let Some(row) = row else {
        return Vec::new();
    };
    let palette = lane_palette(cx);
    let color = |i: usize| palette[i % palette.len()];
    let inside = |i: usize| row.span.is_some_and(|(low, high)| i >= low && i < high);
    let crossed = |i: usize| row.span.is_some_and(|(low, high)| i > low && i < high);
    let mut spans = Vec::with_capacity(row.cells.len() * 2);
    for (i, cell) in row.cells.iter().enumerate() {
        let glyph = match cell {
            GraphCell::Commit => "●",
            GraphCell::Join => "╯",
            GraphCell::Fork => "╮",
            GraphCell::Line if crossed(i) => "┼",
            GraphCell::Line => "│",
            GraphCell::Empty if crossed(i) => "─",
            GraphCell::Empty => " ",
        };
        spans.push(Span::raw(glyph).fg(color(i)));
        let after = if inside(i) { "─" } else { " " };
        spans.push(Span::raw(after).fg(color(i)));
    }
    spans
}

fn render_commits(frame: &mut Frame, area: Rect, panel: &GitPanel, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let filtering = panel.field == GitField::Filter || !panel.filter.text().is_empty();
    let [filter, list] = Layout::vertical([
        Constraint::Length(u16::from(filtering)),
        Constraint::Fill(1),
    ])
    .areas(area);
    if filtering {
        let prompt = " / ";
        let text = panel.filter.text();
        frame.render_widget(
            Line::from(vec![
                Span::raw(prompt).fg(theme.accent),
                Span::raw(text.to_owned()).fg(theme.fg_primary),
            ])
            .bg(theme.bg_panel),
            filter,
        );
        if active(GitField::Filter, panel, cx) {
            let before: String = text.chars().take(panel.filter.cursor()).collect();
            let x = filter.x + (prompt.width() + before.width()) as u16;
            frame.set_cursor_position(Position::new(x, filter.y));
        }
    }
    if panel.visible.is_empty() {
        let text = if panel.commits.is_empty() {
            " No commits yet"
        } else {
            " Nothing matches"
        };
        frame.render_widget(Line::from(text).fg(theme.fg_muted), list);
        return;
    }
    let rows = usize::from(list.height);
    let first = (panel.commit_selected + 1).saturating_sub(rows);
    let wide = list.width >= COMMIT_DETAILS_WIDTH;
    for (n, index) in panel.visible.iter().enumerate().skip(first).take(rows) {
        let Some(commit) = panel.commits.get(*index) else {
            continue;
        };
        let row = Rect::new(list.x, list.y + (n - first) as u16, list.width, 1);
        cx.ui.hits.push(row, Hit::GitRow { index: n });
        let selected = n == panel.commit_selected;
        let style = match (selected, active(GitField::List, panel, cx)) {
            (true, true) => theme.selected,
            (true, false) => theme.selected_inactive,
            _ => Style::new(),
        };
        let mut spans = vec![Span::raw(" ")];
        if panel.filter.text().is_empty() {
            spans.extend(graph_spans(panel.graph.get(*index), cx));
        }
        spans.push(Span::raw(" "));
        for name in &commit.refs {
            let chip = Span::raw(format!("[{}]", name.name)).fg(ref_color(name.kind, cx));
            spans.push(if name.kind == GitRefKind::Head {
                chip.bold()
            } else {
                chip
            });
            spans.push(Span::raw(" "));
        }
        let used: usize = spans.iter().map(Span::width).sum();
        let tail = if wide {
            AUTHOR_WIDTH + AGE_WIDTH + SHORT_HASH + 5
        } else {
            0
        };
        let room = usize::from(row.width).saturating_sub(used + tail);
        let subject_color = if commit.parents.len() > 1 {
            theme.fg_muted
        } else {
            theme.fg_primary
        };
        spans.push(Span::raw(ellipsize(&commit.subject, room)).fg(subject_color));
        frame.render_widget(Line::from(spans).style(style), row);
        if wide {
            let right = Line::from(vec![
                Span::raw(format!(
                    "{:<AUTHOR_WIDTH$} ",
                    ellipsize(&commit.author, AUTHOR_WIDTH)
                ))
                .fg(author_color(&commit.author, cx)),
                Span::raw(format!("{:>AGE_WIDTH$}  ", age(panel.now, commit.time)))
                    .fg(theme.fg_muted),
                Span::raw(format!("{:.SHORT_HASH$} ", commit.hash)).fg(theme.syntax.attribute),
            ]);
            frame.render_widget(right.right_aligned(), row);
        }
    }
}

fn render_commit_files(frame: &mut Frame, area: Rect, panel: &GitPanel, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let rows = usize::from(area.height);
    let first = (panel.commit_file_selected + 1).saturating_sub(rows);
    for (n, file) in panel.commit_files.iter().enumerate().skip(first).take(rows) {
        let row = Rect::new(area.x, area.y + (n - first) as u16, area.width, 1);
        let selected = n == panel.commit_file_selected;
        let style = match (selected, active(GitField::CommitFiles, panel, cx)) {
            (true, true) => theme.selected,
            (true, false) => theme.selected_inactive,
            _ => Style::new(),
        };
        let room = usize::from(row.width).saturating_sub(3);
        let line = Line::from(vec![
            Span::raw(format!(" {} ", file.change.letter())).fg(change_color(file.change, cx)),
            Span::raw(ellipsize(&file_label(file), room)).fg(theme.fg_primary),
        ]);
        frame.render_widget(line.style(style), row);
    }
}

fn render_side_by_side(frame: &mut Frame, area: Rect, panel: &GitPanel, cx: &mut Ctx<'_>) {
    let chosen = panel
        .hunk_starts
        .get(panel.hunk_selected)
        .filter(|_| panel.field == GitField::Hunks && panel.tab == GitTab::Commit);
    let half = area.width.saturating_sub(1) / 2;
    let rows = side_by_side(&panel.diff);
    let visible = rows
        .iter()
        .skip_while(|r| r.last_index() < panel.diff_scroll);
    for (y, row) in (area.y..area.bottom()).zip(visible) {
        let whole = Rect::new(area.x, y, area.width, 1);
        match *row {
            SideRow::Across(n) => {
                let Some(row) = panel.diff.get(n) else {
                    continue;
                };
                let line = diff_line(row, cx);
                let line = if chosen == Some(&n) && row.kind == DiffKind::Hunk {
                    line.style(cx.theme.selected)
                } else {
                    line
                };
                frame.render_widget(line, whole);
            }
            SideRow::Pair(left, right) => {
                let cell = |n: Option<usize>, old: bool| {
                    n.and_then(|n| panel.diff.get(n))
                        .map(|row| side_cell(row, old, cx))
                };
                if let Some(line) = cell(left, true) {
                    frame.render_widget(line, Rect::new(area.x, y, half, 1));
                }
                frame.render_widget(
                    Line::from(glyphs::V_LINE).fg(cx.theme.border),
                    Rect::new(area.x + half, y, 1, 1),
                );
                if let Some(line) = cell(right, false) {
                    let x = area.x + half + 1;
                    frame.render_widget(line, Rect::new(x, y, area.right().saturating_sub(x), 1));
                }
            }
        }
    }
}

fn side_cell(row: &DiffRow, old: bool, cx: &Ctx<'_>) -> Line<'static> {
    let theme = cx.theme;
    let number = if old { row.old_line } else { row.new_line };
    let number = number.map_or_else(|| "    ".to_owned(), |n| format!("{n:>4}"));
    let (sign, style) = match row.kind {
        DiffKind::Removed => ("-", theme.diff_removed),
        DiffKind::Added => ("+", theme.diff_added),
        _ => (" ", Style::new().fg(theme.fg_primary)),
    };
    Line::from(vec![
        Span::raw(format!("{number} ")).fg(theme.fg_muted),
        Span::raw(format!("{sign} {}", row.text.replace('\t', "    "))),
    ])
    .style(style)
}

fn render_diff(frame: &mut Frame, area: Rect, panel: &GitPanel, cx: &mut Ctx<'_>) {
    if cx.ws.focus() == Focus::Git {
        cx.ui.page_rows = usize::from(area.height);
    }
    if panel.diff.is_empty() {
        let text = if panel.diff_request.is_some() {
            " Loading…"
        } else if panel.tab == GitTab::History && !panel.history.is_empty() {
            " No difference from the first parent"
        } else {
            ""
        };
        frame.render_widget(Line::from(text).fg(cx.theme.fg_muted), area);
        return;
    }
    let enough = if panel.tab == GitTab::Log {
        SIDE_BY_SIDE_LOG_WIDTH
    } else {
        SIDE_BY_SIDE_WIDTH
    };
    if panel.side_by_side && panel.line_pick.is_none() && cx.ui.git_panel_width >= enough {
        return render_side_by_side(frame, area, panel, cx);
    }
    let chosen = panel
        .hunk_starts
        .get(panel.hunk_selected)
        .filter(|_| panel.field == GitField::Hunks && panel.tab == GitTab::Commit);
    let rows = panel.diff.iter().enumerate().skip(panel.diff_scroll);
    for (y, (n, row)) in (area.y..area.bottom()).zip(rows) {
        let line = diff_line(row, cx);
        let line = if chosen == Some(&n) && row.kind == DiffKind::Hunk {
            line.style(cx.theme.selected)
        } else {
            line
        };
        frame.render_widget(
            picked_line(line, n, panel, cx),
            Rect::new(area.x, y, area.width, 1),
        );
    }
}

fn picked_line(line: Line<'static>, row: usize, panel: &GitPanel, cx: &Ctx<'_>) -> Line<'static> {
    let Some(pick) = &panel.line_pick else {
        return line;
    };
    let which = panel.changed_line_at(row);
    let mark = match which {
        Some(n) if pick.picked.contains(&n) => "[x]",
        Some(_) => "[ ]",
        None => "   ",
    };
    let at_cursor = which == Some(pick.cursor);
    let mut spans = vec![
        Span::raw(if at_cursor { glyphs::SELECTED } else { " " }).fg(cx.theme.accent),
        Span::raw(mark).fg(cx.theme.fg_primary),
    ];
    let style = line.style;
    spans.extend(line.spans);
    let out = Line::from(spans).style(style);
    if at_cursor {
        out.patch_style(cx.theme.selected)
    } else {
        out
    }
}
