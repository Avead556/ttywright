use super::image_art::{ImageRequest, ImageState, PictureView};
use crate::core::git_command::{GitEffect, GitEvent};
use crate::core::lsp_command::{LspEffect, LspEvent};
use std::collections::{BTreeMap, BTreeSet, HashSet};
use std::path::{Path, PathBuf};

use ropey::Rope;
use serde::{Deserialize, Serialize};

use super::action::Action;
use super::change::Assoc;
use super::command::{Command, Effect, Event, Motion, RecoveredSwap, WriteGuard, WriteOrigin};
use super::conflict::ConflictChoice;
use super::document::Language;
use super::document::{Applied, Conflict, DiskState, Document, DocumentError};
use super::find::{
    ClosedPlan, FilePlan, FileProblem, FindBar, FindOptions, PlanSource, ProjectSearch, Query,
    ReplacePreview, ReplaceReport, ReplaceStage, SearchMatch, merge_plan,
};
use super::git::{
    CommitMode, GitBranch, GitChange, GitDiffSource, GitField, GitFile, GitNotice, GitOperation,
    GitPanel, GitRemoteOp, GitRepoState, GitTab,
};
use super::input::LineInput;
use super::layout::{Axis, Layout, PaneId};
use super::local_history::{HistoryEntry, LocalHistoryPanel, SnapshotLabel, diff_rows};
use super::lsp::{
    Completion, Diagnostic, Hover, HoverLine, LspAnswer, LspCodeAction, LspCompletion, LspFileEdit,
    LspLocation, LspPanel, LspQuery, LspServerOption, LspState, LspStatus, LspTextEdit, LspUsage,
    PositionEncoding, Problem, RawDiagnostic, ServerKey, ServerStatus, Severity, char_at,
    hover_lines, merge_edits, position_of,
};
use super::picker::{Picker, PickerKind};
use super::run::{self, ConfiguredRun, RunFacts, RunTarget};
use super::run_output::{RunOutcome, RunPanel, RunStreamMessage};
use super::session::{SavedLayout, SavedView, Session, SessionFile};
mod analysis_ops;
mod claude_ops;
mod completion_ops;
mod db_ops;
mod debug_ops;
mod deps_ops;
mod docker_ops;
mod editing_ops;
mod file_ops;
mod github_ops;
mod macro_ops;
mod notify_ops;
mod panel_ops;
mod refactor_ops;
mod spell_ops;
mod test_ops;
use refactor_ops::{PendingSafeDelete, RefactorRow};
mod coverage_ops;
mod duplicates_ops;
mod find_ops;
pub use crate::core::coverage::CoverageState;
mod git_blame_ops;
mod git_branch_ops;
mod git_commit_ops;
mod git_compare_ops;
mod git_conflict_ops;
mod git_history_ops;
mod git_panel_ops;
mod git_stage_ops;
mod git_stash_ops;
mod history_ops;
mod http_ops;
mod list_ops;
mod lsp_ops;
mod lsp_view_ops;
mod mcp_ops;
mod merge_ops;
mod navigate_ops;
mod persist_ops;
mod plugin_ops;
mod run_ops;
mod terminal_ops;
mod trust_ops;
mod view_ops;
use super::text;
use super::tree::{ProjectTree, TreeRequest};
use super::wrap::{Metrics, row_at};
use git_blame_ops::BlameState;
pub use git_blame_ops::BlameView;

const DEFAULT_TAB_WIDTH: usize = 4;
const LSP_MAX_BYTES: usize = 5 * 1024 * 1024;
const MAX_JUMPS_BACK: usize = 100;
const HOVER_PAGE: usize = 10;
const COMPLETION_PAGE: usize = 8;
const MAX_RECENT_FILES: usize = 50;
const TOOL_RESIZE_STEP: u16 = 2;
const USAGES_ARE_NOT_TEXT: &str = "These are usages: run a text search to replace";

fn screen_size(cells: u16) -> u16 {
    cells.clamp(1, 2000)
}

