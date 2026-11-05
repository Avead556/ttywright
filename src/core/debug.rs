use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::run::visible;

pub const MAX_CONSOLE_LINES: usize = 10_000;
const XDEBUG_PORT: u16 = 9003;
pub const PORT_PLACEHOLDER: &str = "{port}";

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum AdapterId {
    CodeLldb,
    LldbDap,
    Gdb,
    Debugpy,
    Delve,
    PhpDebug,
    JsDebug,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum Transport {
    Stdio,
    Tcp,
}

#[derive(Debug, Clone, Copy)]
pub struct AdapterRow {
    pub id: AdapterId,
    pub config_name: &'static str,
    pub dap_type: &'static str,
    pub transport: Transport,
    pub commands: &'static [&'static [&'static str]],
    pub needs_module: Option<&'static str>,
    pub node_scripts: &'static [&'static str],
    pub script_args: &'static [&'static str],
    pub install: &'static str,
}

pub const ADAPTERS: &[AdapterRow] = &[
    AdapterRow {
        id: AdapterId::CodeLldb,
        config_name: "codelldb",
        dap_type: "lldb",
        transport: Transport::Tcp,
        commands: &[&["codelldb", "--port", PORT_PLACEHOLDER]],
        needs_module: None,
        node_scripts: &[],
        script_args: &[],
        install: "install CodeLLDB and put codelldb on PATH, or use lldb-dap",
    },
    AdapterRow {
        id: AdapterId::LldbDap,
        config_name: "lldb",
        dap_type: "lldb-dap",
        transport: Transport::Stdio,
        commands: &[&["lldb-dap"], &["lldb-vscode"]],
        needs_module: None,
        node_scripts: &[],
        script_args: &[],
        install: "xcode-select --install, or brew install llvm, or apt install lldb",
    },
    AdapterRow {
        id: AdapterId::Gdb,
        config_name: "gdb",
        dap_type: "gdb",
        transport: Transport::Stdio,
        commands: &[&["gdb", "--interpreter=dap"]],
        needs_module: None,
        node_scripts: &[],
        script_args: &[],
        install: "install gdb 14 or newer",
    },
    AdapterRow {
        id: AdapterId::Debugpy,
        config_name: "python",
        dap_type: "python",
        transport: Transport::Stdio,
        commands: &[
            &["python3", "-m", "debugpy.adapter"],
            &["python", "-m", "debugpy.adapter"],
        ],
        needs_module: Some("debugpy"),
        node_scripts: &[],
        script_args: &[],
        install: "pip install debugpy",
    },
    AdapterRow {
        id: AdapterId::Delve,
        config_name: "go",
        dap_type: "go",
        transport: Transport::Tcp,
        commands: &[&["dlv", "dap", "--listen", "127.0.0.1:{port}"]],
        needs_module: None,
        node_scripts: &[],
        script_args: &[],
        install: "go install github.com/go-delve/delve/cmd/dlv@latest",
    },
    AdapterRow {
        id: AdapterId::PhpDebug,
        config_name: "php",
        dap_type: "php",
        transport: Transport::Stdio,
        commands: &[&["php-debug-adapter"]],
        needs_module: None,
        node_scripts: &[
            "~/.vscode/extensions/xdebug.php-debug-*/out/phpDebug.js",
            "~/.local/share/nvim/mason/packages/php-debug-adapter/extension/out/phpDebug.js",
        ],
        script_args: &[],
        install: "install Xdebug (pecl install xdebug) and the VS Code extension xdebug.php-debug, \
                  or set [debug_adapter.php] command",
    },
    AdapterRow {
        id: AdapterId::JsDebug,
        config_name: "js",
        dap_type: "pwa-node",
        transport: Transport::Tcp,
        commands: &[&["js-debug-adapter", PORT_PLACEHOLDER, "127.0.0.1"]],
        needs_module: None,
        node_scripts: &[
            "~/.local/share/nvim/mason/packages/js-debug-adapter/js-debug/src/dapDebugServer.js",
        ],
        script_args: &[PORT_PLACEHOLDER, "127.0.0.1"],
        install: "get js-debug-dap from the vscode-js-debug releases, \
                  or set [debug_adapter.js] command",
    },
];

