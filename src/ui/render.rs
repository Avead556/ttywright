use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use super::text::ellipsize;
use crate::core::Action;
use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Position, Rect};
use ratatui::style::{Color, Style, Stylize};
use ratatui::text::{Line, Span};
use unicode_width::UnicodeWidthStr;

use super::completion::render_completion;
use super::database::render_db_confirm;
use super::docker::render_docker_confirm;
use super::editor::render_layout;
use super::git::render_git;
use super::github::render_github_windows;
use super::glyphs;
use super::history::render_history;
use super::hover::render_hover;
use super::keymap::Keymap;
use super::mcp::render_mcp_review;
use super::menu::{render_file_prompt, render_menu, title_span, titles_width};
use super::merge::render_merge;
use super::mouse::Resizing;
use super::mouse::{Hit, HitMap};
use super::panels::{render_project, render_tool_window};
use super::picker::render_picker;
use super::run::{render_analysis_confirm, render_run, render_run_confirm};
use super::theme::Theme;
use crate::core::menu::MENUS;
use crate::core::{
    CoverageState, DocumentId, Level, LineMark, LspState, ToolTab, ViewId, Workspace, text,
};

pub const MIN_WIDTH: u16 = 40;
const GIT_MIN_ROWS: u16 = 10;
const TOOL_MIN_ROWS: u16 = 5;
const EDITOR_MIN_ROWS: u16 = 5;
const TOOL_SCREEN_ROWS: u16 = 16;
pub const MIN_HEIGHT: u16 = 10;
const PROJECT_WIDTH: u16 = 28;
const PROJECT_MIN_WIDTH: u16 = 12;
const EDITOR_MIN_COLUMNS: u16 = 40;
const STRIPE_WIDTH: u16 = 3;

#[derive(Debug, Default, Clone)]
pub struct UiState {
    pub scroll: HashMap<ViewId, (usize, usize)>,
    pub tree_scroll: usize,
    pub results_scroll: usize,
    pub picker_scroll: usize,
    pub page_rows: usize,
    pub hits: HitMap,
    pub last_click: Option<(Instant, u16, u16)>,
    pub followed: HashMap<ViewId, (usize, u64, Option<usize>)>,
    pub viewport: usize,
    pub tool_room: (u16, u16, u16),
    pub tree_followed: Option<usize>,
    pub results_followed: Option<usize>,
    pub problems_scroll: usize,
    pub todo_scroll: usize,
    pub todo_followed: Option<usize>,
    pub tests_scroll: usize,
    pub tests_followed: Option<usize>,
    pub deps_scroll: usize,
    pub deps_followed: Option<usize>,
    pub duplicates_scroll: usize,
    pub duplicates_followed: Option<usize>,
    pub bookmarks_scroll: usize,
    pub bookmarks_followed: Option<usize>,
    pub problems_followed: Option<usize>,
    pub editor_cursor: Option<Position>,
    pub claude_size: Option<(u16, u16)>,
    pub terminal_size: Option<(u16, u16)>,
    pub completion_scroll: usize,
    pub double_shift: super::input::DoubleShift,
    pub run_scroll: usize,
    pub dragging: Option<ViewId>,
    pub resizing: Option<super::mouse::Resizing>,
    pub nerd_icons: bool,
    pub pointer: super::mouse::Pointer,
    pub git_panel_width: u16,
    pub markdown: HashMap<DocumentId, MarkdownCache>,
    pub image_wants: Vec<crate::core::image_art::ImageRequest>,
    pub preview_alone: Option<DocumentId>,
    pub git_marks: HashMap<DocumentId, GitMarks>,
}

#[derive(Debug, Clone)]
pub struct MarkdownCache {
    pub version: u64,
    pub width: usize,
    pub lines: Arc<Vec<crate::lang::markdown::MdLine>>,
    pub top: usize,
    pub followed: Option<usize>,
}

#[derive(Debug, Clone)]
pub struct GitMarks {
    pub version: u64,
    pub base_stamp: u64,
    pub marks: Arc<Vec<Option<LineMark>>>,
}

pub struct Ctx<'a> {
    pub ws: &'a Workspace,
    pub ui: &'a mut UiState,
    pub theme: &'a Theme,
    pub keymap: &'a Keymap,
}

