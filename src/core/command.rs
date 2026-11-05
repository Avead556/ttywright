use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::action::Action;
use super::change::ChangeSet;
use super::document::DiskState;
use super::document::Language;
use super::find::{ClosedPlan, FileProblem, FindOptions, SearchMatch};
use super::local_history::{HistoryEntry, SnapshotLabel};
use super::lsp::{LspSymbol, ProjectSymbol};
use super::run_output::RunStreamMessage;
use super::session::{Session, SessionFile};
use super::tree::DirEntry;
use super::workspace::{DocumentId, Focus, ViewId};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Command {
    TerminalSize {
        rows: u16,
        columns: u16,
    },
    TerminalInput(Vec<u8>),
    TerminalScroll {
        delta: isize,
    },
    SelectTerminalTab {
        index: usize,
    },
    CloseTerminalTab {
        index: usize,
    },
    CloseView {
        view: ViewId,
    },
    ClaudeSize {
        rows: u16,
        columns: u16,
    },
    ClaudeInput(Vec<u8>),
    ClaudeScroll {
        delta: isize,
    },
    OpenMenu {
        menu: usize,
    },
    PickMenuItem {
        entry: usize,
    },
    Insert(String),
    Newline,
    Indent,
    Outdent,
    DeleteBackward,
    DeleteForward,
    Move(Motion),
    Select(Motion),
    SelectTo {
        line: usize,
        col: usize,
    },
    SelectWord,
    Escape,
    Autosave,
    CheckDisk,
    WriteSwaps,
    SaveSession,
    Viewport {
        text_cols: usize,
    },
    ResizeTool {
        rows: u16,
    },
    ResizeProject {
        columns: u16,
    },
    ToolRoom {
        rows: u16,
        min: u16,
        max: u16,
    },
    Run(Action),
    WantImage(super::image_art::ImageRequest),
    OpenFile {
        path: PathBuf,
        at: Option<(usize, usize)>,
    },
    FocusView {
        view: ViewId,
        at: Option<(usize, usize)>,
    },
    Focus(Focus),
    SelectTreeRow {
        path: PathBuf,
        activate: bool,
    },
    SelectSearchResult {
        index: usize,
        open: bool,
    },
    SelectPickerItem {
        index: usize,
        accept: bool,
    },
    SelectHistoryEntry {
        index: usize,
    },
    SelectDuplicateRow {
        index: usize,
        open: bool,
    },
    SelectTodoRow {
        index: usize,
        open: bool,
    },
    SelectTestRow {
        index: usize,
        open: bool,
    },
    NotificationTick,
    TerminalFocus {
        focused: bool,
    },
    NotificationClick {
        id: u64,
    },
    NotificationClose {
        id: u64,
    },
    SelectDependencyRow {
        index: usize,
        open: bool,
    },
    SelectBookmarkRow {
        index: usize,
        open: bool,
    },
    SelectProblem {
        index: usize,
        open: bool,
    },
    SelectGitRow {
        index: usize,
    },
    SelectRunTab {
        index: usize,
    },
    SelectRunLine {
        line: usize,
    },
    ScrollGitHub {
        delta: isize,
    },
    PickGitHubRow {
        index: usize,
    },
    ScrollDocker {
        delta: isize,
    },
    PickDockerRow {
        section: usize,
        index: usize,
    },
    ScrollDatabase {
        delta: isize,
    },
    PickDatabaseRow {
        table: bool,
        index: usize,
    },
    ScrollHttp {
        delta: isize,
    },
    ScrollRun {
        delta: isize,
    },
    ToggleBreakpointAt {
        view: ViewId,
        line: usize,
    },
    SelectDebugRow {
        part: super::debug::DebugPart,
        index: usize,
        open: bool,
    },
    ScrollDebugConsole {
        delta: isize,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Motion {
    Left,
    Right,
    WordLeft,
    WordRight,
    Up,
    Down,
    LineStart,
    LineEnd,
    PageUp { rows: usize },
    PageDown { rows: usize },
    DocStart,
    DocEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteOrigin {
    User,
    Auto,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum WriteGuard {
    Unchanged(Option<DiskState>),
    Overwrite,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RecoveredSwap {
    pub path: PathBuf,
    pub base: Option<DiskState>,
    pub current: Option<DiskState>,
    pub on_disk: Option<Vec<u8>>,
    pub contents: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Effect {
    ReadFile {
        path: PathBuf,
        at: Option<(usize, usize)>,
    },
    WriteFile {
        doc: DocumentId,
        version: u64,
        path: PathBuf,
        bytes: Vec<u8>,
        guard: WriteGuard,
        origin: WriteOrigin,
    },
    CheckFile {
        doc: DocumentId,
        path: PathBuf,
        known: Option<DiskState>,
    },
    ReloadFile {
        doc: DocumentId,
        path: PathBuf,
    },
    WatchProject {
        root: PathBuf,
    },
    WriteSwap {
        path: PathBuf,
        base: Option<DiskState>,
        contents: Vec<u8>,
    },
    RemoveSwap {
        path: PathBuf,
    },
    JournalAppend {
        path: PathBuf,
        disk: Option<DiskState>,
        start: Option<Vec<u8>>,
        changes: Vec<ChangeSet>,
    },
    RemoveJournal {
        path: PathBuf,
    },
    RecoverSwaps,
    LoadSession,
    Lsp(super::lsp_command::LspEffect),
    Git(super::git_command::GitEffect),
    Duplicates(super::duplicates::DuplicatesEffect),
    DecodeImage {
        root: PathBuf,
        request: super::image_art::ImageRequest,
    },
    Coverage(super::coverage::CoverageEffect),
    OpenInMainWindow {
        root: PathBuf,
        path: PathBuf,
        at: Option<(usize, usize)>,
    },
    OpenGitWindow {
        root: PathBuf,
    },
    OpenRunWindow {
        root: PathBuf,
    },
    RunFollow {
        root: PathBuf,
    },
    RunStreamSend(RunStreamMessage),
    RunStreamStart(RunStreamMessage),
    WriteClipboard {
        text: String,
    },
    ReadClipboard,
    Run(super::run_state::RunEffect),
    Debug(super::debug::DebugEffect),
    Snapshot {
        path: PathBuf,
        bytes: Vec<u8>,
        label: SnapshotLabel,
    },
    ListHistory {
        doc: DocumentId,
        path: PathBuf,
    },
    ReadHistoryVersion {
        hash: String,
    },
    WriteSession(Session),
    McpReadFiles {
        id: u64,
        call: super::mcp::McpCall,
        paths: Vec<PathBuf>,
    },
    McpReply {
        id: u64,
        result: Result<String, String>,
    },
    OpenClaude {
        root: PathBuf,
    },
    Terminal(super::terminal::TerminalEffect),
    ClaudeStart {
        root: PathBuf,
        rows: u16,
        columns: u16,
    },
    ClaudeWrite {
        bytes: Vec<u8>,
    },
    ClaudeResize {
        rows: u16,
        columns: u16,
    },
    IdeNotify {
        method: String,
        params: serde_json::Value,
    },
    FormatWithTool {
        doc: DocumentId,
        version: u64,
        path: PathBuf,
        root: PathBuf,
        text: String,
    },
    SystemNotify {
        title: String,
        text: String,
    },
    SpellCheck {
        path: PathBuf,
        text: String,
        language: crate::lang::languages::Language,
    },
    SpellAddWord {
        word: String,
    },
    ReadDependencies {
        root: PathBuf,
    },
    OpenImage {
        root: PathBuf,
        request: super::image_art::ImageRequest,
    },
    OpenProject {
        dir: PathBuf,
    },
    Analysis(super::analysis::AnalysisEffect),
    Http(super::http::HttpEffect),
    Db(super::db::DbEffect),
    Cli(super::cli::CliEffect),
    LoadGrammar {
        language: crate::lang::languages::Language,
    },
    LoadPlugins,
    RunPlugin {
        plugin: String,
        command: String,
        root: PathBuf,
        context: super::plugin::PluginContext,
    },
    ScratchCreate {
        extension: String,
    },
    ScratchList,
    SaveRunHistory {
        commands: Vec<String>,
    },
    SaveTrust {
        root: PathBuf,
        trusted: bool,
    },
    FileOp(super::file_op::FileOp),
    ListDir {
        path: PathBuf,
    },
    IndexProject {
        root: PathBuf,
    },
    RescanIndex,
    QuerySymbols {
        id: u64,
        query: String,
        limit: usize,
    },
    FileSymbols {
        path: PathBuf,
        version: u64,
        language: Language,
        text: String,
    },
    IndexFiles {
        root: PathBuf,
    },
    SearchProject {
        id: u64,
        root: PathBuf,
        query: String,
        options: FindOptions,
    },
    PlanProjectReplace {
        id: u64,
        files: Vec<PathBuf>,
        query: String,
        options: FindOptions,
        replacement: String,
    },
    ApplyProjectReplace {
        id: u64,
        files: Vec<(PathBuf, Option<DiskState>)>,
        query: String,
        options: FindOptions,
        replacement: String,
    },
}

impl Effect {
    pub const fn needs_trust(&self) -> bool {
        match self {
            Self::Run(effect) => effect.needs_trust(),
            Self::Debug(effect) => effect.needs_trust(),
            Self::Git(effect) => effect.needs_trust(),
            Self::Cli(effect) => effect.needs_trust(),
            Self::Http(effect) => effect.needs_trust(),
            Self::Analysis(effect) => effect.needs_trust(),
            Self::Terminal(effect) => effect.needs_trust(),
            Self::Lsp(effect) => effect.needs_trust(),
            Self::OpenGitWindow { .. } | Self::FormatWithTool { .. } => true,
            Self::Coverage(..)
            | Self::ReadDependencies { .. }
            | Self::SpellCheck { .. }
            | Self::SystemNotify { .. }
            | Self::SpellAddWord { .. }
            | Self::OpenImage { .. }
            | Self::DecodeImage { .. }
            | Self::Duplicates(..)
            | Self::ReadFile { .. }
            | Self::WriteFile { .. }
            | Self::CheckFile { .. }
            | Self::ReloadFile { .. }
            | Self::WatchProject { .. }
            | Self::WriteSwap { .. }
            | Self::RemoveSwap { .. }
            | Self::JournalAppend { .. }
            | Self::RemoveJournal { .. }
            | Self::RecoverSwaps
            | Self::LoadSession
            | Self::OpenInMainWindow { .. }
            | Self::OpenRunWindow { .. }
            | Self::RunFollow { .. }
            | Self::RunStreamSend(..)
            | Self::RunStreamStart(..)
            | Self::WriteClipboard { .. }
            | Self::ReadClipboard
            | Self::Snapshot { .. }
            | Self::ListHistory { .. }
            | Self::ReadHistoryVersion { .. }
            | Self::WriteSession(..)
            | Self::McpReadFiles { .. }
            | Self::McpReply { .. }
            | Self::OpenClaude { .. }
            | Self::ClaudeStart { .. }
            | Self::ClaudeWrite { .. }
            | Self::ClaudeResize { .. }
            | Self::IdeNotify { .. }
            | Self::OpenProject { .. }
            | Self::Db(..)
            | Self::LoadGrammar { .. }
            | Self::LoadPlugins
            | Self::RunPlugin { .. }
            | Self::ScratchCreate { .. }
            | Self::ScratchList
            | Self::SaveRunHistory { .. }
            | Self::SaveTrust { .. }
            | Self::FileOp(..)
            | Self::ListDir { .. }
            | Self::IndexProject { .. }
            | Self::RescanIndex
            | Self::QuerySymbols { .. }
            | Self::FileSymbols { .. }
            | Self::IndexFiles { .. }
            | Self::SearchProject { .. }
            | Self::PlanProjectReplace { .. }
            | Self::ApplyProjectReplace { .. } => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Event {
    Cli(super::cli::CliEvent),
    Db(super::db::DbEvent),
    Http(super::http::HttpEvent),
    Analysis(super::analysis::AnalysisEvent),
    GrammarLoaded {
        language: crate::lang::languages::Language,
        error: Option<String>,
    },
    PluginsLoaded {
        plugins: Vec<super::plugin::PluginInfo>,
        problems: Vec<String>,
    },
    PluginInvoked {
        plugin: String,
        command: String,
        context: super::plugin::PluginContext,
    },
    PluginDone {
        plugin: String,
        result: Result<super::plugin::PluginReport, String>,
    },
    FileRead {
        path: PathBuf,
        at: Option<(usize, usize)>,
        disk: Option<DiskState>,
        result: Result<Option<Vec<u8>>, String>,
    },
    FileWritten {
        doc: DocumentId,
        version: u64,
        disk: Option<DiskState>,
        origin: WriteOrigin,
    },
    WriteFailed {
        doc: DocumentId,
        version: u64,
        message: String,
        origin: WriteOrigin,
    },
    WriteConflict {
        doc: DocumentId,
        disk: Option<DiskState>,
        origin: WriteOrigin,
    },
    FileChanged {
        doc: DocumentId,
        disk: Option<DiskState>,
        result: Result<Option<Vec<u8>>, String>,
    },
    FileReloaded {
        doc: DocumentId,
        disk: Option<DiskState>,
        result: Result<Option<Vec<u8>>, String>,
    },
    PathsChanged {
        paths: Vec<PathBuf>,
    },
    McpCalled {
        id: u64,
        tool: String,
        args: serde_json::Value,
    },
    McpFilesRead {
        id: u64,
        call: super::mcp::McpCall,
        disk: std::collections::BTreeMap<PathBuf, Result<String, String>>,
    },
    Terminal(super::terminal::TerminalEvent),
    ClaudeOutput {
        bytes: Vec<u8>,
    },
    ClaudeEnded {
        state: super::claude::ClaudeState,
    },
    IdeConnected,
    IdeDisconnected,
    ClaudeOpened {
        result: Result<(), String>,
    },
    Formatted {
        doc: DocumentId,
        version: u64,
        result: Result<String, String>,
    },
    ProjectOpened {
        result: Result<(), String>,
    },
    ImageOpened {
        result: Result<(), String>,
    },
    SpellChecked {
        result: Result<Vec<super::find::SearchMatch>, String>,
    },
    WordAdded {
        word: String,
        result: Result<(), String>,
    },
    DependenciesRead {
        dependencies: Vec<super::deps::Dependency>,
        problems: Vec<String>,
    },
    FileOpDone {
        op: super::file_op::FileOp,
        result: Result<(), String>,
    },
    WatchFailed {
        message: String,
    },
    SwapsFound {
        swaps: Vec<RecoveredSwap>,
    },
    SwapFailed {
        message: String,
    },
    SessionLoaded {
        session: Session,
        files: Vec<SessionFile>,
    },
    SessionSaveFailed {
        message: String,
    },
    HistoryListed {
        doc: DocumentId,
        entries: Vec<HistoryEntry>,
        now: u64,
    },
    HistoryVersion {
        hash: String,
        result: Result<Vec<u8>, String>,
    },
    HistoryFailed {
        message: String,
    },
    Lsp(super::lsp_command::LspEvent),
    Git(super::git_command::GitEvent),
    ExternalOpen {
        path: PathBuf,
        at: Option<(usize, usize)>,
    },
    RunWatcherAttached,
    RunStream(RunStreamMessage),
    RunStreamState {
        connected: bool,
    },
    RunWindowOpened {
        result: Result<(), String>,
    },
    OpenedInMainWindow {
        result: Result<(), String>,
    },
    GitWindowOpened {
        result: Result<(), String>,
    },
    Duplicates(super::duplicates::DuplicatesEvent),
    ImageDecoded {
        request: super::image_art::ImageRequest,
        result: Result<super::image_art::AsciiImage, String>,
    },
    Coverage(super::coverage::CoverageEvent),
    ClipboardRead {
        text: Option<String>,
    },
    ScratchCreated {
        result: Result<PathBuf, String>,
    },
    ScratchesListed {
        paths: Vec<PathBuf>,
    },
    ConfigProblems {
        messages: Vec<String>,
    },
    Run(super::run_state::RunEvent),
    Debug(super::debug::DebugEvent),
    DirListed {
        path: PathBuf,
        result: Result<Vec<DirEntry>, String>,
    },
    IndexProgress {
        done: usize,
        total: usize,
    },
    SymbolsFound {
        id: u64,
        symbols: Vec<ProjectSymbol>,
    },
    FileSymbolsRead {
        path: PathBuf,
        version: u64,
        symbols: Vec<LspSymbol>,
    },
    FilesIndexed {
        files: Vec<PathBuf>,
    },
    SearchFinished {
        id: u64,
        matches: Vec<SearchMatch>,
        truncated: bool,
    },
    ProjectReplacePlanned {
        id: u64,
        files: Vec<ClosedPlan>,
        failed: Vec<FileProblem>,
    },
    ProjectReplaceApplied {
        id: u64,
        changed: Vec<PathBuf>,
        skipped: Vec<FileProblem>,
        failed: Vec<FileProblem>,
    },
}
