use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};

use super::mouse::Hit;
use super::render::{Ctx, follow_or_clamp};
use super::text::ellipsize;
use crate::core::run::visible;
use crate::core::test_results::TestState;
use crate::core::{DuplicateRow, Focus};

fn row(area: Rect, y: u16) -> Rect {
    Rect::new(area.x, area.y + y, area.width, 1)
}

fn title(frame: &mut Frame, area: Rect, cx: &Ctx<'_>, focused: bool, spans: Vec<Span<'static>>) {
    let theme = cx.theme;
    let style = if focused {
        Style::new()
            .fg(theme.fg_primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.fg_muted)
    };
    let mut line: Vec<Span<'static>> = Vec::new();
    for (i, span) in spans.into_iter().enumerate() {
        line.push(if i == 0 {
            span.style(style)
        } else {
            span.fg(theme.fg_muted)
        });
    }
    frame.render_widget(Line::from(line).bg(theme.bg_panel), row(area, 0));
}

pub fn render_todo(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let ws = cx.ws;
    let theme = cx.theme;
    let panel = ws.todo_panel();
    let focused = ws.focus() == Focus::Todo && ws.picker().is_none();
    let count = if panel.running.is_some() {
        "   searching…".to_owned()
    } else if panel.truncated {
        format!("   first {}", panel.results.len())
    } else {
        format!("   {}", panel.results.len())
    };
    title(
        frame,
        area,
        cx,
        focused,
        vec![
            Span::raw(" TODO"),
            Span::raw(count),
            Span::raw("   r refresh"),
        ],
    );
    cx.ui.hits.push(area, Hit::TodoPanel);
    let height = usize::from(area.height.saturating_sub(1));
    if focused {
        cx.ui.page_rows = height;
    }
    if panel.results.is_empty() {
        if panel.running.is_none() {
            frame.render_widget(
                Line::from(" No TODO, FIXME, XXX or HACK found").fg(theme.fg_muted),
                row(area, 1),
            );
        }
        return;
    }
    cx.ui.todo_scroll = follow_or_clamp(
        cx.ui.todo_scroll,
        &mut cx.ui.todo_followed,
        panel.selected,
        height,
        panel.results.len(),
    );
    let width = usize::from(area.width);
    for (n, hit) in panel
        .results
        .iter()
        .enumerate()
        .skip(cx.ui.todo_scroll)
        .take(height)
    {
        let y = u16::try_from(n - cx.ui.todo_scroll + 1).unwrap_or(u16::MAX);
        let place = visible(&format!(
            " {}:{}  ",
            ws.display_path(&hit.path).display(),
            hit.line + 1
        ));
        let room = width.saturating_sub(place.chars().count());
        let style = match (n == panel.selected, focused) {
            (true, true) => theme.selected,
            (true, false) => theme.selected_inactive,
            _ => Style::new().bg(theme.bg_panel),
        };
        let line = Line::from(vec![
            Span::raw(place).fg(theme.file_color(&hit.path)),
            Span::raw(ellipsize(&visible(hit.text.trim()), room)).fg(theme.fg_primary),
        ]);
        frame.render_widget(line.style(style), row(area, y));
        cx.ui.hits.push(row(area, y), Hit::TodoRow { index: n });
    }
}