pub fn render(frame: &mut Frame, ws: &Workspace, ui: &mut UiState, theme: &Theme, keymap: &Keymap) {
    let area = frame.area();
    frame
        .buffer_mut()
        .set_style(area, Style::new().bg(theme.bg));
    ui.hits.clear();
    ui.editor_cursor = None;
    if area.width < MIN_WIDTH || area.height < MIN_HEIGHT {
        render_too_small(frame, area, theme);
        return;
    }
    let mut cx = Ctx {
        ws,
        ui,
        theme,
        keymap,
    };
    if ws.is_window() {
        let [panel, status] =
            Layout::vertical([Constraint::Fill(1), Constraint::Length(1)]).areas(area);
        if ws.is_run_window() {
            render_run(frame, panel, &mut cx);
        } else {
            render_git(frame, panel, &mut cx);
        }
        render_status(frame, status, &cx);
        render_picker(frame, area, &mut cx);
        return;
    }
    let [title, divider, middle, status] = Layout::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Fill(1),
        Constraint::Length(1),
    ])
    .areas(area);

    let [stripe, stripe_border, content] = Layout::horizontal([
        Constraint::Length(STRIPE_WIDTH),
        Constraint::Length(1),
        Constraint::Fill(1),
    ])
    .areas(middle);
    let room = tool_room(ws.tool_height(), ws.tool_tab(), middle.height);
    cx.ui.tool_room = room;
    let show_tool = ws.tool_window_visible() && room.2 > 0;
    let tool_height = if show_tool { room.0 } else { 0 };
    let [main, tool_divider, tool] = Layout::vertical([
        Constraint::Fill(1),
        Constraint::Length(u16::from(show_tool)),
        Constraint::Length(tool_height),
    ])
    .areas(content);
    let show_project = ws.project_visible() && content.width >= 80;
    let project_width = if show_project {
        project_room(ws.project_width(), content.width)
    } else {
        0
    };
    let [project, project_border, editor] = Layout::horizontal([
        Constraint::Length(project_width),
        Constraint::Length(u16::from(show_project)),
        Constraint::Fill(1),
    ])
    .areas(main);

    render_title(frame, title, &mut cx);
    let mut junctions = vec![(stripe_border.x, glyphs::TEE_DOWN)];
    if show_project {
        junctions.push((project_border.x, glyphs::TEE_DOWN));
    }
    render_rule(frame, divider, &junctions, theme);
    render_stripe(frame, stripe, &cx);
    cx.ui.hits.push(
        Rect::new(stripe.x, stripe.y, stripe.width, 1),
        Hit::StripeProject,
    );
    if stripe.height > 2 {
        cx.ui.hits.push(
            Rect::new(stripe.x, stripe.y + 1, stripe.width, 1),
            Hit::StripeGit,
        );
    }
    if stripe.height > 3 {
        cx.ui.hits.push(
            Rect::new(stripe.x, stripe.y + 2, stripe.width, 1),
            Hit::StripeClaude,
        );
    }
    if stripe.height > 4 {
        cx.ui.hits.push(
            Rect::new(stripe.x, stripe.y + 3, stripe.width, 1),
            Hit::StripeTerminal,
        );
    }
    if stripe.height > 5 {
        cx.ui.hits.push(
            Rect::new(stripe.x, stripe.y + 4, stripe.width, 1),
            Hit::StripeDebug,
        );
    }
    if stripe.height > 6 {
        cx.ui.hits.push(
            Rect::new(stripe.x, stripe.y + 5, stripe.width, 1),
            Hit::StripeLanguage,
        );
    }
    if stripe.height > 7 {
        cx.ui.hits.push(
            Rect::new(stripe.x, stripe.y + 6, stripe.width, 1),
            Hit::StripeHttp,
        );
    }
    if stripe.height > 8 {
        cx.ui.hits.push(
            Rect::new(stripe.x, stripe.y + 7, stripe.width, 1),
            Hit::StripeDatabase,
        );
    }
    if stripe.height > 9 {
        cx.ui.hits.push(
            Rect::new(stripe.x, stripe.y + 8, stripe.width, 1),
            Hit::StripeTodo,
        );
    }
    if stripe.height > 14 {
        cx.ui.hits.push(
            Rect::new(stripe.x, stripe.y + 13, stripe.width, 1),
            Hit::StripeDependencies,
        );
    }
    if stripe.height > 13 {
        cx.ui.hits.push(
            Rect::new(stripe.x, stripe.y + 12, stripe.width, 1),
            Hit::StripeTests,
        );
    }
    if stripe.height > 10 {
        cx.ui.hits.push(
            Rect::new(stripe.x, stripe.y + 9, stripe.width, 1),
            Hit::StripeBookmarks,
        );
    }
    if stripe.height > 11 {
        cx.ui.hits.push(
            Rect::new(stripe.x, stripe.y + 10, stripe.width, 1),
            Hit::StripeDocker,
        );
    }
    if stripe.height > 12 {
        cx.ui.hits.push(
            Rect::new(stripe.x, stripe.y + 11, stripe.width, 1),
            Hit::StripeGitHub,
        );
    }
    if stripe.height > 1 {
        cx.ui.hits.push(
            Rect::new(stripe.x, stripe.bottom() - 1, stripe.width, 1),
            Hit::StripeFind,
        );
    }
    render_vertical(frame, stripe_border, theme);
    if show_project {
        render_project(frame, project, &mut cx);
        render_vertical(frame, project_border, theme);
    }
    render_layout(frame, editor, ws.layout(), &mut cx);
    if show_project {
        let left = project.x;
        cx.ui
            .hits
            .push(project_border, Hit::Edge(Resizing::Project { left }));
    }
    if show_tool {
        let junctions: &[(u16, &str)] = if show_project {
            &[(project_border.x, glyphs::TEE_UP)]
        } else {
            &[]
        };
        render_rule(frame, tool_divider, junctions, theme);
        let tee = Rect::new(stripe_border.x, tool_divider.y, 1, 1);
        frame.render_widget(
            Line::from(glyphs::TEE_RIGHT).fg(theme.border).bg(theme.bg),
            tee,
        );
        render_tool_window(frame, tool, &mut cx);
        let below = tool.bottom();
        cx.ui
            .hits
            .push(tool_divider, Hit::Edge(Resizing::Tool { below }));
    }
    render_status(frame, status, &cx);
    render_hover(frame, area, &mut cx);
    render_completion(frame, area, &mut cx);
    render_picker(frame, area, &mut cx);
    render_run_confirm(frame, area, &mut cx);
    render_analysis_confirm(frame, area, &mut cx);
    render_db_confirm(frame, area, &mut cx);
    render_docker_confirm(frame, area, &mut cx);
    render_github_windows(frame, area, &mut cx);
    render_history(frame, area, &mut cx);
    super::notify::render_notifications(frame, area, &mut cx);
    render_merge(frame, area, &mut cx);
    render_mcp_review(frame, area, &mut cx);
    render_menu(frame, area, &mut cx);
    render_file_prompt(frame, area, &mut cx);
}

