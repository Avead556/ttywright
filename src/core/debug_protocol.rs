use std::collections::HashMap;
use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::command::{Command, Motion};
use super::debug::{
    Breakpoint, BreakpointState, DebugChoice, DebugEffect, DebugEvent, DebugFacts, DebugFrame,
    DebugPart, DebugPhase, DebugSession, DebugState, DebugTarget, Incoming, Pending, VarNode,
    VarRow, debug_choices, parse_frame, parse_incoming,
};
use super::picker::PickerKind;
use super::run::visible;

const MAX_FRAMES: i64 = 200;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DebugReply {
    Fx(DebugEffect),
    Info(String),
    Error(String),
    Pick(PickerKind),
    Show,
    Started,
    OpenFile {
        path: PathBuf,
        line: usize,
        col: usize,
    },
    Autosave,
}

impl DebugState {
    pub fn detect(
        &mut self,
        last: bool,
        root: &Path,
        file: Option<PathBuf>,
        out: &mut Vec<DebugReply>,
    ) {
        if last && let Some(target) = self.last.clone() {
            return self.start(target, out);
        }
        out.push(DebugReply::Autosave);
        self.wants_last = last;
        out.push(DebugReply::Fx(DebugEffect::Detect {
            root: root.to_path_buf(),
            file,
        }));
    }

    fn detected(&mut self, facts: &DebugFacts, root: &Path, out: &mut Vec<DebugReply>) {
        let choices = debug_choices(facts, root, &self.configured);
        if choices.is_empty() {
            return out.push(DebugReply::Info(
                "Nothing to debug here: open a source file, or add a [[debug]] entry".to_owned(),
            ));
        }
        if self.wants_last
            && let [DebugChoice::Ready(target)] = choices.as_slice()
        {
            let target = target.clone();
            return self.start(target, out);
        }
        self.choices = choices;
        out.push(DebugReply::Pick(PickerKind::DebugTargets));
    }

    pub fn choice_labels(&self) -> Vec<String> {
        self.choices.iter().map(DebugChoice::label).collect()
    }

    pub fn choice_taken(&mut self, index: usize, out: &mut Vec<DebugReply>) {
        match self.choices.get(index).cloned() {
            Some(DebugChoice::Ready(target)) => self.start(target, out),
            Some(DebugChoice::Missing { hint, .. }) => {
                out.push(DebugReply::Error(format!("Needed: {hint}")))
            }
            None => {}
        }
    }

    pub fn executable_labels(&self) -> Vec<String> {
        self.executables.iter().map(|e| visible(e)).collect()
    }

    pub fn executable_taken(&mut self, index: usize, out: &mut Vec<DebugReply>) {
        if let Some(program) = self.executables.get(index).cloned() {
            self.launch_program(program, out);
        }
    }

    pub fn thread_labels(&self) -> Vec<String> {
        self.session.as_ref().map_or_else(Vec::new, |s| {
            s.threads
                .iter()
                .map(|(id, name)| {
                    let mark = if s.thread == Some(*id) { "●" } else { " " };
                    format!("{mark} {name}")
                })
                .collect()
        })
    }

    pub fn thread_taken(&mut self, index: usize, out: &mut Vec<DebugReply>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(id) = session.threads.get(index).map(|t| t.0) else {
            return;
        };
        session.thread = Some(id);
        session.generation += 1;
        self.ask_stack(out);
    }

    pub fn start(&mut self, target: DebugTarget, out: &mut Vec<DebugReply>) {
        out.push(DebugReply::Autosave);
        self.end_quietly(out);
        self.last = Some(target.clone());
        self.next_session += 1;
        let mut session = DebugSession::new(self.next_session, target.clone());
        session.watch_values = vec![None; self.watches.len()];
        session.add_line(&format!("$ {}", adapter_line(&target)));
        for b in &mut self.breakpoints {
            b.state = BreakpointState::Unknown;
            b.id = None;
        }
        self.session = Some(session);
        out.push(DebugReply::Started);
        match target.build {
            Some(argv) => {
                if let Some(session) = self.session.as_mut() {
                    session.phase = DebugPhase::Building;
                    session.add_line(&format!("$ {}", argv.join(" ")));
                    out.push(DebugReply::Fx(DebugEffect::Build {
                        session: session.id,
                        root: target.cwd,
                        argv,
                    }));
                }
            }
            None => self.start_adapter(out),
        }
    }

    fn built(&mut self, id: u64, result: Result<Vec<String>, String>, out: &mut Vec<DebugReply>) {
        if self
            .session
            .as_ref()
            .is_none_or(|s| s.id != id || s.phase != DebugPhase::Building)
        {
            return;
        }
        match result {
            Err(message) => self.failed(out, &format!("Build failed: {message}")),
            Ok(executables) => match executables.as_slice() {
                [] => self.failed(out, "The build made nothing that can run"),
                [one] => self.launch_program(one.clone(), out),
                _ => {
                    self.executables = executables;
                    out.push(DebugReply::Pick(PickerKind::DebugExecutables));
                }
            },
        }
    }

