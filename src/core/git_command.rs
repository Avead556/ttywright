use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::git::{
    GitBlame, GitBranch, GitCommit, GitDiffSource, GitFile, GitFileCommit, GitHunk, GitOp,
    GitRemoteOp, GitRepoState, GitStash,
};
use super::local_history::DiffRow;
use super::workspace::DocumentId;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitEffect {
    Status {
        root: PathBuf,
    },
    Log {
        root: PathBuf,
    },
    CommitFiles {
        root: PathBuf,
        hash: String,
    },
    Diff {
        id: u64,
        root: PathBuf,
        source: GitDiffSource,
    },
    Commit {
        root: PathBuf,
        message: String,
        paths: Vec<PathBuf>,
        untracked: Vec<PathBuf>,
        amend: bool,
    },
    ReadLastCommit {
        root: PathBuf,
    },
    Rollback {
        root: PathBuf,
        file: GitFile,
        absolute: PathBuf,
    },
    ListBranches {
        root: PathBuf,
    },
    Switch {
        root: PathBuf,
        branch: GitBranch,
        create: bool,
    },
    Base {
        root: PathBuf,
        path: PathBuf,
    },
    Remote {
        root: PathBuf,
        op: GitRemoteOp,
    },
    Hunks {
        id: u64,
        root: PathBuf,
        file: GitFile,
    },
    ApplyHunk {
        root: PathBuf,
        file: GitFile,
        hunk: GitHunk,
        lines: Option<Vec<usize>>,
    },
    StageFiles {
        root: PathBuf,
        files: Vec<GitFile>,
        stage: bool,
    },
    CommitStaged {
        root: PathBuf,
        message: String,
        amend: bool,
        staged_id: String,
    },
    Op {
        root: PathBuf,
        op: GitOp,
        keep: Vec<PathBuf>,
    },
    ListStashes {
        root: PathBuf,
    },
    Compare {
        root: PathBuf,
        base: String,
        path: Option<PathBuf>,
    },
    CommitChecks {
        root: PathBuf,
        staged: bool,
        paths: Vec<PathBuf>,
        untracked: Vec<PathBuf>,
        todo: bool,
        command: Option<String>,
    },
    FileHistory {
        root: PathBuf,
        path: PathBuf,
    },
    Blame {
        root: PathBuf,
        doc: DocumentId,
        path: PathBuf,
        version: u64,
    },
}

impl GitEffect {
    pub const fn needs_trust(&self) -> bool {
        true
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitEvent {
    StatusRead {
        branch: Option<String>,
        top: Option<PathBuf>,
        files: Result<Vec<GitFile>, String>,
        state: GitRepoState,
    },
    LogRead {
        result: Result<Vec<GitCommit>, String>,
        now: u64,
    },
    CommitFilesRead {
        hash: String,
        result: Result<Vec<GitFile>, String>,
    },
    DiffRead {
        id: u64,
        result: Result<Vec<DiffRow>, String>,
    },
    Committed {
        result: Result<String, String>,
    },
    LastCommit {
        result: Result<(String, bool), String>,
    },
    RolledBack {
        path: PathBuf,
        result: Result<(), String>,
    },
    BranchesRead {
        result: Result<Vec<GitBranch>, String>,
    },
    Switched {
        name: String,
        result: Result<(), String>,
    },
    BaseRead {
        path: PathBuf,
        text: Option<String>,
    },
    RemoteDone {
        op: GitRemoteOp,
        result: Result<String, String>,
    },
    HunksRead {
        id: u64,
        result: Result<Vec<GitHunk>, String>,
    },
    StageDone {
        result: Result<(), String>,
    },
    OpDone {
        op: GitOp,
        result: Result<String, String>,
    },
    StashesRead {
        result: Result<Vec<GitStash>, String>,
        now: u64,
    },
    ComparedRead {
        base: String,
        path: Option<PathBuf>,
        result: Result<super::git::GitCompared, String>,
    },
    CommitChecked {
        warnings: Vec<String>,
    },
    FileHistoryRead {
        path: PathBuf,
        result: Result<Vec<GitFileCommit>, String>,
        now: u64,
    },
    BlameRead {
        doc: DocumentId,
        version: u64,
        result: Result<GitBlame, String>,
        now: u64,
    },
}
