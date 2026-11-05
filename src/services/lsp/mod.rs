pub mod bundled;
pub mod choice;
pub mod provision;
pub mod servers;
pub mod snippet;
pub mod transport;

use crate::core::lsp_command::LspEvent;
use std::collections::{BTreeMap, HashMap, HashSet, VecDeque};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::mpsc as std_mpsc;
use std::thread;
use std::time::{Duration, Instant};

use lsp_types::{
    CompletionTextEdit, GotoDefinitionResponse, HoverContents, InsertTextFormat, MarkedString,
    PublishDiagnosticsParams,
};
use serde_json::{Value, json};
use tokio::io::{AsyncBufReadExt, AsyncReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, Command};
use tokio::runtime::Runtime;
use tokio::sync::{mpsc, oneshot, watch};

use self::provision::Download;
use self::transport::{FrameDecoder, encode};
use crate::core::{
    Event, Language, LspAnswer, LspCodeAction, LspCompletion, LspFileEdit, LspLocation, LspQuery,
    LspState, LspSymbol, LspTextEdit, LspUsage, PositionEncoding, RawDiagnostic, RefactorOffer,
    ServerKey, Severity,
};

const SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(2);
const TOTAL_SHUTDOWN_TIMEOUT: Duration = Duration::from_secs(3);
const CRASH_WINDOW: Duration = Duration::from_secs(180);
const MAX_CRASHES: usize = 5;
const MAX_BAD_MESSAGES: usize = 10;
const MAX_BACKOFF_STEPS: u32 = 30;
const PROBE_TIMEOUT: Duration = Duration::from_secs(60);
const INITIALIZE_ID: i64 = 1;
const SHUTDOWN_ID: i64 = 2;
const QUERY_ID_BASE: i64 = 2;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerCommand {
    pub command: String,
    pub args: Vec<String>,
    pub initialization: Option<Value>,
}

#[derive(Debug, Clone)]
pub struct LspConfig {
    pub server: Language,
    pub command: String,
    pub args: Vec<String>,
    pub root: PathBuf,
    pub initialization: Option<Value>,
    pub candidates: Vec<ServerCommand>,
    pub search_path: Option<OsString>,
    pub name: Option<String>,
    pub backoff: Duration,
    pub initialize_timeout: Duration,
    pub query_timeout: Duration,
    pub check_first: bool,
    pub download: Option<Download>,
    pub bundled: Option<Bundled>,
    pub bundled_file: Option<PathBuf>,
}

#[derive(Debug, Clone)]
pub struct Bundled {
    pub compressed: &'static [u8],
    pub dir: PathBuf,
}

impl LspConfig {
    pub fn new(command: String, root: PathBuf) -> Self {
        Self {
            server: Language::Rust,
            command,
            args: Vec::new(),
            root,
            initialization: None,
            candidates: Vec::new(),
            search_path: None,
            name: None,
            backoff: Duration::from_secs(1),
            initialize_timeout: Duration::from_secs(60),
            query_timeout: Duration::from_secs(10),
            check_first: false,
            download: None,
            bundled: None,
            bundled_file: None,
        }
    }
}

#[derive(Debug, Clone)]
enum ToServer {
    DidOpen {
        path: PathBuf,
        language_id: String,
        version: u64,
        text: String,
    },
    DidChange {
        path: PathBuf,
        version: u64,
        text: String,
    },
    DidSave {
        path: PathBuf,
    },
    DidClose {
        path: PathBuf,
    },
    Query {
        id: u64,
        path: PathBuf,
        query: LspQuery,
    },
    Cancel {
        id: u64,
    },
    Restart,
    Shutdown,
}

pub struct LspHandle {
    to_server: mpsc::UnboundedSender<ToServer>,
    finished: Option<oneshot::Receiver<()>>,
    runtime: Option<Runtime>,
}

impl LspHandle {
    pub fn start(
        config: LspConfig,
        emit: impl Fn(Event) + Send + 'static,
    ) -> std::io::Result<Self> {
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(1)
            .enable_io()
            .enable_time()
            .build()?;
        let (sender, event_rx) = std_mpsc::channel();
        let events = Events {
            sender,
            server: ServerKey {
                server: config.server,
                root: config.root.clone(),
            },
        };
        thread::spawn(move || {
            for event in event_rx {
                emit(event);
            }
        });
        let (to_server, rx) = mpsc::unbounded_channel();
        let (done, finished) = oneshot::channel();
        runtime.spawn(async move {
            supervise(config, rx, events).await;
            let _ = done.send(());
        });
        Ok(Self {
            to_server,
            finished: Some(finished),
            runtime: Some(runtime),
        })
    }

    fn send(&self, message: ToServer) {
        let _ = self.to_server.send(message);
    }

    pub fn did_open(&self, path: PathBuf, language_id: &str, version: u64, text: String) {
        self.send(ToServer::DidOpen {
            path,
            language_id: language_id.to_owned(),
            version,
            text,
        });
    }

    pub fn did_change(&self, path: PathBuf, version: u64, text: String) {
        self.send(ToServer::DidChange {
            path,
            version,
            text,
        });
    }

    pub fn did_save(&self, path: PathBuf) {
        self.send(ToServer::DidSave { path });
    }

    pub fn did_close(&self, path: PathBuf) {
        self.send(ToServer::DidClose { path });
    }

    pub fn query(&self, id: u64, path: PathBuf, query: LspQuery) {
        self.send(ToServer::Query { id, path, query });
    }

    pub fn cancel(&self, id: u64) {
        self.send(ToServer::Cancel { id });
    }

    pub fn restart(&self) {
        self.send(ToServer::Restart);
    }
}

impl LspHandle {
    fn finish_by(&mut self, deadline: Instant) {
        if let (Some(runtime), Some(finished)) = (self.runtime.take(), self.finished.take()) {
            let left = deadline.saturating_duration_since(Instant::now());
            runtime.block_on(async {
                let _ = tokio::time::timeout(left, finished).await;
            });
            runtime.shutdown_timeout(Duration::from_millis(100));
        }
    }
}

impl Drop for LspHandle {
    fn drop(&mut self) {
        self.send(ToServer::Shutdown);
        self.finish_by(Instant::now() + TOTAL_SHUTDOWN_TIMEOUT);
    }
}

#[derive(Default)]
pub struct LspSessions {
    running: BTreeMap<ServerKey, LspHandle>,
}

impl LspSessions {
    pub fn insert(&mut self, key: ServerKey, handle: LspHandle) {
        self.running.insert(key, handle);
    }

    pub fn get(&self, key: &ServerKey) -> Option<&LspHandle> {
        self.running.get(key)
    }

    pub fn keys_of(&self, server: Language) -> Vec<ServerKey> {
        self.running
            .keys()
            .filter(|key| key.server == server)
            .cloned()
            .collect()
    }

    pub fn shut_down(&mut self, which: impl Fn(&ServerKey) -> bool) {
        let (mut stopping, kept): (BTreeMap<_, _>, BTreeMap<_, _>) =
            std::mem::take(&mut self.running)
                .into_iter()
                .partition(|(key, _)| which(key));
        self.running = kept;
        for handle in stopping.values() {
            handle.send(ToServer::Shutdown);
        }
        let deadline = Instant::now() + TOTAL_SHUTDOWN_TIMEOUT;
        for handle in stopping.values_mut() {
            handle.finish_by(deadline);
        }
    }
}