fn markdown_image_source(line: &str) -> Option<String> {
    let after = &line[line.find("![")?..];
    let open = after.find("](")? + 2;
    let rest = after[open..].trim_start();
    let end = rest.find([')', ' ', '\t'])?;
    let source = rest[..end].trim_matches(['<', '>']);
    (!source.is_empty()).then(|| source.to_owned())
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

fn word_start(text: &ropey::Rope, cursor: usize) -> usize {
    let before = text.chars_at(cursor).reversed();
    cursor - before.take_while(|c| is_word_char(*c)).count()
}

fn lsp_wanted(d: &Document) -> bool {
    d.language().server().is_some() && d.text().len_bytes() <= LSP_MAX_BYTES
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct DocumentId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ViewId(pub u32);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Level {
    Info,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StatusMessage {
    pub level: Level,
    pub text: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Focus {
    Editor,
    Project,
    FindBar,
    FindInFiles,
    Problems,
    Git,
    Run,
    Debug,
    Claude,
    Terminal,
    Language,
    Http,
    Database,
    Docker,
    GitHub,
    Duplicates,
    Todo,
    Tests,
    Dependencies,
    Bookmarks,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum ToolTab {
    #[default]
    Find,
    Problems,
    Git,
    Run,
    Debug,
    Claude,
    Terminal,
    Language,
    Http,
    Database,
    Docker,
    GitHub,
    Duplicates,
    Todo,
    Tests,
    Dependencies,
    Bookmarks,
}

pub use crate::core::run_state::RunConfirm;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum QuitState {
    Running,
    SavingBeforeQuit,
    SaveFailed,
    Done,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct Cursor {
    pub idx: usize,
    preferred_col: Option<usize>,
    anchor: Option<usize>,
}

impl Cursor {
    fn at(idx: usize) -> Self {
        Self {
            idx,
            preferred_col: None,
            anchor: None,
        }
    }

    pub fn selection(&self) -> Option<std::ops::Range<usize>> {
        let anchor = self.anchor?;
        (anchor != self.idx).then(|| anchor.min(self.idx)..anchor.max(self.idx))
    }
}

#[derive(Debug, Clone)]
pub struct View {
    pub doc: DocumentId,
    pub cursor: Cursor,
}

#[derive(Debug, Clone, Default)]
pub struct Pane {
    pub views: Vec<ViewId>,
    pub active: usize,
}

impl Pane {
    pub fn active_view(&self) -> Option<ViewId> {
        self.views.get(self.active).copied()
    }
}

type IdeSeen = (PathBuf, usize, Option<std::ops::Range<usize>>, u64);

pub struct Workspace {
    root: PathBuf,
    documents: BTreeMap<DocumentId, Document>,
    views: BTreeMap<ViewId, View>,
    panes: BTreeMap<PaneId, Pane>,
    layout: Layout,
    active_pane: PaneId,
    next_id: u32,
    focus: Focus,
    tree: ProjectTree,
    project_visible: bool,
    files: Vec<String>,
    recent: Vec<PathBuf>,
    picker: Option<Picker>,
    find_bar: Option<FindBar>,
    bar_options: FindOptions,
    search: ProjectSearch,
    tool_window_visible: bool,
    tool_height: Option<u16>,
    project_width: Option<u16>,
    tool_room: (u16, u16, u16),
    status: Option<StatusMessage>,
    quit: QuitState,
    saved_session: Option<Session>,
    local_history: Option<LocalHistoryPanel>,
    tool_tab: ToolTab,
    lsp: crate::core::lsp_client::LspClient,
    answer_encoding: PositionEncoding,
    diagnostics: BTreeMap<PathBuf, Vec<Diagnostic>>,
    problem_selected: Option<usize>,
    locations: Vec<LspLocation>,
    pending_jump: Option<LspLocation>,
    jumps_back: Vec<(PathBuf, usize, usize)>,
    hover: Option<Hover>,
    completion: Option<Completion>,
    completion_dismissed: Option<(ViewId, usize)>,
    server_options: Vec<LspServerOption>,
    code_actions: Vec<LspCodeAction>,
    project_symbols: Vec<crate::core::ProjectSymbol>,
    symbols_asked: Option<u64>,
    index_progress: Option<(usize, usize)>,
    pending_rename: Vec<LspFileEdit>,
    refactor_rows: Vec<RefactorRow>,
    coverage: crate::core::coverage::CoverageView,
    changelists: crate::core::git::Changelists,
    changelist_target: Option<PathBuf>,
    commit_checks: crate::core::git::CommitChecks,
    pending_safe_delete: Option<PendingSafeDelete>,
    rename_undo: Option<RenameUndo>,
    pub(super) code_actions_stamp: u64,
    pending_edits: Vec<LspFileEdit>,
    return_to: Option<ViewId>,
    tab_width: usize,
    soft_wrap: bool,
    text_cols: usize,
    autosave_on_switch: bool,
    settings_file: Option<PathBuf>,
    clipboard: String,
    paste_at: Option<PasteTarget>,
    git_panel: Option<GitPanel>,
    repo: crate::core::git_repo::GitRepo,
    git_branch_picker_asked: bool,
    git_conflict_jump: Option<(PathBuf, Focus)>,
    merge: Option<crate::core::merge::MergeView>,
    terminal: crate::core::terminal::TerminalState,
    bookmarks: Vec<navigate_ops::Bookmark>,
    bookmark_selected: usize,
    todo: list_ops::TodoPanel,
    tests: crate::core::test_results::TestPanel,
    deps: deps_ops::DepsPanel,
    duplicates: crate::core::DuplicatesPanel,
    debug: crate::core::debug::DebugState,
    symbols: Vec<crate::core::lsp::LspSymbol>,
    symbols_path: Option<PathBuf>,
    calls_of: Option<(String, bool)>,
    pending_accept: Option<PendingAccept>,
    claude: Option<crate::core::claude::ClaudePanel>,
    ide_clients: usize,
    ide_sent: Option<IdeSeen>,
    menu: Option<crate::core::menu::MenuState>,
    mcp_proposals: Vec<crate::core::mcp::McpProposal>,
    mcp_review: Option<mcp_ops::McpReview>,
    file_prompt: Option<crate::core::file_op::FilePrompt>,
    macros: macro_ops::Macros,
    notifications: crate::core::notify::Notifications,
    notify_outbox: Vec<Effect>,
    trusted: bool,
    untrusted_swaps: Vec<RecoveredSwap>,
    http: crate::core::http::HttpState,
    db: crate::core::db::DbState,
    docker: crate::core::docker::DockerPanel,
    github: crate::core::github::GitHubPanel,
    plugins: Vec<crate::core::plugin::PluginInfo>,
    plugin_running: Option<plugin_ops::PluginRun>,
    file_op_reopen: Vec<PathBuf>,
    merge_pending: Option<PathBuf>,
    markdown_previews: BTreeSet<DocumentId>,
    picture_views: BTreeMap<DocumentId, PictureView>,
    markdown_on_open: bool,
    markdown_choices: BTreeMap<PathBuf, bool>,
    images: BTreeMap<ImageRequest, ImageState>,
    git_blames: BTreeMap<DocumentId, BlameState>,
    window: bool,
    run_window: bool,
    run_connected: bool,
    run_watched: bool,
    run: crate::core::run_state::RunState,
    scratches: Vec<PathBuf>,
    scratch_dir: Option<PathBuf>,
    analysis: crate::core::analysis::AnalysisState,
}

struct RenameUndo {
    documents: Vec<(DocumentId, usize)>,
    applying: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct PasteTarget {
    view: Option<ViewId>,
    cursor: Option<Cursor>,
    focus: Focus,
    stamp: u64,
}

#[derive(Debug, Clone)]
struct PendingAccept {
    server: ServerKey,
    id: u64,
    view: ViewId,
    version: u64,
    from: usize,
    to: usize,
    insert: String,
}

use crate::core::lsp_client::PendingQuery;

impl Workspace {
    pub fn new(root: PathBuf) -> (Self, Vec<Effect>) {
        let pane = PaneId(0);
        let ws = Self {
            tree: ProjectTree::new(root.clone()),
            documents: BTreeMap::new(),
            views: BTreeMap::new(),
            panes: BTreeMap::from([(pane, Pane::default())]),
            layout: Layout::Pane(pane),
            active_pane: pane,
            next_id: 1,
            focus: Focus::Project,
            project_visible: true,
            files: Vec::new(),
            recent: Vec::new(),
            picker: None,
            find_bar: None,
            bar_options: FindOptions::default(),
            search: ProjectSearch::default(),
            tool_window_visible: false,
            tool_height: None,
            project_width: None,
            tool_room: (0, 0, 0),
            status: None,
            quit: QuitState::Running,
            saved_session: None,
            local_history: None,
            tool_tab: ToolTab::default(),
            diagnostics: BTreeMap::new(),
            problem_selected: None,
            lsp: crate::core::lsp_client::LspClient::default(),
            answer_encoding: PositionEncoding::Utf16,
            locations: Vec::new(),
            pending_jump: None,
            jumps_back: Vec::new(),
            hover: None,
            completion: None,
            completion_dismissed: None,
            server_options: Vec::new(),
            code_actions: Vec::new(),
            project_symbols: Vec::new(),
            symbols_asked: None,
            index_progress: None,
            pending_rename: Vec::new(),
            refactor_rows: Vec::new(),
            coverage: crate::core::coverage::CoverageView::default(),
            changelists: crate::core::git::Changelists::default(),
            changelist_target: None,
            commit_checks: crate::core::git::CommitChecks::default(),
            pending_safe_delete: None,
            rename_undo: None,
            code_actions_stamp: 0,
            pending_edits: Vec::new(),
            return_to: None,
            tab_width: DEFAULT_TAB_WIDTH,
            soft_wrap: false,
            text_cols: 0,
            autosave_on_switch: true,
            settings_file: None,
            clipboard: String::new(),
            paste_at: None,
            git_panel: None,
            repo: crate::core::git_repo::GitRepo::default(),
            git_branch_picker_asked: false,
            git_conflict_jump: None,
            merge: None,
            terminal: crate::core::terminal::TerminalState::default(),
            bookmarks: Vec::new(),
            bookmark_selected: 0,
            todo: list_ops::TodoPanel::default(),
            tests: crate::core::test_results::TestPanel::default(),
            deps: deps_ops::DepsPanel::default(),
            duplicates: crate::core::DuplicatesPanel::default(),
            debug: crate::core::debug::DebugState::default(),
            symbols: Vec::new(),
            symbols_path: None,
            calls_of: None,
            pending_accept: None,
            claude: None,
            ide_clients: 0,
            ide_sent: None,
            menu: None,
            mcp_proposals: Vec::new(),
            mcp_review: None,
            file_prompt: None,
            macros: macro_ops::Macros::default(),
            notifications: crate::core::notify::Notifications::default(),
            notify_outbox: Vec::new(),
            trusted: true,
            untrusted_swaps: Vec::new(),
            http: crate::core::http::HttpState::default(),
            db: crate::core::db::DbState::default(),
            docker: crate::core::docker::DockerPanel::default(),
            github: crate::core::github::GitHubPanel::default(),
            plugins: Vec::new(),
            plugin_running: None,
            file_op_reopen: Vec::new(),
            merge_pending: None,
            markdown_previews: BTreeSet::new(),
            picture_views: BTreeMap::new(),
            markdown_on_open: true,
            markdown_choices: BTreeMap::new(),
            images: BTreeMap::new(),
            git_blames: BTreeMap::new(),
            window: false,
            run_window: false,
            run: crate::core::run_state::RunState::default(),
            run_connected: false,
            run_watched: false,
            scratches: Vec::new(),
            scratch_dir: None,
            analysis: crate::core::analysis::AnalysisState::default(),
            root: root.clone(),
        };
        let git_root = root.clone();
        let effects = vec![
            Effect::ListDir { path: root.clone() },
            Effect::IndexFiles { root: root.clone() },
            Effect::IndexProject { root: root.clone() },
            Effect::WatchProject { root },
            Effect::LoadSession,
            Effect::RecoverSwaps,
            Effect::Git(GitEffect::Status { root: git_root }),
        ];
        (ws, effects)
    }

    pub fn configure_scratch_dir(&mut self, dir: Option<PathBuf>) {
        self.scratch_dir = dir;
    }

    pub fn configure(&mut self, autosave_on_switch: bool, settings_file: Option<PathBuf>) {
        self.autosave_on_switch = autosave_on_switch;
        self.settings_file = settings_file;
    }

    pub fn configure_editor(&mut self, tab_width: usize, soft_wrap: bool) {
        self.tab_width = tab_width;
        self.soft_wrap = soft_wrap;
    }

    pub fn configure_run(&mut self, entries: Vec<ConfiguredRun>) {
        self.run.configure(entries);
    }

    pub fn run_panel(&self) -> Option<&RunPanel> {
        self.run.panel()
    }

    pub fn run_tabs(&self) -> &[RunPanel] {
        self.run.tabs()
    }

    pub fn run_active(&self) -> usize {
        self.run.active()
    }

    pub fn run_confirm(&self) -> Option<&RunConfirm> {
        self.run.confirm()
    }

    pub fn soft_wrap(&self) -> bool {
        self.soft_wrap
    }

    fn metrics(&self) -> Metrics {
        Metrics {
            tab_width: self.tab_width,
            wrap: (self.soft_wrap && self.text_cols > 0).then_some(self.text_cols),
        }
    }

    pub fn tab_width(&self) -> usize {
        self.tab_width
    }

    pub fn new_git_window(root: PathBuf) -> (Self, Vec<Effect>) {
        let (mut ws, _) = Self::new(root);
        ws.window = true;
        ws.project_visible = false;
        let mut fx = Vec::new();
        ws.git_show(GitTab::Log, &mut fx);
        (ws, fx)
    }

    pub fn new_run_window(root: PathBuf) -> (Self, Vec<Effect>) {
        let (mut ws, _) = Self::new(root.clone());
        ws.window = true;
        ws.run_window = true;
        ws.project_visible = false;
        ws.tool_window_visible = true;
        ws.tool_tab = ToolTab::Run;
        ws.focus = Focus::Run;
        (ws, vec![Effect::RunFollow { root }])
    }

    pub fn is_window(&self) -> bool {
        self.window
    }

    pub fn is_run_window(&self) -> bool {
        self.run_window
    }

    pub fn run_connected(&self) -> bool {
        self.run_connected
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn document(&self, id: DocumentId) -> Option<&Document> {
        self.documents.get(&id)
    }

    pub fn view(&self, id: ViewId) -> Option<&View> {
        self.views.get(&id)
    }

    pub fn pane(&self, id: PaneId) -> Option<&Pane> {
        self.panes.get(&id)
    }

    pub fn layout(&self) -> &Layout {
        &self.layout
    }

    pub fn active_pane(&self) -> PaneId {
        self.active_pane
    }

    pub fn active_view(&self) -> Option<ViewId> {
        self.panes.get(&self.active_pane)?.active_view()
    }

    pub fn active(&self) -> Option<(&View, &Document)> {
        let view = self.views.get(&self.active_view()?)?;
        Some((view, self.documents.get(&view.doc)?))
    }

    pub fn focus(&self) -> Focus {
        self.focus
    }

    pub fn tree(&self) -> &ProjectTree {
        &self.tree
    }

    pub fn project_visible(&self) -> bool {
        self.project_visible
    }

    pub fn picker(&self) -> Option<&Picker> {
        self.picker.as_ref()
    }

    pub fn picker_items(&self, kind: PickerKind) -> Vec<String> {
        match kind {
            PickerKind::Actions => Action::ALL
                .iter()
                .map(|a| a.spec().title.to_owned())
                .chain(self.plugin_titles())
                .collect(),
            PickerKind::Files => self.files.clone(),
            PickerKind::RecentFiles => self
                .recent_others()
                .iter()
                .map(|p| self.display_path(p).to_string_lossy().into_owned())
                .collect(),
            PickerKind::Everywhere => self.everywhere_items(),
            PickerKind::Branches => self
                .repo
                .branches()
                .iter()
                .map(|b| {
                    let current = self.repo.is_current(b);
                    format!("{} {}", if current { "●" } else { " " }, b.name)
                })
                .collect(),
            PickerKind::CodeActions => self.code_actions.iter().map(|a| a.title.clone()).collect(),
            PickerKind::CompareBranches => self
                .repo
                .branches()
                .iter()
                .map(|b| b.name.clone())
                .collect(),
            PickerKind::Changelists => self.changelist_rows(),
            PickerKind::Refactorings => self.refactor_rows.iter().map(RefactorRow::label).collect(),
            PickerKind::Run => self.run.targets().iter().map(RunTarget::label).collect(),
            PickerKind::ProjectSymbols => self.project_symbol_rows(),
            PickerKind::Symbols => self
                .symbols
                .iter()
                .map(|s| format!("{}{}  {}", "  ".repeat(s.depth), s.name, s.kind))
                .collect(),
            PickerKind::Breakpoints => self
                .debug
                .breakpoints
                .iter()
                .map(|b| self.breakpoint_label(b))
                .collect(),
            PickerKind::DebugTargets => self.debug_choice_labels(),
            PickerKind::DebugExecutables => self.debug_executable_labels(),
            PickerKind::DebugThreads => self.debug_thread_labels(),
            PickerKind::ScratchKinds => crate::core::scratch::SCRATCH_KINDS
                .iter()
                .map(|(label, _)| (*label).to_owned())
                .collect(),
            PickerKind::Scratches => self
                .scratches
                .iter()
                .map(|p| {
                    p.file_name()
                        .map_or_else(String::new, |n| n.to_string_lossy().into_owned())
                })
                .collect(),
            PickerKind::FileTemplates => crate::core::templates::FILE_TEMPLATES
                .iter()
                .map(|t| t.label.to_owned())
                .collect(),
            PickerKind::ProjectTemplates => crate::core::templates::PROJECT_TEMPLATES
                .iter()
                .map(|t| t.label.to_owned())
                .collect(),
            PickerKind::Servers => self
                .server_options
                .iter()
                .map(|o| {
                    let mark = if o.current { "● " } else { "  " };
                    format!("{mark}{}", o.label)
                })
                .collect(),
            PickerKind::Notifications => self.notification_rows(),
            PickerKind::Locations => self
                .locations
                .iter()
                .map(|l| {
                    let name = self.display_path(&l.path).display();
                    format!("{name}:{}", l.start.0 + 1)
                })
                .collect(),
        }
    }

    fn everywhere_items(&self) -> Vec<String> {
        let recent: Vec<String> = self
            .recent_others()
            .iter()
            .map(|p| self.display_path(p).to_string_lossy().into_owned())
            .collect();
        let known: HashSet<&str> = recent.iter().map(String::as_str).collect();
        let mut items = recent.clone();
        items.extend(
            self.files
                .iter()
                .filter(|f| !known.contains(f.as_str()))
                .cloned(),
        );
        items.extend(self.project_symbol_rows());
        items.extend(Action::ALL.iter().map(|a| a.spec().title.to_owned()));
        items
    }

    pub fn everywhere_action(index: usize, len: usize) -> Option<Action> {
        let first = len.checked_sub(Action::ALL.len())?;
        Action::ALL.get(index.checked_sub(first)?).copied()
    }

    fn recent_others(&self) -> Vec<PathBuf> {
        let active = self.active().map(|(_, d)| d.path());
        self.recent
            .iter()
            .filter(|p| Some(p.as_path()) != active)
            .cloned()
            .collect()
    }

    fn note_recent(&mut self) {
        let Some(path) = self.active().map(|(_, d)| d.path().to_path_buf()) else {
            return;
        };
        if self.recent.first() != Some(&path) {
            self.recent.retain(|p| *p != path);
            self.recent.insert(0, path);
            self.recent.truncate(MAX_RECENT_FILES);
        }
    }

    pub fn tool_tab(&self) -> ToolTab {
        self.tool_tab
    }

    pub fn lsp_state(&self) -> LspState {
        self.active_server().map_or(LspState::Off, |(_, s)| s.state)
    }

    pub fn lsp_busy(&self) -> bool {
        self.active_server().is_some_and(|(_, s)| s.busy)
    }

    pub fn lsp_status(&self) -> Option<LspStatus> {
        let (key, status) = self.active_server()?;
        let spec = key.server.server()?;
        let name = status
            .name
            .clone()
            .unwrap_or_else(|| spec.name().to_owned());
        let install = if status.state == LspState::NotFound && !spec.provisioned {
            spec.candidates
                .iter()
                .find(|c| c.command == name)
                .map(|c| c.install)
        } else {
            None
        };
        Some(LspStatus {
            state: status.state,
            busy: status.busy,
            name,
            install,
        })
    }

    pub fn lsp_panel(&self) -> Option<LspPanel> {
        let (key, session) = self.active_server()?;
        let language = self.active()?.1.language();
        Some(LspPanel {
            status: self.lsp_status()?,
            language: language.name(),
            root: key.root.clone(),
            open_files: session.open.len(),
            failure: session.failure.clone(),
            can_choose: language == Language::Rust,
        })
    }

    fn language_input(&mut self, input: &Command, fx: &mut Vec<Effect>) {
        let Command::Insert(letter) = input else {
            return;
        };
        match letter.to_lowercase().as_str() {
            "r" => self.run_action(Action::RestartLanguageServer, fx),
            "s" => self.run_action(Action::SelectLanguageServer, fx),
            _ => {}
        }
    }

    fn active_server(&self) -> Option<(&ServerKey, &ServerStatus)> {
        let (_, d) = self.active()?;
        self.lsp.servers.get_key_value(d.lsp_server()?)
    }

    pub fn diagnostics(&self, path: &Path) -> &[Diagnostic] {
        self.diagnostics.get(path).map_or(&[], Vec::as_slice)
    }

    pub fn problems(&self) -> Vec<Problem> {
        let mut problems = Vec::new();
        for (path, items) in &self.diagnostics {
            let open = self.documents.values().find(|d| d.path() == path);
            for d in items {
                let (line, col) = match (open, &d.chars) {
                    (Some(doc), Some(chars)) => {
                        let t = doc.text();
                        let at = chars.start.min(t.len_chars());
                        let line = t.char_to_line(at);
                        (line, at - t.line_to_char(line))
                    }
                    _ => (d.raw.start.0 as usize, d.raw.start.1 as usize),
                };
                problems.push(Problem {
                    path: path.clone(),
                    line,
                    col,
                    severity: d.raw.severity,
                    message: d.raw.message.clone(),
                    source: "lsp",
                });
            }
        }
        problems.extend(self.analysis_problems());
        problems
    }

    pub fn completion(&self) -> Option<&Completion> {
        self.completion.as_ref()
    }

    pub fn git_panel(&self) -> Option<&GitPanel> {
        self.git_panel.as_ref()
    }

    pub fn git_change(&self, path: &Path) -> Option<GitChange> {
        self.repo.change(path)
    }

    pub fn git_dir_changed(&self, path: &Path) -> bool {
        self.repo.dir_changed(path)
    }

    pub fn git_base(&self, path: &Path) -> (Option<&str>, u64) {
        self.repo.base(path)
    }

    pub fn markdown_preview(&self, doc: DocumentId) -> bool {
        self.markdown_previews.contains(&doc)
    }

    fn toggle_markdown_preview(&mut self) {
        let Some((view, d)) = self.active() else {
            return self.info("Open a Markdown file to preview it".to_owned());
        };
        if d.language() != Language::Markdown {
            return self.info("Markdown Preview is for Markdown files".to_owned());
        }
        let doc = view.doc;
        let path = d.path().to_path_buf();
        if !self.markdown_previews.remove(&doc) {
            self.markdown_previews.insert(doc);
        }
        let shown = self.markdown_previews.contains(&doc);
        self.markdown_choices.insert(path, shown);
    }

    pub fn configure_markdown(&mut self, preview_on_open: bool) {
        self.markdown_on_open = preview_on_open;
    }

    pub(super) fn preview_if_wanted(&mut self, doc: DocumentId) {
        let Some(d) = self.documents.get(&doc) else {
            return;
        };
        if d.language() != Language::Markdown {
            return;
        }
        let chosen = self.markdown_choices.get(d.path()).copied();
        if chosen.unwrap_or(self.markdown_on_open) {
            self.markdown_previews.insert(doc);
        }
    }

    pub(super) fn is_asking(&self) -> bool {
        self.file_prompt.is_some()
            || self.menu.is_some()
            || self.mcp_review.is_some()
            || self.picker.is_some()
    }

    fn on_a_picture(&self) -> bool {
        self.focus == Focus::Editor && self.active().is_some_and(|(_, d)| d.is_image())
    }

    fn open_image_outside(&mut self, fx: &mut Vec<Effect>) {
        let Some((view, d)) = self.active() else {
            return self.info("Open a picture first".to_owned());
        };
        let dir = d.path().parent().unwrap_or(&self.root).to_path_buf();
        let source = if d.is_image() {
            Some(d.path().display().to_string())
        } else {
            let line = d.text().char_to_line(view.cursor.idx);
            markdown_image_source(&d.text().line(line).to_string())
        };
        let Some(source) = source else {
            return self.info("Put the cursor on a line with a picture".to_owned());
        };
        if crate::core::image_art::is_remote(&source) {
            return self.info("This picture is not a local file".to_owned());
        }
        fx.push(Effect::OpenImage {
            root: self.root.clone(),
            request: ImageRequest {
                dir,
                source,
                cols: 1,
                rows: 1,
            },
        });
    }

    pub fn picture_view(&self, doc: DocumentId) -> PictureView {
        self.picture_views.get(&doc).copied().unwrap_or_default()
    }

    pub fn image_state(&self, request: &ImageRequest) -> Option<&ImageState> {
        self.images.get(request)
    }

    fn want_image(&mut self, request: ImageRequest) -> Vec<Effect> {
        if request.cols > 1000 || request.rows > 1000 || self.images.contains_key(&request) {
            return Vec::new();
        }
        if self.images.len() >= crate::core::image_art::MAX_KEPT {
            self.images.clear();
        }
        self.images.insert(request.clone(), ImageState::Decoding);
        vec![Effect::DecodeImage {
            root: self.root.clone(),
            request,
        }]
    }

    fn recheck_images(&self, fx: &mut Vec<Effect>) {
        fx.extend(self.images.keys().map(|request| Effect::DecodeImage {
            root: self.root.clone(),
            request: request.clone(),
        }));
    }

    pub fn git_operation(&self) -> Option<GitOperation> {
        self.repo.operation()
    }

    pub fn git_branches(&self) -> &[GitBranch] {
        self.repo.branches()
    }

    pub fn git_branch(&self) -> Option<&str> {
        self.repo.branch()
    }

    pub fn hover(&self) -> Option<&Hover> {
        self.hover.as_ref()
    }

    pub fn problem_selected(&self) -> Option<usize> {
        self.problem_selected
    }

    pub fn problem_counts(&self) -> (usize, usize) {
        let count = |severity| {
            self.diagnostics
                .values()
                .flatten()
                .filter(|d| d.raw.severity == severity)
                .count()
        };
        (
            count(Severity::Error) + self.analysis_count(Severity::Error),
            count(Severity::Warning) + self.analysis_count(Severity::Warning),
        )
    }

    pub fn local_history(&self) -> Option<&LocalHistoryPanel> {
        self.local_history.as_ref()
    }

    pub fn find_bar(&self) -> Option<&FindBar> {
        self.find_bar.as_ref()
    }

    pub fn bar_options(&self) -> FindOptions {
        self.bar_options
    }

    pub fn search(&self) -> &ProjectSearch {
        &self.search
    }

    pub fn tool_window_visible(&self) -> bool {
        self.tool_window_visible
    }

    pub fn project_width(&self) -> Option<u16> {
        self.project_width
    }

    pub fn tool_height(&self) -> Option<u16> {
        self.tool_height
    }

    pub fn status(&self) -> Option<&StatusMessage> {
        self.status.as_ref()
    }

    pub fn edit_stamp(&self) -> u64 {
        self.documents.values().map(Document::version).sum()
    }

    pub fn has_autosavable(&self) -> bool {
        self.documents
            .values()
            .any(|d| d.is_dirty() && d.conflict().is_none())
    }

    pub fn has_unswapped(&self) -> bool {
        self.documents.values().any(Document::needs_swap)
    }

    pub fn should_quit(&self) -> bool {
        self.quit == QuitState::Done
    }

    pub fn display_path<'a>(&self, path: &'a Path) -> &'a Path {
        path.strip_prefix(&self.root).unwrap_or(path)
    }

    pub fn dispatch(&mut self, command: Command) -> Vec<Effect> {
        let mut fx = self.dispatch_any(command);
        fx.append(&mut self.notify_outbox);
        self.without_untrusted(fx)
    }

    fn dispatch_any(&mut self, command: Command) -> Vec<Effect> {
        match command {
            Command::Autosave => {
                let mut fx = Vec::new();
                self.autosave(&mut fx);
                return fx;
            }
            Command::CheckDisk => {
                let mut fx = self.check_docs(|_| true);
                self.git_refresh_all(&mut fx);
                fx.push(Effect::RescanIndex);
                self.recheck_images(&mut fx);
                return fx;
            }
            Command::WriteSwaps => return self.write_swaps(),
            Command::SaveSession => return self.save_session(),
            Command::NotificationTick => {
                self.notifications.tick();
                return Vec::new();
            }
            Command::TerminalFocus { focused } => {
                self.notifications.terminal_in_front = focused;
                return Vec::new();
            }
            Command::NotificationClose { id } => {
                self.notifications.close(id);
                return Vec::new();
            }
            Command::WantImage(request) => return self.want_image(request),
            Command::OpenFile { path, at } => {
                let mut fx = Vec::new();
                self.open_file(path, at, &mut fx);
                return fx;
            }
            Command::Viewport { text_cols } => {
                self.text_cols = text_cols;
                return Vec::new();
            }
            Command::ResizeTool { rows } => {
                let (_, min, max) = self.tool_room;
                let rows = if max == 0 { rows } else { rows.clamp(min, max) };
                self.tool_height = Some(rows);
                self.tool_room.0 = rows;
                return Vec::new();
            }
            Command::ResizeProject { columns } => {
                self.project_width = Some(columns);
                return Vec::new();
            }
            Command::ToolRoom { rows, min, max } => {
                self.tool_room = (rows, min, max);
                return Vec::new();
            }
            _ => {}
        }
        self.macro_note(&command);
        if self.hover_input(&command) {
            return Vec::new();
        }
        self.completion_stop_waiting();
        let active_before = self.active_view();
        let from_the_user = !matches!(
            command,
            Command::Viewport { .. }
                | Command::ToolRoom { .. }
                | Command::TerminalSize { .. }
                | Command::ClaudeSize { .. }
                | Command::CheckDisk
        );
        if from_the_user {
            self.status = None;
        }
        if command != Command::Run(Action::Quit) && self.quit == QuitState::SaveFailed {
            self.quit = QuitState::Running;
        }
        let mut fx = Vec::new();
        if command == Command::Escape {
            self.dismiss_completion(&mut fx);
        }
        let asking = self.is_asking();
        let for_popup = !asking && self.completion_takes(&command);
        let typed = (!asking && matches!(command, Command::Insert(_) | Command::DeleteBackward))
            .then(|| command.clone());
        match command {
            Command::TerminalSize { rows, columns } => {
                self.terminal_sized(screen_size(rows), screen_size(columns), &mut fx);
            }
            Command::TerminalInput(bytes) => self.terminal_typed(bytes, &mut fx),
            Command::TerminalScroll { delta } => self.terminal_scrolled(delta),
            Command::SelectTerminalTab { index } => self.terminal_select(index),
            Command::CloseTerminalTab { index } => self.terminal_close(index, &mut fx),
            Command::CloseView { view } => self.close_view(view, &mut fx),
            Command::ClaudeSize { rows, columns } => {
                self.claude_sized(screen_size(rows), screen_size(columns), &mut fx);
            }
            Command::ClaudeInput(bytes) => self.claude_typed(bytes, &mut fx),
            Command::ClaudeScroll { delta } => {
                if let Some(panel) = self.claude.as_mut() {
                    panel.scroll_by(delta);
                }
            }
            input if for_popup => self.completion_input(input, &mut fx),
            input if self.menu.is_some() && input != Command::Run(Action::Quit) => {
                self.menu_input(input, &mut fx);
            }
            input if self.mcp_review.is_some() && input != Command::Run(Action::Quit) => {
                self.mcp_review_input(input, &mut fx);
            }
            input if self.file_prompt.is_some() && input != Command::Run(Action::Quit) => {
                self.file_prompt_input(input, &mut fx);
            }
            Command::OpenMenu { menu } => {
                self.menu = Some(crate::core::menu::MenuState::open(menu))
            }
            Command::PickMenuItem { .. } => {}
            input if self.github.confirm.is_some() && input != Command::Run(Action::Quit) => {
                self.github_confirm_input(input, &mut fx);
            }
            input if self.github.compose.is_some() && input != Command::Run(Action::Quit) => {
                self.github_compose_input(input);
            }
            Command::Escape if self.github_back() => {}
            input if self.docker.confirm.is_some() && input != Command::Run(Action::Quit) => {
                self.docker_confirm_input(&input, &mut fx);
            }
            input if self.db.asking() && input != Command::Run(Action::Quit) => {
                self.db_confirm_input(input, &mut fx);
            }
            input if self.analysis.confirm().is_some() && input != Command::Run(Action::Quit) => {
                self.analysis_confirm_input(input, &mut fx);
            }
            input if self.run.confirm().is_some() && input != Command::Run(Action::Quit) => {
                self.run_confirm_input(input, &mut fx);
            }
            input
                if self.merge.is_some()
                    && !matches!(input, Command::Run(Action::Quit | Action::Save)) =>
            {
                self.merge_input(input, &mut fx);
            }
            input if self.local_history.is_some() && input != Command::Run(Action::Quit) => {
                self.history_input(input, &mut fx);
            }
            Command::Run(action) => self.run_action(action, &mut fx),
            Command::Escape if self.focus == Focus::Git && self.picker.is_none() => {
                self.git_input(Command::Escape, &mut fx);
            }
            Command::Escape => self.escape(),
            Command::SelectRunLine { line } => self.run_click(line, &mut fx),
            Command::SelectRunTab { index } => {
                if self.run.select(index) {
                    self.set_focus(Focus::Run);
                }
            }
            Command::ScrollHttp { delta } => self.http.scroll(delta),
            Command::ScrollGitHub { delta } => self.github_scroll(delta),
            Command::PickGitHubRow { index } => self.github_pick(index),
            Command::ScrollDocker { delta } => self.docker.step(delta),
            Command::PickDockerRow { section, index } => self.docker_pick(section, index),
            Command::ScrollDatabase { delta } => self.db.scroll(delta),
            Command::PickDatabaseRow { table, index } => self.db_pick(table, index, &mut fx),
            Command::ScrollRun { delta } => {
                if let Some(panel) = self.run.panel_mut() {
                    panel.select_by(delta);
                }
            }
            Command::SelectGitRow { .. } => {
                self.picker = None;
                self.focus = Focus::Git;
                self.git_input(command, &mut fx);
            }
            Command::FocusView { view, at } => self.focus_view(view, at),
            Command::Focus(focus) => self.set_focus(focus),
            Command::SelectTreeRow { path, activate } => {
                self.picker = None;
                self.project_visible = true;
                self.focus = Focus::Project;
                if self.tree.select(&path) && activate {
                    self.tree_input(Command::Newline, &mut fx);
                }
            }
            Command::SelectSearchResult { index, open } => {
                self.picker = None;
                if index < self.search.results.len() {
                    self.focus = Focus::FindInFiles;
                    self.search.selected = Some(index);
                    if open {
                        self.search_input(Command::Newline, &mut fx);
                    }
                }
            }
            Command::SelectDuplicateRow { index, open } => {
                self.duplicate_row_clicked(index, open, &mut fx);
            }
            Command::SelectTestRow { index, open } => self.test_row_clicked(index, open, &mut fx),
            Command::NotificationClick { id } => self.notification_clicked(id, &mut fx),
            Command::SelectDependencyRow { index, open } => {
                self.dep_row_clicked(index, open, &mut fx);
            }
            Command::SelectTodoRow { index, open } => {
                self.todo_row_clicked(index, open, &mut fx);
            }
            Command::SelectBookmarkRow { index, open } => {
                self.bookmark_row_clicked(index, open, &mut fx);
            }
            Command::SelectProblem { index, open } => {
                self.picker = None;
                self.focus = Focus::Problems;
                self.problem_selected = Some(index);
                if open {
                    self.problems_input(Command::Newline, &mut fx);
                }
            }
            Command::ToggleBreakpointAt { view, line } => {
                self.picker = None;
                self.breakpoint_toggle_at(view, line, &mut fx);
            }
            Command::SelectDebugRow { part, index, open } => {
                self.debug_row_clicked(part, index, open, &mut fx);
            }
            Command::ScrollDebugConsole { delta } => self.debug_scroll_console(delta),
            Command::SelectPickerItem { index, accept } => {
                if let Some(p) = self.picker.as_mut()
                    && index < p.matches.len()
                {
                    p.selected = index;
                    if accept {
                        self.picker_input(Command::Newline, &mut fx);
                    }
                }
            }
            input if self.picker.is_some() => self.picker_input(input, &mut fx),
            Command::Insert(s) if s == "o" && self.on_a_picture() => {
                self.open_image_outside(&mut fx)
            }
            input => match self.focus {
                Focus::Editor => self.editor_input(input),
                Focus::Project => self.tree_input(input, &mut fx),
                Focus::FindBar => self.find_bar_input(input),
                Focus::FindInFiles => self.search_input(input, &mut fx),
                Focus::Problems => self.problems_input(input, &mut fx),
                Focus::Git => self.git_input(input, &mut fx),
                Focus::Run => self.run_input(input, &mut fx),
                Focus::Debug => self.debug_input(input, &mut fx),
                Focus::Language => self.language_input(&input, &mut fx),
                Focus::Http => self.http_input(&input, &mut fx),
                Focus::Duplicates => self.duplicates_input(&input, &mut fx),
                Focus::Todo => self.todo_input(&input, &mut fx),
                Focus::Tests => self.tests_input(&input, &mut fx),
                Focus::Dependencies => self.deps_input(&input, &mut fx),
                Focus::Bookmarks => self.bookmarks_input(&input, &mut fx),
                Focus::Database => self.db_input(&input, &mut fx),
                Focus::Docker => self.docker_input(&input, &mut fx),
                Focus::GitHub => self.github_input(&input, &mut fx),
                Focus::Claude | Focus::Terminal => {}
            },
        }
        self.git_conflict_catch_up();
        self.merge_catch_up();
        self.ide_sync(&mut fx);
        self.bookmarks_sync();
        self.breakpoints_sync();
        self.note_recent();
        self.autosave_on_switch(active_before, &mut fx);
        self.sweep_swaps(&mut fx);
        self.sync_lsp(&mut fx);
        self.ask_for_grammars(&mut fx);
        if let Some(typed) = typed.filter(|_| !for_popup) {
            self.completion_after_typing(&typed, &mut fx);
        }
        self.git_blame_catch_up(&mut fx);
        fx
    }

    pub fn handle(&mut self, event: Event) -> Vec<Effect> {
        let mut fx = self.handle_any(event);
        fx.append(&mut self.notify_outbox);
        self.without_untrusted(fx)
    }

    fn handle_any(&mut self, event: Event) -> Vec<Effect> {
        let mut fx = Vec::new();
        match event {
            Event::FileRead {
                path,
                at,
                disk,
                result,
            } => {
                let active_before = self.active_view();
                let jump = self.pending_jump.take_if(|j| j.path == path);
                let waiting = self.take_pending_edits(&path);
                let failed = result.is_err();
                self.file_read(path.clone(), at, disk, result);
                self.finish_pending_edits(&path, waiting, failed);
                self.mcp_file_read(&path, failed, &mut fx);
                fx.push(Effect::Git(GitEffect::Base {
                    root: self.root.clone(),
                    path: path.clone(),
                }));
                if let Some(location) = jump {
                    self.place_cursor(&location);
                }
                self.autosave_on_switch(active_before, &mut fx);
            }
            Event::FileWritten {
                doc,
                version,
                disk,
                origin,
            } => {
                if let Some(d) = self.documents.get_mut(&doc) {
                    d.mark_saved(version);
                    d.set_disk(disk);
                    d.set_conflict(None);
                    d.clear_write_failure();
                    let name = d.file_name();
                    let path = d.path().to_path_buf();
                    if origin == WriteOrigin::User {
                        self.info(format!("Saved {name}"));
                    }
                    self.debug_file_saved(&path, &mut fx);
                    if let Some(d) = self.documents.get(&doc)
                        && let Some(server) = d.lsp_server()
                        && self
                            .lsp
                            .servers
                            .get(server)
                            .is_some_and(|s| s.open.contains(d.path()))
                    {
                        fx.push(Effect::Lsp(LspEffect::DidSave {
                            server: server.clone(),
                            path: d.path().to_path_buf(),
                        }));
                    }
                }
                fx.push(Effect::Git(GitEffect::Status {
                    root: self.root.clone(),
                }));
                self.sweep_swaps(&mut fx);
                self.drop_if_orphan(doc);
                if self.quit == QuitState::SavingBeforeQuit && !self.any_dirty() {
                    self.quit = QuitState::Done;
                }
            }
            Event::WriteFailed {
                doc,
                message,
                origin,
                ..
            } => {
                let first_failure = self
                    .documents
                    .get_mut(&doc)
                    .is_some_and(Document::note_write_failure);
                let text = format!("Could not save {}: {message}", self.doc_name(doc));
                self.write_refused(doc, text, origin == WriteOrigin::User || first_failure);
            }
            Event::WriteConflict { doc, disk, origin } => {
                if let Some(d) = self.documents.get_mut(&doc) {
                    d.set_conflict(Some(conflict_for(disk)));
                }
                let text = format!(
                    "{} changed on disk: use Reload from Disk or Keep My Version",
                    self.doc_name(doc)
                );
                self.write_refused(doc, text, origin == WriteOrigin::User);
            }
            Event::FileChanged { doc, disk, result } => self.file_changed(doc, disk, result),
            Event::FileReloaded { doc, disk, result } => match result {
                Ok(Some(bytes)) => self.reload_document(doc, &bytes, disk),
                Ok(None) => self.error(format!("{} no longer exists", self.doc_name(doc))),
                Err(e) => self.error(format!("Could not reload {}: {e}", self.doc_name(doc))),
            },
            Event::PathsChanged { paths } => {
                if paths
                    .iter()
                    .any(|p| crate::core::image_art::has_image_extension(p))
                {
                    self.recheck_images(&mut fx);
                }
                self.paths_changed(&paths, &mut fx);
            }
            Event::ImageDecoded { request, result } => {
                if let Some(state) = self.images.get_mut(&request) {
                    *state = match result {
                        Ok(image) => ImageState::Ready(std::sync::Arc::new(image)),
                        Err(reason) => ImageState::Failed(reason),
                    };
                }
            }
            Event::GrammarLoaded { language, error } => self.grammar_loaded(language, error),
            Event::PluginsLoaded { plugins, problems } => self.plugins_loaded(plugins, problems),
            Event::PluginInvoked { .. } => {}
            Event::PluginDone { plugin, result } => fx.extend(self.plugin_done(&plugin, result)),
            Event::Duplicates(event) => self.duplicates_finished(event),
            Event::Coverage(event) => self.coverage_read(event),
            Event::Git(GitEvent::ComparedRead { base, path, result }) => {
                self.git_compared(&base, path.as_deref(), result, &mut fx);
            }
            Event::Git(GitEvent::CommitChecked { warnings }) => {
                self.git_checks_done(warnings, &mut fx)
            }
            Event::FileOpDone { op, result } => self.file_op_done(&op, result, &mut fx),
            Event::Formatted {
                doc,
                version,
                result,
            } => self.formatted(doc, version, result),
            Event::SpellChecked { result } => self.spell_checked(result),
            Event::WordAdded { word, result } => match result {
                Ok(()) => self.info(format!("Added {word}: Check Spelling again to see it go")),
                Err(reason) => self.error(format!("Could not add the word: {reason}")),
            },
            Event::DependenciesRead {
                dependencies,
                problems,
            } => self.deps_read(dependencies, problems),
            Event::ImageOpened { result } => {
                if let Err(message) = result {
                    self.error(format!("Could not open the image: {message}"));
                }
            }
            Event::ProjectOpened { result } => {
                if let Err(message) = result {
                    self.error(message);
                }
            }
            Event::McpCalled { id, tool, args } => self.mcp_called(id, &tool, &args, &mut fx),
            Event::McpFilesRead { id, call, disk } => {
                self.mcp_files_read(id, &call, &disk, &mut fx);
            }
            Event::Terminal(event) => self.terminal_handle(event, &mut fx),
            Event::ClaudeOutput { bytes } => self.claude_output(&bytes, &mut fx),
            Event::ClaudeEnded { state } => self.claude_ended(state, &mut fx),
            Event::IdeConnected => self.ide_connected(&mut fx),
            Event::IdeDisconnected => self.ide_disconnected(&mut fx),
            Event::ClaudeOpened { result } => {
                if let Err(message) = result {
                    self.error(message);
                }
            }
            Event::WatchFailed { message } => {
                self.error(format!("Not watching for file changes: {message}"));
            }
            Event::SwapsFound { swaps } => self.recover_swaps(swaps),
            Event::SwapFailed { message } => {
                self.error(format!("Could not write swap file: {message}"));
            }
            Event::SessionLoaded { session, files } => self.restore_session(session, files),
            Event::SessionSaveFailed { message } => {
                self.error(format!("Could not save the session: {message}"));
            }
            Event::HistoryListed { doc, entries, now } => {
                self.history_listed(doc, entries, now, &mut fx);
            }
            Event::HistoryVersion { hash, result } => self.history_version(&hash, result),
            Event::HistoryFailed { message } => {
                self.error(format!("Local History: {message}"));
            }
            Event::Lsp(LspEvent::RootFound { path, server }) => self.lsp_root_found(&path, server),
            Event::Lsp(LspEvent::Ready {
                server,
                encoding,
                completion_triggers,
            }) => self.lsp_ready(server, encoding, completion_triggers),
            Event::Lsp(LspEvent::StateChanged { server, state }) => {
                self.lsp_state_changed(&server, state)
            }
            Event::Lsp(LspEvent::Failed { server, reason }) => {
                self.lsp.failed(&server, &reason);
                self.notify_lsp_failed(&reason);
            }
            Event::Lsp(LspEvent::ServerNamed { server, name }) => {
                self.lsp.named(&server, name);
            }
            Event::Lsp(LspEvent::Progress { server, busy }) => {
                self.lsp.progress(&server, busy);
            }
            Event::Lsp(LspEvent::Diagnostics {
                server,
                path,
                items,
            }) => self.set_diagnostics(&server, path, items),
            Event::Git(GitEvent::StatusRead {
                branch,
                top,
                files,
                state,
            }) => {
                self.git_status_read(branch, top, files, state, &mut fx);
            }
            Event::Git(GitEvent::LogRead { result, now }) => {
                self.git_log_read(result, now, &mut fx)
            }
            Event::Git(GitEvent::CommitFilesRead { hash, result }) => {
                self.git_commit_files_read(&hash, result, &mut fx);
            }
            Event::Git(GitEvent::DiffRead { id, result }) => {
                if let Some(panel) = self.git_panel.as_mut()
                    && panel.diff_request == Some(id)
                {
                    panel.diff_request = None;
                    match result {
                        Ok(rows) => {
                            let last = rows.len().saturating_sub(1);
                            panel.diff_scroll = panel.diff_scroll.min(last);
                            panel.diff = rows;
                        }
                        Err(text) => panel.notice = Some(GitNotice { text, error: true }),
                    }
                }
            }
            Event::Git(GitEvent::HunksRead { id, result }) => {
                if let Some(panel) = self.git_panel.as_mut()
                    && panel.diff_request == Some(id)
                {
                    panel.diff_request = None;
                    match result {
                        Ok(hunks) => panel.set_hunks(hunks),
                        Err(text) => panel.notice = Some(GitNotice { text, error: true }),
                    }
                }
            }
            Event::Git(GitEvent::StageDone { result }) => self.git_stage_done(result, &mut fx),
            Event::Git(GitEvent::OpDone { op, result }) => self.git_op_done(&op, result, &mut fx),
            Event::Git(GitEvent::StashesRead { result, now }) => {
                self.git_stashes_read(result, now, &mut fx)
            }
            Event::Git(GitEvent::Committed { result }) => self.git_committed(result, &mut fx),
            Event::Git(GitEvent::LastCommit { result }) => self.git_last_commit(result),
            Event::ExternalOpen { path, at } => {
                self.picker = None;
                self.open_file(path, at, &mut fx);
            }
            Event::OpenedInMainWindow { result } => match result {
                Ok(()) => {
                    if self.run_window {
                        self.info("Opened in the main window".to_owned());
                    }
                    self.git_notice("Opened in the main window", false);
                }
                Err(text) => {
                    if self.run_window {
                        self.error(text.clone());
                    }
                    self.git_notice(text, true);
                }
            },
            Event::GitWindowOpened { result } => match result {
                Ok(()) => self.info("Opened the git log in a new window".to_owned()),
                Err(text) => {
                    self.error(text.clone());
                    self.git_notice(text, true);
                }
            },
            Event::Git(GitEvent::RolledBack { path, result }) => {
                self.git_rolled_back(&path, result, &mut fx)
            }
            Event::Git(GitEvent::BranchesRead { result }) => self.git_branches_read(result),
            Event::Git(GitEvent::Switched { name, result }) => {
                self.git_switched(&name, result, &mut fx)
            }
            Event::Git(GitEvent::BaseRead { path, text }) => {
                self.repo.set_base(path, text);
            }
            Event::Git(GitEvent::RemoteDone { op, result }) => {
                self.git_remote_done(op, result, &mut fx)
            }
            Event::Git(GitEvent::FileHistoryRead { path, result, now }) => {
                self.git_history_read(&path, result, now, &mut fx);
            }
            Event::Git(GitEvent::BlameRead {
                doc,
                version,
                result,
                now,
            }) => self.git_blame_read(doc, version, result, now),
            Event::Run(event) => self.run_handle(event, &mut fx),
            Event::RunWatcherAttached => {
                self.run_watched = true;
                let snapshot = self.run.panel().map(RunPanel::snapshot);
                fx.push(Effect::RunStreamStart(RunStreamMessage::Snapshot(snapshot)));
            }
            Event::RunStream(message) => self.run_stream(message),
            Event::RunStreamState { connected } => self.run_connected = connected,
            Event::RunWindowOpened { result } => match result {
                Ok(()) => self.info("Opened the run output in a new window".to_owned()),
                Err(text) => self.error(text),
            },
            Event::Debug(event) => self.debug_handle(event, &mut fx),
            Event::ClipboardRead { text } => {
                let text = text.unwrap_or_else(|| self.clipboard.clone());
                let target = self.paste_at.take();
                if target != Some(self.paste_target()) {
                    self.info("The clipboard answered too late: paste again".to_owned());
                } else if !text.is_empty() {
                    fx.extend(self.dispatch(Command::Insert(text)));
                }
            }
            Event::Analysis(event) => self.analysis_handle(event, &mut fx),
            Event::Http(event) => self.http.handle(event),
            Event::Db(event) => self.db.handle(event),
            Event::Cli(event) => self.cli_finished(event, &mut fx),
            Event::ScratchCreated { result } => match result {
                Ok(path) => {
                    self.focus = Focus::Editor;
                    self.open_file(path, None, &mut fx);
                }
                Err(message) => self.error(format!("No scratch file: {message}")),
            },
            Event::ScratchesListed { paths } => {
                if paths.is_empty() {
                    self.info("No scratch files yet: `New Scratch File` makes one".to_owned());
                } else {
                    self.scratches = paths;
                    self.open_picker(PickerKind::Scratches);
                }
            }
            Event::ConfigProblems { messages } => {
                if let Some(first) = messages.first() {
                    let more = messages.len() - 1;
                    let rest = if more > 0 {
                        format!(" (and {more} more)")
                    } else {
                        String::new()
                    };
                    self.error(format!("{first}{rest}"));
                }
            }
            Event::Lsp(LspEvent::ChoiceNotSaved { message }) => {
                self.error(format!("Could not remember the language server: {message}"));
            }
            Event::Lsp(LspEvent::ServersListed { options }) => {
                if self.picker.is_none() && self.local_history.is_none() {
                    self.server_options = options;
                    self.open_picker(PickerKind::Servers);
                }
            }
            Event::Lsp(LspEvent::Answered { server, id, result }) => {
                self.lsp_answered(&server, id, result, &mut fx);
            }
            Event::DirListed { path, result } => match result {
                Ok(entries) => self.tree.set_children(path, entries),
                Err(e) => self.error(format!("Could not list {}: {e}", path.display())),
            },
            Event::ProjectReplacePlanned { id, files, failed } => {
                self.replace_planned(id, files, failed);
            }
            Event::ProjectReplaceApplied {
                id,
                changed,
                skipped,
                failed,
            } => self.replace_applied(id, changed, skipped, failed, &mut fx),
            Event::IndexProgress { done, total } => {
                self.index_progress = (done < total).then_some((done, total));
            }
            Event::SymbolsFound { id, symbols } => self.symbols_found(id, symbols),
            Event::FileSymbolsRead {
                path,
                version,
                symbols,
            } => self.file_symbols_read(&path, version, symbols),
            Event::FilesIndexed { files } => {
                let mut files: Vec<String> = files
                    .iter()
                    .map(|p| self.display_path(p).to_string_lossy().into_owned())
                    .collect();
                files.sort();
                self.files = files;
            }
            Event::SearchFinished {
                id,
                matches,
                truncated,
            } => {
                if self.todos_finished(id, &matches, truncated) {
                } else if self.search.running == Some(id) {
                    self.search.running = None;
                    self.search.results = matches;
                    self.search.truncated = truncated;
                }
            }
        }
        self.drop_stale_completion();
        self.git_conflict_catch_up();
        self.merge_catch_up();
        self.ide_sync(&mut fx);
        self.bookmarks_sync();
        self.note_recent();
        self.sweep_swaps(&mut fx);
        self.sync_lsp(&mut fx);
        self.ask_for_grammars(&mut fx);
        self.git_blame_catch_up(&mut fx);
        fx
    }

    fn run_action(&mut self, action: Action, fx: &mut Vec<Effect>) {
        if self.run_window
            && !matches!(
                action,
                Action::Quit | Action::NextRunError | Action::PreviousRunError
            )
        {
            return self.info("This window only follows the main window's run".to_owned());
        }
        self.close_undo_group();
        if self.trust_refuses(action) {
            return;
        }
        match action {
            Action::Quit => self.quit(fx),
            Action::TrustProject => self.trust_ask(None),
            Action::OpenPalette => self.open_picker(PickerKind::Actions),
            Action::MainMenu => {
                if !self.window {
                    self.menu = Some(crate::core::menu::MenuState::open(0));
                }
            }
            Action::TerminalToggle => self.terminal_toggle(fx),
            Action::TerminalNew => self.terminal_new(fx),
            Action::TerminalCloseTab => self.terminal_close(self.terminal.active(), fx),
            Action::TerminalNext => self.terminal_step(true),
            Action::TerminalPrevious => self.terminal_step(false),
            Action::ClaudeOpen => self.claude_toggle(fx),
            Action::ToggleBreakpoint
            | Action::EditBreakpoint
            | Action::AddLogpoint
            | Action::ToggleBreakpointEnabled
            | Action::ViewBreakpoints
            | Action::RemoveAllBreakpoints
            | Action::DebugChoose
            | Action::DebugLast
            | Action::DebugStop
            | Action::DebugShow
            | Action::ResumeProgram
            | Action::DebugPause
            | Action::DebugStepOver
            | Action::DebugStepInto
            | Action::DebugStepOut
            | Action::DebugRunToCursor
            | Action::DebugEvaluate
            | Action::DebugAddWatch
            | Action::DebugExceptions => self.debug_action(action, fx),
            Action::StopRun if self.debug_owns_stop() => self.debug_stop(fx),
            Action::ClaudeWindow => self.claude_new_window(fx),
            Action::ClaudeAsk => self.claude_mention(fx),
            Action::ClaudeFix => self.claude_fix(fx),
            Action::ClaudeReview => self.mcp_open_review(),
            Action::NewScratchFile if !self.window => self.open_picker(PickerKind::ScratchKinds),
            Action::ShowScratches if !self.window => fx.push(Effect::ScratchList),
            Action::NewFromTemplate if !self.window => self.open_picker(PickerKind::FileTemplates),
            Action::NewProject if !self.window => self.open_picker(PickerKind::ProjectTemplates),
            Action::NewFromTemplate
            | Action::NewProject
            | Action::NewScratchFile
            | Action::ShowScratches => {
                self.info("Files are changed from the main window".to_owned());
            }
            Action::NewFile | Action::NewFolder | Action::RenameFile | Action::DeleteFile => {
                self.file_action(action);
            }
            Action::GotoFile => self.open_picker(PickerKind::Files),
            Action::RecentFiles => self.open_picker(PickerKind::RecentFiles),
            Action::SearchEverywhere => self.open_picker(PickerKind::Everywhere),
            Action::GotoSymbol => self.open_picker(PickerKind::ProjectSymbols),
            Action::Save => {
                if let Some((view, d)) = self.active() {
                    let doc = view.doc;
                    if d.conflict() == Some(Conflict::Deleted) {
                        fx.extend(self.overwrite_effect(doc));
                    } else {
                        fx.extend(self.write_effect(doc, WriteOrigin::User));
                    }
                }
            }
            Action::RunChoose
            | Action::RunAnything
            | Action::RunTest
            | Action::RerunLast
            | Action::StopRun
            | Action::ShowRun
            | Action::NextRunError
            | Action::PreviousRunError => self.run_action_in_window(action, fx),
            Action::AnalyzeProject => self.analysis_ask(fx),
            Action::StopAnalysis => self.analysis_stop(fx),
            Action::ShowProblems => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Problems;
                self.focus = Focus::Problems;
            }
            Action::HttpRun | Action::HttpStop | Action::ShowHttp => self.http_action(action, fx),
            Action::DbRun | Action::DbStop | Action::ShowDatabase => self.db_action(action, fx),
            Action::ShowDocker => self.docker_show(fx),
            Action::ShowGitHub => self.github_show(fx),
            Action::ShowLanguageServer => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Language;
                self.focus = Focus::Language;
            }
            Action::RestartLanguageServer => match self.active_server() {
                Some((server, status)) if status.state != LspState::Disabled => {
                    fx.push(Effect::Lsp(LspEffect::Restart {
                        server: server.clone(),
                    }));
                }
                _ => self.info("No language server is running".to_owned()),
            },
            Action::GotoDefinition => {
                self.lsp_query(|at| LspQuery::Definition { at }, fx);
            }
            Action::FindUsages => {
                self.lsp_query(|at| LspQuery::References { at }, fx);
            }
            Action::GitLog => self.open_git(GitTab::Log, fx),
            Action::GitCommit => self.open_git(GitTab::Commit, fx),
            Action::GitPush => self.git_remote(GitRemoteOp::Push, fx),
            Action::GitPull => self.git_remote(GitRemoteOp::Pull, fx),
            Action::GitBranches => self.git_ask_branches(fx),
            Action::GitFileHistory => self.git_show_history(fx),
            Action::LoadCoverage => self.coverage_load_ask(),
            Action::ToggleCoverage => self.coverage_toggle(),
            Action::GitCompareBranch => self.git_compare_ask(None, fx),
            Action::GitComparePath => self.git_compare_path(fx),
            Action::GitBlame => self.git_toggle_blame(fx),
            Action::GitMergeView => self.merge_open(fx),
            Action::GitNextConflict => self.git_step_conflict(true, fx),
            Action::GitPreviousConflict => self.git_step_conflict(false, fx),
            Action::GitAcceptOurs => self.git_accept(ConflictChoice::Ours),
            Action::GitAcceptTheirs => self.git_accept(ConflictChoice::Theirs),
            Action::GitAcceptBoth => self.git_accept(ConflictChoice::Both),
            Action::GitContinue => self.git_continue(fx),
            Action::GitAbort => self.git_abort(fx),
            Action::GitStash => self.git_show_stash(fx),
            Action::GitManageBranches => self.git_show_branches(fx),
            Action::GitSideBySide => {
                if let Some(panel) = self.git_panel.as_mut() {
                    panel.side_by_side = !panel.side_by_side;
                }
            }
            Action::MarkdownPreview => self.toggle_markdown_preview(),
            Action::OpenInSystemViewer => self.open_image_outside(fx),
            Action::GitLogWindow => {
                self.info("Opening a window…".to_owned());
                fx.push(Effect::OpenGitWindow {
                    root: self.git_root(),
                });
            }
            Action::RunOutputWindow => {
                if self.window {
                    return self.info("Running is in the main window".to_owned());
                }
                self.info("Opening a window…".to_owned());
                fx.push(Effect::OpenRunWindow {
                    root: self.git_root(),
                });
            }
            Action::ContextActions if self.in_commit_message() => {
                if let Some(panel) = self.git_panel.as_mut() {
                    panel.message.insert_lines("\n");
                }
            }
            Action::Copy => self.copy(false, fx),
            Action::Cut => self.copy(true, fx),
            Action::Paste => {
                self.paste_at = Some(self.paste_target());
                fx.push(Effect::ReadClipboard);
            }
            Action::SelectAll => {
                if self.focus == Focus::Editor
                    && let Some((_, d)) = self.active()
                {
                    let mut all = Cursor::at(d.text().len_chars());
                    all.anchor = Some(0);
                    self.set_active_cursor(all);
                }
            }
            Action::OpenSettings => match self.settings_file.clone() {
                Some(file) => {
                    self.open_file(file, None, fx);
                    self.info("Settings apply after a restart".to_owned());
                }
                None => self.info("This system has no config folder".to_owned()),
            },
            Action::ContextActions => {
                self.lsp_ask(
                    |text, cursor, encoding| {
                        let range = cursor.selection().unwrap_or(cursor.idx..cursor.idx);
                        LspQuery::CodeActions {
                            at: position_of(text, range.start, encoding),
                            to: position_of(text, range.end, encoding),
                        }
                    },
                    fx,
                );
            }
            Action::RenameSymbol => self.rename_ask(),
            Action::RefactorThis => self.refactor_ask(fx),
            Action::SafeDelete => self.safe_delete_ask(fx),
            Action::ReloadPlugins => fx.push(Effect::LoadPlugins),
            Action::ReformatCode => self.reformat(fx),
            Action::FileStructure => self.structure_ask(fx),
            Action::CallersOf => self.calls_ask(false, fx),
            Action::CalleesOf => self.calls_ask(true, fx),
            Action::ToggleBookmark => self.bookmark_toggle(),
            Action::ShowBookmarks => self.bookmarks_show(),
            Action::LocateDuplicates => self.duplicates_show(fx),
            Action::ShowTodos => self.todos_show(fx),
            Action::ShowTests => self.tests_show(),
            Action::ShowNotifications => self.notifications_list(),
            Action::ClearNotifications => self.notifications.clear(),
            Action::CheckSpelling => self.spell_check(fx),
            Action::AddToDictionary => self.spell_add_word(fx),
            Action::ShowDependencies => self.deps_show(fx),
            Action::MacroRecord => self.macro_toggle(),
            Action::MacroPlay => self.macro_play(fx),
            Action::SelectLanguageServer => {
                let other = self
                    .active()
                    .map(|(_, d)| d.language())
                    .filter(|l| *l != Language::Rust);
                if let Some(language) = other {
                    self.info(format!(
                        "This list is for Rust. {language} uses its server from PATH, or [lsp.{key}] in the config",
                        language = language.name(),
                        key = language.spec().config_name,
                    ));
                } else {
                    self.info("Looking for language servers…".to_owned());
                    fx.push(Effect::Lsp(LspEffect::ListServers));
                }
            }
            Action::Completion => {
                self.lsp_query(|at| LspQuery::Completion { at }, fx);
            }
            Action::QuickDocumentation => {
                self.lsp_query(|at| LspQuery::Hover { at }, fx);
            }
            Action::NavigateBack => match self.jumps_back.pop() {
                Some((path, line, col)) => self.open_file(path, Some((line, col)), fx),
                None => self.info("Nowhere to go back to".to_owned()),
            },
            Action::ShowLocalHistory => {
                if let Some((view, d)) = self.active() {
                    fx.push(Effect::ListHistory {
                        doc: view.doc,
                        path: d.path().to_path_buf(),
                    });
                }
            }
            Action::ToolWindowTaller => self.resize_tool(true),
            Action::ToolWindowShorter => self.resize_tool(false),
            Action::ReloadFromDisk => {
                if let Some((view, d)) = self.active() {
                    if d.is_dirty() {
                        fx.push(Effect::Snapshot {
                            path: d.path().to_path_buf(),
                            bytes: d.to_bytes(),
                            label: SnapshotLabel::BeforeReload,
                        });
                    }
                    fx.push(Effect::ReloadFile {
                        doc: view.doc,
                        path: d.path().to_path_buf(),
                    });
                }
            }
            Action::KeepMine => {
                if let Some((view, d)) = self.active()
                    && d.conflict().is_some()
                {
                    let doc = view.doc;
                    fx.extend(self.overwrite_effect(doc));
                }
            }
            Action::SaveAll => {
                let dirty: Vec<_> = self.dirty_docs();
                fx.extend(
                    dirty
                        .into_iter()
                        .filter_map(|d| self.write_effect(d, WriteOrigin::User)),
                );
            }
            Action::CloseTab => {
                if let Some(v) = self.active_view() {
                    self.close_view(v, fx);
                }
            }
            Action::NextTab => self.cycle_tab(1),
            Action::PreviousTab => self.cycle_tab(-1),
            Action::Undo => {
                if !self.undo_rename() {
                    self.travel(Document::undo, "Nothing to undo");
                }
            }
            Action::Redo => self.travel(Document::redo, "Nothing to redo"),
            Action::Find => self.open_find_bar(false),
            Action::Replace => self.open_find_bar(true),
            Action::ReplaceAll => self.replace_all(),
            Action::ToggleFindRegex => self.toggle_find_option(|o| o.regex = !o.regex, fx),
            Action::ToggleFindCase => {
                self.toggle_find_option(|o| o.case_sensitive = !o.case_sensitive, fx);
            }
            Action::ToggleFindWord => {
                self.toggle_find_option(|o| o.whole_word = !o.whole_word, fx);
            }
            Action::ToggleSoftWrap => {
                self.soft_wrap = !self.soft_wrap;
                for view in self.views.values_mut() {
                    view.cursor.preferred_col = None;
                }
                let state = if self.soft_wrap { "on" } else { "off" };
                self.info(format!("Soft wrap {state}"));
            }
            Action::FindNext => self.find_step(true),
            Action::FindPrevious => self.find_step(false),
            Action::StructuralSearch => {
                self.search.options.structural = true;
                self.run_action(Action::FindInProject, fx);
            }
            Action::ToggleStructural if self.focus == Focus::FindInFiles => {
                self.toggle_find_option(|o| o.structural = !o.structural, fx);
            }
            Action::ToggleStructural => self.info("Open Find in Files first".to_owned()),
            Action::FindInProject => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Find;
                self.search.selected = None;
                self.search.replacement = None;
                self.search.in_replacement = false;
                self.focus = Focus::FindInFiles;
            }
            Action::ReplaceInFiles => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Find;
                self.focus = Focus::FindInFiles;
                let s = &mut self.search;
                s.selected = None;
                s.replacement.get_or_insert_with(LineInput::default).end();
                s.in_replacement = !s.input.text().is_empty();
                if s.usages_of.is_some() {
                    self.info(USAGES_ARE_NOT_TEXT.to_owned());
                }
            }
            Action::SplitVertically => self.split(Axis::Columns),
            Action::SplitHorizontally => self.split(Axis::Rows),
            Action::Unsplit => self.unsplit(fx),
            Action::NextSplitter => {
                let panes = self.layout.panes();
                let i = panes
                    .iter()
                    .position(|p| *p == self.active_pane)
                    .unwrap_or(0);
                self.active_pane = panes[(i + 1) % panes.len()];
                self.focus = Focus::Editor;
            }
            Action::ToggleProject => {
                if !self.project_visible {
                    self.project_visible = true;
                    self.focus = Focus::Project;
                } else if self.focus == Focus::Project {
                    self.project_visible = false;
                    self.focus = Focus::Editor;
                } else {
                    self.focus = Focus::Project;
                }
            }
            Action::HideToolWindow => {
                self.tool_window_visible = false;
                if matches!(
                    self.focus,
                    Focus::Terminal
                        | Focus::Claude
                        | Focus::FindInFiles
                        | Focus::Problems
                        | Focus::Git
                        | Focus::Run
                        | Focus::Debug
                        | Focus::Language
                        | Focus::Http
                        | Focus::Duplicates
                        | Focus::Todo
                        | Focus::Tests
                        | Focus::Dependencies
                        | Focus::Bookmarks
                        | Focus::Database
                        | Focus::Docker
                        | Focus::GitHub
                ) {
                    self.focus = Focus::Editor;
                }
            }
        }
    }

    fn resize_tool(&mut self, taller: bool) {
        if self.window {
            return self.info("This window has only the log".to_owned());
        }
        if !self.tool_window_visible {
            self.tool_window_visible = true;
            return;
        }
        let (rows, min, max) = self.tool_room;
        if max == 0 {
            return self.info("The terminal is too small for a tool window".to_owned());
        }
        let wanted = if taller {
            rows.saturating_add(TOOL_RESIZE_STEP).min(max)
        } else {
            rows.saturating_sub(TOOL_RESIZE_STEP).max(min)
        };
        if wanted == rows {
            let edge = if taller { "largest" } else { "smallest" };
            return self.info(format!("The tool window is at its {edge}"));
        }
        self.tool_height = Some(wanted);
        self.tool_room.0 = wanted;
    }

    fn escape(&mut self) {
        if self.picker.take().is_some() {
            return;
        }
        if self.focus == Focus::Editor
            && let Some((view, _)) = self.active()
            && view.cursor.selection().is_some()
        {
            let idx = view.cursor.idx;
            return self.set_active_cursor(Cursor::at(idx));
        }
        match self.focus {
            Focus::FindBar => {
                self.find_bar = None;
                self.focus = Focus::Editor;
            }
            Focus::FindInFiles if self.search.replace.is_some() => self.leave_replace_stage(),
            Focus::FindInFiles if self.search.replacement.is_some() => {
                self.search.replacement = None;
                self.search.in_replacement = false;
            }
            Focus::Project
            | Focus::FindInFiles
            | Focus::Problems
            | Focus::Git
            | Focus::Run
            | Focus::Debug
            | Focus::Language
            | Focus::Http
            | Focus::Duplicates
            | Focus::Todo
            | Focus::Tests
            | Focus::Dependencies
            | Focus::Bookmarks
            | Focus::Database
            | Focus::Docker
            | Focus::GitHub
            | Focus::Claude
            | Focus::Terminal => {
                self.focus = Focus::Editor;
            }
            Focus::Editor if self.tool_window_visible => self.tool_window_visible = false,
            Focus::Editor => self.find_bar = None,
        }
    }

    fn check_docs(&self, wanted: impl Fn(&Document) -> bool) -> Vec<Effect> {
        self.documents
            .iter()
            .filter(|(_, d)| wanted(d))
            .map(|(id, d)| Effect::CheckFile {
                doc: *id,
                path: d.path().to_path_buf(),
                known: d.disk(),
            })
            .collect()
    }

    fn file_changed(
        &mut self,
        doc: DocumentId,
        disk: Option<DiskState>,
        result: Result<Option<Vec<u8>>, String>,
    ) {
        let bytes = match result {
            Ok(Some(_)) if self.documents.get(&doc).is_some_and(Document::is_image) => {
                if let Some(d) = self.documents.get_mut(&doc) {
                    d.set_disk(disk);
                }
                return;
            }
            Ok(Some(bytes)) => bytes,
            Ok(None) => {
                if let Some(d) = self.documents.get_mut(&doc) {
                    d.set_conflict(Some(Conflict::Deleted));
                }
                return;
            }
            Err(e) => return self.error(format!("Could not read {}: {e}", self.doc_name(doc))),
        };
        let Some(d) = self.documents.get_mut(&doc) else {
            return;
        };
        if d.to_bytes() == bytes {
            let version = d.version();
            d.mark_saved(version);
            d.set_disk(disk);
            d.set_conflict(None);
        } else if d.is_dirty() {
            d.set_conflict(Some(Conflict::Modified));
        } else {
            self.reload_document(doc, &bytes, disk);
        }
    }

    fn reload_document(&mut self, doc: DocumentId, bytes: &[u8], disk: Option<DiskState>) {
        let Some(d) = self.documents.get_mut(&doc) else {
            return;
        };
        if d.is_image() {
            return d.set_disk(disk);
        }
        match d.reload(bytes, disk) {
            Ok(applied) => {
                self.after_change(None, doc, &applied);
            }
            Err(e) => self.error(e.to_string()),
        }
    }

    fn paths_changed(&self, paths: &[PathBuf], fx: &mut Vec<Effect>) {
        fx.extend(self.check_docs(|d| paths.iter().any(|p| p == d.path())));
        let mut dirs: Vec<PathBuf> = paths
            .iter()
            .filter_map(|p| p.parent())
            .filter(|dir| self.tree.is_listed(dir))
            .map(Path::to_path_buf)
            .collect();
        dirs.sort();
        dirs.dedup();
        fx.extend(dirs.into_iter().map(|path| Effect::ListDir { path }));
        let is_open = |p: &PathBuf| self.documents.values().any(|d| d.path() == p);
        if !paths.iter().all(is_open) {
            fx.push(Effect::IndexFiles {
                root: self.root.clone(),
            });
        }
        fx.push(Effect::Git(GitEffect::Status {
            root: self.root.clone(),
        }));
        fx.push(Effect::RescanIndex);
    }

    fn dirty_docs(&self) -> Vec<DocumentId> {
        self.documents
            .iter()
            .filter(|(_, d)| d.is_dirty())
            .map(|(id, _)| *id)
            .collect()
    }

    fn any_dirty(&self) -> bool {
        self.documents.values().any(Document::is_dirty)
    }

    fn quit(&mut self, fx: &mut Vec<Effect>) {
        if !self.any_dirty() || self.quit == QuitState::SaveFailed {
            self.quit = QuitState::Done;
            return;
        }
        self.quit = QuitState::SavingBeforeQuit;
        let dirty = self.dirty_docs();
        fx.extend(
            dirty
                .into_iter()
                .filter_map(|d| self.write_effect(d, WriteOrigin::User)),
        );
    }

    fn open_picker(&mut self, kind: PickerKind) {
        self.project_symbols.clear();
        self.symbols_asked = None;
        let items = self.picker_items(kind);
        self.picker = Some(Picker::new(kind, &items));
    }

    fn picker_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let Some(mut picker) = self.picker.take() else {
            return;
        };
        let items = self.picker_items(picker.kind);
        match input {
            Command::Insert(s) => {
                picker.input.insert(&s);
                picker.refilter(&items);
                self.symbols_typed(&mut picker, fx);
            }
            Command::DeleteBackward => {
                picker.input.delete_backward();
                picker.refilter(&items);
                self.symbols_typed(&mut picker, fx);
            }
            Command::DeleteForward => {
                picker.input.delete_forward();
                picker.refilter(&items);
                self.symbols_typed(&mut picker, fx);
            }
            Command::Move(m) => match m {
                Motion::Up => picker.select_previous(1),
                Motion::Down => picker.select_next(1),
                Motion::PageUp { rows } => picker.select_previous(rows),
                Motion::PageDown { rows } => picker.select_next(rows),
                other => move_input(&mut picker.input, other),
            },
            Command::Newline => {
                match (picker.kind, picker.selected_index()) {
                    (PickerKind::Actions, Some(i)) => match Action::ALL.get(i) {
                        Some(action) => self.run_action(*action, fx),
                        None => self.plugin_chosen(i - Action::ALL.len(), fx),
                    },
                    (PickerKind::Files, Some(i)) => {
                        let path = PathBuf::from(&items[i]);
                        self.open_file(path, None, fx);
                    }
                    (PickerKind::Everywhere, Some(i)) => {
                        if let Some(symbol) = self.everywhere_symbol(i, items.len()).cloned() {
                            self.project_symbol_jump(symbol, fx);
                        } else {
                            match Self::everywhere_action(i, items.len()) {
                                Some(action) => self.run_action(action, fx),
                                None => self.open_file(PathBuf::from(&items[i]), None, fx),
                            }
                        }
                    }
                    (PickerKind::ProjectSymbols, Some(i)) => {
                        if let Some(symbol) = self.project_symbols.get(i).cloned() {
                            self.project_symbol_jump(symbol, fx);
                        }
                    }
                    (PickerKind::RecentFiles, Some(i)) => {
                        if let Some(path) = self.recent_others().get(i).cloned() {
                            self.open_file(path, None, fx);
                        }
                    }
                    (PickerKind::Branches, Some(i)) => {
                        let branch = self.repo.branches()[i].clone();
                        if !self.repo.is_current(&branch) {
                            self.git_switch(branch, false, fx);
                        }
                    }
                    (PickerKind::Branches, None) if !picker.input.text().trim().is_empty() => {
                        let branch = GitBranch {
                            name: picker.input.text().trim().to_owned(),
                            remote: false,
                        };
                        self.git_switch(branch, true, fx);
                    }
                    (PickerKind::CodeActions, Some(i)) => {
                        let action = self.code_actions[i].clone();
                        self.apply_as_one(action.changes, fx);
                    }
                    (PickerKind::Refactorings, Some(i)) => self.refactor_chosen(i, fx),
                    (PickerKind::CompareBranches, Some(i)) => {
                        if let Some(branch) = self.repo.branches().get(i).cloned() {
                            self.git_compare_start(branch.name, fx);
                        }
                    }
                    (PickerKind::Changelists, chosen) => {
                        let typed = picker.input.text().trim().to_owned();
                        self.changelist_chosen(chosen, &typed);
                    }
                    (PickerKind::Servers, Some(i)) => {
                        let option = self.server_options[i].clone();
                        self.info(format!("Language server: {}", option.label));
                        fx.push(Effect::Lsp(LspEffect::SelectServer(option.choice)));
                    }
                    (PickerKind::Run, Some(i)) => self.run_pick(i, fx),
                    (
                        kind @ (PickerKind::FileTemplates | PickerKind::ProjectTemplates),
                        Some(i),
                    ) => {
                        self.template_chosen(kind, i);
                    }
                    (PickerKind::ScratchKinds, Some(i)) => {
                        let (_, extension) = crate::core::scratch::SCRATCH_KINDS[i];
                        fx.push(Effect::ScratchCreate {
                            extension: extension.to_owned(),
                        });
                    }
                    (PickerKind::Scratches, Some(i)) => {
                        let path = self.scratches[i].clone();
                        self.focus = Focus::Editor;
                        self.open_file(path, None, fx);
                    }
                    (PickerKind::Symbols, Some(i)) => self.symbol_jump(i, fx),
                    (PickerKind::Breakpoints, Some(i)) => self.breakpoint_jump(i, fx),
                    (PickerKind::DebugTargets, Some(i)) => self.debug_choice_taken(i, fx),
                    (PickerKind::DebugExecutables, Some(i)) => self.debug_executable_taken(i, fx),
                    (PickerKind::DebugThreads, Some(i)) => self.debug_thread_taken(i, fx),
                    (PickerKind::Notifications, Some(i)) => self.notification_chosen(i, fx),
                    (PickerKind::Locations, Some(i)) => {
                        let location = self.locations[i].clone();
                        self.jump_to(location, fx);
                    }
                    (_, None) => {}
                }
                return;
            }
            _ => {}
        }
        self.picker = Some(picker);
    }

    fn tree_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let request = match input {
            Command::Move(Motion::Up) => {
                self.tree.move_selection(-1);
                TreeRequest::None
            }
            Command::Move(Motion::Down) => {
                self.tree.move_selection(1);
                TreeRequest::None
            }
            Command::Move(Motion::PageUp { rows }) => {
                self.tree.move_selection(-(rows as isize));
                TreeRequest::None
            }
            Command::Move(Motion::PageDown { rows }) => {
                self.tree.move_selection(rows as isize);
                TreeRequest::None
            }
            Command::Move(Motion::DocStart | Motion::LineStart) => {
                self.tree.move_selection(isize::MIN);
                TreeRequest::None
            }
            Command::Move(Motion::DocEnd | Motion::LineEnd) => {
                self.tree.move_selection(isize::MAX);
                TreeRequest::None
            }
            Command::Move(Motion::Right) => self.tree.expand(),
            Command::Move(Motion::Left) => {
                self.tree.collapse();
                TreeRequest::None
            }
            Command::Newline => self.tree.activate(),
            Command::Insert(s) => {
                match s.as_str() {
                    "n" => self.file_action(Action::NewFile),
                    "m" => self.file_action(Action::NewFolder),
                    "r" => self.file_action(Action::RenameFile),
                    "d" => self.file_action(Action::DeleteFile),
                    _ if self.active().is_none() => self.no_file_hint(),
                    _ => {}
                }
                TreeRequest::None
            }
            _ => TreeRequest::None,
        };
        match request {
            TreeRequest::None => {}
            TreeRequest::List(path) => fx.push(Effect::ListDir { path }),
            TreeRequest::Open(path) => self.open_file(path, None, fx),
        }
    }

    fn set_active_cursor_line_col(&mut self, line: usize, col: usize) {
        if let Some((_, doc)) = self.active() {
            let t = doc.text();
            let line = line.min(t.len_lines() - 1);
            let idx = t.line_to_char(line) + col.min(text::line_content_len(t, line));
            self.set_active_cursor(Cursor::at(idx));
        }
    }

    fn info(&mut self, text: String) {
        self.status = Some(StatusMessage {
            level: Level::Info,
            text,
        });
    }

    fn error(&mut self, text: String) {
        self.status = Some(StatusMessage {
            level: Level::Error,
            text,
        });
    }
}

