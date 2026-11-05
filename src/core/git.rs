use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use similar::{DiffTag, TextDiff};

use super::input::LineInput;
use super::local_history::{DiffKind, DiffRow};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitChange {
    Added,
    Modified,
    Deleted,
    Renamed,
    Untracked,
    Conflicted,
}

impl GitFile {
    pub fn new(path: PathBuf, change: GitChange) -> Self {
        Self {
            path,
            old_path: None,
            change,
            code: GitCode::default(),
        }
    }
}

impl GitChange {
    pub fn letter(self) -> char {
        match self {
            Self::Added => 'A',
            Self::Modified => 'M',
            Self::Deleted => 'D',
            Self::Renamed => 'R',
            Self::Untracked => '?',
            Self::Conflicted => '!',
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCode {
    pub index: char,
    pub worktree: char,
}

impl GitCode {
    pub fn new(index: char, worktree: char) -> Self {
        Self { index, worktree }
    }

    fn is_change(letter: char) -> bool {
        !matches!(letter, ' ' | '?' | '\0')
    }

    pub fn conflicted(self) -> bool {
        self.index == 'U'
            || self.worktree == 'U'
            || (self.index == self.worktree && matches!(self.index, 'A' | 'D'))
    }

    pub fn staged(self) -> bool {
        !self.conflicted() && Self::is_change(self.index)
    }

    pub fn unstaged(self) -> bool {
        !self.conflicted() && (Self::is_change(self.worktree) || self.worktree == '?')
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitFile {
    pub path: PathBuf,
    pub old_path: Option<PathBuf>,
    pub change: GitChange,
    pub code: GitCode,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitRefKind {
    Head,
    Local,
    Remote,
    Tag,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRef {
    pub name: String,
    pub kind: GitRefKind,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCommit {
    pub hash: String,
    pub parents: Vec<String>,
    pub author: String,
    pub time: u64,
    pub refs: Vec<GitRef>,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitDiffSource {
    Stash(String),
    Commit {
        hash: String,
        file: GitFile,
    },
    WholeCommit(String),
    Compare {
        base: String,
        worktree: bool,
        file: GitFile,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitCompareCommit {
    pub commit: GitCommit,
    pub ours: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, Default)]
pub struct GitCompared {
    pub commits: Vec<GitCompareCommit>,
    pub files: Vec<GitFile>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitCompare {
    pub base: String,
    pub path: Option<PathBuf>,
    pub found: GitCompared,
    pub selected: usize,
    pub loading: bool,
}

impl GitCompare {
    pub fn row_count(&self) -> usize {
        self.found.commits.len() + self.found.files.len()
    }

    pub fn selected_commit(&self) -> Option<&GitCompareCommit> {
        self.found.commits.get(self.selected)
    }

    pub fn selected_file(&self) -> Option<&GitFile> {
        self.found
            .files
            .get(self.selected.checked_sub(self.found.commits.len())?)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct Changelists {
    lists: BTreeMap<String, BTreeSet<PathBuf>>,
}

impl Changelists {
    pub fn from_saved(saved: BTreeMap<String, BTreeSet<PathBuf>>) -> Self {
        Self { lists: saved }
    }

    pub fn saved(&self) -> &BTreeMap<String, BTreeSet<PathBuf>> {
        &self.lists
    }

    pub fn names(&self) -> impl Iterator<Item = &str> {
        self.lists.keys().map(String::as_str)
    }

    pub fn name_of(&self, path: &Path) -> Option<&str> {
        self.lists
            .iter()
            .find(|(_, files)| files.contains(path))
            .map(|(name, _)| name.as_str())
    }

    pub fn files_of(&self, name: &str) -> Option<&BTreeSet<PathBuf>> {
        self.lists.get(name)
    }

    pub fn assign(&mut self, path: &Path, name: Option<&str>) {
        for files in self.lists.values_mut() {
            files.remove(path);
        }
        if let Some(name) = name {
            self.lists
                .entry(name.to_owned())
                .or_default()
                .insert(path.to_path_buf());
        }
        self.lists.retain(|_, files| !files.is_empty());
    }

    pub fn keep_only(&mut self, changed: &[GitFile]) {
        for files in self.lists.values_mut() {
            files.retain(|path| changed.iter().any(|f| f.path == *path));
        }
        self.lists.retain(|_, files| !files.is_empty());
    }

    pub fn next_after(&self, current: Option<&str>) -> Option<&str> {
        let after = current.and_then(|c| self.names().find(|n| *n > c));
        after.or_else(|| self.names().next())
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct CommitChecks {
    pub todo: bool,
    pub problems: bool,
    pub command: Option<String>,
}

impl CommitChecks {
    pub const fn any(&self) -> bool {
        self.todo || self.problems || self.command.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitRemoteOp {
    Push,
    Pull,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBranch {
    pub name: String,
    pub remote: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LineMark {
    Added,
    Modified,
    DeletedBelow,
}

pub fn line_marks(base: &str, text: &str) -> Vec<Option<LineMark>> {
    let diff = TextDiff::from_lines(base, text);
    let lines = text.split_inclusive('\n').count().max(1);
    let mut marks = vec![None; lines + 1];
    for op in diff.ops() {
        let new = op.new_range();
        match op.tag() {
            DiffTag::Equal => {}
            DiffTag::Insert | DiffTag::Replace => {
                let mark = if op.tag() == DiffTag::Insert {
                    LineMark::Added
                } else {
                    LineMark::Modified
                };
                for line in new {
                    marks[line] = Some(mark);
                }
            }
            DiffTag::Delete => {
                let line = new.start.saturating_sub(1);
                if marks[line].is_none() {
                    marks[line] = Some(LineMark::DeletedBelow);
                }
            }
        }
    }
    marks
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GraphCell {
    Empty,
    Line,
    Commit,
    Join,
    Fork,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphRow {
    pub cells: Vec<GraphCell>,
    pub span: Option<(usize, usize)>,
}

pub fn commit_graph(commits: &[GitCommit]) -> Vec<GraphRow> {
    let mut lanes: Vec<Option<&str>> = Vec::new();
    let mut rows = Vec::with_capacity(commits.len());
    for commit in commits {
        let hash = commit.hash.as_str();
        let column = lanes
            .iter()
            .position(|l| *l == Some(hash))
            .or_else(|| lanes.iter().position(Option::is_none))
            .unwrap_or_else(|| {
                lanes.push(None);
                lanes.len() - 1
            });
        let mut cells: Vec<GraphCell> = lanes
            .iter()
            .map(|l| {
                if *l == Some(hash) {
                    GraphCell::Join
                } else if l.is_some() {
                    GraphCell::Line
                } else {
                    GraphCell::Empty
                }
            })
            .collect();
        cells[column] = GraphCell::Commit;
        for (cell, lane) in cells.iter().zip(lanes.iter_mut()) {
            if *cell == GraphCell::Join {
                *lane = None;
            }
        }
        let mut parents = commit.parents.iter().map(String::as_str);
        lanes[column] = parents.next();
        for parent in parents {
            if lanes.contains(&Some(parent)) {
                continue;
            }
            let free = lanes.iter().position(Option::is_none).unwrap_or_else(|| {
                lanes.push(None);
                cells.push(GraphCell::Empty);
                lanes.len() - 1
            });
            lanes[free] = Some(parent);
            cells[free] = GraphCell::Fork;
        }
        while lanes.last() == Some(&None) {
            lanes.pop();
        }
        let ends = cells
            .iter()
            .enumerate()
            .filter(|(_, c)| matches!(c, GraphCell::Join | GraphCell::Fork))
            .map(|(i, _)| i);
        let low = ends.clone().min().map_or(column, |i| i.min(column));
        let high = ends.max().map_or(column, |i| i.max(column));
        rows.push(GraphRow {
            cells,
            span: (low != high).then_some((low, high)),
        });
    }
    rows
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitTab {
    Commit,
    Log,
    History,
    Stash,
    Branches,
    Compare,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitFileCommit {
    pub commit: GitCommit,
    pub file: GitFile,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum HunkSide {
    Unstaged,
    Staged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitHunk {
    pub side: HunkSide,
    pub whole: bool,
    pub patch: Vec<u8>,
    pub title: String,
    pub rows: Vec<DiffRow>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LinePick {
    pub cursor: usize,
    pub picked: BTreeSet<usize>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CommitMode {
    Ticked,
    Staged,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitOperation {
    Merging,
    Rebasing,
    CherryPicking,
    Reverting,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitStash {
    pub index: usize,
    pub hash: String,
    pub time: u64,
    pub subject: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum GitOp {
    Continue(GitOperation),
    Abort(GitOperation),
    StashPush { message: String, untracked: bool },
    StashApply(GitStash),
    StashPop(GitStash),
    StashDrop(GitStash),
    BranchDelete { name: String, force: bool },
    Merge { branch: String },
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitRepoState {
    pub operation: Option<GitOperation>,
    pub staged_id: String,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SideRow {
    Across(usize),
    Pair(Option<usize>, Option<usize>),
}

impl SideRow {
    pub fn last_index(self) -> usize {
        match self {
            Self::Across(i) => i,
            Self::Pair(left, right) => left.max(right).unwrap_or(0),
        }
    }
}

pub fn side_by_side(rows: &[DiffRow]) -> Vec<SideRow> {
    let mut out = Vec::with_capacity(rows.len());
    let mut at = 0;
    while let Some(row) = rows.get(at) {
        match row.kind {
            DiffKind::Same => {
                out.push(SideRow::Pair(Some(at), Some(at)));
                at += 1;
            }
            DiffKind::Gap | DiffKind::Section | DiffKind::Hunk => {
                out.push(SideRow::Across(at));
                at += 1;
            }
            DiffKind::Removed | DiffKind::Added => {
                let run = |kind: DiffKind, from: usize| {
                    rows[from..].iter().take_while(|r| r.kind == kind).count()
                };
                let removed = run(DiffKind::Removed, at);
                let added = run(DiffKind::Added, at + removed);
                for n in 0..removed.max(added) {
                    out.push(SideRow::Pair(
                        (n < removed).then_some(at + n),
                        (n < added).then_some(at + removed + n),
                    ));
                }
                at += removed + added;
            }
        }
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBlameCommit {
    pub hash: String,
    pub author: String,
    pub time: u64,
}

impl GitBlameCommit {
    pub fn is_uncommitted(&self) -> bool {
        self.hash.bytes().all(|b| b == b'0')
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct GitBlame {
    pub commits: Vec<GitBlameCommit>,
    pub lines: Vec<usize>,
}

impl GitBlame {
    pub fn line(&self, line: usize) -> Option<&GitBlameCommit> {
        self.commits.get(*self.lines.get(line)?)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GitField {
    List,
    Message,
    CommitFiles,
    Filter,
    Hunks,
    HunkLines,
    StashMessage,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GitNotice {
    pub text: String,
    pub error: bool,
}

#[derive(Debug, Clone)]
pub struct GitPanel {
    pub tab: GitTab,
    pub field: GitField,
    pub unavailable: Option<String>,

    pub files: Vec<GitFile>,
    pub file_selected: usize,
    pub checked: BTreeSet<PathBuf>,
    pub message: LineInput,
    pub amend: bool,
    pub confirm_rollback: Option<PathBuf>,

    pub commits: Vec<GitCommit>,
    pub graph: Vec<GraphRow>,
    pub filter: LineInput,
    pub visible: Vec<usize>,
    pub commit_selected: usize,
    pub commit_files: Vec<GitFile>,
    pub commit_loading: Option<String>,
    pub commit_shown: Option<String>,
    pub commit_file_selected: usize,

    pub diff: Vec<DiffRow>,
    pub diff_scroll: usize,
    pub diff_request: Option<u64>,

    pub hunks: Vec<GitHunk>,
    pub hunk_starts: Vec<usize>,
    pub line_pick: Option<LinePick>,
    pub hunk_selected: usize,
    pub mode_choice: Option<CommitMode>,
    pub repo: GitRepoState,

    pub confirm_abort: bool,
    pub confirm_resolve: Option<PathBuf>,
    pub confirm_drop: Option<String>,
    pub confirm_force: Option<String>,

    pub stashes: Vec<GitStash>,
    pub stash_selected: usize,
    pub stash_message: LineInput,
    pub stash_untracked: bool,
    pub branch_selected: usize,

    pub history_path: Option<PathBuf>,
    pub history: Vec<GitFileCommit>,
    pub history_selected: usize,
    pub history_loading: bool,

    pub compare: Option<GitCompare>,
    pub compare_asked: Option<Option<PathBuf>>,
    pub compare_target: Option<PathBuf>,
    pub changelist_ticked: Option<String>,
    pub checks_passed: bool,
    pub confirm_checks: bool,

    pub busy: bool,
    pub notice: Option<GitNotice>,
    pub now: u64,
    pub side_by_side: bool,
}

impl GitPanel {
    pub fn new(tab: GitTab) -> Self {
        Self {
            tab,
            field: GitField::List,
            unavailable: None,
            files: Vec::new(),
            file_selected: 0,
            checked: BTreeSet::new(),
            message: LineInput::default(),
            amend: false,
            confirm_rollback: None,
            compare: None,
            compare_asked: None,
            compare_target: None,
            changelist_ticked: None,
            checks_passed: false,
            confirm_checks: false,
            commits: Vec::new(),
            graph: Vec::new(),
            filter: LineInput::default(),
            visible: Vec::new(),
            commit_selected: 0,
            commit_files: Vec::new(),
            commit_loading: None,
            commit_shown: None,
            commit_file_selected: 0,
            diff: Vec::new(),
            diff_scroll: 0,
            diff_request: None,
            hunks: Vec::new(),
            hunk_starts: Vec::new(),
            line_pick: None,
            hunk_selected: 0,
            mode_choice: None,
            repo: GitRepoState::default(),
            confirm_abort: false,
            confirm_resolve: None,
            confirm_drop: None,
            confirm_force: None,
            stashes: Vec::new(),
            stash_selected: 0,
            stash_message: LineInput::default(),
            stash_untracked: false,
            branch_selected: 0,
            history_path: None,
            history: Vec::new(),
            history_selected: 0,
            history_loading: false,
            busy: false,
            notice: None,
            now: 0,
            side_by_side: true,
        }
    }

    pub fn selected_commit(&self) -> Option<&GitCommit> {
        self.commits.get(*self.visible.get(self.commit_selected)?)
    }

    pub fn set_files(&mut self, files: Vec<GitFile>) {
        let known: BTreeSet<&PathBuf> = self.files.iter().map(|f| &f.path).collect();
        let fresh: Vec<PathBuf> = files
            .iter()
            .filter(|f| f.change != GitChange::Untracked && !known.contains(&f.path))
            .map(|f| f.path.clone())
            .collect();
        self.checked
            .retain(|path| files.iter().any(|f| f.path == *path));
        self.checked.extend(fresh);
        self.files = files;
        self.file_selected = self.file_selected.min(self.files.len().saturating_sub(1));
    }

    pub fn set_commits(&mut self, commits: Vec<GitCommit>) {
        self.graph = commit_graph(&commits);
        self.commits = commits;
        self.refilter();
    }

    pub fn refilter(&mut self) {
        let query = self.filter.text().to_lowercase();
        self.visible = self
            .commits
            .iter()
            .enumerate()
            .filter(|(_, c)| {
                query.is_empty()
                    || c.subject.to_lowercase().contains(&query)
                    || c.author.to_lowercase().contains(&query)
                    || c.hash.starts_with(&query)
                    || c.refs
                        .iter()
                        .any(|r| r.name.to_lowercase().contains(&query))
            })
            .map(|(i, _)| i)
            .collect();
        self.commit_selected = self
            .commit_selected
            .min(self.visible.len().saturating_sub(1));
    }

    pub fn clear_diff(&mut self) {
        self.diff.clear();
        self.diff_scroll = 0;
        self.hunks.clear();
        self.hunk_starts.clear();
        self.hunk_selected = 0;
        self.line_pick = None;
        if self.field == GitField::HunkLines {
            self.field = GitField::Hunks;
        }
    }

    pub fn selected_file(&self) -> Option<&GitFile> {
        self.files.get(self.file_selected)
    }

    pub fn commit_mode(&self) -> CommitMode {
        let conflicted = self.files.iter().any(|f| f.change == GitChange::Conflicted);
        if self.repo.operation.is_some() || conflicted {
            return CommitMode::Staged;
        }
        self.mode_choice.unwrap_or_else(|| {
            if self.files.iter().any(|f| f.code.staged()) {
                CommitMode::Staged
            } else {
                CommitMode::Ticked
            }
        })
    }

    pub fn set_hunks(&mut self, hunks: Vec<GitHunk>) {
        let mut rows = Vec::new();
        let mut starts = Vec::with_capacity(hunks.len());
        let label = |text: String, kind| DiffRow {
            kind,
            old_line: None,
            new_line: None,
            text,
        };
        for (side, name) in [
            (HunkSide::Unstaged, "Not staged"),
            (HunkSide::Staged, "Staged"),
        ] {
            let count = hunks.iter().filter(|h| h.side == side).count();
            if count == 0 {
                continue;
            }
            rows.push(label(format!("{name} ({count})"), DiffKind::Section));
            for hunk in hunks.iter().filter(|h| h.side == side) {
                starts.push(rows.len());
                rows.push(label(hunk.title.clone(), DiffKind::Hunk));
                rows.extend(hunk.rows.iter().cloned());
            }
        }
        let before = self.hunks.get(self.hunk_selected).map(|h| h.patch.clone());
        let (unstaged, staged): (Vec<_>, Vec<_>) = hunks
            .into_iter()
            .partition(|h| h.side == HunkSide::Unstaged);
        self.hunks = unstaged.into_iter().chain(staged).collect();
        self.hunk_starts = starts;
        self.hunk_selected = self.hunk_selected.min(self.hunks.len().saturating_sub(1));
        self.diff_scroll = self.diff_scroll.min(rows.len().saturating_sub(1));
        self.diff = rows;
        let now = self.hunks.get(self.hunk_selected).map(|h| &h.patch);
        if self.line_pick.is_some() && before.as_ref() != now {
            self.line_pick = None;
            self.field = GitField::Hunks;
        }
    }

    pub fn changed_lines(&self) -> usize {
        self.hunks.get(self.hunk_selected).map_or(0, |h| {
            h.rows
                .iter()
                .filter(|r| matches!(r.kind, DiffKind::Added | DiffKind::Removed))
                .count()
        })
    }

    pub fn changed_line_at(&self, row: usize) -> Option<usize> {
        let start = *self.hunk_starts.get(self.hunk_selected)?;
        let end = start + 1 + self.hunks.get(self.hunk_selected)?.rows.len();
        if row <= start || row >= end {
            return None;
        }
        let is_changed = |r: &DiffRow| matches!(r.kind, DiffKind::Added | DiffKind::Removed);
        if !is_changed(self.diff.get(row)?) {
            return None;
        }
        Some(
            self.diff[start + 1..row]
                .iter()
                .filter(|r| is_changed(r))
                .count(),
        )
    }

    pub fn show_line_cursor(&mut self) {
        let Some(cursor) = self.line_pick.as_ref().map(|p| p.cursor) else {
            return;
        };
        let found = (0..self.diff.len()).find(|n| self.changed_line_at(*n) == Some(cursor));
        if let Some(row) = found {
            self.diff_scroll = row.saturating_sub(2);
        }
    }

    pub fn select_hunk(&mut self, index: usize) {
        self.hunk_selected = index;
        if let Some(start) = self.hunk_starts.get(index) {
            self.diff_scroll = *start;
        }
    }
}

pub fn parse_hunk_header(line: &str) -> Option<(usize, usize)> {
    let mut parts = line.strip_prefix("@@ ")?.split(' ');
    let number = |part: &str| part.get(1..)?.split(',').next()?.parse::<usize>().ok();
    let old = number(parts.next()?)?;
    let new = number(parts.next()?)?;
    Some((old, new))
}

pub fn parse_unified(text: &str) -> Vec<DiffRow> {
    let row = |kind, old_line, new_line, text: &str| DiffRow {
        kind,
        old_line,
        new_line,
        text: text.trim_end_matches('\r').to_owned(),
    };
    let mut rows = Vec::new();
    let mut lines = (0, 0);
    let mut in_hunk = false;
    for line in text.lines() {
        if line.starts_with("Binary files ") && line.ends_with(" differ") {
            rows.push(row(DiffKind::Same, None, None, "Binary file"));
            continue;
        }
        if line.starts_with("@@ ") {
            if let Some(start) = parse_hunk_header(line) {
                if in_hunk {
                    rows.push(row(DiffKind::Gap, None, None, ""));
                }
                lines = start;
                in_hunk = true;
            }
            continue;
        }
        if !in_hunk {
            continue;
        }
        match line.as_bytes().first() {
            Some(b'-') => {
                rows.push(row(DiffKind::Removed, Some(lines.0), None, &line[1..]));
                lines.0 += 1;
            }
            Some(b'+') => {
                rows.push(row(DiffKind::Added, None, Some(lines.1), &line[1..]));
                lines.1 += 1;
            }
            Some(b' ') => {
                rows.push(row(
                    DiffKind::Same,
                    Some(lines.0),
                    Some(lines.1),
                    &line[1..],
                ));
                lines.0 += 1;
                lines.1 += 1;
            }
            Some(b'\\') => {}
            _ => in_hunk = line.is_empty() && in_hunk,
        }
    }
    rows
}

#[cfg(test)]
mod tests {
    use super::*;
    use GraphCell::{Commit, Empty, Fork, Join, Line};

    fn commit(hash: &str, parents: &[&str]) -> GitCommit {
        GitCommit {
            hash: hash.to_owned(),
            parents: parents.iter().map(|p| (*p).to_owned()).collect(),
            author: "A".to_owned(),
            time: 0,
            refs: Vec::new(),
            subject: format!("subject {hash}"),
        }
    }

    fn cells(commits: &[GitCommit]) -> Vec<Vec<GraphCell>> {
        commit_graph(commits).into_iter().map(|r| r.cells).collect()
    }

    fn row(kind: DiffKind) -> DiffRow {
        DiffRow {
            kind,
            old_line: None,
            new_line: None,
            text: String::new(),
        }
    }

    #[test]
    fn side_by_side_faces_removed_lines_with_added_ones() {
        use DiffKind::{Added, Gap, Hunk, Removed, Same};
        let rows: Vec<DiffRow> = [
            Same, Removed, Removed, Added, Hunk, Added, Gap, Removed, Same,
        ]
        .into_iter()
        .map(row)
        .collect();
        assert_eq!(
            side_by_side(&rows),
            [
                SideRow::Pair(Some(0), Some(0)),
                SideRow::Pair(Some(1), Some(3)),
                SideRow::Pair(Some(2), None),
                SideRow::Across(4),
                SideRow::Pair(None, Some(5)),
                SideRow::Across(6),
                SideRow::Pair(Some(7), None),
                SideRow::Pair(Some(8), Some(8)),
            ]
        );
        assert!(side_by_side(&[]).is_empty());
        assert_eq!(SideRow::Pair(Some(1), Some(3)).last_index(), 3);
        assert_eq!(SideRow::Pair(Some(2), None).last_index(), 2);
    }

    #[test]
    fn a_straight_history_stays_in_one_column() {
        let log = [commit("c", &["b"]), commit("b", &["a"]), commit("a", &[])];
        assert_eq!(cells(&log), [vec![Commit], vec![Commit], vec![Commit]]);
    }

    #[test]
    fn a_merge_forks_and_the_branch_comes_back() {
        let log = [
            commit("m", &["a", "b"]),
            commit("b", &["base"]),
            commit("a", &["base"]),
            commit("base", &[]),
        ];
        assert_eq!(
            cells(&log),
            [
                vec![Commit, Fork],
                vec![Line, Commit],
                vec![Commit, Line],
                vec![Commit, Join],
            ]
        );
        let rows = commit_graph(&log);
        assert_eq!(rows[0].span, Some((0, 1)));
        assert_eq!(rows[1].span, None);
        assert_eq!(rows[3].span, Some((0, 1)));
    }

    #[test]
    fn two_tips_take_two_columns_and_a_free_column_is_reused() {
        let log = [
            commit("x", &["base"]),
            commit("y", &["base"]),
            commit("base", &[]),
            commit("other", &[]),
        ];
        assert_eq!(
            cells(&log),
            [
                vec![Commit],
                vec![Line, Commit],
                vec![Commit, Join],
                vec![Commit],
            ]
        );
    }

    #[test]
    fn a_parent_another_column_already_waits_for_adds_no_column() {
        let log = [
            commit("x", &["p"]),
            commit("m", &["q", "p"]),
            commit("q", &["p"]),
            commit("p", &[]),
        ];
        let rows = cells(&log);
        assert_eq!(rows[1], [Line, Commit]);
        assert_eq!(rows[3], [Commit, Join]);
        assert!(!rows.iter().flatten().any(|c| *c == Empty));
    }

    #[test]
    fn commits_with_parents_outside_the_list_do_not_break_it() {
        let log = [commit("x", &["gone"]), commit("y", &["gone"])];
        assert_eq!(cells(&log).len(), 2);
    }

    fn marks(base: &str, text: &str) -> Vec<Option<LineMark>> {
        line_marks(base, text)
    }

    #[test]
    fn lines_are_marked_by_how_they_differ_from_the_commit() {
        use LineMark::{Added, DeletedBelow, Modified};
        let base = "a\nb\nc\n";
        assert!(marks(base, base).iter().all(Option::is_none));
        assert_eq!(marks(base, "a\nB\nc\n")[1], Some(Modified));
        let added = marks(base, "a\nx\nb\nc\n");
        assert_eq!(added[1], Some(Added));
        assert_eq!(added[0], None);
        let removed = marks(base, "a\nc\n");
        assert_eq!(removed[0], Some(DeletedBelow));
        assert_eq!(removed[1], None);
        assert_eq!(marks("a\n", "")[0], Some(DeletedBelow), "everything gone");
        assert_eq!(marks("", "a\n")[0], Some(Added));
    }

    fn file(path: &str, change: GitChange) -> GitFile {
        GitFile::new(PathBuf::from(path), change)
    }

    #[test]
    fn ticks_start_on_tracked_files_and_survive_a_refresh() {
        let mut panel = GitPanel::new(GitTab::Commit);
        panel.set_files(vec![
            file("a", GitChange::Modified),
            file("new", GitChange::Untracked),
        ]);
        assert_eq!(panel.checked, BTreeSet::from([PathBuf::from("a")]));
        panel.checked.clear();
        panel.checked.insert(PathBuf::from("new"));
        panel.set_files(vec![
            file("a", GitChange::Modified),
            file("new", GitChange::Untracked),
            file("b", GitChange::Deleted),
        ]);
        assert_eq!(
            panel.checked,
            BTreeSet::from([PathBuf::from("new"), PathBuf::from("b")])
        );
        panel.set_files(vec![file("b", GitChange::Deleted)]);
        assert_eq!(panel.checked, BTreeSet::from([PathBuf::from("b")]));
        assert_eq!(panel.file_selected, 0);
    }

    #[test]
    fn the_filter_looks_at_subject_author_hash_and_refs() {
        let mut panel = GitPanel::new(GitTab::Log);
        let mut tagged = commit("abc123", &[]);
        tagged.refs = vec![GitRef {
            name: "main".to_owned(),
            kind: GitRefKind::Head,
        }];
        tagged.author = "Daniil".to_owned();
        panel.set_commits(vec![commit("fff", &["abc123"]), tagged]);
        assert_eq!(panel.visible, [0, 1]);
        for (query, expected) in [
            ("SUBJECT FFF", vec![0]),
            ("daniil", vec![1]),
            ("abc", vec![1]),
            ("main", vec![1]),
            ("nothing", vec![]),
        ] {
            panel.filter = LineInput::default();
            panel.filter.insert(query);
            panel.refilter();
            assert_eq!(panel.visible, expected, "{query}");
        }
    }
}
