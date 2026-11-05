use ratatui::Frame;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::editor::{find_options, severity_color};
use super::git::{change_color, render_git};
use super::glyphs;
use super::mouse::Hit;
use super::render::{Ctx, follow_or_clamp};
use super::run::render_run;
use super::text::{ellipsize, highlighted};
use crate::core::http::{HttpOutcome, HttpRowKind};
use crate::core::{Action, Focus, LspState, ReplaceStage, Severity, ToolTab};

pub fn render_project(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    frame
        .buffer_mut()
        .set_style(area, Style::new().bg(cx.theme.bg_panel));
    let ws = cx.ws;
    let theme = cx.theme;
    let focused = ws.focus() == Focus::Project && ws.picker().is_none();
    frame.render_widget(Line::default().bg(theme.bg_panel), area);
    let title_style = if focused {
        Style::new()
            .fg(theme.fg_primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.fg_muted)
    };
    frame.render_widget(
        Line::from(Span::styled(" Project", title_style)).bg(theme.bg_panel),
        row(area, 0),
    );

    let rows = ws.tree().rows();
    let selected = ws.tree().selected_index();
    let height = usize::from(area.height.saturating_sub(1));
    cx.ui.hits.push(area, Hit::Tree);
    cx.ui.tree_scroll = follow_or_clamp(
        cx.ui.tree_scroll,
        &mut cx.ui.tree_followed,
        selected,
        height,
        rows.len(),
    );
    if focused {
        cx.ui.page_rows = height;
    }
    let width = usize::from(area.width);
    for (i, r) in rows.iter().enumerate().skip(cx.ui.tree_scroll).take(height) {
        let is_selected = i == selected;
        let row_style = match (is_selected, focused) {
            (true, true) => theme.selected,
            (true, false) => theme.selected_inactive,
            _ => Style::new().bg(theme.bg_panel),
        };
        let marker = if is_selected && focused {
            Span::raw(glyphs::SELECTED).fg(theme.accent)
        } else {
            Span::raw(" ")
        };
        let (glyph, glyph_color) = match (r.is_dir, r.expanded) {
            (true, true) => (glyphs::EXPANDED, theme.syntax.attribute),
            (true, false) => (glyphs::COLLAPSED, theme.syntax.attribute),
            (false, _) => (glyphs::FILE, theme.file_color(&r.path)),
        };
        let glyph = match (cx.ui.nerd_icons, r.is_dir, r.expanded) {
            (false, ..) => glyph.to_owned(),
            (true, true, true) => format!("{glyph} {}", glyphs::NERD_FOLDER_OPEN),
            (true, true, false) => format!("{glyph} {}", glyphs::NERD_FOLDER),
            (true, false, _) => glyphs::nerd_file(&r.path).to_owned(),
        };
        let indent = "  ".repeat(r.depth);
        let prefix = format!("{indent}{glyph} ");
        let mut spans = vec![
            marker,
            Span::raw(indent),
            Span::raw(format!("{glyph} ")).fg(glyph_color),
        ];
        let room = width.saturating_sub(1 + prefix.width());
        let name_color = match ws.git_change(&r.path) {
            Some(change) => change_color(change, cx),
            None if ws.git_dir_changed(&r.path) => theme.accent,
            None => theme.fg_primary,
        };
        spans.push(Span::raw(ellipsize(&r.name, room)).fg(name_color));
        if r.depth == 0 {
            let shown = home_relative(&ws.root().display().to_string());
            let left = room.saturating_sub(r.name.width() + 2);
            if left > 3 {
                spans.push(Span::raw(format!("  {}", ellipsize(&shown, left))).fg(theme.fg_muted));
            }
        }
        let y = (i - cx.ui.tree_scroll) as u16 + 1;
        cx.ui.hits.push(
            row(area, y),
            Hit::TreeRow {
                path: r.path.clone(),
            },
        );
        frame.render_widget(Line::from(spans).style(row_style), row(area, y));
    }
}