    fn launch_program(&mut self, program: String, out: &mut Vec<DebugReply>) {
        if let Some(session) = self
            .session
            .as_mut()
            .filter(|s| s.phase == DebugPhase::Building)
        {
            session.target.settings["program"] = json!(program);
            self.start_adapter(out);
        }
    }

    fn start_adapter(&mut self, out: &mut Vec<DebugReply>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.phase = DebugPhase::Starting;
        let cwd = if session.target.adapter.id == crate::core::debug::AdapterId::Debugpy {
            PathBuf::from("/")
        } else {
            session.target.cwd.clone()
        };
        out.push(DebugReply::Fx(DebugEffect::Start {
            session: session.id,
            adapter: session.target.adapter.clone(),
            cwd,
        }));
        self.initialize(0, out);
    }

    fn initialize(&mut self, conn: u32, out: &mut Vec<DebugReply>) {
        let arguments = json!({
            "clientID": "ttywright",
            "clientName": "ttywright",
            "adapterID": self.session.as_ref().map_or("", |s|
                crate::core::debug::adapter_row(s.target.adapter.id).dap_type),
            "locale": "en",
            "linesStartAt1": true,
            "columnsStartAt1": true,
            "pathFormat": "path",
            "supportsVariableType": true,
            "supportsRunInTerminalRequest": false,
            "supportsStartDebuggingRequest": true,
        });
        self.send(conn, "initialize", &arguments, Pending::Initialize, out);
    }

    fn send(
        &mut self,
        conn: u32,
        command: &str,
        arguments: &Value,
        pending: Pending,
        out: &mut Vec<DebugReply>,
    ) {
        let Some(session) = self.session.as_mut().filter(|s| s.is_live()) else {
            return;
        };
        let seq = session.next_seq;
        session.next_seq += 1;
        session.pending.insert((conn, seq), pending);
        out.push(DebugReply::Fx(DebugEffect::Send {
            session: session.id,
            conn,
            message: json!({
                "seq": seq,
                "type": "request",
                "command": command,
                "arguments": arguments,
            }),
        }));
    }

    fn active(&self) -> u32 {
        self.session.as_ref().map_or(0, |s| s.active)
    }

    pub fn handle(&mut self, event: DebugEvent, root: &Path, out: &mut Vec<DebugReply>) {
        match event {
            DebugEvent::Detected { facts } => self.detected(&facts, root, out),
            DebugEvent::Built { session, result } => self.built(session, result, out),
            DebugEvent::Output { session, text } => {
                if let Some(s) = self.session.as_mut().filter(|s| s.id == session) {
                    s.add_line(&text);
                }
            }
            DebugEvent::Message {
                session,
                conn,
                message,
            } => {
                if self.session.as_ref().is_some_and(|s| s.id == session)
                    && let Some(incoming) = parse_incoming(&message)
                {
                    self.incoming(conn, incoming, out);
                }
            }
            DebugEvent::Ended { session, failure } => self.ended(session, failure, out),
            DebugEvent::FilesProbed {
                session,
                generation,
                existing,
            } => self.files_probed(session, generation, &existing, out),
        }
    }

    fn incoming(&mut self, conn: u32, incoming: Incoming, out: &mut Vec<DebugReply>) {
        match incoming {
            Incoming::Response {
                request_seq,
                success,
                message,
                body,
            } => {
                let pending = self
                    .session
                    .as_mut()
                    .and_then(|s| s.pending.remove(&(conn, request_seq)));
                if let Some(pending) = pending {
                    self.response(conn, pending, success, message, &body, out);
                }
            }
            Incoming::Event { event, body } => self.adapter_event(conn, &event, &body, out),
            Incoming::Request {
                seq,
                command,
                arguments,
            } => self.reverse_request(conn, seq, &command, &arguments, out),
        }
    }