impl Drop for LspSessions {
    fn drop(&mut self) {
        self.shut_down(|_| true);
    }
}

#[derive(Clone)]
struct Events {
    sender: std_mpsc::Sender<Event>,
    server: ServerKey,
}

impl Events {
    fn send(&self, event: Event) {
        let _ = self.sender.send(event);
    }

    fn key(&self) -> ServerKey {
        self.server.clone()
    }
}

fn emit_state(events: &Events, state: LspState) {
    events.send(Event::Lsp(LspEvent::StateChanged {
        server: events.key(),
        state,
    }));
}

fn emit_name(events: &Events, name: String) {
    events.send(Event::Lsp(LspEvent::ServerNamed {
        server: events.key(),
        name,
    }));
}

pub fn find_on_path(command: &str, search_path: Option<&OsString>) -> Option<PathBuf> {
    let inherited = std::env::var_os("PATH");
    let search = search_path.or(inherited.as_ref())?;
    std::env::split_paths(search)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(command))
        .find(|path| is_executable(path))
}

#[cfg(unix)]
fn is_executable(path: &Path) -> bool {
    use std::os::unix::fs::PermissionsExt;
    std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

#[cfg(not(unix))]
fn is_executable(path: &Path) -> bool {
    path.is_file()
}

pub fn command_name(command: &str) -> String {
    Path::new(command)
        .file_name()
        .map_or_else(|| command.to_owned(), |n| n.to_string_lossy().into_owned())
}

fn choose_candidate(config: &mut LspConfig, events: &Events) -> bool {
    let found = config.candidates.iter().find_map(|candidate| {
        let path = find_on_path(&candidate.command, config.search_path.as_ref())?;
        Some((candidate.clone(), path))
    });
    let Some((candidate, path)) = found else {
        if let Some(first) = config.candidates.first() {
            emit_name(events, first.command.clone());
        }
        return false;
    };
    emit_name(events, candidate.command);
    config.command = path.to_string_lossy().into_owned();
    config.args = candidate.args;
    config.initialization = candidate.initialization;
    true
}

const LAST_WORDS_WAIT: Duration = Duration::from_millis(300);

enum SessionEnd {
    Shutdown,
    Restart,
    Crashed(String),
}

async fn supervise(
    mut config: LspConfig,
    mut rx: mpsc::UnboundedReceiver<ToServer>,
    events: Events,
) {
    let mut crashes: VecDeque<Instant> = VecDeque::new();
    if let Some(name) = config.name.clone() {
        emit_name(&events, name);
    }
    loop {
        emit_state(&events, LspState::Starting);
        if !config.candidates.is_empty() && !choose_candidate(&mut config, &events) {
            emit_state(&events, LspState::NotFound);
            match wait_for_restart(&mut rx).await {
                SessionEnd::Shutdown => return,
                _ => continue,
            }
        }
        let runs = if config.check_first {
            tokio::select! {
                runs = server_runs(&config.command, &config.root) => runs,
                end = wait_for_restart(&mut rx) => match end {
                    SessionEnd::Shutdown => return,
                    _ => continue,
                },
            }
        } else {
            true
        };
        if !runs {
            let found = tokio::select! {
                found = find_server(&config, &events) => found,
                end = wait_for_restart(&mut rx) => match end {
                    SessionEnd::Shutdown => return,
                    _ => continue,
                },
            };
            let Some(found) = found else {
                emit_state(&events, LspState::NotFound);
                match wait_for_restart(&mut rx).await {
                    SessionEnd::Shutdown => return,
                    _ => continue,
                }
            };
            config.command = found;
        }
        let mut command = Command::new(&config.command);
        command
            .args(&config.args)
            .current_dir(&config.root)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        let end = match command.spawn() {
            Ok(child) => run_session(child, &config, &mut rx, &events).await,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                emit_state(&events, LspState::NotFound);
                match wait_for_restart(&mut rx).await {
                    SessionEnd::Shutdown => return,
                    _ => continue,
                }
            }
            Err(e) => SessionEnd::Crashed(format!("could not start: {e}")),
        };
        match end {
            SessionEnd::Shutdown => return,
            SessionEnd::Restart => crashes.clear(),
            SessionEnd::Crashed(reason) => {
                events.send(Event::Lsp(LspEvent::Failed {
                    server: events.key(),
                    reason,
                }));
                let now = Instant::now();
                crashes.push_back(now);
                while crashes
                    .front()
                    .is_some_and(|t| now.duration_since(*t) > CRASH_WINDOW)
                {
                    crashes.pop_front();
                }
                if crashes.len() >= MAX_CRASHES {
                    emit_state(&events, LspState::Stopped);
                    match wait_for_restart(&mut rx).await {
                        SessionEnd::Shutdown => return,
                        _ => {
                            crashes.clear();
                            continue;
                        }
                    }
                }
                emit_state(&events, LspState::Crashed);
                let delay = backoff(config.backoff, crashes.len());
                tokio::select! {
                    () = tokio::time::sleep(delay) => {}
                    end = wait_for_restart(&mut rx) => {
                        if matches!(end, SessionEnd::Shutdown) {
                            return;
                        }
                    }
                }
            }
        }
    }
}

async fn find_server(config: &LspConfig, events: &Events) -> Option<String> {
    if let Some(download) = &config.download
        && let Some(found) =
            provision::fetch_server(download, &config.command, &config.root, events).await
    {
        return Some(found);
    }
    if let Some(file) = &config.bundled_file {
        let command = file.to_string_lossy().into_owned();
        if server_runs(&command, &config.root).await {
            return Some(command);
        }
    }
    let bundled = config.bundled.clone()?;
    unpack_bundled(bundled, &config.root).await
}

async fn unpack_bundled(bundled: Bundled, root: &Path) -> Option<String> {
    let Bundled { compressed, dir } = bundled;
    let unpacked = tokio::task::spawn_blocking(move || bundled::extract(compressed, &dir)).await;
    let command = unpacked.ok()?.ok()?.to_string_lossy().into_owned();
    server_runs(&command, root).await.then_some(command)
}

async fn server_runs(command: &str, root: &Path) -> bool {
    let mut probe = Command::new(command);
    probe
        .arg("--version")
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    matches!(
        tokio::time::timeout(PROBE_TIMEOUT, probe.status()).await,
        Ok(Ok(status)) if status.success()
    )
}

fn backoff(base: Duration, crashes: usize) -> Duration {
    let exponent = u32::try_from(crashes.saturating_sub(1))
        .unwrap_or(u32::MAX)
        .min(5);
    base * (1u32 << exponent).min(MAX_BACKOFF_STEPS)
}

async fn wait_for_restart(rx: &mut mpsc::UnboundedReceiver<ToServer>) -> SessionEnd {
    while let Some(message) = rx.recv().await {
        match message {
            ToServer::Restart => return SessionEnd::Restart,
            ToServer::Shutdown => return SessionEnd::Shutdown,
            _ => {}
        }
    }
    SessionEnd::Shutdown
}