pub fn render_tool_window(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    frame
        .buffer_mut()
        .set_style(area, Style::new().bg(cx.theme.bg_panel));
    match cx.ws.tool_tab() {
        ToolTab::Problems => return render_problems(frame, area, cx),
        ToolTab::Git => return render_git(frame, area, cx),
        ToolTab::Run => return render_run(frame, area, cx),
        ToolTab::Debug => return super::debug::render_debug(frame, area, cx),
        ToolTab::Claude => return super::terminal::render_claude(frame, area, cx),
        ToolTab::Terminal => return super::terminal::render_terminal(frame, area, cx),
        ToolTab::Language => return render_language(frame, area, cx),
        ToolTab::Http => return render_http(frame, area, cx),
        ToolTab::Todo => return super::lists::render_todo(frame, area, cx),
        ToolTab::Tests => return super::lists::render_tests(frame, area, cx),
        ToolTab::Dependencies => return super::lists::render_dependencies(frame, area, cx),
        ToolTab::Duplicates => return super::lists::render_duplicates(frame, area, cx),
        ToolTab::Bookmarks => return super::lists::render_bookmarks(frame, area, cx),
        ToolTab::Database => return super::database::render_database(frame, area, cx),
        ToolTab::Docker => return super::docker::render_docker(frame, area, cx),
        ToolTab::GitHub => return super::github::render_github(frame, area, cx),
        ToolTab::Find => {}
    }
    let ws = cx.ws;
    let theme = cx.theme;
    let search = ws.search();
    let focused = ws.focus() == Focus::FindInFiles && ws.picker().is_none();
    frame.render_widget(Line::default().bg(theme.bg_panel), area);
    if let Some(stage) = &search.replace {
        return render_replace_stage(frame, area, stage, focused, cx);
    }

    let title_style = if focused {
        Style::new()
            .fg(theme.fg_primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.fg_muted)
    };
    let files = {
        let mut paths: Vec<_> = search.results.iter().map(|m| &m.path).collect();
        paths.dedup();
        paths.len()
    };
    let summary = match (&search.running, &search.searched) {
        (Some(_), _) => format!("{} Searching…", glyphs::SPINNER),
        (None, _) if search.usages_of.is_some() => format!(
            "{} {}{} of {} in {files} {}",
            search.results.len(),
            if search.usages_word.is_empty() {
                "usage"
            } else {
                search.usages_word
            },
            if search.results.len() == 1 { "" } else { "s" },
            search.usages_of.as_deref().unwrap_or_default(),
            if files == 1 { "file" } else { "files" },
        ),
        (None, Some(_)) if search.results.is_empty() => "No matches".to_owned(),
        (None, Some(_)) => {
            let more = if search.truncated { "+" } else { "" };
            format!("{}{more} matches in {files} files", search.results.len())
        }
        (None, None) if search.error.is_some() => search.error.clone().unwrap_or_default(),
        (None, None) => "Type a query and press Enter".to_owned(),
    };
    let summary_color = if search.error.is_some() && search.running.is_none() {
        theme.error
    } else {
        theme.fg_muted
    };
    let title = Line::from(vec![
        Span::styled(" Find", title_style),
        Span::raw(format!("   {summary}")).fg(summary_color),
    ]);
    frame.render_widget(title.bg(theme.bg_panel), row(area, 0));
    frame.render_widget(find_options(search.options, theme), row(area, 0));
    let replacing = search.replacement.is_some();
    if focused && !replacing {
        frame.render_widget(
            Line::from(glyphs::TAB_UNDERLINE.repeat(6))
                .fg(theme.accent)
                .bg(theme.bg_panel),
            row(area, 1),
        );
    }
    let query_y = if replacing { 1 } else { 2 };

    let prompt = format!(" {} ", glyphs::PROMPT);
    let query = search.input.text();
    let field = Line::from(vec![
        Span::raw(prompt.clone()).fg(theme.accent),
        if query.is_empty() && search.selected.is_none() {
            Span::raw("Find in files…").fg(theme.fg_subtle).italic()
        } else {
            Span::raw(query.to_owned()).fg(theme.fg_primary)
        },
    ]);
    frame.render_widget(field.bg(theme.bg_panel), row(area, query_y));
    if let Some(replacement) = &search.replacement {
        let empty = replacement.text().is_empty() && !search.in_replacement;
        let line = Line::from(vec![
            Span::raw(prompt.clone()).fg(theme.accent),
            if empty {
                Span::raw("Replace with…").fg(theme.fg_subtle).italic()
            } else {
                Span::raw(replacement.text().to_owned()).fg(theme.fg_primary)
            },
        ]);
        frame.render_widget(line.bg(theme.bg_panel), row(area, 2));
        frame.render_widget(
            Line::from("⏎ preview  Tab switch  Esc close ")
                .fg(theme.fg_muted)
                .right_aligned(),
            row(area, 2),
        );
    }
    if focused && search.selected.is_none() {
        let (field, y) = match &search.replacement {
            Some(replacement) if search.in_replacement => (replacement, 2),
            _ => (&search.input, query_y),
        };
        let before: String = field.text().chars().take(field.cursor()).collect();
        let x = area.x + (prompt.width() + before.width()) as u16;
        frame.set_cursor_position(Position::new(
            x.min(area.right().saturating_sub(1)),
            area.y + y,
        ));
    }

    enum Row<'a> {
        File(&'a std::path::Path, usize),
        Match(usize),
    }
    let mut rows = Vec::new();
    for (i, m) in search.results.iter().enumerate() {
        if i == 0 || search.results[i - 1].path != m.path {
            let count = search.results[i..]
                .iter()
                .take_while(|x| x.path == m.path)
                .count();
            rows.push(Row::File(&m.path, count));
        }
        rows.push(Row::Match(i));
    }
    let height = usize::from(area.height.saturating_sub(3));
    if focused {
        cx.ui.page_rows = height;
    }
    let selected_row = search
        .selected
        .and_then(|sel| {
            rows.iter()
                .position(|r| matches!(r, Row::Match(i) if *i == sel))
        })
        .unwrap_or(0);
    cx.ui.hits.push(area, Hit::Search);
    cx.ui.results_scroll = follow_or_clamp(
        cx.ui.results_scroll,
        &mut cx.ui.results_followed,
        selected_row,
        height,
        rows.len(),
    );
    let width = usize::from(area.width);
    for (n, r) in rows
        .iter()
        .enumerate()
        .skip(cx.ui.results_scroll)
        .take(height)
    {
        let y = (n - cx.ui.results_scroll) as u16 + 3;
        let line = match r {
            Row::File(path, count) => Line::from(vec![
                Span::raw(format!(" {} ", glyphs::FILE)).fg(theme.file_color(path)),
                Span::raw(ws.display_path(path).display().to_string()).fg(theme.syntax.function),
                Span::raw(format!("  {count}")).fg(theme.accent),
            ])
            .bg(theme.bg_panel),
            Row::Match(i) => {
                let m = &search.results[*i];
                let is_selected = search.selected == Some(*i);
                let row_style = match (is_selected, focused) {
                    (true, true) => theme.selected,
                    (true, false) => theme.selected_inactive,
                    _ => Style::new().bg(theme.bg_panel),
                };
                let marker = if is_selected && focused {
                    Span::raw(glyphs::SELECTED).fg(theme.accent)
                } else {
                    Span::raw(" ")
                };
                let number = format!("  {:>5}  ", m.line + 1);
                let room = width.saturating_sub(1 + number.width());
                let shown = ellipsize(m.text.trim_start(), room);
                let indent = m.text.chars().count() - m.text.trim_start().chars().count();
                let start = m.col.saturating_sub(indent);
                let hits: Vec<u32> = (start..start + m.len)
                    .filter_map(|i| u32::try_from(i).ok())
                    .collect();
                let base = Style::new().fg(theme.fg_primary);
                let hit = Style::new().fg(theme.accent).add_modifier(Modifier::BOLD);
                let mut spans = vec![marker, Span::raw(number).fg(theme.fg_muted)];
                spans.extend(highlighted(&shown, &hits, base, hit, ws.tab_width()));
                Line::from(spans).style(row_style)
            }
        };
        if let Row::Match(index) = r {
            cx.ui
                .hits
                .push(row(area, y), Hit::SearchResult { index: *index });
        }
        frame.render_widget(line, row(area, y));
    }
}

