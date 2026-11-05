mod action;
pub mod analysis;
mod change;
pub mod claude;
pub mod cli;
mod command;
pub mod conflict;
pub mod db;
pub mod debug;
mod debug_breakpoints;
pub mod debug_protocol;
pub mod deps;
pub mod docker;
mod docker_panel;
mod document;
pub mod file_op;
mod find;
mod git;
pub mod git_command;
mod git_repo;
pub mod github;
mod github_panel;
mod history;
pub mod http;
pub mod image_art;
mod input;
mod layout;
mod local_history;
mod lsp;
mod lsp_client;
pub mod lsp_command;
pub mod mcp;
pub mod menu;
pub mod merge;
pub mod notify;
mod picker;
pub mod plugin;
pub mod run;
pub mod run_location;
pub mod run_output;
pub mod run_state;
pub mod run_test;
pub mod scratch;
mod session;
pub mod spell;
pub mod templates;
pub mod terminal;
pub mod test_results;
pub mod text;
mod tree;
pub mod trigram;
mod workspace;
pub mod wrap;

pub use action::Action;
pub use change::{Assoc, ChangeSet, Operation};
pub use claude::ClaudeState;
pub use command::{Command, Effect, Event, Motion, RecoveredSwap, WriteGuard, WriteOrigin};
pub use document::{Conflict, DiskState, Document, DocumentError, Language, LineEnding};
pub use find::{
    ClosedPlan, Edit, FilePlan, FileProblem, FindBar, FindOptions, LineChange, PlanSource,
    ProjectSearch, Query, ReplacePreview, ReplaceReport, ReplaceStage, Replacer, SearchMatch,
    Template, check_query, merge_plan,
};
pub use git::{parse_hunk_header, parse_unified};
pub mod coverage;
mod duplicates;
pub use duplicates::{
    DuplicateFragment, DuplicateGroup, DuplicateRow, DuplicatesEffect, DuplicatesEvent,
    DuplicatesPanel, DuplicatesReport,
};
pub use git::{
    Changelists, CommitChecks, CommitMode, GitBlame, GitBlameCommit, GitBranch, GitChange, GitCode,
    GitCommit, GitCompare, GitCompareCommit, GitCompared, GitDiffSource, GitField, GitFile,
    GitFileCommit, GitHunk, GitNotice, GitOp, GitOperation, GitPanel, GitRef, GitRefKind,
    GitRemoteOp, GitRepoState, GitStash, GitTab, GraphCell, GraphRow, HunkSide, LineMark, SideRow,
    commit_graph, line_marks, side_by_side,
};
pub use input::LineInput;
pub use layout::{Axis, Layout, PaneId};
pub use local_history::{DiffKind, DiffRow, HistoryEntry, LocalHistoryPanel, SnapshotLabel};
pub use lsp::ProjectSymbol;
pub use lsp::{
    Completion, Diagnostic, Hover, HoverLine, LspAnswer, LspCodeAction, LspCompletion, LspFileEdit,
    LspLocation, LspPanel, LspQuery, LspServerOption, LspState, LspStatus, LspSymbol, LspTextEdit,
    LspUsage, PositionEncoding, Problem, RawDiagnostic, RefactorOffer, ServerChoice, ServerKey,
    Severity,
};
pub use picker::{Picker, PickerKind, PickerMatch};
pub use plugin::{PluginCommand, PluginContext, PluginInfo, PluginReport};
pub use run::{ConfiguredRun, RunTarget};
pub use run_output::{
    RunLine, RunLocation, RunOutcome, RunPanel, RunSnapshot, RunStatus, RunStreamMessage,
};
pub use session::{SavedLayout, SavedView, Session, SessionFile};
pub use tree::{DirEntry, ProjectTree, TreeRow};
pub use workspace::{
    BlameView, CoverageState, Cursor, DocumentId, Focus, Level, Pane, RunConfirm, StatusMessage,
    ToolTab, View, ViewId, Workspace,
};

#[cfg(test)]
mod debug_tests;
#[cfg(test)]
mod workspace_tests;