fn watch_stderr(
    child: &mut Child,
) -> (watch::Receiver<String>, Option<tokio::task::JoinHandle<()>>) {
    let (last_line, seen) = watch::channel(String::new());
    let task = child.stderr.take().map(|stderr| {
        tokio::spawn(async move {
            let mut lines = BufReader::new(stderr);
            let mut line = Vec::new();
            while matches!(lines.read_until(b'\n', &mut line).await, Ok(n) if n > 0) {
                let text = String::from_utf8_lossy(&line);
                if !text.trim().is_empty() {
                    let _ = last_line.send(text.trim().to_owned());
                }
                line.clear();
            }
        })
    });
    (seen, task)
}

async fn run_session(
    mut child: Child,
    config: &LspConfig,
    rx: &mut mpsc::UnboundedReceiver<ToServer>,
    events: &Events,
) -> SessionEnd {
    let (last_line, stderr_task) = watch_stderr(&mut child);
    let end = session_body(&mut child, config, rx, events).await;
    let SessionEnd::Crashed(why) = end else {
        return end;
    };
    let waited = tokio::time::timeout(LAST_WORDS_WAIT, child.wait()).await;
    if let Some(task) = stderr_task {
        let _ = tokio::time::timeout(LAST_WORDS_WAIT, task).await;
    }
    let mut reason = why;
    if let Ok(Ok(status)) = waited {
        reason.push_str(&match status.code() {
            Some(code) => format!(", exited with code {code}"),
            None => ", killed by a signal".to_owned(),
        });
    }
    let said = last_line.borrow();
    if !said.is_empty() {
        reason.push_str(": ");
        reason.push_str(&said);
    }
    SessionEnd::Crashed(reason)
}

async fn session_body(
    child: &mut Child,
    config: &LspConfig,
    rx: &mut mpsc::UnboundedReceiver<ToServer>,
    events: &Events,
) -> SessionEnd {
    let (Some(mut stdin), Some(mut stdout)) = (child.stdin.take(), child.stdout.take()) else {
        return SessionEnd::Crashed("no pipes to it".to_owned());
    };
    let (incoming_tx, mut incoming) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut decoder = FrameDecoder::default();
        let mut chunk = vec![0; 64 * 1024];
        while let Ok(n) = stdout.read(&mut chunk).await {
            if n == 0 {
                break;
            }
            decoder.push(&chunk[..n]);
            while let Some(message) = decoder.next_message() {
                if incoming_tx.send(message).is_err() {
                    return;
                }
            }
        }
    });

    let initialize = initialize_request(&config.root, config.initialization.as_ref());
    if write(&mut stdin, &initialize).await.is_err() {
        return SessionEnd::Crashed("it stopped reading".to_owned());
    }
    let deadline = tokio::time::Instant::now() + config.initialize_timeout;
    let (encoding, completion_triggers) = loop {
        tokio::select! {
            () = tokio::time::sleep_until(deadline) => return SessionEnd::Crashed("no answer to initialize in time".to_owned()),
            message = rx.recv() => match message {
                Some(ToServer::Restart) => return SessionEnd::Restart,
                Some(ToServer::Shutdown) | None => return SessionEnd::Shutdown,
                Some(_) => {}
            },
            message = incoming.recv() => match message {
                None => return SessionEnd::Crashed("it closed its output before initializing".to_owned()),
                Some(Ok(value)) if value.get("id") == Some(&json!(INITIALIZE_ID)) => {
                    if let Some(error) = value.get("error") {
                        let text = error.get("message").and_then(Value::as_str);
                        let text = text.unwrap_or("no reason given");
                        return SessionEnd::Crashed(format!("it refused to initialize: {text}"));
                    }
                    break (negotiated_encoding(&value), completion_triggers(&value));
                }
                Some(Ok(value)) => {
                    let method = value.get("method").and_then(Value::as_str);
                    if let (Some(id), Some(method)) = (value.get("id"), method)
                        && write(&mut stdin, &reply_to_server(id, method, &value)).await.is_err()
                    {
                        return SessionEnd::Crashed("it stopped reading".to_owned());
                    }
                }
                Some(Err(_)) => {}
            },
        }
    };
    if write(&mut stdin, &notification("initialized", json!({})))
        .await
        .is_err()
    {
        return SessionEnd::Crashed("it stopped reading".to_owned());
    }
    events.send(Event::Lsp(LspEvent::Ready {
        server: events.key(),
        encoding,
        completion_triggers,
    }));

    let mut bad_messages = 0;
    let mut progress: HashSet<String> = HashSet::new();
    let mut queries: Queries = HashMap::new();
    loop {
        let next_deadline = queries.values().map(|q| q.deadline).min();
        let wake = next_deadline.unwrap_or_else(tokio::time::Instant::now);
        tokio::select! {
            () = tokio::time::sleep_until(wake), if next_deadline.is_some() => {
                let now = tokio::time::Instant::now();
                let expired: Vec<u64> = queries
                    .iter()
                    .filter(|(_, q)| q.deadline <= now)
                    .map(|(id, _)| *id)
                    .collect();
                for id in expired {
                    queries.remove(&id);
                    events.send(Event::Lsp(LspEvent::Answered {
                        server: events.key(),
                        id,
                        result: Err("no answer in time".to_owned()),
                    }));
                    if write(&mut stdin, &cancel_notification(id)).await.is_err() {
                        return SessionEnd::Crashed("it stopped reading".to_owned());
                    }
                }
            }
            message = rx.recv() => match message {
                Some(ToServer::Restart) => {
                    shut_down(child, &mut stdin, &mut incoming).await;
                    return SessionEnd::Restart;
                }
                Some(ToServer::Shutdown) | None => {
                    shut_down(child, &mut stdin, &mut incoming).await;
                    return SessionEnd::Shutdown;
                }
                Some(message) => {
                    let outgoing = match message {
                        ToServer::Query { id, path, query } => {
                            let deadline = tokio::time::Instant::now() + config.query_timeout;
                            let request = query_request(id, &path, query.clone());
                            queries.insert(id, WaitingQuery { query, deadline });
                            request
                        }
                        ToServer::Cancel { id } => {
                            queries.remove(&id);
                            cancel_notification(id)
                        }
                        other => document_notification(other),
                    };
                    if write(&mut stdin, &outgoing).await.is_err() {
                        return SessionEnd::Crashed("it stopped reading".to_owned());
                    }
                }
            },
            message = incoming.recv() => match message {
                None => return SessionEnd::Crashed("it closed its output".to_owned()),
                Some(Err(_)) => {
                    bad_messages += 1;
                    if bad_messages >= MAX_BAD_MESSAGES {
                        return SessionEnd::Crashed("too many unreadable messages".to_owned());
                    }
                }
                Some(Ok(value)) => {
                    bad_messages = 0;
                    if handle_incoming(&value, &mut stdin, events, &mut queries, &mut progress)
                        .await
                        .is_err()
                    {
                        return SessionEnd::Crashed("it stopped reading".to_owned());
                    }
                }
            },
        }
    }
}

struct WaitingQuery {
    query: LspQuery,
    deadline: tokio::time::Instant,
}

type Queries = HashMap<u64, WaitingQuery>;

async fn write(stdin: &mut ChildStdin, message: &Value) -> std::io::Result<()> {
    stdin.write_all(&encode(message)).await?;
    stdin.flush().await
}

fn notification(method: &str, params: Value) -> Value {
    json!({"jsonrpc": "2.0", "method": method, "params": params})
}