fn render_replace_stage(
    frame: &mut Frame,
    area: Rect,
    stage: &ReplaceStage,
    focused: bool,
    cx: &mut Ctx<'_>,
) {
    let ws = cx.ws;
    let theme = cx.theme;
    cx.ui.hits.push(area, Hit::Search);
    let height = usize::from(area.height.saturating_sub(1));
    if focused {
        cx.ui.page_rows = height;
    }
    let count =
        |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let path = |p: &std::path::Path| ws.display_path(p).display().to_string();
    let mut rows: Vec<Line<'static>> = Vec::new();
    let (title, color, scroll) = match stage {
        ReplaceStage::Planning { .. } => (
            format!("{} Working out the changes…", glyphs::SPINNER),
            theme.fg_muted,
            0,
        ),
        ReplaceStage::Applying { .. } => {
            (format!("{} Replacing…", glyphs::SPINNER), theme.fg_muted, 0)
        }
        ReplaceStage::Preview(preview) => {
            for (file, why) in &preview.failed {
                rows.push(
                    Line::from(format!(" {} {}: {why}", glyphs::ERROR, path(file))).fg(theme.error),
                );
            }
            for (plan, _) in &preview.files {
                rows.push(Line::from(vec![
                    Span::raw(format!(" {} ", glyphs::FILE)).fg(theme.file_color(&plan.path)),
                    Span::raw(path(&plan.path)).fg(theme.syntax.function),
                    Span::raw(format!("  {}", plan.occurrences)).fg(theme.accent),
                ]));
                for change in &plan.changes {
                    rows.push(Line::from(vec![
                        Span::raw(format!("  {:>5} - ", change.line + 1)).fg(theme.fg_muted),
                        Span::raw(change.old.clone()).fg(theme.error),
                    ]));
                    rows.push(Line::from(vec![
                        Span::raw("        + ").fg(theme.fg_muted),
                        Span::raw(change.new.clone()).fg(theme.success),
                    ]));
                }
                if plan.more_lines > 0 {
                    rows.push(
                        Line::from(format!("        … and {} more lines", plan.more_lines))
                            .fg(theme.fg_muted),
                    );
                }
            }
            let more = if preview.truncated {
                ", search stopped early: other files are not included"
            } else {
                ""
            };
            let left_out = match preview.failed.len() {
                0 => String::new(),
                n => format!(", {n} cannot be changed"),
            };
            let title = format!(
                "Replace {} in {}{left_out}{more}?  y apply  Esc cancel",
                count(preview.occurrences(), "occurrence", "occurrences"),
                count(preview.files.len(), "file", "files"),
            );
            (title, theme.warning, preview.scroll)
        }
        ReplaceStage::Report(report) => {
            for file in &report.changed {
                rows.push(
                    Line::from(format!(" {} {}", glyphs::SUCCESS, path(file))).fg(theme.success),
                );
            }
            for (file, why) in &report.skipped {
                rows.push(
                    Line::from(format!(
                        " {} {}: skipped, {why}",
                        glyphs::WARNING,
                        path(file)
                    ))
                    .fg(theme.warning),
                );
            }
            for (file, why) in &report.failed {
                rows.push(
                    Line::from(format!(" {} {}: {why}", glyphs::ERROR, path(file))).fg(theme.error),
                );
            }
            let title = format!(
                "Replaced in {}, {} skipped, {} failed  Esc close",
                count(report.changed.len(), "file", "files"),
                report.skipped.len(),
                report.failed.len(),
            );
            let bad = report.skipped.len() + report.failed.len() > 0;
            (
                title,
                if bad { theme.warning } else { theme.success },
                report.scroll,
            )
        }
    };
    frame.render_widget(
        Line::from(vec![
            Span::raw(" Replace in Files").fg(theme.fg_primary).bold(),
            Span::raw(format!("   {title}")).fg(color),
        ])
        .bg(theme.bg_panel),
        row(area, 0),
    );
    let top = scroll.min(rows.len().saturating_sub(height));
    for (n, line) in rows.into_iter().skip(top).take(height).enumerate() {
        frame.render_widget(line.bg(theme.bg_panel), row(area, n as u16 + 1));
    }
}