fn project_room(wanted: Option<u16>, content: u16) -> u16 {
    let max = content
        .saturating_sub(1 + EDITOR_MIN_COLUMNS)
        .max(PROJECT_MIN_WIDTH);
    wanted
        .unwrap_or(PROJECT_WIDTH)
        .clamp(PROJECT_MIN_WIDTH, max)
}

pub fn tool_room(wanted: Option<u16>, tab: ToolTab, screen: u16) -> (u16, u16, u16) {
    if screen < TOOL_SCREEN_ROWS {
        return (0, 0, 0);
    }
    let (min, share) = if matches!(
        tab,
        ToolTab::Git | ToolTab::Run | ToolTab::Debug | ToolTab::Claude | ToolTab::Terminal
    ) {
        (GIT_MIN_ROWS, (screen * 45 / 100).max(GIT_MIN_ROWS))
    } else {
        (TOOL_MIN_ROWS, (screen * 3 / 10).max(6))
    };
    let max = screen.saturating_sub(1 + EDITOR_MIN_ROWS).max(min);
    (wanted.unwrap_or(share).clamp(min, max), min, max)
}

fn render_too_small(frame: &mut Frame, area: Rect, theme: &Theme) {
    frame.render_widget(Line::default().bg(theme.bg), area);
    let lines = [
        Line::from("Terminal is too small").fg(theme.fg_primary),
        Line::from(format!(
            "Need at least {MIN_WIDTH}×{MIN_HEIGHT}, now {}×{}",
            area.width, area.height
        ))
        .fg(theme.fg_muted),
    ];
    let top = area.y + area.height.saturating_sub(2) / 2;
    for (i, line) in lines.into_iter().enumerate() {
        let row = Rect::new(area.x, top + i as u16, area.width, 1).intersection(area);
        frame.render_widget(line.centered().bg(theme.bg), row);
    }
}