fn initialize_request(root: &Path, initialization: Option<&Value>) -> Value {
    let uri = path_to_uri(root);
    json!({
        "jsonrpc": "2.0",
        "id": INITIALIZE_ID,
        "method": "initialize",
        "params": {
            "processId": std::process::id(),
            "rootUri": uri,
            "workspaceFolders": [{"uri": uri, "name": "root"}],
            "initializationOptions": initialization,
            "capabilities": {
                "general": {"positionEncodings": ["utf-8", "utf-16"]},
                "window": {"workDoneProgress": true},
                "textDocument": {
                    "publishDiagnostics": {},
                    "definition": {"linkSupport": true},
                    "hover": {"contentFormat": ["markdown", "plaintext"]},
                    "completion": {"completionItem": {
                        "snippetSupport": false,
                        "resolveSupport": {"properties": ["additionalTextEdits"]},
                    }},
                    "documentSymbol": {"hierarchicalDocumentSymbolSupport": true},
                    "callHierarchy": {},
                    "references": {},
                    "rename": {},
                    "formatting": {},
                    "codeAction": {"codeActionLiteralSupport": {"codeActionKind": {"valueSet": [
                        "", "quickfix", "refactor", "refactor.extract", "refactor.inline",
                        "refactor.rewrite", "source", "source.organizeImports",
                    ]}}},
                    "synchronization": {"didSave": true},
                },
            },
        },
    })
}

fn negotiated_encoding(response: &Value) -> PositionEncoding {
    let name = response
        .pointer("/result/capabilities/positionEncoding")
        .and_then(Value::as_str);
    if name == Some("utf-8") {
        PositionEncoding::Utf8
    } else {
        PositionEncoding::Utf16
    }
}

fn completion_triggers(response: &Value) -> Vec<String> {
    response
        .pointer("/result/capabilities/completionProvider/triggerCharacters")
        .and_then(Value::as_array)
        .map_or_else(Vec::new, |list| {
            list.iter()
                .filter_map(Value::as_str)
                .map(str::to_owned)
                .collect()
        })
}

fn document_notification(message: ToServer) -> Value {
    match message {
        ToServer::DidOpen {
            path,
            language_id,
            version,
            text,
        } => notification(
            "textDocument/didOpen",
            json!({"textDocument": {
                "uri": path_to_uri(&path),
                "languageId": language_id,
                "version": version,
                "text": text,
            }}),
        ),
        ToServer::DidChange {
            path,
            version,
            text,
        } => notification(
            "textDocument/didChange",
            json!({
                "textDocument": {"uri": path_to_uri(&path), "version": version},
                "contentChanges": [{"text": text}],
            }),
        ),
        ToServer::DidSave { path } => notification(
            "textDocument/didSave",
            json!({"textDocument": {"uri": path_to_uri(&path)}}),
        ),
        ToServer::DidClose { path } => notification(
            "textDocument/didClose",
            json!({"textDocument": {"uri": path_to_uri(&path)}}),
        ),
        ToServer::Query { .. }
        | ToServer::Cancel { .. }
        | ToServer::Restart
        | ToServer::Shutdown => Value::Null,
    }
}

fn wire_id(id: u64) -> i64 {
    i64::try_from(id)
        .unwrap_or(i64::MAX)
        .saturating_add(QUERY_ID_BASE)
}

fn query_request(id: u64, path: &Path, query: LspQuery) -> Value {
    let document = json!({"uri": path_to_uri(path)});
    let at_cursor = |(line, character): (u32, u32)| json!({"textDocument": document, "position": {"line": line, "character": character}});
    let (method, params) = match query {
        LspQuery::Definition { at } => ("textDocument/definition", at_cursor(at)),
        LspQuery::Hover { at } => ("textDocument/hover", at_cursor(at)),
        LspQuery::Completion { at } => ("textDocument/completion", at_cursor(at)),
        LspQuery::References { at } => {
            let mut params = at_cursor(at);
            params["context"] = json!({"includeDeclaration": true});
            ("textDocument/references", params)
        }
        LspQuery::SafeDeleteUsages { at } => {
            let mut params = at_cursor(at);
            params["context"] = json!({"includeDeclaration": true});
            ("textDocument/references", params)
        }
        LspQuery::Refactorings { at, to } => {
            let place =
                |(line, character): (u32, u32)| json!({"line": line, "character": character});
            (
                "textDocument/codeAction",
                json!({
                    "textDocument": document,
                    "range": {"start": place(at), "end": place(to)},
                    "context": {"diagnostics": [], "only": ["refactor"]},
                }),
            )
        }
        LspQuery::CodeActions { at, to } => {
            let place =
                |(line, character): (u32, u32)| json!({"line": line, "character": character});
            (
                "textDocument/codeAction",
                json!({
                    "textDocument": document,
                    "range": {"start": place(at), "end": place(to)},
                    "context": {"diagnostics": []},
                }),
            )
        }
        LspQuery::Rename { at, new_name } => {
            let mut params = at_cursor(at);
            params["newName"] = json!(new_name);
            ("textDocument/rename", params)
        }
        LspQuery::Symbols => (
            "textDocument/documentSymbol",
            json!({"textDocument": document}),
        ),
        LspQuery::PrepareCalls { at, .. } => ("textDocument/prepareCallHierarchy", at_cursor(at)),
        LspQuery::Calls { item, outgoing } => (
            if outgoing {
                "callHierarchy/outgoingCalls"
            } else {
                "callHierarchy/incomingCalls"
            },
            json!({"item": raw_json(&item)}),
        ),
        LspQuery::ResolveCompletion { item } => ("completionItem/resolve", raw_json(&item)),
        LspQuery::Formatting { tab_size } => (
            "textDocument/formatting",
            json!({
                "textDocument": document,
                "options": {"tabSize": tab_size, "insertSpaces": true},
            }),
        ),
    };
    json!({"jsonrpc": "2.0", "id": wire_id(id), "method": method, "params": params})
}

fn cancel_notification(id: u64) -> Value {
    notification("$/cancelRequest", json!({"id": wire_id(id)}))
}

fn answered_query(message: &Value, queries: &mut Queries) -> Option<(u64, LspQuery)> {
    let id = message.get("id")?.as_i64()?.checked_sub(QUERY_ID_BASE)?;
    let id = u64::try_from(id).ok()?;
    queries.remove(&id).map(|q| (id, q.query))
}

fn raw_json(text: &str) -> Value {
    serde_json::from_str(text).unwrap_or(Value::Null)
}

fn position_of_value(value: &Value) -> Option<(u32, u32)> {
    let number = |key: &str| u32::try_from(value.get(key)?.as_u64()?).ok();
    Some((number("line")?, number("character")?))
}

const SYMBOL_KINDS: [&str; 27] = [
    "",
    "file",
    "module",
    "namespace",
    "package",
    "class",
    "method",
    "property",
    "field",
    "constructor",
    "enum",
    "interface",
    "function",
    "variable",
    "constant",
    "string",
    "number",
    "boolean",
    "array",
    "object",
    "key",
    "null",
    "enum member",
    "struct",
    "event",
    "operator",
    "type parameter",
];

