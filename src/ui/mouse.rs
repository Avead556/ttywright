use std::path::PathBuf;
use std::time::{Duration, Instant};

use ratatui::crossterm::event::{MouseButton, MouseEvent, MouseEventKind};
use ratatui::layout::{Position, Rect};

use super::render::UiState;
use crate::core::wrap::Metrics;
use crate::core::{Action, Command, Document, DocumentId, Focus, ToolTab, ViewId, Workspace};

const DOUBLE_CLICK: Duration = Duration::from_millis(400);
const WHEEL_LINES: usize = 3;

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub enum Pointer {
    #[default]
    Arrow,
    Text,
    Hand,
    ResizeRows,
    ResizeColumns,
}

impl Pointer {
    pub fn sequence(self) -> String {
        let name = match self {
            Self::Arrow => "default",
            Self::Text => "text",
            Self::Hand => "pointer",
            Self::ResizeRows => "ns-resize",
            Self::ResizeColumns => "ew-resize",
        };
        format!("\x1b]22;{name}\x1b\\")
    }
}

pub fn pointer_at(ui: &UiState, x: u16, y: u16) -> Pointer {
    match ui.resizing {
        Some(Resizing::Tool { .. }) => return Pointer::ResizeRows,
        Some(Resizing::Project { .. }) => return Pointer::ResizeColumns,
        None => {}
    }
    if ui.dragging.is_some() {
        return Pointer::Text;
    }
    match ui.hits.at(x, y) {
        Some(Hit::Text { .. } | Hit::ClaudePanel | Hit::TerminalPanel | Hit::Preview { .. }) => {
            Pointer::Text
        }
        Some(Hit::Edge(Resizing::Tool { .. })) => Pointer::ResizeRows,
        Some(Hit::Edge(Resizing::Project { .. })) => Pointer::ResizeColumns,
        Some(
            Hit::Tab { .. }
            | Hit::CloseTab { .. }
            | Hit::MarkdownSwitch
            | Hit::TreeRow { .. }
            | Hit::SearchResult { .. }
            | Hit::MenuTitle { .. }
            | Hit::MenuItem { .. }
            | Hit::StripeProject
            | Hit::StripeFind
            | Hit::StripeGit
            | Hit::StripeClaude
            | Hit::StripeTerminal
            | Hit::StripeDebug
            | Hit::StripeLanguage
            | Hit::StripeHttp
            | Hit::StripeDatabase
            | Hit::StripeDocker
            | Hit::StripeGitHub
            | Hit::GitHubRow { .. }
            | Hit::DockerRow { .. }
            | Hit::StripeTodo
            | Hit::Notification { .. }
            | Hit::NotificationClose { .. }
            | Hit::StripeTests
            | Hit::TestRow { .. }
            | Hit::StripeDependencies
            | Hit::DependencyRow { .. }
            | Hit::StripeBookmarks
            | Hit::TodoRow { .. }
            | Hit::DuplicateRow { .. }
            | Hit::BookmarkRow { .. }
            | Hit::DatabaseConnection { .. }
            | Hit::DatabaseTable { .. }
            | Hit::Button(_)
            | Hit::Gutter { .. }
            | Hit::DebugRow { .. }
            | Hit::TerminalTab { .. }
            | Hit::TerminalCloseTab { .. }
            | Hit::TerminalNewTab
            | Hit::PickerItem { .. }
            | Hit::Problem { .. }
            | Hit::GitRow { .. }
            | Hit::RunLine { .. }
            | Hit::RunTab { .. }
            | Hit::HistoryEntry { .. },
        ) => Pointer::Hand,
        _ => Pointer::Arrow,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resizing {
    Tool { below: u16 },
    Project { left: u16 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Hit {
    Text {
        view: ViewId,
        origin: Position,
        top: usize,
        left: usize,
        wrap: Option<usize>,
    },
    Tab {
        view: ViewId,
    },
    TreeRow {
        path: PathBuf,
    },
    Tree,
    SearchResult {
        index: usize,
    },
    Search,
    MenuTitle {
        menu: usize,
    },
    MenuItem {
        entry: usize,
    },
    Menu,
    Edge(Resizing),
    StripeProject,
    StripeFind,
    StripeGit,
    StripeClaude,
    StripeDebug,
    StripeLanguage,
    LanguagePanel,
    StripeHttp,
    HttpPanel,
    StripeDatabase,
    StripeDocker,
    StripeGitHub,
    GitHubPanel,
    GitHubRow {
        index: usize,
    },
    DockerPanel,
    DockerRow {
        section: usize,
        index: usize,
    },
    StripeTodo,
    TodoPanel,
    StripeDependencies,
    DependenciesPanel,
    DependencyRow {
        index: usize,
    },
    Notification {
        id: u64,
    },
    NotificationClose {
        id: u64,
    },
    StripeTests,
    TestsPanel,
    TestRow {
        index: usize,
    },
    DuplicatesPanel,
    DuplicateRow {
        index: usize,
    },
    TodoRow {
        index: usize,
    },
    StripeBookmarks,
    BookmarksPanel,
    BookmarkRow {
        index: usize,
    },
    DatabasePanel,
    DatabaseConnection {
        index: usize,
    },
    DatabaseTable {
        index: usize,
    },
    Button(Action),
    Gutter {
        view: ViewId,
        line: usize,
    },
    DebugPanel,
    DebugRow {
        part: crate::core::debug::DebugPart,
        index: usize,
    },
    ClaudePanel,
    StripeTerminal,
    TerminalPanel,
    TerminalTab {
        index: usize,
    },
    TerminalCloseTab {
        index: usize,
    },
    CloseTab {
        view: crate::core::ViewId,
    },
    TerminalNewTab,
    PickerItem {
        index: usize,
    },
    Picker,
    History,
    Problems,
    Problem {
        index: usize,
    },
    GitPanel,
    GitRow {
        index: usize,
    },
    MarkdownSwitch,
    Preview {
        doc: DocumentId,
    },
    RunTab {
        index: usize,
    },
    RunPanel,
    RunLine {
        line: usize,
    },
    HistoryEntry {
        index: usize,
    },
}

#[derive(Debug, Default, Clone)]
pub struct HitMap {
    regions: Vec<(Rect, Hit)>,
}

impl HitMap {
    pub fn clear(&mut self) {
        self.regions.clear();
    }

    pub fn push(&mut self, area: Rect, hit: Hit) {
        if area.width > 0 && area.height > 0 {
            self.regions.push((area, hit));
        }
    }

    pub fn at(&self, x: u16, y: u16) -> Option<&Hit> {
        let p = Position::new(x, y);
        self.regions
            .iter()
            .rev()
            .find(|(r, _)| r.contains(p))
            .map(|(_, h)| h)
    }
}

pub fn handle(event: &MouseEvent, ui: &mut UiState, ws: &Workspace, now: Instant) -> Vec<Command> {
    let (x, y) = (event.column, event.row);
    let hit = ui.hits.at(x, y).cloned();
    match event.kind {
        MouseEventKind::Down(MouseButton::Left) => {
            let double = ui.last_click.is_some_and(|(t, cx, cy)| {
                cy == y && cx.abs_diff(x) <= 1 && now.duration_since(t) <= DOUBLE_CLICK
            });
            ui.last_click = if double { None } else { Some((now, x, y)) };
            if let Some(Hit::Edge(edge)) = hit
                && ws.picker().is_none()
                && ws.menu().is_none()
            {
                ui.resizing = Some(edge);
                return Vec::new();
            }
            ui.dragging = match &hit {
                Some(Hit::Text { view, .. }) if ws.picker().is_none() => Some(*view),
                _ => None,
            };
            let mut commands = click(hit.clone(), (x, y), double, ws);
            if double && matches!(hit, Some(Hit::Text { .. })) && ws.picker().is_none() {
                commands.push(Command::SelectWord);
            }
            commands
        }
        MouseEventKind::Drag(MouseButton::Left) => match ui.resizing {
            Some(Resizing::Tool { below }) => vec![Command::ResizeTool {
                rows: below.saturating_sub(y.saturating_add(1)),
            }],
            Some(Resizing::Project { left }) => vec![Command::ResizeProject {
                columns: x.saturating_sub(left),
            }],
            None => drag((x, y), ui, ws),
        },
        MouseEventKind::Up(_) => {
            ui.dragging = None;
            ui.resizing = None;
            Vec::new()
        }
        MouseEventKind::ScrollDown => wheel(hit, ui, ws, true),
        MouseEventKind::ScrollUp => wheel(hit, ui, ws, false),
        _ => Vec::new(),
    }
}

fn text_position(ws: &Workspace, hit: &TextHit, (x, y): (u16, u16)) -> (usize, usize) {
    let dy = usize::from(y.saturating_sub(hit.origin.y));
    let dx = usize::from(x.saturating_sub(hit.origin.x));
    let Some(cols) = hit.wrap else {
        return (hit.top + dy, hit.left + dx);
    };
    let Some(t) = ws
        .view(hit.view)
        .and_then(|v| ws.document(v.doc))
        .map(Document::text)
    else {
        return (hit.top, dx);
    };
    let metrics = Metrics {
        tab_width: ws.tab_width(),
        wrap: Some(cols),
    };
    let (line, row) = metrics.step(t, (hit.top, hit.left), dy as isize);
    let rows = metrics.rows(t, line);
    let cell = &rows[row];
    let col = cell.cols.start + dx;
    let col = if row + 1 < rows.len() {
        col.min(cell.cols.end.saturating_sub(1).max(cell.cols.start))
    } else {
        col
    };
    (line, col)
}

struct TextHit {
    view: ViewId,
    origin: Position,
    top: usize,
    left: usize,
    wrap: Option<usize>,
}

fn drag((x, y): (u16, u16), ui: &UiState, ws: &Workspace) -> Vec<Command> {
    let Some(dragging) = ui.dragging else {
        return Vec::new();
    };
    let area = ui.hits.regions.iter().rev().find_map(|(_, hit)| match hit {
        Hit::Text {
            view,
            origin,
            top,
            left,
            wrap,
        } if *view == dragging => Some(TextHit {
            view: *view,
            origin: *origin,
            top: *top,
            left: *left,
            wrap: *wrap,
        }),
        _ => None,
    });
    let Some(hit) = area else {
        return Vec::new();
    };
    let (line, col) = text_position(ws, &hit, (x, y));
    vec![Command::SelectTo { line, col }]
}

fn click(hit: Option<Hit>, (x, y): (u16, u16), double: bool, ws: &Workspace) -> Vec<Command> {
    let picker_open = ws.picker().is_some();
    let command = match hit {
        Some(Hit::MenuItem { entry }) => Command::PickMenuItem { entry },
        Some(Hit::Menu) => return Vec::new(),
        Some(Hit::MenuTitle { .. }) if picker_open => Command::Escape,
        Some(Hit::MenuTitle { menu }) => Command::OpenMenu { menu },
        _ if ws.menu().is_some() => Command::Escape,
        Some(Hit::PickerItem { index }) => Command::SelectPickerItem {
            index,
            accept: true,
        },
        Some(Hit::Picker | Hit::History) => return Vec::new(),
        Some(Hit::HistoryEntry { index }) => Command::SelectHistoryEntry { index },
        Some(Hit::GitRow { index }) => Command::SelectGitRow { index },
        _ if picker_open => Command::Escape,
        Some(Hit::Edge(_)) => return Vec::new(),
        Some(Hit::Button(action)) => Command::Run(action),
        Some(Hit::Gutter { view, line }) => Command::ToggleBreakpointAt { view, line },
        Some(Hit::StripeDebug) if ws.tool_window_visible() && ws.tool_tab() == ToolTab::Debug => {
            Command::Run(Action::HideToolWindow)
        }
        Some(Hit::StripeDebug) => Command::Run(Action::DebugShow),
        Some(Hit::DebugPanel) => Command::Focus(Focus::Debug),
        Some(Hit::DebugRow { part, index }) => Command::SelectDebugRow {
            part,
            index,
            open: double,
        },
        Some(Hit::Text {
            view,
            origin,
            top,
            left,
            wrap,
        }) => {
            let hit = TextHit {
                view,
                origin,
                top,
                left,
                wrap,
            };
            let (line, col) = text_position(ws, &hit, (x, y));
            Command::FocusView {
                view,
                at: Some((line, col)),
            }
        }
        Some(Hit::Tab { view }) => Command::FocusView { view, at: None },
        Some(Hit::CloseTab { view }) => Command::CloseView { view },
        Some(Hit::MarkdownSwitch) => Command::Run(Action::MarkdownPreview),
        Some(Hit::TreeRow { path }) => Command::SelectTreeRow {
            path,
            activate: double,
        },
        Some(Hit::Tree) => Command::Focus(Focus::Project),
        Some(Hit::SearchResult { index }) => Command::SelectSearchResult {
            index,
            open: double,
        },
        Some(Hit::Search) => Command::Focus(Focus::FindInFiles),
        Some(Hit::StripeLanguage)
            if ws.tool_window_visible() && ws.tool_tab() == ToolTab::Language =>
        {
            Command::Run(Action::HideToolWindow)
        }
        Some(Hit::StripeLanguage) => Command::Run(Action::ShowLanguageServer),
        Some(Hit::LanguagePanel) => Command::Focus(Focus::Language),
        Some(Hit::StripeTodo) if ws.tool_window_visible() && ws.tool_tab() == ToolTab::Todo => {
            Command::Run(Action::HideToolWindow)
        }
        Some(Hit::StripeTodo) => Command::Run(Action::ShowTodos),
        Some(Hit::TodoPanel) => Command::Focus(Focus::Todo),
        Some(Hit::StripeDependencies)
            if ws.tool_window_visible() && ws.tool_tab() == ToolTab::Dependencies =>
        {
            Command::Run(Action::HideToolWindow)
        }
        Some(Hit::StripeDependencies) => Command::Run(Action::ShowDependencies),
        Some(Hit::DependenciesPanel) => Command::Focus(Focus::Dependencies),
        Some(Hit::DependencyRow { index }) => Command::SelectDependencyRow {
            index,
            open: double,
        },
        Some(Hit::StripeTests) if ws.tool_window_visible() && ws.tool_tab() == ToolTab::Tests => {
            Command::Run(Action::HideToolWindow)
        }
        Some(Hit::StripeTests) => Command::Run(Action::ShowTests),
        Some(Hit::TestsPanel) => Command::Focus(Focus::Tests),
        Some(Hit::TestRow { index }) => Command::SelectTestRow {
            index,
            open: double,
        },
        Some(Hit::DuplicatesPanel) => Command::Focus(Focus::Duplicates),
        Some(Hit::DuplicateRow { index }) => Command::SelectDuplicateRow {
            index,
            open: double,
        },
        Some(Hit::Notification { id }) => Command::NotificationClick { id },
        Some(Hit::NotificationClose { id }) => Command::NotificationClose { id },
        Some(Hit::TodoRow { index }) => Command::SelectTodoRow {
            index,
            open: double,
        },
        Some(Hit::StripeBookmarks)
            if ws.tool_window_visible() && ws.tool_tab() == ToolTab::Bookmarks =>
        {
            Command::Run(Action::HideToolWindow)
        }
        Some(Hit::StripeBookmarks) => Command::Run(Action::ShowBookmarks),
        Some(Hit::BookmarksPanel) => Command::Focus(Focus::Bookmarks),
        Some(Hit::BookmarkRow { index }) => Command::SelectBookmarkRow {
            index,
            open: double,
        },
        Some(Hit::StripeHttp) if ws.tool_window_visible() && ws.tool_tab() == ToolTab::Http => {
            Command::Run(Action::HideToolWindow)
        }
        Some(Hit::StripeHttp) => Command::Run(Action::ShowHttp),
        Some(Hit::HttpPanel) => Command::Focus(Focus::Http),
        Some(Hit::StripeDatabase)
            if ws.tool_window_visible() && ws.tool_tab() == ToolTab::Database =>
        {
            Command::Run(Action::HideToolWindow)
        }
        Some(Hit::StripeDatabase) => Command::Run(Action::ShowDatabase),
        Some(Hit::StripeDocker) if ws.tool_window_visible() && ws.tool_tab() == ToolTab::Docker => {
            Command::Run(Action::HideToolWindow)
        }
        Some(Hit::StripeDocker) => Command::Run(Action::ShowDocker),
        Some(Hit::StripeGitHub) if ws.tool_window_visible() && ws.tool_tab() == ToolTab::GitHub => {
            Command::Run(Action::HideToolWindow)
        }
        Some(Hit::StripeGitHub) => Command::Run(Action::ShowGitHub),
        Some(Hit::GitHubPanel) => Command::Focus(Focus::GitHub),
        Some(Hit::GitHubRow { index }) => Command::PickGitHubRow { index },
        Some(Hit::DockerPanel) => Command::Focus(Focus::Docker),
        Some(Hit::DockerRow { section, index }) => Command::PickDockerRow { section, index },
        Some(Hit::DatabasePanel) => Command::Focus(Focus::Database),
        Some(Hit::DatabaseConnection { index }) => Command::PickDatabaseRow {
            table: false,
            index,
        },
        Some(Hit::DatabaseTable { index }) => Command::PickDatabaseRow { table: true, index },
        Some(Hit::Problems) => Command::Focus(Focus::Problems),
        Some(Hit::Problem { index }) => Command::SelectProblem {
            index,
            open: double,
        },
        Some(Hit::StripeProject) => {
            if ws.project_visible() && ws.focus() != Focus::Project {
                return vec![
                    Command::Run(Action::ToggleProject),
                    Command::Run(Action::ToggleProject),
                ];
            }
            Command::Run(Action::ToggleProject)
        }
        Some(Hit::StripeGit) if ws.tool_window_visible() && ws.tool_tab() == ToolTab::Git => {
            Command::Run(Action::HideToolWindow)
        }
        Some(Hit::StripeGit) => Command::Run(Action::GitLog),
        Some(Hit::GitPanel) => Command::Focus(Focus::Git),
        Some(Hit::StripeClaude) if ws.tool_window_visible() && ws.tool_tab() == ToolTab::Claude => {
            Command::Run(Action::HideToolWindow)
        }
        Some(Hit::StripeClaude) => Command::Run(Action::ClaudeOpen),
        Some(Hit::ClaudePanel) => Command::Focus(Focus::Claude),
        Some(Hit::StripeTerminal)
            if ws.tool_window_visible() && ws.tool_tab() == ToolTab::Terminal =>
        {
            Command::Run(Action::HideToolWindow)
        }
        Some(Hit::StripeTerminal) => Command::Run(Action::TerminalToggle),
        Some(Hit::TerminalPanel) => Command::Focus(Focus::Terminal),
        Some(Hit::TerminalTab { index }) => Command::SelectTerminalTab { index },
        Some(Hit::TerminalCloseTab { index }) => Command::CloseTerminalTab { index },
        Some(Hit::TerminalNewTab) => Command::Run(Action::TerminalNew),
        Some(Hit::RunPanel) => Command::Focus(Focus::Run),
        Some(Hit::RunTab { index }) => Command::SelectRunTab { index },
        Some(Hit::RunLine { line }) => Command::SelectRunLine { line },
        Some(Hit::StripeFind) if ws.tool_window_visible() => Command::Run(Action::HideToolWindow),
        Some(Hit::StripeFind) => Command::Run(Action::FindInProject),
        Some(Hit::Preview { .. }) | None => return Vec::new(),
    };
    vec![command]
}

fn wheel(hit: Option<Hit>, ui: &mut UiState, ws: &Workspace, down: bool) -> Vec<Command> {
    match hit {
        Some(Hit::DebugRow {
            part: crate::core::debug::DebugPart::Console,
            ..
        }) => vec![Command::ScrollDebugConsole {
            delta: if down { -3 } else { 3 },
        }],
        Some(Hit::Text {
            view,
            wrap: Some(cols),
            ..
        }) => {
            if let Some(t) = ws
                .view(view)
                .and_then(|v| ws.document(v.doc))
                .map(Document::text)
            {
                let metrics = Metrics {
                    tab_width: ws.tab_width(),
                    wrap: Some(cols),
                };
                let delta = if down {
                    WHEEL_LINES as isize
                } else {
                    -(WHEEL_LINES as isize)
                };
                let scroll = ui.scroll.entry(view).or_default();
                *scroll = metrics.step(t, *scroll, delta);
            }
            Vec::new()
        }
        Some(Hit::Text { view, .. }) => {
            let lines = ws
                .view(view)
                .and_then(|v| ws.document(v.doc))
                .map_or(1, |d| d.text().len_lines());
            let scroll = ui.scroll.entry(view).or_default();
            scroll.0 = if down {
                (scroll.0 + WHEEL_LINES).min(lines.saturating_sub(1))
            } else {
                scroll.0.saturating_sub(WHEEL_LINES)
            };
            Vec::new()
        }
        Some(Hit::TreeRow { .. } | Hit::Tree) => {
            ui.tree_scroll = if down {
                ui.tree_scroll + WHEEL_LINES
            } else {
                ui.tree_scroll.saturating_sub(WHEEL_LINES)
            };
            Vec::new()
        }
        Some(Hit::SearchResult { .. } | Hit::Search) => {
            ui.results_scroll = if down {
                ui.results_scroll + WHEEL_LINES
            } else {
                ui.results_scroll.saturating_sub(WHEEL_LINES)
            };
            Vec::new()
        }
        Some(Hit::DuplicatesPanel | Hit::DuplicateRow { .. }) => {
            ui.duplicates_scroll = if down {
                ui.duplicates_scroll + WHEEL_LINES
            } else {
                ui.duplicates_scroll.saturating_sub(WHEEL_LINES)
            };
            Vec::new()
        }
        Some(Hit::DependenciesPanel | Hit::DependencyRow { .. }) => {
            ui.deps_scroll = if down {
                ui.deps_scroll + WHEEL_LINES
            } else {
                ui.deps_scroll.saturating_sub(WHEEL_LINES)
            };
            Vec::new()
        }
        Some(Hit::TestsPanel | Hit::TestRow { .. }) => {
            ui.tests_scroll = if down {
                ui.tests_scroll + WHEEL_LINES
            } else {
                ui.tests_scroll.saturating_sub(WHEEL_LINES)
            };
            Vec::new()
        }
        Some(Hit::TodoPanel | Hit::TodoRow { .. }) => {
            ui.todo_scroll = if down {
                ui.todo_scroll + WHEEL_LINES
            } else {
                ui.todo_scroll.saturating_sub(WHEEL_LINES)
            };
            Vec::new()
        }
        Some(Hit::BookmarksPanel | Hit::BookmarkRow { .. }) => {
            ui.bookmarks_scroll = if down {
                ui.bookmarks_scroll + WHEEL_LINES
            } else {
                ui.bookmarks_scroll.saturating_sub(WHEEL_LINES)
            };
            Vec::new()
        }
        Some(Hit::Problems | Hit::Problem { .. }) => {
            ui.problems_scroll = if down {
                ui.problems_scroll + WHEEL_LINES
            } else {
                ui.problems_scroll.saturating_sub(WHEEL_LINES)
            };
            Vec::new()
        }
        Some(Hit::Preview { doc }) => {
            if let Some(cache) = ui.markdown.get_mut(&doc) {
                cache.top = if down {
                    cache.top + WHEEL_LINES
                } else {
                    cache.top.saturating_sub(WHEEL_LINES)
                };
            }
            Vec::new()
        }
        Some(Hit::TerminalPanel) => {
            let delta = WHEEL_LINES as isize;
            vec![Command::TerminalScroll {
                delta: if down { -delta } else { delta },
            }]
        }
        Some(Hit::ClaudePanel) => {
            let delta = WHEEL_LINES as isize;
            vec![Command::ClaudeScroll {
                delta: if down { -delta } else { delta },
            }]
        }
        Some(Hit::DatabasePanel | Hit::DatabaseConnection { .. } | Hit::DatabaseTable { .. }) => {
            let lines = WHEEL_LINES as isize;
            vec![Command::ScrollDatabase {
                delta: if down { lines } else { -lines },
            }]
        }
        Some(Hit::GitHubPanel | Hit::GitHubRow { .. }) => {
            let lines = WHEEL_LINES as isize;
            vec![Command::ScrollGitHub {
                delta: if down { lines } else { -lines },
            }]
        }
        Some(Hit::DockerPanel | Hit::DockerRow { .. }) => {
            let lines = WHEEL_LINES as isize;
            vec![Command::ScrollDocker {
                delta: if down { lines } else { -lines },
            }]
        }
        Some(Hit::HttpPanel) => {
            let lines = WHEEL_LINES as isize;
            vec![Command::ScrollHttp {
                delta: if down { lines } else { -lines },
            }]
        }
        Some(Hit::RunPanel | Hit::RunLine { .. }) => {
            let lines = WHEEL_LINES as isize;
            ui.run_scroll = if down {
                ui.run_scroll + WHEEL_LINES
            } else {
                ui.run_scroll.saturating_sub(WHEEL_LINES)
            };
            vec![Command::ScrollRun {
                delta: if down { lines } else { -lines },
            }]
        }
        Some(Hit::History | Hit::HistoryEntry { .. }) => {
            let rows = WHEEL_LINES;
            vec![Command::Move(if down {
                crate::core::Motion::PageDown { rows }
            } else {
                crate::core::Motion::PageUp { rows }
            })]
        }
        Some(Hit::PickerItem { .. } | Hit::Picker) => {
            let step = if down {
                crate::core::Motion::Down
            } else {
                crate::core::Motion::Up
            };
            vec![Command::Move(step); WHEEL_LINES]
        }
        _ => Vec::new(),
    }
}

#[cfg(test)]
mod tests {
    use ratatui::crossterm::event::KeyModifiers;

    use super::*;

    fn mouse(kind: MouseEventKind, x: u16, y: u16) -> MouseEvent {
        MouseEvent {
            kind,
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        }
    }

    fn setup() -> (Workspace, UiState) {
        let (ws, _) = Workspace::new(PathBuf::from("/p"));
        let mut ui = UiState::default();
        ui.hits.push(Rect::new(0, 0, 10, 10), Hit::Tree);
        ui.hits.push(
            Rect::new(0, 2, 10, 1),
            Hit::TreeRow {
                path: "/p/a".into(),
            },
        );
        ui.hits.push(
            Rect::new(20, 0, 40, 10),
            Hit::Text {
                view: ViewId(7),
                origin: Position::new(26, 1),
                top: 10,
                left: 2,
                wrap: None,
            },
        );
        (ws, ui)
    }

    #[test]
    fn click_in_text_maps_to_line_and_display_column() {
        let (ws, mut ui) = setup();
        let cmds = handle(
            &mouse(MouseEventKind::Down(MouseButton::Left), 30, 4),
            &mut ui,
            &ws,
            Instant::now(),
        );
        assert_eq!(
            cmds,
            [Command::FocusView {
                view: ViewId(7),
                at: Some((13, 6))
            }]
        );
    }

    #[test]
    fn second_click_on_the_same_row_soon_after_is_a_double_click() {
        let (ws, mut ui) = setup();
        let t = Instant::now();
        let down = mouse(MouseEventKind::Down(MouseButton::Left), 3, 2);
        let path = PathBuf::from("/p/a");
        assert_eq!(
            handle(&down, &mut ui, &ws, t),
            [Command::SelectTreeRow {
                path: path.clone(),
                activate: false
            }]
        );
        let again = handle(&down, &mut ui, &ws, t + Duration::from_millis(200));
        assert_eq!(
            again,
            [Command::SelectTreeRow {
                path: path.clone(),
                activate: true
            }]
        );
        let late = handle(&down, &mut ui, &ws, t + Duration::from_secs(5));
        assert_eq!(
            late,
            [Command::SelectTreeRow {
                path,
                activate: false
            }]
        );
    }

    #[test]
    fn topmost_region_wins() {
        let (_, ui) = setup();
        assert!(matches!(ui.hits.at(3, 2), Some(Hit::TreeRow { .. })));
        assert_eq!(ui.hits.at(3, 5), Some(&Hit::Tree));
        assert_eq!(ui.hits.at(70, 5), None);
    }

    #[test]
    fn wheel_over_text_scrolls_without_commands() {
        let (ws, mut ui) = setup();
        let cmds = handle(
            &mouse(MouseEventKind::ScrollDown, 30, 4),
            &mut ui,
            &ws,
            Instant::now(),
        );
        assert!(cmds.is_empty());
        assert_eq!(ui.scroll.get(&ViewId(7)), Some(&(0, 0)));
    }

    fn text_hit(ui: &mut UiState, view: ViewId) {
        ui.hits.push(
            Rect::new(10, 5, 50, 10),
            Hit::Text {
                view,
                origin: Position::new(16, 5),
                top: 20,
                left: 0,
                wrap: None,
            },
        );
    }

    #[test]
    fn a_drag_extends_from_the_view_it_started_in_even_outside_it() {
        let (ws, _) = Workspace::new("/p".into());
        let mut ui = UiState::default();
        let view = crate::core::ViewId(1);
        text_hit(&mut ui, view);
        ui.dragging = Some(view);
        assert_eq!(
            handle(
                &mouse(MouseEventKind::Drag(MouseButton::Left), 20, 7),
                &mut ui,
                &ws,
                Instant::now()
            ),
            [Command::SelectTo { line: 22, col: 4 }]
        );
        assert_eq!(
            handle(
                &mouse(MouseEventKind::Drag(MouseButton::Left), 2, 2),
                &mut ui,
                &ws,
                Instant::now()
            ),
            [Command::SelectTo { line: 20, col: 0 }],
            "above and left of the text"
        );
        handle(
            &mouse(MouseEventKind::Up(MouseButton::Left), 20, 7),
            &mut ui,
            &ws,
            Instant::now(),
        );
        assert!(
            handle(
                &mouse(MouseEventKind::Drag(MouseButton::Left), 20, 7),
                &mut ui,
                &ws,
                Instant::now()
            )
            .is_empty()
        );
    }

    #[test]
    fn the_git_icon_opens_the_log() {
        let (ws, _) = Workspace::new("/p".into());
        let mut ui = UiState::default();
        ui.hits.push(Rect::new(0, 3, 3, 1), Hit::StripeGit);
        assert_eq!(
            handle(
                &mouse(MouseEventKind::Down(MouseButton::Left), 1, 3),
                &mut ui,
                &ws,
                Instant::now()
            ),
            [Command::Run(Action::GitLog)]
        );
    }

    #[test]
    fn dragging_an_edge_sizes_the_panel_from_where_the_pointer_is() {
        let (ws, _) = Workspace::new("/p".into());
        let mut ui = UiState::default();
        ui.hits.push(
            Rect::new(0, 20, 80, 1),
            Hit::Edge(Resizing::Tool { below: 30 }),
        );
        ui.hits.push(
            Rect::new(32, 2, 1, 18),
            Hit::Edge(Resizing::Project { left: 4 }),
        );
        let mut send = |kind, x, y| handle(&mouse(kind, x, y), &mut ui, &ws, Instant::now());
        let down = MouseEventKind::Down(MouseButton::Left);
        let drag = MouseEventKind::Drag(MouseButton::Left);
        let up = MouseEventKind::Up(MouseButton::Left);
        assert_eq!(
            send(down, 40, 20),
            [],
            "pressing it changes and focuses nothing"
        );
        assert_eq!(send(drag, 70, 15), [Command::ResizeTool { rows: 14 }]);
        assert_eq!(
            send(drag, 70, 40),
            [Command::ResizeTool { rows: 0 }],
            "the core limits it"
        );
        assert_eq!(send(up, 70, 15), []);
        assert_eq!(send(drag, 70, 15), [], "let go");
        assert_eq!(send(down, 32, 5), []);
        assert_eq!(send(drag, 44, 9), [Command::ResizeProject { columns: 40 }]);
        assert_eq!(send(up, 44, 9), []);
    }

    #[test]
    fn the_pointer_says_what_is_under_it_and_a_drag_keeps_its_shape() {
        let mut ui = UiState::default();
        let view = crate::core::ViewId(1);
        text_hit(&mut ui, view);
        ui.hits.push(
            Rect::new(0, 20, 80, 1),
            Hit::Edge(Resizing::Tool { below: 30 }),
        );
        ui.hits.push(Rect::new(0, 0, 3, 1), Hit::StripeGit);
        assert_eq!(pointer_at(&ui, 20, 7), Pointer::Text);
        assert_eq!(pointer_at(&ui, 5, 20), Pointer::ResizeRows);
        assert_eq!(pointer_at(&ui, 1, 0), Pointer::Hand);
        assert_eq!(pointer_at(&ui, 100, 100), Pointer::Arrow);
        ui.resizing = Some(Resizing::Project { left: 4 });
        assert_eq!(pointer_at(&ui, 20, 7), Pointer::ResizeColumns);
        assert_eq!(Pointer::Text.sequence(), "\x1b]22;text\x1b\\");
    }
}