    fn reverse_request(
        &mut self,
        conn: u32,
        seq: i64,
        command: &str,
        arguments: &Value,
        out: &mut Vec<DebugReply>,
    ) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let understood = command == "startDebugging";
        let our_seq = session.next_seq;
        session.next_seq += 1;
        out.push(DebugReply::Fx(DebugEffect::Send {
            session: session.id,
            conn,
            message: json!({
                "seq": our_seq,
                "type": "response",
                "request_seq": seq,
                "command": command,
                "success": understood,
            }),
        }));
        if !understood {
            return;
        }
        let child = session.next_conn;
        session.next_conn += 1;
        session.conns.insert(child);
        let request = arguments["request"].as_str().unwrap_or("launch").to_owned();
        let settings = arguments["configuration"].clone();
        session.child_settings.insert(child, (request, settings));
        out.push(DebugReply::Fx(DebugEffect::Connect {
            session: session.id,
            conn: child,
        }));
        self.initialize(child, out);
    }

    fn response(
        &mut self,
        conn: u32,
        pending: Pending,
        success: bool,
        message: Option<String>,
        body: &Value,
        out: &mut Vec<DebugReply>,
    ) {
        let why = message.unwrap_or_else(|| "no reason given".to_owned());
        match pending {
            Pending::Initialize if !success => {
                self.failed(out, &format!("The adapter refused to start: {why}"));
                self.shut_down(out);
            }
            Pending::Initialize => self.initialized_answer(conn, body, out),
            Pending::Launch if !success => {
                self.failed(out, &format!("Could not start: {why}"));
                self.shut_down(out);
            }
            Pending::Launch => {
                if let Some(s) = self.session.as_mut()
                    && s.phase == DebugPhase::Starting
                {
                    s.phase = DebugPhase::Running;
                }
            }
            Pending::SetBreakpoints { path, lines } => {
                self.breakpoints_answer(&path, &lines, success, &why, body);
            }
            Pending::Threads if success => {
                if let Some(s) = self.session.as_mut() {
                    s.threads = body["threads"]
                        .as_array()
                        .map(|t| {
                            t.iter()
                                .filter_map(|t| {
                                    let name = visible(t["name"].as_str().unwrap_or("?"));
                                    Some((t["id"].as_i64()?, name))
                                })
                                .collect()
                        })
                        .unwrap_or_default();
                }
            }
            Pending::Stack { generation } if self.is_now(generation) && success => {
                self.stack_answer(generation, body, out);
            }
            Pending::Scopes { generation } if self.is_now(generation) && success => {
                self.scopes_answer(generation, body, out);
            }
            Pending::Variables { key, generation } if self.is_now(generation) && success => {
                self.variables_answer(&key, generation, body, out);
            }
            Pending::Watch { index, generation } if self.is_now(generation) => {
                self.watch_answer(index, generation, success, &why, body, out);
            }
            Pending::Evaluate { expression } => {
                let text = if success {
                    let value = visible(body["result"].as_str().unwrap_or(""));
                    format!("{} = {value}", visible(&expression))
                } else {
                    format!("{}: {why}", visible(&expression))
                };
                if let Some(s) = self.session.as_mut() {
                    s.add_line(&text);
                    s.console_back = 0;
                }
                out.push(DebugReply::Info(text));
            }
            Pending::Other { what } if !success => {
                out.push(DebugReply::Error(format!("{what}: {why}")))
            }
            _ => {}
        }
    }

    fn is_now(&self, generation: u64) -> bool {
        self.session
            .as_ref()
            .is_some_and(|s| s.generation == generation)
    }

    fn initialized_answer(&mut self, conn: u32, body: &Value, out: &mut Vec<DebugReply>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let filters = body["exceptionBreakpointFilters"]
            .as_array()
            .map(|all| {
                all.iter()
                    .filter_map(|f| {
                        let name = f["filter"].as_str()?.to_owned();
                        Some((name, f["default"].as_bool().unwrap_or(false)))
                    })
                    .collect()
            })
            .unwrap_or_default();
        session.exception_filters.insert(conn, filters);
        let (request, settings) = session
            .child_settings
            .get(&conn)
            .cloned()
            .unwrap_or_else(|| {
                (
                    session.target.request.clone(),
                    session.target.settings.clone(),
                )
            });
        self.send(conn, &request, &settings, Pending::Launch, out);
    }

    fn breakpoints_answer(
        &mut self,
        path: &Path,
        lines: &[usize],
        success: bool,
        why: &str,
        body: &Value,
    ) {
        let answers = body["breakpoints"].as_array();
        for (index, line) in lines.iter().enumerate() {
            let Some(b) = self
                .breakpoints
                .iter_mut()
                .find(|b| b.line == *line && b.path == path)
            else {
                continue;
            };
            let answer = answers.and_then(|a| a.get(index));
            apply_breakpoint_answer(b, answer, success, why);
        }
    }

    fn adapter_event(&mut self, conn: u32, event: &str, body: &Value, out: &mut Vec<DebugReply>) {
        match event {
            "initialized" => self.configure_adapter(conn, out),
            "stopped" => self.stopped(conn, body, out),
            "continued" => {
                if let Some(s) = self.session.as_mut() {
                    s.phase = DebugPhase::Running;
                }
            }
            "output" => {
                let category = body["category"].as_str().unwrap_or("console");
                if category != "telemetry"
                    && let Some(s) = self.session.as_mut()
                {
                    s.add_output(category, body["output"].as_str().unwrap_or(""));
                }
            }
            "breakpoint" => {
                let item = &body["breakpoint"];
                if let Some(b) = item["id"]
                    .as_i64()
                    .and_then(|id| self.breakpoints.iter_mut().find(|b| b.id == Some(id)))
                {
                    apply_breakpoint_answer(b, Some(item), true, "");
                }
            }
            "exited" => {
                if let Some(s) = self.session.as_mut() {
                    s.exit_code = body["exitCode"].as_i64();
                }
            }
            "terminated" => self.terminated(conn, out),
            _ => {}
        }
    }

    fn configure_adapter(&mut self, conn: u32, out: &mut Vec<DebugReply>) {
        let mut paths: Vec<PathBuf> = self.breakpoints.iter().map(|b| b.path.clone()).collect();
        paths.dedup();
        for path in paths {
            self.send_breakpoints_on(conn, &path, out);
        }
        self.send_exceptions(conn, out);
        self.send(
            conn,
            "configurationDone",
            &json!({}),
            Pending::Other {
                what: "The adapter refused the settings",
            },
            out,
        );
    }

    fn send_exceptions(&mut self, conn: u32, out: &mut Vec<DebugReply>) {
        let all = self.all_exceptions;
        let filters: Vec<String> = self
            .session
            .as_ref()
            .and_then(|s| s.exception_filters.get(&conn))
            .map(|f| {
                f.iter()
                    .filter(|(_, default)| all || *default)
                    .map(|(name, _)| name.clone())
                    .collect()
            })
            .unwrap_or_default();
        self.send(
            conn,
            "setExceptionBreakpoints",
            &json!({"filters": filters}),
            Pending::Other {
                what: "Exception breakpoints",
            },
            out,
        );
    }

    pub fn toggle_exceptions(&mut self, out: &mut Vec<DebugReply>) {
        self.all_exceptions = !self.all_exceptions;
        let conns: Vec<u32> = self
            .session
            .as_ref()
            .filter(|s| s.is_live())
            .map(|s| s.conns.iter().copied().collect())
            .unwrap_or_default();
        for conn in conns {
            self.send_exceptions(conn, out);
        }
        out.push(DebugReply::Info(
            if self.all_exceptions {
                "Stopping at every exception"
            } else {
                "Stopping only at uncaught exceptions"
            }
            .to_owned(),
        ));
    }

    pub(super) fn send_breakpoints(&mut self, path: &Path, out: &mut Vec<DebugReply>) {
        let conns: Vec<u32> = self
            .session
            .as_ref()
            .filter(|s| s.is_live() && s.phase != DebugPhase::Building)
            .map(|s| s.conns.iter().copied().collect())
            .unwrap_or_default();
        for conn in conns {
            self.send_breakpoints_on(conn, path, out);
        }
    }

    fn send_breakpoints_on(&mut self, conn: u32, path: &Path, out: &mut Vec<DebugReply>) {
        let mut sent: Vec<Value> = Vec::new();
        let mut lines = Vec::new();
        for b in self
            .breakpoints
            .iter()
            .filter(|b| b.path == path && b.enabled)
        {
            sent.push(b.to_dap());
            lines.push(b.line);
        }
        if let Some(line) = self
            .session
            .as_ref()
            .and_then(|s| s.run_to.as_ref())
            .filter(|(p, _)| p == path)
            .map(|(_, line)| *line)
            .filter(|line| !lines.contains(line))
        {
            sent.push(json!({"line": line + 1}));
        }
        let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
        let arguments = json!({
            "source": {"path": path, "name": name},
            "breakpoints": sent,
            "sourceModified": false,
        });
        let pending = Pending::SetBreakpoints {
            path: path.to_path_buf(),
            lines,
        };
        self.send(conn, "setBreakpoints", &arguments, pending, out);
    }

    fn stopped(&mut self, conn: u32, body: &Value, out: &mut Vec<DebugReply>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.active = conn;
        let reason = body["description"]
            .as_str()
            .or_else(|| body["reason"].as_str())
            .unwrap_or("stopped");
        session.phase = DebugPhase::Paused {
            reason: visible(reason),
        };
        session.generation += 1;
        if let Some(thread) = body["threadId"].as_i64() {
            session.thread = Some(thread);
        }
        let run_to = session.run_to.take();
        if let Some((path, _)) = run_to {
            self.send_breakpoints(&path, out);
        }
        out.push(DebugReply::Show);
        self.send(conn, "threads", &json!({}), Pending::Threads, out);
        self.ask_stack(out);
    }

    fn ask_stack(&mut self, out: &mut Vec<DebugReply>) {
        let conn = self.active();
        let Some(session) = self.session.as_ref() else {
            return;
        };
        let generation = session.generation;
        let Some(thread) = session
            .thread
            .or_else(|| session.threads.first().map(|t| t.0))
        else {
            return out.push(DebugReply::Error(
                "The adapter stopped without saying which thread".to_owned(),
            ));
        };
        self.send(
            conn,
            "stackTrace",
            &json!({"threadId": thread, "startFrame": 0, "levels": MAX_FRAMES}),
            Pending::Stack { generation },
            out,
        );
    }

    fn stack_answer(&mut self, generation: u64, body: &Value, out: &mut Vec<DebugReply>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let frames: Vec<DebugFrame> = body["stackFrames"]
            .as_array()
            .map(|f| f.iter().filter_map(parse_frame).collect())
            .unwrap_or_default();
        let paths: Vec<PathBuf> = frames.iter().filter_map(|f| f.path.clone()).collect();
        session.frames = frames;
        session.frame = 0;
        out.push(DebugReply::Fx(DebugEffect::ProbeFiles {
            session: session.id,
            generation,
            paths,
        }));
    }

    fn files_probed(
        &mut self,
        id: u64,
        generation: u64,
        existing: &[PathBuf],
        out: &mut Vec<DebugReply>,
    ) {
        let Some(session) = self
            .session
            .as_mut()
            .filter(|s| s.id == id && s.generation == generation)
        else {
            return;
        };
        for frame in &mut session.frames {
            frame.exists = frame.path.as_ref().is_some_and(|p| existing.contains(p));
        }
        let first = session.frames.iter().position(|f| f.exists).unwrap_or(0);
        self.select_frame(first, out);
    }

    pub fn select_frame(&mut self, index: usize, out: &mut Vec<DebugReply>) {
        let conn = self.active();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(frame) = session.frames.get(index).cloned() else {
            return;
        };
        session.frame = index;
        let generation = session.generation;
        self.send(
            conn,
            "scopes",
            &json!({"frameId": frame.id}),
            Pending::Scopes { generation },
            out,
        );
        self.refresh_watches(out);
        if let (true, Some(path)) = (frame.exists, frame.path) {
            out.push(DebugReply::OpenFile {
                path,
                line: frame.line,
                col: frame.col,
            });
        }
    }

    fn scopes_answer(&mut self, generation: u64, body: &Value, out: &mut Vec<DebugReply>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut seen = HashMap::new();
        let scopes: Vec<VarNode> = body["scopes"]
            .as_array()
            .map(|all| {
                all.iter()
                    .map(|scope| {
                        let item = json!({
                            "name": scope["name"],
                            "value": "",
                            "variablesReference": scope["variablesReference"],
                        });
                        VarNode::from_dap(&[], &item, &mut seen)
                    })
                    .collect()
            })
            .unwrap_or_default();
        if let Some(first) = scopes.first()
            && session.seen_scopes.insert(first.name.clone())
        {
            session.expanded.insert(first.key.clone());
        }
        let wanted: Vec<VarNode> = scopes
            .iter()
            .filter(|s| session.expanded.contains(&s.key))
            .cloned()
            .collect();
        session.scopes = scopes;
        for node in wanted {
            self.ask_variables(&node, generation, out);
        }
    }

    fn ask_variables(&mut self, node: &VarNode, generation: u64, out: &mut Vec<DebugReply>) {
        if node.reference <= 0 {
            return;
        }
        let conn = self.active();
        self.send(
            conn,
            "variables",
            &json!({"variablesReference": node.reference}),
            Pending::Variables {
                key: node.key.clone(),
                generation,
            },
            out,
        );
    }

    fn variables_answer(
        &mut self,
        key: &[String],
        generation: u64,
        body: &Value,
        out: &mut Vec<DebugReply>,
    ) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let mut seen = HashMap::new();
        let children: Vec<VarNode> = body["variables"]
            .as_array()
            .map(|all| {
                all.iter()
                    .map(|item| VarNode::from_dap(key, item, &mut seen))
                    .collect()
            })
            .unwrap_or_default();
        let wanted: Vec<VarNode> = children
            .iter()
            .filter(|c| session.expanded.contains(&c.key))
            .cloned()
            .collect();
        let node = session
            .scopes
            .iter_mut()
            .chain(session.watch_values.iter_mut().flatten())
            .find_map(|root| root.find_mut(key));
        if let Some(node) = node {
            node.children = Some(children);
        }
        for child in wanted {
            self.ask_variables(&child, generation, out);
        }
    }

    pub fn refresh_watches(&mut self, out: &mut Vec<DebugReply>) {
        let conn = self.active();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.watch_values.resize(self.watches.len(), None);
        if !session.is_paused() {
            return;
        }
        let generation = session.generation;
        let frame = session.frames.get(session.frame).map(|f| f.id);
        let watches = self.watches.clone();
        for (index, expression) in watches.iter().enumerate() {
            let arguments = json!({
                "expression": expression,
                "frameId": frame,
                "context": "watch",
            });
            self.send(
                conn,
                "evaluate",
                &arguments,
                Pending::Watch { index, generation },
                out,
            );
        }
    }

    fn watch_answer(
        &mut self,
        index: usize,
        generation: u64,
        success: bool,
        why: &str,
        body: &Value,
        out: &mut Vec<DebugReply>,
    ) {
        let Some(expression) = self.watches.get(index).cloned() else {
            return;
        };
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let item = if success {
            json!({
                "name": expression,
                "value": body["result"],
                "type": body["type"],
                "variablesReference": body["variablesReference"],
            })
        } else {
            json!({"name": expression, "value": why, "variablesReference": 0})
        };
        let mut node = VarNode::from_dap(&[], &item, &mut HashMap::new());
        node.key = vec![format!("watch:{expression}")];
        let wanted = session.expanded.contains(&node.key).then(|| node.clone());
        if let Some(slot) = session.watch_values.get_mut(index) {
            *slot = Some(node);
        }
        if let Some(node) = wanted {
            self.ask_variables(&node, generation, out);
        }
    }

    fn paused_thread(&self, out: &mut Vec<DebugReply>) -> Option<i64> {
        let thread = self
            .session
            .as_ref()
            .filter(|s| s.is_paused())
            .and_then(|s| s.thread);
        if thread.is_none() {
            out.push(DebugReply::Info("The program is not paused".to_owned()));
        }
        thread
    }

    fn set_running(&mut self) {
        if let Some(s) = self.session.as_mut() {
            s.phase = DebugPhase::Running;
        }
    }

    pub fn step(&mut self, command: &'static str, out: &mut Vec<DebugReply>) {
        let Some(thread) = self.paused_thread(out) else {
            return;
        };
        out.push(DebugReply::Autosave);
        let conn = self.active();
        self.set_running();
        self.send(
            conn,
            command,
            &json!({"threadId": thread}),
            Pending::Other {
                what: "Could not step",
            },
            out,
        );
    }

    pub fn resume(&mut self, out: &mut Vec<DebugReply>) {
        let Some(thread) = self.paused_thread(out) else {
            return;
        };
        let conn = self.active();
        self.set_running();
        self.send(
            conn,
            "continue",
            &json!({"threadId": thread}),
            Pending::Other {
                what: "Could not resume",
            },
            out,
        );
    }

    pub fn pause(&mut self, out: &mut Vec<DebugReply>) {
        let Some(session) = self
            .session
            .as_ref()
            .filter(|s| s.phase == DebugPhase::Running)
        else {
            return out.push(DebugReply::Info("The program is not running".to_owned()));
        };
        let thread = session
            .thread
            .or_else(|| session.threads.first().map(|t| t.0))
            .unwrap_or(0);
        let conn = session.active;
        self.send(
            conn,
            "pause",
            &json!({"threadId": thread}),
            Pending::Other {
                what: "Could not pause",
            },
            out,
        );
    }

    pub fn run_to_cursor(&mut self, path: PathBuf, line: usize, out: &mut Vec<DebugReply>) {
        if self.paused_thread(out).is_none() {
            return;
        }
        if let Some(s) = self.session.as_mut() {
            s.run_to = Some((path.clone(), line));
        }
        self.send_breakpoints(&path, out);
        self.resume(out);
    }

    pub fn evaluate(&mut self, expression: String, out: &mut Vec<DebugReply>) {
        let Some(session) = self.session.as_ref().filter(|s| s.is_paused()) else {
            return out.push(DebugReply::Info("Pause the program to evaluate".to_owned()));
        };
        let conn = session.active;
        let frame = session.frames.get(session.frame).map(|f| f.id);
        self.send(
            conn,
            "evaluate",
            &json!({"expression": expression, "frameId": frame, "context": "repl"}),
            Pending::Evaluate { expression },
            out,
        );
    }

    pub fn stop(&mut self, out: &mut Vec<DebugReply>) {
        if !self.session.as_ref().is_some_and(DebugSession::is_live) {
            return out.push(DebugReply::Info("Nothing is being debugged".to_owned()));
        }
        self.shut_down(out);
    }

    fn shut_down(&mut self, out: &mut Vec<DebugReply>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if session.stopping {
            return;
        }
        session.stopping = true;
        let id = session.id;
        let building = session.phase == DebugPhase::Building;
        let conns: Vec<u32> = session.conns.iter().rev().copied().collect();
        if building {
            session.add_line("Stopped");
            session.phase = DebugPhase::Ended {
                note: "Stopped".to_owned(),
            };
            return;
        }
        {
            for conn in conns {
                self.send_disconnect(conn, out);
            }
        }
        out.push(DebugReply::Fx(DebugEffect::Stop { session: id }));
    }

    fn send_disconnect(&mut self, conn: u32, out: &mut Vec<DebugReply>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let seq = session.next_seq;
        session.next_seq += 1;
        out.push(DebugReply::Fx(DebugEffect::Send {
            session: session.id,
            conn,
            message: json!({
                "seq": seq,
                "type": "request",
                "command": "disconnect",
                "arguments": {"terminateDebuggee": true, "restart": false},
            }),
        }));
    }

    fn terminated(&mut self, conn: u32, out: &mut Vec<DebugReply>) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if conn != 0 {
            session.conns.remove(&conn);
            if session.active == conn {
                session.active = 0;
                session.phase = DebugPhase::Running;
                session.frames.clear();
            }
            return;
        }
        session.program_over = !session.stopping;
        self.shut_down(out);
    }

    fn end_quietly(&mut self, out: &mut Vec<DebugReply>) {
        if self.session.as_ref().is_some_and(DebugSession::is_live) {
            self.shut_down(out);
        }
    }

    fn failed(&mut self, out: &mut Vec<DebugReply>, message: &str) {
        if let Some(s) = self.session.as_mut() {
            s.finish_output();
            s.add_line(message);
            s.phase = DebugPhase::Ended {
                note: message.to_owned(),
            };
        }
        out.push(DebugReply::Error(message.to_owned()));
    }

    fn ended(&mut self, id: u64, failure: Option<String>, out: &mut Vec<DebugReply>) {
        let Some(session) = self.session.as_mut().filter(|s| s.id == id) else {
            return;
        };
        if !session.is_live() {
            return;
        }
        session.finish_output();
        let asked = session.stopping;
        let code = session.exit_code;
        let over = session.program_over;
        let note = match (failure, code, asked && !over) {
            (_, _, true) => "Stopped".to_owned(),
            (Some(failure), _, _) if !asked => failure,
            (_, Some(code), _) => format!("Program exited with code {code}"),
            (_, None, _) if over => "Program ended".to_owned(),
            (_, None, _) => "The debug adapter ended".to_owned(),
        };
        session.add_line(&note);
        session.phase = DebugPhase::Ended { note: note.clone() };
        session.frames.clear();
        session.scopes.clear();
        session.watch_values.iter_mut().for_each(|v| *v = None);
        session.var_selected = 0;
        session.pending.clear();
        out.push(DebugReply::Info(note));
    }

    pub fn input(&mut self, input: Command, out: &mut Vec<DebugReply>) {
        match input {
            Command::Indent => self.change_part(DebugPart::next),
            Command::Outdent => self.change_part(DebugPart::previous),
            Command::Newline => self.enter(out),
            Command::Move(motion) => self.move_by(motion, out),
            Command::Insert(text) => self.letter(&text, out),
            _ => {}
        }
    }

    fn change_part(&mut self, change: fn(DebugPart) -> DebugPart) {
        if let Some(s) = self.session.as_mut() {
            s.part = change(s.part);
        }
    }

    fn letter(&mut self, text: &str, out: &mut Vec<DebugReply>) {
        match text {
            "c" => self.resume(out),
            "n" => self.step("next", out),
            "i" => self.step("stepIn", out),
            "o" => self.step("stepOut", out),
            "p" => self.pause(out),
            "s" => self.stop(out),
            "d" => self.remove_watch(out),
            "t" => {
                if self.session.as_ref().is_some_and(|s| !s.threads.is_empty()) {
                    out.push(DebugReply::Pick(PickerKind::DebugThreads));
                }
            }
            _ => {}
        }
    }

    fn remove_watch(&mut self, out: &mut Vec<DebugReply>) {
        let rows = self.var_rows();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let Some(index) = rows.get(session.var_selected).and_then(|r| r.watch) else {
            return;
        };
        self.watches.remove(index);
        if index < session.watch_values.len() {
            session.watch_values.remove(index);
        }
        self.refresh_watches(out);
    }

    fn move_by(&mut self, motion: Motion, out: &mut Vec<DebugReply>) {
        let rows = self.var_rows();
        let Some(session) = self.session.as_mut() else {
            return;
        };
        let step: isize = match motion {
            Motion::Up => -1,
            Motion::Down => 1,
            Motion::PageUp { rows } => -isize::try_from(rows).unwrap_or(isize::MAX),
            Motion::PageDown { rows } => isize::try_from(rows).unwrap_or(isize::MAX),
            Motion::Left | Motion::Right if session.part == DebugPart::Variables => {
                let open = motion == Motion::Right;
                let row = rows.get(session.var_selected).cloned();
                if let Some(row) = row.filter(|r| r.expandable && r.expanded != open) {
                    self.toggle(&row, out);
                }
                return;
            }
            _ => return,
        };
        match session.part {
            DebugPart::Frames => {
                let moved = shifted(session.frame, step, session.frames.len());
                if moved != session.frame {
                    self.select_frame(moved, out);
                }
            }
            DebugPart::Variables => {
                session.var_selected = shifted(session.var_selected, step, rows.len());
            }
            DebugPart::Console => {
                let most = session.console.len().saturating_sub(1);
                session.console_back = session.console_back.saturating_add_signed(-step).min(most);
            }
        }
    }

    fn enter(&mut self, out: &mut Vec<DebugReply>) {
        let Some(session) = self.session.as_ref() else {
            return;
        };
        match session.part {
            DebugPart::Frames => {
                let frame = session.frame;
                self.select_frame(frame, out);
            }
            DebugPart::Variables => {
                let selected = session.var_selected;
                if let Some(row) = self.var_rows().get(selected).cloned() {
                    self.toggle(&row, out);
                }
            }
            DebugPart::Console => {}
        }
    }

    fn toggle(&mut self, row: &VarRow, out: &mut Vec<DebugReply>) {
        if !row.expandable {
            return;
        }
        let Some(session) = self.session.as_mut() else {
            return;
        };
        if !session.expanded.remove(&row.key) {
            session.expanded.insert(row.key.clone());
            let generation = session.generation;
            let node = session
                .scopes
                .iter_mut()
                .chain(session.watch_values.iter_mut().flatten())
                .find_map(|root| root.find_mut(&row.key))
                .filter(|n| n.children.is_none())
                .map(|n| n.clone());
            if let Some(node) = node {
                self.ask_variables(&node, generation, out);
            }
        }
    }

    pub fn row_clicked(
        &mut self,
        part: DebugPart,
        index: usize,
        open: bool,
        out: &mut Vec<DebugReply>,
    ) {
        let Some(session) = self.session.as_mut() else {
            return;
        };
        session.part = part;
        match part {
            DebugPart::Frames => {
                if index < session.frames.len() && (open || index != session.frame) {
                    self.select_frame(index, out);
                }
            }
            DebugPart::Variables => {
                session.var_selected = index;
                if open {
                    self.enter(out);
                }
            }
            DebugPart::Console => {}
        }
    }

    pub fn scroll_console(&mut self, delta: isize) {
        if let Some(s) = self.session.as_mut() {
            let most = s.console.len().saturating_sub(1);
            s.console_back = s.console_back.saturating_add_signed(delta).min(most);
        }
    }
}