fn read_symbols(result: &Value) -> Vec<LspSymbol> {
    fn walk(list: &[Value], depth: usize, out: &mut Vec<LspSymbol>) {
        for item in list {
            let start = item
                .pointer("/selectionRange/start")
                .or_else(|| item.pointer("/location/range/start"))
                .and_then(position_of_value);
            let (Some(name), Some(start)) = (item.get("name").and_then(Value::as_str), start)
            else {
                continue;
            };
            let kind = item
                .get("kind")
                .and_then(Value::as_u64)
                .and_then(|k| SYMBOL_KINDS.get(usize::try_from(k).ok()?))
                .copied()
                .unwrap_or("");
            out.push(LspSymbol {
                name: name.to_owned(),
                kind: kind.to_owned(),
                depth,
                start,
            });
            if let Some(children) = item.get("children").and_then(Value::as_array) {
                walk(children, depth + 1, out);
            }
        }
    }
    let mut out = Vec::new();
    if let Some(list) = result.as_array() {
        walk(list, 0, &mut out);
    }
    if result
        .as_array()
        .is_some_and(|l| l.iter().any(|i| i.get("location").is_some()))
    {
        out.sort_by_key(|s| s.start);
    }
    out
}

fn read_call_root(result: &Value) -> Option<(String, String)> {
    let item = result.as_array()?.first()?;
    Some((item.get("name")?.as_str()?.to_owned(), item.to_string()))
}

fn read_calls(result: &Value, outgoing: bool) -> Vec<LspLocation> {
    let mut places = Vec::new();
    for call in result.as_array().map_or(&[][..], Vec::as_slice) {
        let side = call.get(if outgoing { "to" } else { "from" });
        let Some(path) = side
            .and_then(|s| s.get("uri"))
            .and_then(Value::as_str)
            .and_then(uri_to_path)
        else {
            continue;
        };
        if outgoing {
            let start = side
                .and_then(|s| s.pointer("/selectionRange/start"))
                .and_then(position_of_value);
            places.extend(start.map(|start| LspLocation {
                path: path.clone(),
                start,
            }));
            continue;
        }
        for range in call
            .get("fromRanges")
            .and_then(Value::as_array)
            .map_or(&[][..], Vec::as_slice)
        {
            let start = range.get("start").and_then(position_of_value);
            places.extend(start.map(|start| LspLocation {
                path: path.clone(),
                start,
            }));
        }
    }
    places
}

fn read_answer(message: &Value, query: LspQuery) -> Result<LspAnswer, String> {
    if let Some(error) = message.get("error") {
        let text = error.get("message").and_then(Value::as_str);
        return Err(text.unwrap_or("request failed").to_owned());
    }
    let result = message.get("result").cloned().unwrap_or(Value::Null);
    match query {
        LspQuery::Definition { .. } => read_locations(result).map(LspAnswer::Locations),
        LspQuery::Hover { .. } => read_hover(result).map(LspAnswer::Hover),
        LspQuery::Completion { .. } => read_completion(result),
        LspQuery::References { .. } => read_locations(result).map(|locations| {
            LspAnswer::References(locations.into_iter().map(read_usage).collect())
        }),
        LspQuery::Formatting { .. } => read_text_edits(result).map(LspAnswer::Edits),
        LspQuery::CodeActions { .. } => Ok(LspAnswer::CodeActions(read_code_actions(&result))),
        LspQuery::Rename { .. } => Ok(read_rename(&result)),
        LspQuery::Refactorings { .. } => Ok(LspAnswer::Refactorings(read_refactorings(&result))),
        LspQuery::SafeDeleteUsages { .. } => read_locations(result).map(|locations| {
            LspAnswer::SafeDeleteUsages(locations.into_iter().map(read_usage).collect())
        }),
        LspQuery::Symbols => Ok(LspAnswer::Symbols(read_symbols(&result))),
        LspQuery::PrepareCalls { outgoing, .. } => Ok(LspAnswer::CallRoot {
            root: read_call_root(&result),
            outgoing,
        }),
        LspQuery::Calls { outgoing, .. } => Ok(LspAnswer::Calls(
            read_calls(&result, outgoing)
                .into_iter()
                .map(read_usage)
                .collect(),
        )),
        LspQuery::ResolveCompletion { .. } => {
            let edits = result
                .get("additionalTextEdits")
                .cloned()
                .unwrap_or(Value::Null);
            read_text_edits(edits).map(LspAnswer::Edits)
        }
    }
}

fn read_rename(result: &Value) -> LspAnswer {
    let changes = read_workspace_edit(result);
    let places = changes.as_deref().map_or_else(Vec::new, edit_places);
    LspAnswer::Rename { changes, places }
}

fn edit_places(changes: &[LspFileEdit]) -> Vec<LspUsage> {
    changes
        .iter()
        .flat_map(|file| {
            file.edits.iter().map(|edit| {
                read_usage(LspLocation {
                    path: file.path.clone(),
                    start: edit.start,
                })
            })
        })
        .collect()
}

fn read_refactorings(result: &Value) -> Vec<RefactorOffer> {
    let refactoring: Vec<Value> = result
        .as_array()
        .map_or(&[][..], Vec::as_slice)
        .iter()
        .filter(|a| {
            a.get("kind")
                .and_then(Value::as_str)
                .is_some_and(|k| k == "refactor" || k.starts_with("refactor."))
        })
        .cloned()
        .collect();
    read_code_actions(&Value::Array(refactoring))
        .into_iter()
        .map(|action| RefactorOffer {
            places: edit_places(&action.changes),
            action,
        })
        .collect()
}

fn read_usage(location: LspLocation) -> LspUsage {
    let line = location.start.0 as usize;
    let disk_line = std::fs::read_to_string(&location.path)
        .ok()
        .and_then(|text| text.lines().nth(line).map(str::to_owned))
        .unwrap_or_default();
    LspUsage {
        location,
        disk_line,
    }
}

fn read_code_actions(result: &Value) -> Vec<LspCodeAction> {
    let Some(actions) = result.as_array() else {
        return Vec::new();
    };
    actions
        .iter()
        .filter(|action| action.get("disabled").is_none())
        .filter(|action| action.get("command").is_none())
        .filter_map(|action| {
            Some(LspCodeAction {
                title: action.get("title")?.as_str()?.to_owned(),
                changes: read_workspace_edit(action.get("edit")?)?,
            })
        })
        .collect()
}

fn read_workspace_edit(edit: &Value) -> Option<Vec<LspFileEdit>> {
    let mut changes = Vec::new();
    if let Some(files) = edit.get("documentChanges").and_then(Value::as_array) {
        for file in files {
            if file.get("kind").is_some() {
                return None;
            }
            let uri = file.pointer("/textDocument/uri")?.as_str()?;
            changes.push((uri, file.get("edits")?));
        }
    } else {
        for (uri, edits) in edit.get("changes")?.as_object()? {
            changes.push((uri.as_str(), edits));
        }
    }
    let mut files: Vec<LspFileEdit> = changes
        .into_iter()
        .map(|(uri, edits)| {
            Some(LspFileEdit {
                path: uri_to_path(uri)?,
                edits: read_text_edits(edits.clone()).ok()?,
            })
        })
        .collect::<Option<_>>()?;
    files.sort_by(|a, b| a.path.cmp(&b.path));
    (!files.is_empty()).then_some(files)
}

fn read_text_edits(result: Value) -> Result<Vec<LspTextEdit>, String> {
    if result.is_null() {
        return Ok(Vec::new());
    }
    let edits: Vec<lsp_types::TextEdit> =
        serde_json::from_value(result).map_err(|_| "unreadable answer".to_owned())?;
    Ok(edits
        .into_iter()
        .map(|e| LspTextEdit {
            start: (e.range.start.line, e.range.start.character),
            end: (e.range.end.line, e.range.end.character),
            text: e.new_text,
        })
        .collect())
}