pub fn render_tests(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let ws = cx.ws;
    let theme = cx.theme;
    let panel = ws.test_panel();
    let focused = ws.focus() == Focus::Tests && ws.picker().is_none();
    let counts = format!(
        "   {} failed   {} passed   {} ignored",
        panel.count(TestState::Failed),
        panel.count(TestState::Passed),
        panel.count(TestState::Ignored),
    );
    title(
        frame,
        area,
        cx,
        focused,
        vec![
            Span::raw(" Test Results"),
            Span::raw(counts),
            Span::raw("   Enter go   r again   f only failed"),
        ],
    );
    cx.ui.hits.push(area, Hit::TestsPanel);
    let height = usize::from(area.height.saturating_sub(1));
    if focused {
        cx.ui.page_rows = height;
    }
    if panel.rows.is_empty() {
        frame.render_widget(
            Line::from(" No results yet: run tests, with -v for go and pytest").fg(theme.fg_muted),
            row(area, 1),
        );
        return;
    }
    cx.ui.tests_scroll = follow_or_clamp(
        cx.ui.tests_scroll,
        &mut cx.ui.tests_followed,
        panel.selected,
        height,
        panel.rows.len(),
    );
    let width = usize::from(area.width);
    for (n, test) in panel
        .rows
        .iter()
        .enumerate()
        .skip(cx.ui.tests_scroll)
        .take(height)
    {
        let y = u16::try_from(n - cx.ui.tests_scroll + 1).unwrap_or(u16::MAX);
        let (mark, color) = match test.state {
            TestState::Failed => ("✗", theme.error),
            TestState::Passed => ("✓", theme.success),
            TestState::Ignored => ("○", theme.fg_muted),
        };
        let style = match (n == panel.selected, focused) {
            (true, true) => theme.selected,
            (true, false) => theme.selected_inactive,
            _ => Style::new().bg(theme.bg_panel),
        };
        let line = Line::from(vec![
            Span::raw(format!(" {mark} ")).fg(color),
            Span::raw(ellipsize(&visible(&test.name), width.saturating_sub(3)))
                .fg(theme.fg_primary),
        ]);
        frame.render_widget(line.style(style), row(area, y));
        cx.ui.hits.push(row(area, y), Hit::TestRow { index: n });
    }
}

pub fn render_dependencies(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let ws = cx.ws;
    let theme = cx.theme;
    let panel = ws.deps_panel();
    let focused = ws.focus() == Focus::Dependencies && ws.picker().is_none();
    let mut spans = vec![
        Span::raw(" Dependencies"),
        Span::raw(format!("   {}", panel.rows.len())),
        Span::raw("   Enter go   r reload   o what is old"),
    ];
    if let Some(problem) = panel.problems.first() {
        spans.push(Span::raw(format!("   {}", visible(problem))));
    }
    title(frame, area, cx, focused, spans);
    cx.ui.hits.push(area, Hit::DependenciesPanel);
    let height = usize::from(area.height.saturating_sub(1));
    if focused {
        cx.ui.page_rows = height;
    }
    if panel.rows.is_empty() {
        let text = if panel.loading {
            " Reading…"
        } else {
            " No dependencies found in this folder's manifests"
        };
        frame.render_widget(Line::from(text).fg(theme.fg_muted), row(area, 1));
        return;
    }
    cx.ui.deps_scroll = follow_or_clamp(
        cx.ui.deps_scroll,
        &mut cx.ui.deps_followed,
        panel.selected,
        height,
        panel.rows.len(),
    );
    let width = usize::from(area.width);
    let names = (width / 2).clamp(10, 40);
    for (n, dep) in panel
        .rows
        .iter()
        .enumerate()
        .skip(cx.ui.deps_scroll)
        .take(height)
    {
        let y = u16::try_from(n - cx.ui.deps_scroll + 1).unwrap_or(u16::MAX);
        let style = match (n == panel.selected, focused) {
            (true, true) => theme.selected,
            (true, false) => theme.selected_inactive,
            _ => Style::new().bg(theme.bg_panel),
        };
        let name = ellipsize(&visible(&dep.name), names);
        let padding = " ".repeat(names.saturating_sub(name.chars().count()));
        let line = Line::from(vec![
            Span::raw(format!(" {name}{padding}  ")).fg(theme.fg_primary),
            Span::raw(format!("{:<12}", ellipsize(&visible(&dep.version), 12))).fg(theme.accent),
            Span::raw(format!(
                " {} · {}",
                visible(&dep.section),
                visible(&dep.file)
            ))
            .fg(theme.fg_muted),
        ]);
        frame.render_widget(line.style(style), row(area, y));
        cx.ui
            .hits
            .push(row(area, y), Hit::DependencyRow { index: n });
    }
}