fn shifted(from: usize, step: isize, len: usize) -> usize {
    from.saturating_add_signed(step).min(len.saturating_sub(1))
}

fn apply_breakpoint_answer(b: &mut Breakpoint, answer: Option<&Value>, success: bool, why: &str) {
    if !success {
        b.state = BreakpointState::Pending(Some(why.to_owned()));
        return;
    }
    let Some(answer) = answer else {
        b.state = BreakpointState::Pending(None);
        return;
    };
    b.id = answer["id"].as_i64().or(b.id);
    b.state = if answer["verified"].as_bool().unwrap_or(false) {
        BreakpointState::Verified
    } else {
        BreakpointState::Pending(answer["message"].as_str().map(visible))
    };
}

fn adapter_line(target: &DebugTarget) -> String {
    let mut words = vec![visible(&target.adapter.program)];
    words.extend(target.adapter.args.iter().map(|a| visible(a)));
    words.join(" ")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::debug::{AdapterCommand, AdapterId, Transport};

    fn target(build: Option<Vec<String>>) -> DebugTarget {
        DebugTarget {
            name: "a.py".to_owned(),
            adapter: AdapterCommand {
                id: AdapterId::Debugpy,
                program: "python3".to_owned(),
                args: Vec::new(),
                transport: Transport::Stdio,
            },
            request: "launch".to_owned(),
            settings: json!({}),
            build,
            cwd: PathBuf::from("/p"),
        }
    }

    fn commands(out: &[DebugReply]) -> Vec<String> {
        out.iter()
            .filter_map(|r| match r {
                DebugReply::Fx(DebugEffect::Send { message, .. }) => {
                    message["command"].as_str().map(str::to_owned)
                }
                _ => None,
            })
            .collect()
    }

    #[test]
    fn a_session_saves_first_shows_itself_and_greets_the_adapter() {
        let mut state = DebugState::default();
        let mut out = Vec::new();
        state.start(target(None), &mut out);
        assert_eq!(out.first(), Some(&DebugReply::Autosave));
        assert!(out.contains(&DebugReply::Started));
        assert!(
            out.iter()
                .any(|r| matches!(r, DebugReply::Fx(DebugEffect::Start { .. })))
        );
        assert_eq!(commands(&out), ["initialize"]);
        assert!(out.iter().all(|r| match r {
            DebugReply::Fx(e) => !matches!(e, DebugEffect::Detect { .. }),
            _ => true,
        }));
    }

    #[test]
    fn stopping_while_it_builds_ends_it_with_nothing_left_to_stop() {
        let mut state = DebugState::default();
        let mut out = Vec::new();
        state.start(
            target(Some(vec!["cargo".to_owned(), "build".to_owned()])),
            &mut out,
        );
        assert!(
            out.iter()
                .any(|r| matches!(r, DebugReply::Fx(DebugEffect::Build { .. })))
        );
        out.clear();
        state.stop(&mut out);
        assert!(out.is_empty(), "{out:?}");
        let session = state.session.as_ref().expect("kept for its console");
        assert!(!session.is_live());
        out.clear();
        state.stop(&mut out);
        assert_eq!(
            out,
            [DebugReply::Info("Nothing is being debugged".to_owned())]
        );
    }

    #[test]
    fn a_stop_shows_the_panel_and_asks_where_without_taking_the_keys() {
        let mut state = DebugState::default();
        let mut out = Vec::new();
        state.start(target(None), &mut out);
        out.clear();
        let id = state.session.as_ref().map_or(0, |s| s.id);
        let stopped = json!({
            "type": "event", "event": "stopped",
            "body": {"reason": "breakpoint", "threadId": 1},
        });
        state.handle(
            DebugEvent::Message {
                session: id,
                conn: 0,
                message: stopped,
            },
            Path::new("/p"),
            &mut out,
        );
        assert_eq!(out.first(), Some(&DebugReply::Show));
        assert!(!out.contains(&DebugReply::Started));
        assert_eq!(commands(&out), ["threads", "stackTrace"]);
    }
}