fn read_completion(result: Value) -> Result<LspAnswer, String> {
    if result.is_null() {
        return Ok(LspAnswer::Completion {
            items: Vec::new(),
            incomplete: false,
        });
    }
    let (items, incomplete) = match result {
        Value::Array(items) => (items, false),
        mut list => {
            let incomplete = list.get("isIncomplete").and_then(Value::as_bool);
            let Some(Value::Array(items)) = list.get_mut("items").map(Value::take) else {
                return Err("unreadable answer".to_owned());
            };
            (items, incomplete.unwrap_or(false))
        }
    };
    let items = items
        .into_iter()
        .filter_map(|item| {
            let resolve = item.get("data").is_some().then(|| item.to_string());
            let parsed: lsp_types::CompletionItem = serde_json::from_value(item).ok()?;
            Some(read_completion_item(parsed, resolve))
        })
        .collect();
    Ok(LspAnswer::Completion { items, incomplete })
}

fn read_completion_item(item: lsp_types::CompletionItem, resolve: Option<String>) -> LspCompletion {
    let plain = |text: String| {
        if item.insert_text_format == Some(InsertTextFormat::SNIPPET) {
            snippet::to_plain_text(&text)
        } else {
            text
        }
    };
    let (edit_text, start) = match item.text_edit {
        Some(CompletionTextEdit::Edit(edit)) => (Some(edit.new_text), Some(edit.range.start)),
        Some(CompletionTextEdit::InsertAndReplace(edit)) => {
            (Some(edit.new_text), Some(edit.insert.start))
        }
        None => (None, None),
    };
    let additional: Vec<LspTextEdit> = item
        .additional_text_edits
        .unwrap_or_default()
        .into_iter()
        .map(|e| LspTextEdit {
            start: (e.range.start.line, e.range.start.character),
            end: (e.range.end.line, e.range.end.character),
            text: e.new_text,
        })
        .collect();
    LspCompletion {
        resolve: resolve.filter(|_| additional.is_empty()),
        additional,
        detail: item.detail,
        filter: item.filter_text.unwrap_or_else(|| item.label.clone()),
        sort: item.sort_text.unwrap_or_else(|| item.label.clone()),
        insert: edit_text
            .or(item.insert_text)
            .map_or_else(|| item.label.clone(), plain),
        start: start.map(|p| (p.line, p.character)),
        label: item.label,
    }
}

fn read_hover(result: Value) -> Result<Option<String>, String> {
    if result.is_null() {
        return Ok(None);
    }
    let hover: lsp_types::Hover =
        serde_json::from_value(result).map_err(|_| "unreadable answer".to_owned())?;
    let marked = |m: MarkedString| match m {
        MarkedString::String(text) => text,
        MarkedString::LanguageString(code) => {
            format!("```{}\n{}\n```", code.language, code.value)
        }
    };
    let text = match hover.contents {
        HoverContents::Markup(markup) => markup.value,
        HoverContents::Scalar(m) => marked(m),
        HoverContents::Array(list) => list
            .into_iter()
            .map(marked)
            .collect::<Vec<_>>()
            .join("\n\n"),
    };
    Ok(Some(text).filter(|t| !t.trim().is_empty()))
}

fn read_locations(result: Value) -> Result<Vec<LspLocation>, String> {
    if result.is_null() {
        return Ok(Vec::new());
    }
    let response: GotoDefinitionResponse =
        serde_json::from_value(result).map_err(|_| "unreadable answer".to_owned())?;
    let place = |uri: &lsp_types::Uri, start: lsp_types::Position| {
        Some(LspLocation {
            path: uri_to_path(uri.as_str())?,
            start: (start.line, start.character),
        })
    };
    Ok(match response {
        GotoDefinitionResponse::Scalar(l) => place(&l.uri, l.range.start).into_iter().collect(),
        GotoDefinitionResponse::Array(list) => list
            .iter()
            .filter_map(|l| place(&l.uri, l.range.start))
            .collect(),
        GotoDefinitionResponse::Link(list) => list
            .iter()
            .filter_map(|l| place(&l.target_uri, l.target_selection_range.start))
            .collect(),
    })
}

async fn handle_incoming(
    message: &Value,
    stdin: &mut ChildStdin,
    events: &Events,
    queries: &mut Queries,
    progress: &mut HashSet<String>,
) -> std::io::Result<()> {
    let method = message.get("method").and_then(Value::as_str);
    if let (Some(id), Some(method)) = (message.get("id"), method) {
        return write(stdin, &reply_to_server(id, method, message)).await;
    }
    if let Some((id, query)) = answered_query(message, queries) {
        let (message, events) = (message.clone(), events.clone());
        tokio::task::spawn_blocking(move || {
            let result = read_answer(&message, query);
            events.send(Event::Lsp(LspEvent::Answered {
                server: events.key(),
                id,
                result,
            }));
        });
        return Ok(());
    }
    match method {
        Some("textDocument/publishDiagnostics") => {
            if let Some(event) = diagnostics_event(events.key(), message) {
                events.send(event);
            }
        }
        Some("$/progress") => {
            let token = message.pointer("/params/token").map(Value::to_string);
            let kind = message
                .pointer("/params/value/kind")
                .and_then(Value::as_str);
            let was_busy = !progress.is_empty();
            match (token, kind) {
                (Some(token), Some("begin")) => {
                    progress.insert(token);
                }
                (Some(token), Some("end")) => {
                    progress.remove(&token);
                }
                _ => {}
            }
            let busy = !progress.is_empty();
            if busy != was_busy {
                events.send(Event::Lsp(LspEvent::Progress {
                    server: events.key(),
                    busy,
                }));
            }
        }
        _ => {}
    }
    Ok(())
}

fn reply_to_server(id: &Value, method: &str, message: &Value) -> Value {
    if method == "workspace/configuration" {
        let items = message
            .pointer("/params/items")
            .and_then(Value::as_array)
            .map_or(0, Vec::len);
        return json!({"jsonrpc": "2.0", "id": id, "result": vec![Value::Null; items]});
    }
    let known = matches!(
        method,
        "window/workDoneProgress/create"
            | "client/registerCapability"
            | "client/unregisterCapability"
            | "window/showMessageRequest"
    );
    if known {
        json!({"jsonrpc": "2.0", "id": id, "result": null})
    } else {
        json!({"jsonrpc": "2.0", "id": id, "error": {"code": -32601, "message": "not supported"}})
    }
}

fn diagnostics_event(server: ServerKey, message: &Value) -> Option<Event> {
    let params: PublishDiagnosticsParams =
        serde_json::from_value(message.get("params")?.clone()).ok()?;
    let path = uri_to_path(params.uri.as_str())?;
    let items = params
        .diagnostics
        .into_iter()
        .map(|d| RawDiagnostic {
            severity: match d.severity {
                Some(s) if s == lsp_types::DiagnosticSeverity::WARNING => Severity::Warning,
                Some(s) if s == lsp_types::DiagnosticSeverity::INFORMATION => Severity::Info,
                Some(s) if s == lsp_types::DiagnosticSeverity::HINT => Severity::Hint,
                _ => Severity::Error,
            },
            message: d.message,
            start: (d.range.start.line, d.range.start.character),
            end: (d.range.end.line, d.range.end.character),
        })
        .collect();
    Some(Event::Lsp(LspEvent::Diagnostics {
        server,
        path,
        items,
    }))
}