fn render_menu_titles(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let open = cx.ws.menu().map(|state| state.menu);
    for (index, menu) in MENUS.iter().enumerate() {
        let (x, width) = title_span(index);
        let cell = Rect::new(area.x + x, area.y, width, 1).intersection(area);
        let style = if open == Some(index) {
            cx.theme.selected
        } else {
            Style::new().fg(cx.theme.fg_primary)
        };
        frame.render_widget(Line::from(format!(" {} ", menu.title)).style(style), cell);
        cx.ui.hits.push(cell, Hit::MenuTitle { menu: index });
    }
}

const TOOLBAR_BUTTON: u16 = 3;
const TOOLBAR_BUTTONS: u16 = 3 * TOOLBAR_BUTTON;
const TOOLBAR_NAME: usize = 24;

fn render_title(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    frame.render_widget(Line::from("").bg(cx.theme.bg_panel), area);
    render_menu_titles(frame, area, cx);
    let shift = titles_width().saturating_add(1).min(area.width);
    let area = Rect::new(area.x + shift, area.y, area.width - shift, 1);
    let root = cx.ws.root();
    let name = root.file_name().map_or_else(
        || root.display().to_string(),
        |n| n.to_string_lossy().into_owned(),
    );
    let mut spans = vec![Span::raw(format!(" {name}")).fg(cx.theme.accent).bold()];
    if cx.ws.is_recording_macro() {
        spans.push(Span::raw("  ● REC").fg(cx.theme.error).bold());
    }
    if let Some(branch) = cx.ws.git_branch() {
        spans.push(Span::raw(format!("  ⎇ {branch}")).fg(cx.theme.accent));
    }
    if let Some(operation) = cx.ws.git_operation() {
        let label = super::git::operation_label(operation);
        spans.push(Span::raw(format!(" {label}")).fg(cx.theme.warning).bold());
    }
    if !cx.ws.is_trusted() {
        spans.push(Span::raw("  ⚠ untrusted").fg(cx.theme.warning).bold());
    }
    if let Some(panel) = cx.ws.run_panel().filter(|p| p.is_active()) {
        let name = crate::core::run::visible(&panel.target.name);
        spans.push(Span::raw(format!("  ▶ {name}")).fg(cx.theme.success));
    }
    let waiting = cx.ws.mcp_pending();
    if waiting > 0 {
        let text = format!("  ◆ Claude: {waiting} to review");
        spans.push(Span::raw(text).fg(cx.theme.warning).bold());
    }
    let keep = Line::from(spans.clone()).width();
    let shown = super::panels::home_relative(&root.display().to_string());
    spans.push(Span::raw(format!("  {shown}")).fg(cx.theme.fg_muted));
    frame.render_widget(Line::from(spans).bg(cx.theme.bg_panel), area);
    render_toolbar(frame, area, u16::try_from(keep).unwrap_or(u16::MAX), cx);
}

fn render_toolbar(frame: &mut Frame, area: Rect, keep: u16, cx: &mut Ctx<'_>) {
    if cx.ws.is_window() || cx.ws.is_run_window() || area.width < TOOLBAR_BUTTONS {
        return;
    }
    let theme = cx.theme;
    let stop_color = if cx.ws.can_stop() {
        theme.error
    } else {
        theme.fg_subtle
    };
    let buttons = [
        (glyphs::EXECUTION, theme.success, Action::RerunLast),
        (glyphs::DEBUG, theme.warning, Action::DebugLast),
        (glyphs::STOP, stop_color, Action::StopRun),
    ];
    let name = cx.ws.last_run_name().map_or_else(
        || "Run…".to_owned(),
        |n| ellipsize(&crate::core::run::visible(n), TOOLBAR_NAME),
    );
    let chooser = format!(" {name} {} ", glyphs::EXPANDED);
    let chooser_width = u16::try_from(chooser.width()).unwrap_or(u16::MAX);
    let with_chooser = area.width >= keep.saturating_add(TOOLBAR_BUTTONS + chooser_width);
    let width = TOOLBAR_BUTTONS + if with_chooser { chooser_width } else { 0 };
    let mut x = area.right() - width;
    let bar = Rect::new(x, area.y, width, 1);
    frame.render_widget(ratatui::widgets::Clear, bar);
    frame.render_widget(Line::default().bg(theme.bg_panel), bar);
    if with_chooser {
        let cell = Rect::new(x, area.y, chooser_width, 1);
        frame.render_widget(
            Line::from(chooser)
                .bg(theme.bg_panel)
                .fg(theme.fg_primary)
                .patch_style(theme.selected_inactive),
            cell,
        );
        cx.ui.hits.push(cell, Hit::Button(Action::RunChoose));
        x += chooser_width;
    }
    for (glyph, color, action) in buttons {
        let cell = Rect::new(x, area.y, TOOLBAR_BUTTON, 1);
        frame.render_widget(
            Line::from(format!(" {glyph} "))
                .fg(color)
                .bg(theme.bg_panel)
                .bold(),
            cell,
        );
        cx.ui.hits.push(cell, Hit::Button(action));
        x += TOOLBAR_BUTTON;
    }
}