fn row(area: Rect, y: u16) -> Rect {
    Rect::new(area.x, area.y + y, area.width, 1).intersection(area)
}

pub(super) fn home_relative(path: &str) -> String {
    match std::env::var("HOME") {
        Ok(home) if !home.is_empty() && path.starts_with(&home) => {
            format!("~{}", &path[home.len()..])
        }
        _ => path.to_owned(),
    }
}

fn lsp_state_words(state: LspState, busy: bool) -> &'static str {
    match state {
        LspState::Ready if busy => "working",
        LspState::Ready => "ready",
        LspState::Off => "not started",
        LspState::Installing => "being installed",
        LspState::Starting => "starting",
        LspState::Crashed => "crashed, restarting soon",
        LspState::Stopped => "stopped after crashing too often",
        LspState::NotFound => "not installed",
        LspState::Disabled => "turned off in the config",
    }
}

fn render_http(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let panel = cx.ws.http_panel();
    let focused = cx.ws.focus() == Focus::Http && cx.ws.picker().is_none();
    let title_style = if focused {
        Style::new()
            .fg(theme.fg_primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.fg_muted)
    };
    let mut title = vec![Span::styled(" HTTP", title_style)];
    if !panel.title.is_empty() {
        title.push(Span::raw(format!("   {}", panel.title)).fg(theme.fg_muted));
    }
    if panel.current.is_some() {
        title.push(Span::raw("   sending…  s stop").fg(theme.accent));
    }
    frame.render_widget(Line::from(title).bg(theme.bg_panel), row(area, 0));
    cx.ui.hits.push(area, Hit::HttpPanel);
    let rows = panel.rows();
    if rows.is_empty() && panel.current.is_none() {
        frame.render_widget(
            Line::from(" Alt+H on a request in a .http file sends it").fg(theme.fg_muted),
            row(area, 1),
        );
    }
    for (n, (kind, text)) in rows.iter().skip(panel.scroll).enumerate() {
        let y = n as u16 + 1;
        if y >= area.height {
            break;
        }
        let style = match kind {
            HttpRowKind::Status => {
                let ok = matches!(&panel.outcome, Some(HttpOutcome::Done(r)) if r.code < 400);
                Style::new()
                    .fg(if ok { theme.success } else { theme.error })
                    .add_modifier(Modifier::BOLD)
            }
            HttpRowKind::Failure => Style::new().fg(theme.error),
            HttpRowKind::Header | HttpRowKind::Note => Style::new().fg(theme.fg_muted),
            HttpRowKind::Body => Style::new().fg(theme.fg_primary),
        };
        frame.render_widget(Line::from(format!(" {text}")).style(style), row(area, y));
    }
}