pub fn adapter_row(id: AdapterId) -> &'static AdapterRow {
    ADAPTERS
        .iter()
        .find(|row| row.id == id)
        .unwrap_or(&ADAPTERS[0])
}

pub fn adapter_for_config_name(name: &str) -> Option<AdapterId> {
    ADAPTERS
        .iter()
        .find(|row| row.config_name == name)
        .map(|row| row.id)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AdapterCommand {
    pub id: AdapterId,
    pub program: String,
    pub args: Vec<String>,
    pub transport: Transport,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugFacts {
    pub adapters: BTreeMap<AdapterId, AdapterCommand>,
    pub has_cargo: bool,
    pub project_python: Option<PathBuf>,
    pub file: Option<PathBuf>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DebugTarget {
    pub name: String,
    pub adapter: AdapterCommand,
    pub request: String,
    pub settings: Value,
    pub build: Option<Vec<String>>,
    pub cwd: PathBuf,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugChoice {
    Ready(DebugTarget),
    Missing { label: String, hint: String },
}

impl DebugChoice {
    pub fn label(&self) -> String {
        match self {
            Self::Ready(target) => visible(&target.name),
            Self::Missing { label, .. } => format!("[not installed] {}", visible(label)),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConfiguredDebug {
    pub name: String,
    pub adapter: AdapterId,
    pub request: String,
    pub settings: Value,
    pub cwd: Option<String>,
}

#[derive(Clone, Copy)]
struct FileRule {
    extensions: &'static [&'static str],
    adapter: AdapterId,
    settings: fn(&Path, &Path, &DebugFacts) -> Value,
}

fn python_settings(file: &Path, cwd: &Path, facts: &DebugFacts) -> Value {
    let mut settings = json!({
        "program": file,
        "cwd": cwd,
        "console": "internalConsole",
        "redirectOutput": true,
        "justMyCode": true,
    });
    if let Some(python) = &facts.project_python {
        settings["python"] = json!([python]);
    }
    settings
}

fn go_settings(file: &Path, cwd: &Path, _: &DebugFacts) -> Value {
    let package = file.parent().unwrap_or(cwd);
    json!({
        "mode": "debug",
        "program": package,
        "cwd": cwd,
        "outputMode": "remote",
    })
}

fn php_settings(file: &Path, cwd: &Path, _: &DebugFacts) -> Value {
    json!({"program": file, "cwd": cwd, "port": XDEBUG_PORT})
}

fn node_settings(file: &Path, cwd: &Path, _: &DebugFacts) -> Value {
    json!({
        "program": file,
        "cwd": cwd,
        "console": "internalConsole",
        "outputCapture": "std",
    })
}

fn ts_settings(file: &Path, cwd: &Path, facts: &DebugFacts) -> Value {
    let mut settings = node_settings(file, cwd, facts);
    settings["runtimeArgs"] = json!(["--experimental-strip-types"]);
    settings
}

const FILE_RULES: &[FileRule] = &[
    FileRule {
        extensions: &["py", "pyw"],
        adapter: AdapterId::Debugpy,
        settings: python_settings,
    },
    FileRule {
        extensions: &["go"],
        adapter: AdapterId::Delve,
        settings: go_settings,
    },
    FileRule {
        extensions: &["php", "phtml"],
        adapter: AdapterId::PhpDebug,
        settings: php_settings,
    },
    FileRule {
        extensions: &["js", "mjs", "cjs"],
        adapter: AdapterId::JsDebug,
        settings: node_settings,
    },
    FileRule {
        extensions: &["ts", "mts", "cts"],
        adapter: AdapterId::JsDebug,
        settings: ts_settings,
    },
];

const NATIVE_ADAPTERS: &[AdapterId] = &[AdapterId::CodeLldb, AdapterId::LldbDap, AdapterId::Gdb];

fn target(
    name: String,
    adapter: &AdapterCommand,
    request: &str,
    mut settings: Value,
    build: Option<Vec<String>>,
    cwd: &Path,
) -> DebugTarget {
    settings["type"] = json!(adapter_row(adapter.id).dap_type);
    settings["request"] = json!(request);
    settings["name"] = json!(name);
    DebugTarget {
        name,
        adapter: adapter.clone(),
        request: request.to_owned(),
        settings,
        build,
        cwd: cwd.to_path_buf(),
    }
}

fn missing(label: String, id: AdapterId) -> DebugChoice {
    DebugChoice::Missing {
        label,
        hint: adapter_row(id).install.to_owned(),
    }
}

pub fn debug_choices(
    facts: &DebugFacts,
    root: &Path,
    configured: &[ConfiguredDebug],
) -> Vec<DebugChoice> {
    let mut choices = Vec::new();
    for entry in configured {
        let cwd = entry
            .cwd
            .as_ref()
            .map_or_else(|| root.to_path_buf(), |c| root.join(c));
        match facts.adapters.get(&entry.adapter) {
            Some(adapter) => {
                let mut settings = entry.settings.clone();
                if settings.get("cwd").is_none() {
                    settings["cwd"] = json!(cwd);
                }
                choices.push(DebugChoice::Ready(target(
                    entry.name.clone(),
                    adapter,
                    &entry.request,
                    settings,
                    None,
                    &cwd,
                )));
            }
            None => choices.push(missing(entry.name.clone(), entry.adapter)),
        }
    }
    if facts.has_cargo {
        let native = NATIVE_ADAPTERS.iter().find_map(|id| facts.adapters.get(id));
        let label = "Build and debug the Cargo project".to_owned();
        match native {
            Some(adapter) => {
                let mut settings = json!({"cwd": root, "args": []});
                if adapter.id == AdapterId::CodeLldb {
                    settings["sourceLanguages"] = json!(["rust"]);
                    settings["terminal"] = json!("console");
                }
                let build = ["cargo", "build", "--message-format=json-render-diagnostics"]
                    .map(str::to_owned)
                    .to_vec();
                choices.push(DebugChoice::Ready(target(
                    label,
                    adapter,
                    "launch",
                    settings,
                    Some(build),
                    root,
                )));
            }
            None => choices.push(missing(label, AdapterId::LldbDap)),
        }
    }
    if let Some(file) = &facts.file {
        file_choices(file, facts, root, &mut choices);
    }
    choices
}

fn file_choices(file: &Path, facts: &DebugFacts, root: &Path, choices: &mut Vec<DebugChoice>) {
    let extension = file.extension().and_then(|e| e.to_str()).unwrap_or("");
    let Some(rule) = FILE_RULES
        .iter()
        .find(|r| r.extensions.contains(&extension))
    else {
        return;
    };
    let shown = file.strip_prefix(root).unwrap_or(file).display();
    let label = format!("Debug {shown}");
    let Some(adapter) = facts.adapters.get(&rule.adapter) else {
        choices.push(missing(label, rule.adapter));
        return;
    };
    let settings = (rule.settings)(file, root, facts);
    choices.push(DebugChoice::Ready(target(
        label, adapter, "launch", settings, None, root,
    )));
    if rule.adapter == AdapterId::PhpDebug {
        let settings = json!({"cwd": root, "port": XDEBUG_PORT});
        choices.push(DebugChoice::Ready(target(
            format!("Listen for Xdebug on port {XDEBUG_PORT}"),
            adapter,
            "launch",
            settings,
            None,
            root,
        )));
    }
}

pub fn cargo_executables(output: &str) -> Vec<String> {
    let mut found = Vec::new();
    for line in output.lines() {
        let Ok(message) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if message["reason"] != "compiler-artifact" {
            continue;
        }
        let is_script = message["target"]["kind"]
            .as_array()
            .is_some_and(|k| k.iter().any(|v| v == "custom-build"));
        if is_script {
            continue;
        }
        if let Some(executable) = message["executable"].as_str() {
            found.push(executable.to_owned());
        }
    }
    found
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub enum BreakpointState {
    #[default]
    Unknown,
    Pending(Option<String>),
    Verified,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Breakpoint {
    pub path: PathBuf,
    pub line: usize,
    pub(super) at: Option<usize>,
    pub condition: Option<String>,
    pub log_message: Option<String>,
    pub enabled: bool,
    pub state: BreakpointState,
    pub(super) id: Option<i64>,
}

impl Breakpoint {
    pub(super) fn new(path: PathBuf, line: usize, at: Option<usize>) -> Self {
        Self {
            path,
            line,
            at,
            condition: None,
            log_message: None,
            enabled: true,
            state: BreakpointState::Unknown,
            id: None,
        }
    }

    pub(super) fn to_dap(&self) -> Value {
        let mut out = json!({"line": self.line + 1});
        if let Some(condition) = &self.condition {
            out["condition"] = json!(condition);
        }
        if let Some(message) = &self.log_message {
            out["logMessage"] = json!(message);
        }
        out
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugPhase {
    Building,
    Starting,
    Running,
    Paused { reason: String },
    Ended { note: String },
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum DebugPart {
    #[default]
    Frames,
    Variables,
    Console,
}

impl DebugPart {
    pub fn next(self) -> Self {
        match self {
            Self::Frames => Self::Variables,
            Self::Variables => Self::Console,
            Self::Console => Self::Frames,
        }
    }

    pub fn previous(self) -> Self {
        self.next().next()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DebugFrame {
    pub id: i64,
    pub name: String,
    pub path: Option<PathBuf>,
    pub line: usize,
    pub col: usize,
    pub exists: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarNode {
    pub key: Vec<String>,
    pub name: String,
    pub value: String,
    pub kind: Option<String>,
    pub reference: i64,
    pub children: Option<Vec<VarNode>>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VarRow {
    pub depth: usize,
    pub name: String,
    pub value: String,
    pub kind: Option<String>,
    pub expandable: bool,
    pub expanded: bool,
    pub key: Vec<String>,
    pub watch: Option<usize>,
}

impl VarNode {
    pub(super) fn from_dap(
        parent: &[String],
        item: &Value,
        seen: &mut HashMap<String, u32>,
    ) -> Self {
        let name = visible(item["name"].as_str().unwrap_or("?"));
        let count = seen.entry(name.clone()).or_insert(0);
        let part = if *count == 0 {
            name.clone()
        } else {
            format!("{name}#{count}")
        };
        *count += 1;
        let mut key = parent.to_vec();
        key.push(part);
        Self {
            key,
            name,
            value: visible(item["value"].as_str().unwrap_or("")),
            kind: item["type"].as_str().map(visible).filter(|t| !t.is_empty()),
            reference: item["variablesReference"].as_i64().unwrap_or(0),
            children: None,
        }
    }

    pub(super) fn find_mut(&mut self, key: &[String]) -> Option<&mut VarNode> {
        if self.key == key {
            return Some(self);
        }
        self.children
            .as_mut()?
            .iter_mut()
            .find_map(|child| child.find_mut(key))
    }

    fn rows(&self, depth: usize, expanded: &BTreeSet<Vec<String>>, out: &mut Vec<VarRow>) {
        let is_open = self.reference > 0 && expanded.contains(&self.key);
        out.push(VarRow {
            depth,
            name: self.name.clone(),
            value: self.value.clone(),
            kind: self.kind.clone(),
            expandable: self.reference > 0,
            expanded: is_open,
            key: self.key.clone(),
            watch: None,
        });
        if is_open && let Some(children) = &self.children {
            for child in children {
                child.rows(depth + 1, expanded, out);
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum Pending {
    Initialize,
    Launch,
    SetBreakpoints { path: PathBuf, lines: Vec<usize> },
    Threads,
    Stack { generation: u64 },
    Scopes { generation: u64 },
    Variables { key: Vec<String>, generation: u64 },
    Watch { index: usize, generation: u64 },
    Evaluate { expression: String },
    Other { what: &'static str },
}

#[derive(Debug, Clone)]
pub struct DebugSession {
    pub id: u64,
    pub name: String,
    pub phase: DebugPhase,
    pub(super) target: DebugTarget,
    pub(super) conns: BTreeSet<u32>,
    pub(super) next_conn: u32,
    pub(super) active: u32,
    pub(super) child_settings: BTreeMap<u32, (String, Value)>,
    pub(super) next_seq: i64,
    pub(super) pending: HashMap<(u32, i64), Pending>,
    pub(super) generation: u64,
    pub(super) exception_filters: BTreeMap<u32, Vec<(String, bool)>>,
    pub threads: Vec<(i64, String)>,
    pub thread: Option<i64>,
    pub frames: Vec<DebugFrame>,
    pub frame: usize,
    pub part: DebugPart,
    pub(super) scopes: Vec<VarNode>,
    pub(super) watch_values: Vec<Option<VarNode>>,
    pub(super) expanded: BTreeSet<Vec<String>>,
    pub(super) seen_scopes: BTreeSet<String>,
    pub var_selected: usize,
    pub console: Vec<String>,
    pub(super) partial: BTreeMap<String, String>,
    pub console_back: usize,
    pub(super) run_to: Option<(PathBuf, usize)>,
    pub(super) exit_code: Option<i64>,
    pub(super) stopping: bool,
    pub(super) program_over: bool,
}

impl DebugSession {
    pub(super) fn new(id: u64, target: DebugTarget) -> Self {
        Self {
            id,
            name: target.name.clone(),
            phase: DebugPhase::Starting,
            target,
            conns: BTreeSet::from([0]),
            next_conn: 1,
            active: 0,
            child_settings: BTreeMap::new(),
            next_seq: 1,
            pending: HashMap::new(),
            generation: 0,
            exception_filters: BTreeMap::new(),
            threads: Vec::new(),
            thread: None,
            frames: Vec::new(),
            frame: 0,
            part: DebugPart::default(),
            scopes: Vec::new(),
            watch_values: Vec::new(),
            expanded: BTreeSet::new(),
            seen_scopes: BTreeSet::new(),
            var_selected: 0,
            console: Vec::new(),
            partial: BTreeMap::new(),
            console_back: 0,
            run_to: None,
            exit_code: None,
            stopping: false,
            program_over: false,
        }
    }

    pub fn is_live(&self) -> bool {
        !matches!(self.phase, DebugPhase::Ended { .. })
    }

    pub fn is_paused(&self) -> bool {
        matches!(self.phase, DebugPhase::Paused { .. })
    }

    pub fn var_rows(&self, watches: &[String]) -> Vec<VarRow> {
        let mut rows = Vec::new();
        for (index, expression) in watches.iter().enumerate() {
            let start = rows.len();
            match self.watch_values.get(index).and_then(Option::as_ref) {
                Some(node) => node.rows(0, &self.expanded, &mut rows),
                None => rows.push(VarRow {
                    depth: 0,
                    name: expression.clone(),
                    value: "not available".to_owned(),
                    kind: None,
                    expandable: false,
                    expanded: false,
                    key: vec![format!("watch:{index}")],
                    watch: None,
                }),
            }
            rows[start].watch = Some(index);
        }
        for scope in &self.scopes {
            scope.rows(0, &self.expanded, &mut rows);
        }
        rows
    }

    pub(super) fn add_output(&mut self, category: &str, text: &str) {
        let partial = self.partial.entry(category.to_owned()).or_default();
        partial.push_str(text);
        let Some(end) = partial.rfind('\n') else {
            return;
        };
        let rest = partial.split_off(end + 1);
        let done = std::mem::replace(partial, rest);
        for line in done.lines() {
            self.add_line(line);
        }
    }

    pub(super) fn add_line(&mut self, line: &str) {
        self.console.push(clean_output(line));
        if self.console.len() > MAX_CONSOLE_LINES {
            let extra = self.console.len() - MAX_CONSOLE_LINES;
            self.console.drain(..extra);
        }
    }

    pub(super) fn finish_output(&mut self) {
        let left: Vec<String> = std::mem::take(&mut self.partial).into_values().collect();
        for text in left {
            if !text.is_empty() {
                self.add_line(&text);
            }
        }
    }
}

fn clean_output(line: &str) -> String {
    let mut decoder = super::run_output::OutputDecoder::default();
    let mut out = String::new();
    decoder.feed_styled(line.as_bytes(), |done, _| out.push_str(&done));
    out.push_str(&decoder.partial());
    out
}

#[derive(Debug, Clone, Default)]
pub struct DebugState {
    pub breakpoints: Vec<Breakpoint>,
    pub watches: Vec<String>,
    pub all_exceptions: bool,
    pub session: Option<DebugSession>,
    pub(super) next_session: u64,
    pub(super) choices: Vec<DebugChoice>,
    pub(super) executables: Vec<String>,
    pub(super) last: Option<DebugTarget>,
    pub(super) configured: Vec<ConfiguredDebug>,
    pub(super) wants_last: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DebugEffect {
    Detect {
        root: PathBuf,
        file: Option<PathBuf>,
    },
    Build {
        session: u64,
        root: PathBuf,
        argv: Vec<String>,
    },
    Start {
        session: u64,
        adapter: AdapterCommand,
        cwd: PathBuf,
    },
    Connect {
        session: u64,
        conn: u32,
    },
    Send {
        session: u64,
        conn: u32,
        message: Value,
    },
    Stop {
        session: u64,
    },
    ProbeFiles {
        session: u64,
        generation: u64,
        paths: Vec<PathBuf>,
    },
}

impl DebugEffect {
    pub const fn needs_trust(&self) -> bool {
        match self {
            Self::Detect { .. }
            | Self::Build { .. }
            | Self::Start { .. }
            | Self::Connect { .. } => true,
            Self::Send { .. } | Self::Stop { .. } | Self::ProbeFiles { .. } => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DebugEvent {
    Detected {
        facts: DebugFacts,
    },
    Built {
        session: u64,
        result: Result<Vec<String>, String>,
    },
    Output {
        session: u64,
        text: String,
    },
    Message {
        session: u64,
        conn: u32,
        message: Value,
    },
    Ended {
        session: u64,
        failure: Option<String>,
    },
    FilesProbed {
        session: u64,
        generation: u64,
        existing: Vec<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq)]
pub(super) enum Incoming {
    Response {
        request_seq: i64,
        success: bool,
        message: Option<String>,
        body: Value,
    },
    Event {
        event: String,
        body: Value,
    },
    Request {
        seq: i64,
        command: String,
        arguments: Value,
    },
}

pub(super) fn parse_incoming(message: &Value) -> Option<Incoming> {
    match message["type"].as_str()? {
        "response" => Some(Incoming::Response {
            request_seq: message["request_seq"].as_i64()?,
            success: message["success"].as_bool().unwrap_or(false),
            message: message["body"]["error"]["format"]
                .as_str()
                .or_else(|| message["message"].as_str())
                .map(visible),
            body: message["body"].clone(),
        }),
        "event" => Some(Incoming::Event {
            event: message["event"].as_str()?.to_owned(),
            body: message["body"].clone(),
        }),
        "request" => Some(Incoming::Request {
            seq: message["seq"].as_i64()?,
            command: message["command"].as_str()?.to_owned(),
            arguments: message["arguments"].clone(),
        }),
        _ => None,
    }
}

pub(super) fn parse_frame(item: &Value) -> Option<DebugFrame> {
    Some(DebugFrame {
        id: item["id"].as_i64()?,
        name: visible(item["name"].as_str().unwrap_or("?")),
        path: item["source"]["path"].as_str().map(PathBuf::from),
        line: usize::try_from(item["line"].as_i64().unwrap_or(1) - 1).unwrap_or(0),
        col: usize::try_from(item["column"].as_i64().unwrap_or(1) - 1).unwrap_or(0),
        exists: false,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn adapter(id: AdapterId) -> AdapterCommand {
        AdapterCommand {
            id,
            program: "x".to_owned(),
            args: Vec::new(),
            transport: Transport::Stdio,
        }
    }

    fn facts_for(file: &str, ids: &[AdapterId]) -> DebugFacts {
        DebugFacts {
            adapters: ids.iter().map(|id| (*id, adapter(*id))).collect(),
            file: Some(PathBuf::from(file)),
            ..DebugFacts::default()
        }
    }

    fn ready(choices: &[DebugChoice]) -> Vec<&DebugTarget> {
        choices
            .iter()
            .filter_map(|c| match c {
                DebugChoice::Ready(t) => Some(t),
                DebugChoice::Missing { .. } => None,
            })
            .collect()
    }

    #[test]
    fn a_python_file_is_launched_with_the_projects_own_python() {
        let mut facts = facts_for("/p/a.py", &[AdapterId::Debugpy]);
        facts.project_python = Some(PathBuf::from("/p/.venv/bin/python"));
        let choices = debug_choices(&facts, Path::new("/p"), &[]);
        let targets = ready(&choices);
        assert_eq!(targets[0].name, "Debug a.py");
        assert_eq!(targets[0].settings["type"], "python");
        assert_eq!(targets[0].settings["program"], "/p/a.py");
        assert_eq!(targets[0].settings["python"][0], "/p/.venv/bin/python");
    }

    #[test]
    fn a_php_file_can_be_run_or_waited_for() {
        let facts = facts_for("/p/a.php", &[AdapterId::PhpDebug]);
        let choices = debug_choices(&facts, Path::new("/p"), &[]);
        let targets = ready(&choices);
        assert_eq!(targets.len(), 2);
        assert_eq!(targets[0].settings["program"], "/p/a.php");
        assert!(targets[1].name.contains("Listen for Xdebug"));
        assert!(targets[1].settings.get("program").is_none());
        assert_eq!(targets[1].settings["port"], 9003);
    }

    #[test]
    fn a_go_file_debugs_its_package() {
        let facts = facts_for("/p/cmd/a.go", &[AdapterId::Delve]);
        let choices = debug_choices(&facts, Path::new("/p"), &[]);
        assert_eq!(ready(&choices)[0].settings["program"], "/p/cmd");
    }

    #[test]
    fn typescript_is_told_to_strip_types_and_javascript_is_not() {
        let root = Path::new("/p");
        let ts = debug_choices(&facts_for("/p/a.ts", &[AdapterId::JsDebug]), root, &[]);
        assert_eq!(
            ready(&ts)[0].settings["runtimeArgs"][0],
            "--experimental-strip-types"
        );
        let js = debug_choices(&facts_for("/p/a.js", &[AdapterId::JsDebug]), root, &[]);
        assert!(ready(&js)[0].settings.get("runtimeArgs").is_none());
    }

    #[test]
    fn what_cannot_be_debugged_yet_says_what_it_takes() {
        let facts = facts_for("/p/a.py", &[]);
        let choices = debug_choices(&facts, Path::new("/p"), &[]);
        assert_eq!(choices[0].label(), "[not installed] Debug a.py");
        let DebugChoice::Missing { hint, .. } = &choices[0] else {
            panic!("expected a missing adapter");
        };
        assert!(hint.contains("debugpy"));
    }

    #[test]
    fn a_cargo_project_builds_first_with_the_best_native_adapter() {
        let mut facts = facts_for("/p/README", &[AdapterId::Gdb, AdapterId::LldbDap]);
        facts.has_cargo = true;
        let choices = debug_choices(&facts, Path::new("/p"), &[]);
        let targets = ready(&choices);
        assert_eq!(targets[0].adapter.id, AdapterId::LldbDap);
        assert!(targets[0].build.as_ref().is_some_and(|b| b[0] == "cargo"));
    }

    #[test]
    fn the_users_entries_come_first_and_keep_their_settings() {
        let facts = facts_for("/p/a.py", &[AdapterId::Debugpy, AdapterId::LldbDap]);
        let configured = [ConfiguredDebug {
            name: "server".to_owned(),
            adapter: AdapterId::LldbDap,
            request: "attach".to_owned(),
            settings: json!({"pid": 7}),
            cwd: Some("sub".to_owned()),
        }];
        let choices = debug_choices(&facts, Path::new("/p"), &configured);
        let targets = ready(&choices);
        assert_eq!(targets[0].name, "server");
        assert_eq!(targets[0].settings["request"], "attach");
        assert_eq!(targets[0].settings["pid"], 7);
        assert_eq!(targets[0].settings["cwd"], "/p/sub");
    }

    #[test]
    fn only_real_executables_of_a_build_are_offered() {
        let output = r#"
{"reason":"compiler-artifact","target":{"kind":["custom-build"]},"executable":"/t/build-script-build"}
{"reason":"compiler-artifact","target":{"kind":["lib"]},"executable":null}
{"reason":"compiler-artifact","target":{"kind":["bin"]},"executable":"/t/app"}
{"reason":"build-finished","success":true}
not json
"#;
        assert_eq!(cargo_executables(output), vec!["/t/app".to_owned()]);
    }

    #[test]
    fn a_breakpoint_is_sent_with_its_condition_or_message_one_based() {
        let mut b = Breakpoint::new(PathBuf::from("/a"), 4, None);
        assert_eq!(b.to_dap(), json!({"line": 5}));
        b.condition = Some("x > 1".to_owned());
        b.log_message = Some("x={x}".to_owned());
        assert_eq!(
            b.to_dap(),
            json!({"line": 5, "condition": "x > 1", "logMessage": "x={x}"})
        );
    }

    #[test]
    fn output_waits_for_its_newline_and_loses_escape_sequences() {
        let mut session = DebugSession::new(1, ready_target());
        session.add_output("stdout", "hel");
        assert!(session.console.is_empty());
        session.add_output("stdout", "lo\n\u{1b}[31mred\u{1b}[0m\nrest");
        assert_eq!(session.console, ["hello", "red"]);
        session.finish_output();
        assert_eq!(session.console, ["hello", "red", "rest"]);
    }

    #[test]
    fn the_console_forgets_the_oldest_lines() {
        let mut session = DebugSession::new(1, ready_target());
        for n in 0..=MAX_CONSOLE_LINES {
            session.add_line(&n.to_string());
        }
        assert_eq!(session.console.len(), MAX_CONSOLE_LINES);
        assert_eq!(session.console[0], "1");
    }

    #[test]
    fn shadowed_variables_stay_apart_in_the_tree() {
        let mut seen = HashMap::new();
        let a = VarNode::from_dap(&[], &json!({"name": "x", "value": "1"}), &mut seen);
        let b = VarNode::from_dap(&[], &json!({"name": "x", "value": "2"}), &mut seen);
        assert_ne!(a.key, b.key);
        assert_eq!(b.name, "x");
    }

    #[test]
    fn escape_sequences_in_values_are_shown_not_acted_on() {
        let mut seen = HashMap::new();
        let node = VarNode::from_dap(
            &[],
            &json!({"name": "s", "value": "a\u{1b}[2Jb\nc"}),
            &mut seen,
        );
        assert_eq!(node.value, "a\\u{1b}[2Jb\\nc");
    }

    #[test]
    fn messages_that_are_not_the_protocol_are_refused() {
        assert_eq!(parse_incoming(&json!({"type": "nonsense"})), None);
        assert_eq!(parse_incoming(&json!({"type": "response"})), None);
        assert_eq!(parse_incoming(&json!(7)), None);
    }

    fn ready_target() -> DebugTarget {
        target(
            "t".to_owned(),
            &adapter(AdapterId::Debugpy),
            "launch",
            json!({}),
            None,
            Path::new("/p"),
        )
    }
}