fn render_rule(frame: &mut Frame, area: Rect, junctions: &[(u16, &str)], theme: &Theme) {
    let mut cells: Vec<&str> = vec![glyphs::H_LINE; usize::from(area.width)];
    for (x, glyph) in junctions {
        if let Some(cell) = x
            .checked_sub(area.x)
            .and_then(|x| cells.get_mut(usize::from(x)))
        {
            *cell = glyph;
        }
    }
    frame.render_widget(
        Line::from(cells.concat()).fg(theme.border).bg(theme.bg),
        area,
    );
}

fn render_vertical(frame: &mut Frame, area: Rect, theme: &Theme) {
    for y in area.y..area.bottom() {
        frame.render_widget(
            Line::from(glyphs::V_LINE).fg(theme.border).bg(theme.bg),
            Rect::new(area.x, y, 1, 1),
        );
    }
}

fn render_stripe(frame: &mut Frame, area: Rect, cx: &Ctx<'_>) {
    let theme = cx.theme;
    frame.render_widget(Line::default().bg(theme.bg_panel), area);
    let icon = |on: bool, glyph: &'static str, color: Color| {
        let line = Line::from(glyph).fg(color).centered();
        if on {
            line.bold().patch_style(theme.selected_inactive)
        } else {
            line.bg(theme.bg_panel)
        }
    };
    let git_showing = cx.ws.tool_window_visible() && cx.ws.tool_tab() == ToolTab::Git;
    let claude_showing = cx.ws.tool_window_visible() && cx.ws.tool_tab() == ToolTab::Claude;
    let terminal_showing = cx.ws.tool_window_visible() && cx.ws.tool_tab() == ToolTab::Terminal;
    frame.render_widget(
        icon(
            cx.ws.project_visible(),
            glyphs::PROJECT,
            theme.syntax.function,
        ),
        Rect::new(area.x, area.y, area.width, 1),
    );
    if area.height > 2 {
        frame.render_widget(
            icon(git_showing, glyphs::GIT, theme.syntax.keyword),
            Rect::new(area.x, area.y + 1, area.width, 1),
        );
    }
    if area.height > 3 {
        frame.render_widget(
            icon(claude_showing, glyphs::CLAUDE, theme.warning),
            Rect::new(area.x, area.y + 2, area.width, 1),
        );
    }
    if area.height > 4 {
        frame.render_widget(
            icon(terminal_showing, glyphs::PROMPT, theme.success),
            Rect::new(area.x, area.y + 3, area.width, 1),
        );
    }
    let debug_showing = cx.ws.tool_window_visible() && cx.ws.tool_tab() == ToolTab::Debug;
    if area.height > 5 {
        frame.render_widget(
            icon(debug_showing, glyphs::DEBUG, theme.error),
            Rect::new(area.x, area.y + 4, area.width, 1),
        );
    }
    let language_showing = cx.ws.tool_window_visible() && cx.ws.tool_tab() == ToolTab::Language;
    if area.height > 6 {
        frame.render_widget(
            icon(language_showing, glyphs::LANGUAGE_SERVER, theme.success),
            Rect::new(area.x, area.y + 5, area.width, 1),
        );
    }
    let deps_showing = cx.ws.tool_window_visible() && cx.ws.tool_tab() == ToolTab::Dependencies;
    if area.height > 14 {
        frame.render_widget(
            icon(deps_showing, glyphs::DEPENDENCIES, theme.accent),
            Rect::new(area.x, area.y + 13, area.width, 1),
        );
    }
    let tests_showing = cx.ws.tool_window_visible() && cx.ws.tool_tab() == ToolTab::Tests;
    if area.height > 13 {
        frame.render_widget(
            icon(tests_showing, glyphs::TESTS, theme.success),
            Rect::new(area.x, area.y + 12, area.width, 1),
        );
    }
    let todo_showing = cx.ws.tool_window_visible() && cx.ws.tool_tab() == ToolTab::Todo;
    if area.height > 9 {
        frame.render_widget(
            icon(todo_showing, glyphs::TODO, theme.warning),
            Rect::new(area.x, area.y + 8, area.width, 1),
        );
    }
    let bookmarks_showing = cx.ws.tool_window_visible() && cx.ws.tool_tab() == ToolTab::Bookmarks;
    if area.height > 10 {
        frame.render_widget(
            icon(bookmarks_showing, glyphs::BOOKMARKS, theme.warning),
            Rect::new(area.x, area.y + 9, area.width, 1),
        );
    }
    let http_showing = cx.ws.tool_window_visible() && cx.ws.tool_tab() == ToolTab::Http;
    if area.height > 7 {
        frame.render_widget(
            icon(http_showing, glyphs::HTTP, theme.success),
            Rect::new(area.x, area.y + 6, area.width, 1),
        );
    }
    let database_showing = cx.ws.tool_window_visible() && cx.ws.tool_tab() == ToolTab::Database;
    if area.height > 8 {
        frame.render_widget(
            icon(database_showing, glyphs::DATABASE, theme.success),
            Rect::new(area.x, area.y + 7, area.width, 1),
        );
    }
    let docker_showing = cx.ws.tool_window_visible() && cx.ws.tool_tab() == ToolTab::Docker;
    if area.height > 11 {
        frame.render_widget(
            icon(docker_showing, glyphs::DOCKER, theme.success),
            Rect::new(area.x, area.y + 10, area.width, 1),
        );
    }
    let github_showing = cx.ws.tool_window_visible() && cx.ws.tool_tab() == ToolTab::GitHub;
    if area.height > 12 {
        frame.render_widget(
            icon(github_showing, glyphs::GITHUB, theme.success),
            Rect::new(area.x, area.y + 11, area.width, 1),
        );
    }
    if area.height > 1 {
        frame.render_widget(
            icon(
                cx.ws.tool_window_visible()
                    && !git_showing
                    && !claude_showing
                    && !terminal_showing
                    && !debug_showing
                    && !language_showing
                    && !http_showing
                    && !database_showing
                    && !docker_showing
                    && !github_showing,
                glyphs::FIND,
                theme.success,
            ),
            Rect::new(area.x, area.bottom() - 1, area.width, 1),
        );
    }
}