pub fn render_bookmarks(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let ws = cx.ws;
    let theme = cx.theme;
    let bookmarks = ws.bookmarks();
    let selected = ws.bookmark_selected();
    let focused = ws.focus() == Focus::Bookmarks && ws.picker().is_none();
    title(
        frame,
        area,
        cx,
        focused,
        vec![
            Span::raw(" Bookmarks"),
            Span::raw(format!("   {}", bookmarks.len())),
            Span::raw("   Enter go   d remove"),
        ],
    );
    cx.ui.hits.push(area, Hit::BookmarksPanel);
    let height = usize::from(area.height.saturating_sub(1));
    if focused {
        cx.ui.page_rows = height;
    }
    if bookmarks.is_empty() {
        frame.render_widget(
            Line::from(" No bookmarks yet: F11 or `Toggle Bookmark` sets one").fg(theme.fg_muted),
            row(area, 1),
        );
        return;
    }
    cx.ui.bookmarks_scroll = follow_or_clamp(
        cx.ui.bookmarks_scroll,
        &mut cx.ui.bookmarks_followed,
        selected,
        height,
        bookmarks.len(),
    );
    let width = usize::from(area.width).saturating_sub(1);
    for (n, bookmark) in bookmarks
        .iter()
        .enumerate()
        .skip(cx.ui.bookmarks_scroll)
        .take(height)
    {
        let y = u16::try_from(n - cx.ui.bookmarks_scroll + 1).unwrap_or(u16::MAX);
        let style = match (n == selected, focused) {
            (true, true) => theme.selected,
            (true, false) => theme.selected_inactive,
            _ => Style::new().bg(theme.bg_panel),
        };
        let text = ellipsize(&visible(&ws.bookmark_row(bookmark)), width);
        let line = Line::from(vec![Span::raw(" "), Span::raw(text).fg(theme.fg_primary)]);
        frame.render_widget(line.style(style), row(area, y));
        cx.ui.hits.push(row(area, y), Hit::BookmarkRow { index: n });
    }
}

pub fn render_duplicates(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let ws = cx.ws;
    let theme = cx.theme;
    let panel = ws.duplicates_panel();
    let focused = ws.focus() == Focus::Duplicates && ws.picker().is_none();
    let report = &panel.report;
    let state = if panel.running.is_some() {
        "   scanning…  s stop".to_owned()
    } else if report.cancelled {
        "   stopped  r scan".to_owned()
    } else {
        let more = if report.truncated {
            ", limit reached"
        } else {
            ""
        };
        format!(
            "   {} groups in {} files{more}  r scan",
            report.groups.len(),
            report.files
        )
    };
    let mode = if panel.anonymize {
        "   a [names may differ]"
    } else {
        "   a names may differ"
    };
    title(
        frame,
        area,
        cx,
        focused,
        vec![Span::raw(" Duplicates"), Span::raw(state), Span::raw(mode)],
    );
    cx.ui.hits.push(area, Hit::DuplicatesPanel);
    let height = usize::from(area.height.saturating_sub(1));
    if focused {
        cx.ui.page_rows = height;
    }
    let count = panel.row_count();
    if count == 0 {
        if panel.running.is_none() && !report.cancelled {
            frame.render_widget(
                Line::from(" No copies of 50 tokens or more").fg(theme.fg_muted),
                row(area, 1),
            );
        }
        return;
    }
    cx.ui.duplicates_scroll = follow_or_clamp(
        cx.ui.duplicates_scroll,
        &mut cx.ui.duplicates_followed,
        panel.selected,
        height,
        count,
    );
    let width = usize::from(area.width);
    for (n, entry) in panel
        .rows()
        .enumerate()
        .skip(cx.ui.duplicates_scroll)
        .take(height)
    {
        let y = u16::try_from(n - cx.ui.duplicates_scroll + 1).unwrap_or(u16::MAX);
        let style = match (n == panel.selected, focused) {
            (true, true) => theme.selected,
            (true, false) => theme.selected_inactive,
            _ => Style::new().bg(theme.bg_panel),
        };
        let line = match entry {
            DuplicateRow::Group(group) => Line::from(
                Span::raw(format!(
                    " {} places, {} tokens",
                    group.fragments.len(),
                    group.tokens
                ))
                .fg(theme.accent),
            ),
            DuplicateRow::Fragment(fragment) => {
                let place = visible(&format!(
                    "   {}:{}-{}",
                    ws.display_path(&fragment.path).display(),
                    fragment.first_line + 1,
                    fragment.last_line + 1
                ));
                Line::from(Span::raw(ellipsize(&place, width)).fg(theme.file_color(&fragment.path)))
            }
        };
        frame.render_widget(line.style(style), row(area, y));
        cx.ui
            .hits
            .push(row(area, y), Hit::DuplicateRow { index: n });
    }
}