async fn shut_down(
    child: &mut Child,
    stdin: &mut ChildStdin,
    incoming: &mut mpsc::UnboundedReceiver<Result<Value, transport::FrameError>>,
) {
    let request = json!({"jsonrpc": "2.0", "id": SHUTDOWN_ID, "method": "shutdown"});
    let polite = async {
        write(stdin, &request).await?;
        while let Some(message) = incoming.recv().await {
            if matches!(&message, Ok(v) if v.get("id") == Some(&json!(SHUTDOWN_ID))) {
                break;
            }
        }
        write(stdin, &notification("exit", Value::Null)).await?;
        child.wait().await.map(|_| ())
    };
    if !matches!(
        tokio::time::timeout(SHUTDOWN_TIMEOUT, polite).await,
        Ok(Ok(()))
    ) {
        let _ = child.kill().await;
    }
}

pub fn path_to_uri(path: &Path) -> String {
    let mut uri = String::from("file://");
    for &byte in path.as_os_str().as_encoded_bytes() {
        if byte.is_ascii_alphanumeric() || b"-._~/".contains(&byte) {
            uri.push(char::from(byte));
        } else {
            uri.push_str(&format!("%{byte:02X}"));
        }
    }
    uri
}

pub fn uri_to_path(uri: &str) -> Option<PathBuf> {
    let encoded = uri.strip_prefix("file://")?.as_bytes();
    let mut bytes = Vec::with_capacity(encoded.len());
    let mut i = 0;
    while i < encoded.len() {
        if encoded[i] == b'%' {
            let hex = std::str::from_utf8(encoded.get(i + 1..i + 3)?).ok()?;
            bytes.push(u8::from_str_radix(hex, 16).ok()?);
            i += 3;
        } else {
            bytes.push(encoded[i]);
            i += 1;
        }
    }
    Some(PathBuf::from(String::from_utf8(bytes).ok()?))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn uris_round_trip_through_spaces_and_unicode() {
        let path = Path::new("/tmp/мой проект/a#b.rs");
        let uri = path_to_uri(path);
        assert!(uri.starts_with("file:///tmp/"));
        assert!(!uri.contains(' ') && !uri.contains('#'));
        assert_eq!(uri_to_path(&uri).as_deref(), Some(path));
    }

    #[test]
    fn foreign_or_broken_uris_have_no_path() {
        assert_eq!(uri_to_path("untitled:1"), None);
        assert_eq!(uri_to_path("file:///a%2"), None);
    }

    #[test]
    fn backoff_doubles_up_to_the_cap() {
        let s = Duration::from_secs(1);
        let delays: Vec<_> = (1..=7).map(|n| backoff(s, n).as_secs()).collect();
        assert_eq!(delays, [1, 2, 4, 8, 16, 30, 30]);
    }

    #[test]
    fn encoding_defaults_to_utf16() {
        assert_eq!(
            negotiated_encoding(&json!({"result": {}})),
            PositionEncoding::Utf16
        );
        let utf8 = json!({"result": {"capabilities": {"positionEncoding": "utf-8"}}});
        assert_eq!(negotiated_encoding(&utf8), PositionEncoding::Utf8);
    }

    #[test]
    fn server_requests_always_get_an_answer() {
        let ask = |method| json!({"id": 3, "method": method, "params": {"items": [{}, {}]}});
        let config = reply_to_server(&json!(3), "workspace/configuration", &ask("x"));
        assert_eq!(config["result"], json!([null, null]));
        let progress = reply_to_server(&json!(3), "window/workDoneProgress/create", &ask("x"));
        assert_eq!(progress["result"], Value::Null);
        assert!(progress.get("error").is_none());
        let unknown = reply_to_server(&json!(3), "custom/thing", &ask("x"));
        assert!(unknown.get("error").is_some());
    }

    #[test]
    fn a_strange_configuration_request_still_gets_a_safe_answer() {
        for (params, expected) in [
            (json!({"items": []}), json!([])),
            (json!({"items": "not a list"}), json!([])),
            (json!(null), json!([])),
            (json!({"items": [{"section": "a"}]}), json!([null])),
        ] {
            let ask = json!({"id": "x", "method": "workspace/configuration", "params": params});
            let reply = reply_to_server(&json!("x"), "workspace/configuration", &ask);
            assert_eq!(reply["result"], expected, "{ask}");
            assert_eq!(reply["id"], json!("x"));
            assert!(reply.get("error").is_none());
        }
    }

    #[test]
    fn only_actions_that_can_be_done_here_are_offered() {
        let edit = json!({"range": {"start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 2}}, "newText": "x"});
        let actions = read_code_actions(&json!([
            {"title": "map", "edit": {"changes": {"file:///b.rs": [edit], "file:///a.rs": [edit]}}},
            {"title": "doc", "edit": {"documentChanges": [
                {"textDocument": {"uri": "file:///a.rs", "version": 1}, "edits": [edit]},
            ]}},
            {"title": "creates a file", "edit": {"documentChanges": [
                {"kind": "create", "uri": "file:///n.rs"},
            ]}},
            {"title": "needs resolving"},
            {"title": "runs a command", "command": {"title": "x", "command": "y"},
             "edit": {"changes": {"file:///a.rs": [edit]}}},
            {"title": "a bare command", "command": "y"},
            {"title": "disabled", "disabled": {"reason": "no"},
             "edit": {"changes": {"file:///a.rs": [edit]}}},
            {"title": "foreign", "edit": {"changes": {"untitled:1": [edit]}}},
            {"title": "empty", "edit": {"changes": {}}},
        ]));
        let titles: Vec<_> = actions.iter().map(|a| a.title.as_str()).collect();
        assert_eq!(titles, ["map", "doc"]);
        let paths: Vec<_> = actions[0].changes.iter().map(|c| c.path.clone()).collect();
        assert_eq!(paths, [PathBuf::from("/a.rs"), PathBuf::from("/b.rs")]);
        assert_eq!(actions[0].changes[0].edits[0].end, (0, 2));
        assert!(read_code_actions(&Value::Null).is_empty());
    }

    #[test]
    fn diagnostics_are_read_with_their_severity() {
        let message = json!({"params": {
            "uri": "file:///a.rs",
            "diagnostics": [
                {"range": {"start": {"line": 1, "character": 2}, "end": {"line": 1, "character": 5}},
                 "severity": 2, "message": "unused"},
                {"range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 1}},
                 "message": "no severity"},
            ],
        }});
        let key = ServerKey {
            server: Language::Rust,
            root: PathBuf::from("/"),
        };
        let Some(Event::Lsp(LspEvent::Diagnostics { path, items, .. })) =
            diagnostics_event(key, &message)
        else {
            panic!("no event");
        };
        assert_eq!(path, Path::new("/a.rs"));
        assert_eq!(items[0].severity, Severity::Warning);
        assert_eq!((items[0].start, items[0].end), ((1, 2), (1, 5)));
        assert_eq!(items[1].severity, Severity::Error);
    }

    #[test]
    fn a_tree_of_symbols_comes_in_order_with_depth_and_a_flat_list_sorted() {
        let tree = json!([{
            "name": "Foo", "kind": 5,
            "range": {}, "selectionRange": {"start": {"line": 1, "character": 6}},
            "children": [{
                "name": "bar", "kind": 6,
                "selectionRange": {"start": {"line": 2, "character": 8}},
            }],
        }]);
        let symbols = read_symbols(&tree);
        assert_eq!(symbols[0].name, "Foo");
        assert_eq!((symbols[0].kind.as_str(), symbols[0].depth), ("class", 0));
        assert_eq!(
            (
                symbols[1].name.as_str(),
                symbols[1].kind.as_str(),
                symbols[1].depth,
                symbols[1].start
            ),
            ("bar", "method", 1, (2, 8))
        );
        let flat = json!([
            {"name": "b", "kind": 12, "location": {"uri": "file:///a", "range": {"start": {"line": 9, "character": 0}}}},
            {"name": "a", "kind": 12, "location": {"uri": "file:///a", "range": {"start": {"line": 1, "character": 0}}}},
        ]);
        let names: Vec<String> = read_symbols(&flat).into_iter().map(|s| s.name).collect();
        assert_eq!(names, ["a", "b"]);
        assert!(read_symbols(&Value::Null).is_empty());
    }

    #[test]
    fn callers_are_the_calls_and_callees_their_definitions() {
        let incoming = json!([{
            "from": {"uri": "file:///a.rs", "selectionRange": {"start": {"line": 0, "character": 3}}},
            "fromRanges": [
                {"start": {"line": 4, "character": 8}},
                {"start": {"line": 6, "character": 2}},
            ],
        }]);
        let places = read_calls(&incoming, false);
        assert_eq!(places.len(), 2, "each call site");
        assert_eq!(
            (places[1].path.as_path(), places[1].start),
            (Path::new("/a.rs"), (6, 2))
        );
        let outgoing = json!([{
            "to": {"uri": "file:///b.rs", "selectionRange": {"start": {"line": 3, "character": 1}}},
            "fromRanges": [{"start": {"line": 9, "character": 9}}],
        }]);
        let places = read_calls(&outgoing, true);
        assert_eq!(
            (places[0].path.as_path(), places[0].start),
            (Path::new("/b.rs"), (3, 1))
        );
    }

    #[test]
    fn the_item_for_calls_goes_back_exactly_as_it_came() {
        let result = json!([{"name": "f", "kind": 12, "uri": "file:///a", "data": {"x": 1}}]);
        let (name, item) = read_call_root(&result).unwrap();
        assert_eq!(name, "f");
        let request = query_request(
            1,
            Path::new("/a"),
            LspQuery::Calls {
                item,
                outgoing: false,
            },
        );
        assert_eq!(request["method"], "callHierarchy/incomingCalls");
        assert_eq!(request["params"]["item"]["data"]["x"], 1);
        assert_eq!(read_call_root(&Value::Null), None);
    }

    #[test]
    fn an_import_comes_with_the_item_or_after_asking_for_it() {
        let edit = json!({
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 0}},
            "newText": "use x;\n",
        });
        let answer = read_completion(json!([
            {"label": "a", "additionalTextEdits": [edit], "data": 1},
            {"label": "b", "data": {"imports": ["x"]}},
            {"label": "c"},
        ]))
        .unwrap();
        let LspAnswer::Completion { items, .. } = answer else {
            panic!("{answer:?}");
        };
        assert_eq!(items[0].additional[0].text, "use x;\n");
        assert_eq!(items[0].resolve, None, "it has them already");
        assert!(
            items[1]
                .resolve
                .as_ref()
                .is_some_and(|r| r.contains("imports"))
        );
        assert_eq!(items[2].resolve, None, "nothing more to ask for");
        let resolved = json!({"id": 5, "result": {"label": "b", "additionalTextEdits": [edit]}});
        let answer = read_answer(
            &resolved,
            LspQuery::ResolveCompletion {
                item: String::new(),
            },
        )
        .unwrap();
        assert!(matches!(answer, LspAnswer::Edits(edits) if edits[0].text == "use x;\n"));
    }

    #[test]
    fn the_client_asks_for_what_these_need() {
        let init = initialize_request(Path::new("/p"), None);
        let text = init.to_string();
        assert!(text.contains("additionalTextEdits"), "{text}");
        assert!(text.contains("hierarchicalDocumentSymbolSupport"));
        assert!(text.contains("callHierarchy"));
    }
}

#[cfg(test)]
mod rename_tests {
    use super::*;

    #[test]
    fn only_refactorings_are_asked_for_and_kept() {
        let query = LspQuery::Refactorings {
            at: (1, 0),
            to: (1, 4),
        };
        let request = query_request(1, Path::new("/p/a.rs"), query.clone());
        assert_eq!(request["params"]["context"]["only"][0], "refactor");
        assert_eq!(request["params"]["range"]["end"]["character"], 4);
        let edit = json!({"changes": {"file:///nowhere/a.rs": [{
            "range": {"start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 1}},
            "newText": "x"}]}});
        let reply = json!({"result": [
            {"title": "Fix", "kind": "quickfix", "edit": edit},
            {"title": "No kind", "edit": edit},
            {"title": "Extract", "kind": "refactor.extract", "edit": edit},
            {"title": "Needs a command", "kind": "refactor", "command": {"title": "x"}, "edit": edit},
        ]});
        let Ok(LspAnswer::Refactorings(offers)) = read_answer(&reply, query) else {
            panic!("refactorings");
        };
        assert_eq!(offers.len(), 1);
        assert_eq!(offers[0].action.title, "Extract");
        assert_eq!(offers[0].places[0].location.start, (0, 1));
    }

    fn rename_query() -> LspQuery {
        LspQuery::Rename {
            at: (1, 2),
            new_name: "baz".to_owned(),
        }
    }

    #[test]
    fn the_request_carries_the_new_name_and_the_position() {
        let request = query_request(7, Path::new("/p/a.rs"), rename_query());
        assert_eq!(request["method"], "textDocument/rename");
        assert_eq!(request["params"]["newName"], "baz");
        assert_eq!(request["params"]["position"]["line"], 1);
        assert_eq!(request["params"]["position"]["character"], 2);
    }

    #[test]
    fn every_edit_becomes_a_place_to_show() {
        let range =
            json!({"start": {"line": 0, "character": 1}, "end": {"line": 0, "character": 4}});
        let reply = json!({"result": {"changes": {"file:///nowhere/a.rs": [
            {"range": range, "newText": "baz"},
            {"range": range, "newText": "baz"},
        ]}}});
        let Ok(LspAnswer::Rename { changes, places }) = read_answer(&reply, rename_query()) else {
            panic!("a rename answer");
        };
        assert_eq!(changes.map(|c| c.len()), Some(1));
        assert_eq!(places.len(), 2);
        assert_eq!(places[0].location.start, (0, 1));
    }

    #[test]
    fn nothing_to_do_or_a_file_operation_is_no_rename() {
        let move_file = json!({"documentChanges": [{"kind": "rename", "oldUri": "file:///a", "newUri": "file:///b"}]});
        for result in [Value::Null, move_file] {
            let reply = json!({"result": result});
            let answer = read_answer(&reply, rename_query());
            assert!(matches!(
                answer,
                Ok(LspAnswer::Rename { changes: None, .. })
            ));
        }
    }
}