fn render_status(frame: &mut Frame, area: Rect, cx: &Ctx<'_>) {
    let theme = cx.theme;
    let ws = cx.ws;
    let mut left = match ws.status() {
        Some(msg) => {
            let (glyph, color) = match msg.level {
                Level::Info => (glyphs::SUCCESS, theme.success),
                Level::Error => (glyphs::ERROR, theme.error),
            };
            Line::from(Span::raw(format!(" {glyph} {}", msg.text)).fg(color))
        }
        None => {
            let root = ws
                .root()
                .file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default();
            let mut parts = vec![root];
            if let Some((_, doc)) = ws.active() {
                parts.extend(
                    ws.display_path(doc.path())
                        .iter()
                        .map(|c| c.to_string_lossy().into_owned()),
                );
            }
            let last = parts.pop().unwrap_or_default();
            let sep = format!(" {} ", glyphs::BREADCRUMB);
            let mut spans = vec![Span::raw(" ")];
            for p in parts {
                spans.push(Span::raw(format!("{p}{sep}")).fg(theme.fg_muted));
            }
            let color = ws
                .active()
                .map_or(theme.fg_primary, |(_, d)| theme.file_color(d.path()));
            spans.push(Span::raw(last).fg(color));
            Line::from(spans)
        }
    };
    if let Some((done, total)) = ws.index_progress() {
        left.push_span(Span::raw(format!("  Indexing {done}/{total}")).fg(theme.fg_muted));
    }
    frame.render_widget(Line::default().bg(theme.bg_panel), area);
    if let Some((view, doc)) = ws.active() {
        let rope = doc.text();
        let line = rope.char_to_line(view.cursor.idx);
        let col = text::col_of(rope, view.cursor.idx, ws.tab_width());
        let mut spans = vec![
            Span::raw(format!("{}:{}  ", line + 1, col + 1)).fg(theme.fg_primary),
            Span::raw(format!(
                "{}  UTF-8  {} spaces  ",
                doc.line_ending().label(),
                ws.tab_width(),
            ))
            .fg(theme.fg_muted),
            Span::raw(format!("{}  ", doc.language().name())).fg(theme.file_color(doc.path())),
        ];
        match ws.coverage_state(doc) {
            Some(CoverageState::Current(summary)) => {
                spans.push(Span::raw(format!("cov {}%  ", summary.percent())).fg(theme.fg_muted))
            }
            Some(CoverageState::Stale) => {
                spans.push(Span::raw("cov stale  ").fg(theme.warning));
            }
            None => {}
        }
        spans.extend(problem_counts(ws, theme));
        let hint = lsp_status(ws, theme, usize::from(area.width))
            .filter(|_| ws.lsp_status().is_some_and(|s| s.install.is_some()));
        if let Some(hint) = hint {
            let beside: usize = spans.iter().map(Span::width).sum();
            if beside + hint.width() + left.width() > usize::from(area.width) {
                spans.clear();
            }
            let fits_left =
                hint.width() + spans.iter().map(Span::width).sum::<usize>() + left.width()
                    <= usize::from(area.width);
            spans.push(hint);
            frame.render_widget(Line::from(spans).right_aligned(), area);
            if fits_left {
                frame.render_widget(left, area);
            }
            return;
        }
        spans.extend(lsp_status(ws, theme, usize::from(area.width)));
        frame.render_widget(Line::from(spans).right_aligned(), area);
    }
    frame.render_widget(left, area);
}