fn render_language(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let focused = cx.ws.focus() == Focus::Language && cx.ws.picker().is_none();
    let title_style = if focused {
        Style::new()
            .fg(theme.fg_primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.fg_muted)
    };
    frame.render_widget(
        Line::from(Span::styled(" Language Server", title_style)).bg(theme.bg_panel),
        row(area, 0),
    );
    cx.ui.hits.push(area, Hit::LanguagePanel);
    let Some(panel) = cx.ws.lsp_panel() else {
        frame.render_widget(
            Line::from(if cx.ws.is_trusted() {
                " No language server for this file"
            } else {
                " No language server: this project is not trusted (`Trust This Project`)"
            })
            .fg(theme.fg_muted),
            row(area, 1),
        );
        return;
    };
    let mut buttons = vec![(" r Restart ", Action::RestartLanguageServer)];
    if panel.can_choose {
        buttons.push((" s Choose server ", Action::SelectLanguageServer));
    }
    let mut x = area.x + 1;
    for (label, action) in buttons {
        let width = label.chars().count() as u16;
        let place = Rect::new(x, row(area, 1).y, width, 1).intersection(area);
        frame.render_widget(Line::from(label).style(theme.selected_inactive), place);
        cx.ui.hits.push(place, Hit::Button(action));
        x += width + 1;
    }
    let root = panel.root.strip_prefix(cx.ws.root()).unwrap_or(&panel.root);
    let root = match root.display().to_string() {
        shown if shown.is_empty() => ".".to_owned(),
        shown => shown,
    };
    let state = lsp_state_words(panel.status.state, panel.status.busy);
    let mut rows = vec![
        (
            theme.fg_primary,
            format!(" {}: {}", panel.language, panel.status.name),
        ),
        (theme.fg_primary, format!(" State: {state}")),
    ];
    if let Some(failure) = &panel.failure {
        rows.push((theme.error, format!(" Last failure: {failure}")));
    }
    if let Some(install) = panel.status.install {
        rows.push((theme.fg_primary, format!(" Install with: {install}")));
    }
    rows.push((theme.fg_muted, format!(" Root: {root}")));
    rows.push((theme.fg_muted, format!(" Open files: {}", panel.open_files)));
    for (n, (color, text)) in rows.into_iter().enumerate() {
        let y = n as u16 + 2;
        if y < area.height {
            frame.render_widget(Line::from(text).fg(color), row(area, y));
        }
    }
}