fn conflict_for(disk: Option<DiskState>) -> Conflict {
    if disk.is_some() {
        Conflict::Modified
    } else {
        Conflict::Deleted
    }
}

fn move_input(input: &mut super::input::LineInput, motion: Motion) {
    input.motion(motion);
}

pub fn apply_motion(t: &Rope, cursor: Cursor, motion: Motion, metrics: Metrics) -> Cursor {
    let idx = cursor.idx;
    let line = t.char_to_line(idx);
    let vertical = |delta: isize| {
        let (line, row, col) = metrics.position(t, idx);
        let col = cursor.preferred_col.unwrap_or(col);
        let (line, row) = metrics.step(t, (line, row), delta);
        Cursor {
            idx: metrics.idx_at(t, line, row, col),
            preferred_col: Some(col),
            anchor: None,
        }
    };
    match motion {
        Motion::Left => Cursor::at(text::prev_grapheme_boundary(t, idx)),
        Motion::Right => Cursor::at(text::next_grapheme_boundary(t, idx)),
        Motion::WordLeft => Cursor::at(idx - text::word_motion_len(t.chars_at(idx).reversed())),
        Motion::WordRight => match idx + text::word_motion_len(t.chars_at(idx)) {
            0 => cursor,
            end => Cursor::at(text::next_grapheme_boundary(t, end - 1)),
        },
        Motion::Up => vertical(-1),
        Motion::Down => vertical(1),
        Motion::PageUp { rows } => vertical(-(rows as isize)),
        Motion::PageDown { rows } => vertical(rows as isize),
        Motion::LineStart => {
            let start = t.line_to_char(line);
            let rows = metrics.rows(t, line);
            let row_start = start + rows[row_at(&rows, idx - start)].chars.start;
            if row_start > start && idx != row_start {
                return Cursor::at(row_start);
            }
            let indent = text::line_content(t, line)
                .chars()
                .take_while(|c| c.is_whitespace())
                .count();
            Cursor::at(if idx == start + indent {
                start
            } else {
                start + indent
            })
        }
        Motion::LineEnd => Cursor::at(t.line_to_char(line) + text::line_content_len(t, line)),
        Motion::DocStart => Cursor::at(0),
        Motion::DocEnd => Cursor::at(t.len_chars()),
    }
}