fn problem_counts(ws: &Workspace, theme: &Theme) -> Vec<Span<'static>> {
    let (errors, warnings) = ws.problem_counts();
    let mut spans = Vec::new();
    if errors > 0 {
        spans.push(Span::raw(format!("{} {errors}  ", glyphs::ERROR)).fg(theme.error));
    }
    if warnings > 0 {
        spans.push(Span::raw(format!("{} {warnings}  ", glyphs::WARNING)).fg(theme.warning));
    }
    spans
}

fn lsp_status(ws: &Workspace, theme: &Theme, room: usize) -> Option<Span<'static>> {
    let status = ws.lsp_status()?;
    let name = &status.name;
    let span = match status.state {
        LspState::Off | LspState::Disabled => return None,
        LspState::Ready if status.busy => {
            Span::raw(format!("{} ", glyphs::SPINNER)).fg(theme.accent)
        }
        LspState::Ready => Span::raw(format!("{} ", glyphs::SUCCESS)).fg(theme.success),
        LspState::Installing => {
            Span::raw(format!("{} downloading {name} ", glyphs::SPINNER)).fg(theme.accent)
        }
        LspState::Starting => Span::raw(format!("{} ", glyphs::SPINNER)).fg(theme.accent),
        LspState::Crashed => {
            Span::raw(format!("{} {name} crashed ", glyphs::ERROR)).fg(theme.error)
        }
        LspState::Stopped => {
            Span::raw(format!("{} {name} stopped ", glyphs::ERROR)).fg(theme.error)
        }
        LspState::NotFound => {
            let mut text = format!("○ {name} not found ");
            if let Some(install) = status.install {
                let long = format!("○ {name} not found: {install} ");
                text = if Span::raw(long.as_str()).width() <= room {
                    long
                } else {
                    format!("○ not found: {install} ")
                };
            }
            Span::raw(text).fg(theme.fg_muted)
        }
    };
    Some(span)
}

pub fn follow_or_clamp(
    scroll: usize,
    followed: &mut Option<usize>,
    selected: usize,
    height: usize,
    len: usize,
) -> usize {
    if followed.replace(selected) != Some(selected) {
        ensure_visible((scroll, 0), (selected, 0), (height, 1)).0
    } else {
        scroll.min(len.saturating_sub(height))
    }
}

pub fn ensure_visible(
    scroll: (usize, usize),
    cursor: (usize, usize),
    size: (usize, usize),
) -> (usize, usize) {
    fn axis(scroll: usize, pos: usize, len: usize) -> usize {
        if pos < scroll {
            pos
        } else if len > 0 && pos >= scroll + len {
            pos + 1 - len
        } else {
            scroll
        }
    }
    (
        axis(scroll.0, cursor.0, size.0),
        axis(scroll.1, cursor.1, size.1),
    )
}

#[cfg(test)]
#[path = "render_tests.rs"]
mod tests;