fn render_problems(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let ws = cx.ws;
    let theme = cx.theme;
    let focused = ws.focus() == Focus::Problems && ws.picker().is_none();
    for y in area.y..area.bottom() {
        frame.render_widget(
            Line::default().bg(theme.bg_panel),
            Rect::new(area.x, y, area.width, 1),
        );
    }
    let title_style = if focused {
        Style::new()
            .fg(theme.fg_primary)
            .add_modifier(Modifier::BOLD)
    } else {
        Style::new().fg(theme.fg_muted)
    };
    let (errors, warnings) = ws.problem_counts();
    let title = Line::from(vec![
        Span::styled(" Problems", title_style),
        Span::raw(format!(
            "   {errors} {}  {warnings} {}",
            glyphs::ERROR,
            glyphs::WARNING
        ))
        .fg(theme.fg_muted),
        Span::raw(match ws.analysis_running() {
            running if running.is_empty() => String::new(),
            running => format!("   analyzing: {}", running.join(", ")),
        })
        .fg(theme.accent),
    ]);
    frame.render_widget(title.bg(theme.bg_panel), row(area, 0));
    cx.ui.hits.push(area, Hit::Problems);

    let problems = ws.problems();
    let height = usize::from(area.height.saturating_sub(1));
    if focused {
        cx.ui.page_rows = height;
    }
    if problems.is_empty() {
        frame.render_widget(
            Line::from(format!(" {} No problems found", glyphs::SUCCESS)).fg(theme.fg_muted),
            row(area, 1),
        );
        return;
    }
    let selected = ws.problem_selected();
    cx.ui.problems_scroll = follow_or_clamp(
        cx.ui.problems_scroll,
        &mut cx.ui.problems_followed,
        selected.unwrap_or(0),
        height,
        problems.len(),
    );
    let width = usize::from(area.width);
    for (n, p) in problems
        .iter()
        .enumerate()
        .skip(cx.ui.problems_scroll)
        .take(height)
    {
        let y = (n - cx.ui.problems_scroll) as u16 + 1;
        let is_selected = selected == Some(n);
        let row_style = match (is_selected, focused) {
            (true, true) => theme.selected,
            (true, false) => theme.selected_inactive,
            _ => Style::new().bg(theme.bg_panel),
        };
        let glyph = match p.severity {
            Severity::Error => glyphs::ERROR,
            Severity::Warning => glyphs::WARNING,
            Severity::Info | Severity::Hint => "·",
        };
        let place = format!(
            "  [{}] {}:{}",
            p.source,
            ws.display_path(&p.path).display(),
            p.line + 1
        );
        let place_color = theme.file_color(&p.path);
        let room = width.saturating_sub(3 + place.width());
        let message = p.message.lines().next().unwrap_or_default();
        let line = Line::from(vec![
            Span::raw(format!(" {glyph} ")).fg(severity_color(p.severity, theme)),
            Span::raw(ellipsize(message, room)).fg(theme.fg_primary),
            Span::raw(place).fg(place_color),
        ]);
        frame.render_widget(line.style(row_style), row(area, y));
        cx.ui.hits.push(row(area, y), Hit::Problem { index: n });
    }
}
