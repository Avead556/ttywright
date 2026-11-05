use std::path::{Path, PathBuf};

use serde_json::{Value, json};

use super::debug::{AdapterCommand, AdapterId, DebugFacts, DebugPhase, Transport};
use super::debug::{DebugEffect, DebugEvent};
use super::*;

const FILE: &str = "/proj/a.py";

struct Scene {
    ws: Workspace,
    sent: Vec<Value>,
    other: Vec<Effect>,
}

impl Scene {
    fn new(text: &str) -> Self {
        let (ws, _) = Workspace::new(PathBuf::from("/proj"));
        let mut scene = Self {
            ws,
            sent: Vec::new(),
            other: Vec::new(),
        };
        scene.open(FILE, text);
        scene
    }

    fn open(&mut self, path: &str, text: &str) {
        self.ws.dispatch(Command::OpenFile {
            path: PathBuf::from(path),
            at: None,
        });
        let effects = self.ws.handle(Event::FileRead {
            path: PathBuf::from(path),
            at: None,
            disk: None,
            result: Ok(Some(text.as_bytes().to_vec())),
        });
        self.take(effects);
    }

    fn take(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            match effect {
                Effect::Debug(DebugEffect::Send { message, .. }) => self.sent.push(message),
                Effect::Debug(DebugEffect::ProbeFiles {
                    session,
                    generation,
                    paths,
                }) => self.event(Event::Debug(DebugEvent::FilesProbed {
                    session,
                    generation,
                    existing: paths,
                })),
                other => self.other.push(other),
            }
        }
    }

    fn act(&mut self, action: Action) {
        let effects = self.ws.dispatch(Command::Run(action));
        self.take(effects);
    }

    fn command(&mut self, command: Command) {
        let effects = self.ws.dispatch(command);
        self.take(effects);
    }

    fn event(&mut self, event: Event) {
        let effects = self.ws.handle(event);
        self.take(effects);
    }

    fn go_to(&mut self, line: usize) {
        self.command(Command::OpenFile {
            path: PathBuf::from(FILE),
            at: Some((line, 0)),
        });
    }

    fn start(&mut self) {
        self.act(Action::DebugChoose);
        let adapter = AdapterCommand {
            id: AdapterId::Debugpy,
            program: "python3".to_owned(),
            args: Vec::new(),
            transport: Transport::Stdio,
        };
        self.event(Event::Debug(DebugEvent::Detected {
            facts: DebugFacts {
                adapters: [(AdapterId::Debugpy, adapter)].into(),
                file: Some(PathBuf::from(FILE)),
                ..DebugFacts::default()
            },
        }));
        self.command(Command::Newline);
    }

    fn adapter_says(&mut self, message: Value) {
        let session = self.ws.debug_session().map_or(0, |s| s.id);
        self.event(Event::Debug(DebugEvent::Message {
            session,
            conn: 0,
            message,
        }));
    }

    fn adapter_event(&mut self, event: &str, body: Value) {
        self.adapter_says(json!({"type": "event", "event": event, "body": body}));
    }

    fn answer(&mut self, command: &str, body: Value) {
        let seq = self.request(command)["seq"].clone();
        self.adapter_says(json!({
            "type": "response",
            "request_seq": seq,
            "success": true,
            "body": body,
        }));
    }

    fn refuse(&mut self, command: &str, why: &str) {
        let seq = self.request(command)["seq"].clone();
        self.adapter_says(json!({
            "type": "response",
            "request_seq": seq,
            "success": false,
            "message": why,
        }));
    }

    fn request(&self, command: &str) -> &Value {
        self.sent
            .iter()
            .rev()
            .find(|m| m["command"] == command)
            .unwrap_or_else(|| panic!("no {command} among {:?}", self.commands()))
    }

    fn commands(&self) -> Vec<String> {
        self.sent
            .iter()
            .map(|m| m["command"].as_str().unwrap_or("").to_owned())
            .collect()
    }

    fn pause_at(&mut self, line: i64) {
        self.adapter_event("stopped", json!({"reason": "breakpoint", "threadId": 1}));
        self.answer(
            "stackTrace",
            json!({"stackFrames": [
                {"id": 5, "name": "f", "line": line, "column": 1, "source": {"path": FILE}}
            ]}),
        );
    }

    fn phase(&self) -> DebugPhase {
        self.ws.debug_session().map(|s| s.phase.clone()).unwrap()
    }
}

#[test]
fn a_line_number_click_sets_and_clears_a_breakpoint() {
    let mut s = Scene::new("a\nb\nc\n");
    let view = s.ws.active_view().unwrap();
    s.command(Command::ToggleBreakpointAt { view, line: 1 });
    assert!(s.ws.breakpoint_at(Path::new(FILE), 1).is_some());
    s.command(Command::ToggleBreakpointAt { view, line: 1 });
    assert!(s.ws.breakpoint_at(Path::new(FILE), 1).is_none());
    s.command(Command::ToggleBreakpointAt { view, line: 99 });
    assert!(s.ws.debug().breakpoints.is_empty());
}

#[test]
fn a_breakpoint_follows_its_line_through_edits() {
    let mut s = Scene::new("a\nb\nc\n");
    s.go_to(2);
    s.act(Action::ToggleBreakpoint);
    s.go_to(0);
    s.command(Command::Insert("new\n".to_owned()));
    assert!(s.ws.breakpoint_at(Path::new(FILE), 3).is_some());
    assert!(s.ws.breakpoint_at(Path::new(FILE), 2).is_none());
}

#[test]
fn two_breakpoints_pushed_onto_one_line_become_one() {
    let mut s = Scene::new("a\nb\nc\n");
    let view = s.ws.active_view().unwrap();
    s.command(Command::ToggleBreakpointAt { view, line: 0 });
    s.command(Command::ToggleBreakpointAt { view, line: 1 });
    s.go_to(0);
    s.command(Command::Move(Motion::LineEnd));
    s.command(Command::DeleteForward);
    assert_eq!(s.ws.debug().breakpoints.len(), 1);
}

#[test]
fn breakpoints_and_watches_survive_a_restart_of_the_editor() {
    let mut s = Scene::new("a\nb\n");
    s.go_to(1);
    s.act(Action::EditBreakpoint);
    s.command(Command::Insert("x > 1".to_owned()));
    s.command(Command::Newline);
    s.act(Action::DebugAddWatch);
    s.command(Command::Insert("total".to_owned()));
    s.command(Command::Newline);
    let saved =
        s.ws.dispatch(Command::SaveSession)
            .into_iter()
            .find_map(|e| match e {
                Effect::WriteSession(session) => Some(session),
                _ => None,
            })
            .unwrap();

    let (ws, _) = Workspace::new(PathBuf::from("/proj"));
    let mut t = Scene {
        ws,
        sent: Vec::new(),
        other: Vec::new(),
    };
    t.event(Event::SessionLoaded {
        session: saved,
        files: Vec::new(),
    });
    let restored = t.ws.breakpoint_at(Path::new(FILE), 1).unwrap();
    assert_eq!(restored.condition.as_deref(), Some("x > 1"));
    assert!(restored.enabled);
    assert_eq!(t.ws.debug().watches, ["total"]);
}

#[test]
fn the_program_starts_only_after_the_breakpoints_are_set() {
    let mut s = Scene::new("a\nb\n");
    s.go_to(1);
    s.act(Action::ToggleBreakpoint);
    s.start();
    assert_eq!(s.commands(), ["initialize"]);
    s.answer(
        "initialize",
        json!({"exceptionBreakpointFilters": [
            {"filter": "raised", "default": false},
            {"filter": "uncaught", "default": true},
        ]}),
    );
    assert!(s.commands().contains(&"launch".to_owned()));
    s.adapter_event("initialized", json!({}));
    assert_eq!(
        &s.commands()[2..],
        [
            "setBreakpoints",
            "setExceptionBreakpoints",
            "configurationDone"
        ]
    );
    let breakpoints = s.request("setBreakpoints");
    assert_eq!(breakpoints["arguments"]["source"]["path"], FILE);
    assert_eq!(breakpoints["arguments"]["breakpoints"][0]["line"], 2);
    assert_eq!(
        s.request("setExceptionBreakpoints")["arguments"]["filters"],
        json!(["uncaught"])
    );
}

#[test]
fn a_refused_launch_ends_the_session_with_the_reason_in_sight() {
    let mut s = Scene::new("a\n");
    s.start();
    s.answer("initialize", json!({}));
    s.refuse("launch", "no such file");
    let DebugPhase::Ended { note } = s.phase() else {
        panic!("expected the end");
    };
    assert!(note.contains("no such file"));
    assert!(s.ws.status().unwrap().text.contains("no such file"));
    assert!(
        s.other
            .iter()
            .any(|e| matches!(e, Effect::Debug(DebugEffect::Stop { .. })))
    );
}

#[test]
fn a_stop_shows_the_line_and_opens_the_locals() {
    let mut s = Scene::new("a\nb\nc\n");
    s.start();
    s.pause_at(2);
    assert_eq!(s.ws.debug_line(Path::new(FILE)), Some(1));
    assert_eq!(s.ws.tool_tab(), ToolTab::Debug);
    assert_eq!(s.ws.focus(), Focus::Editor, "the keys stay where they were");
    s.answer(
        "scopes",
        json!({"scopes": [
            {"name": "Locals", "variablesReference": 7},
            {"name": "Globals", "variablesReference": 8},
        ]}),
    );
    assert_eq!(s.request("variables")["arguments"]["variablesReference"], 7);
    s.answer(
        "variables",
        json!({"variables": [{"name": "x", "value": "1", "variablesReference": 0}]}),
    );
    let rows = s.ws.debug_var_rows();
    let names: Vec<&str> = rows.iter().map(|r| r.name.as_str()).collect();
    assert_eq!(names, ["Locals", "x", "Globals"]);
}

#[test]
fn what_was_asked_before_a_step_cannot_paint_over_the_new_stop() {
    let mut s = Scene::new("a\nb\nc\n");
    s.start();
    s.pause_at(1);
    s.answer(
        "scopes",
        json!({"scopes": [{"name": "Locals", "variablesReference": 7}]}),
    );
    let stale = s.request("variables")["seq"].clone();
    s.pause_at(2);
    s.answer(
        "scopes",
        json!({"scopes": [{"name": "Locals", "variablesReference": 9}]}),
    );
    s.adapter_says(json!({
        "type": "response",
        "request_seq": stale,
        "success": true,
        "body": {"variables": [{"name": "old", "value": "1", "variablesReference": 0}]},
    }));
    let rows = s.ws.debug_var_rows();
    assert!(rows.iter().all(|r| r.name != "old"));
}

#[test]
fn what_was_opened_stays_open_from_one_stop_to_the_next() {
    let mut s = Scene::new("a\nb\nc\n");
    s.start();
    s.pause_at(1);
    s.answer(
        "scopes",
        json!({"scopes": [{"name": "Locals", "variablesReference": 7}]}),
    );
    s.answer(
        "variables",
        json!({"variables": [{"name": "p", "value": "{}", "variablesReference": 30}]}),
    );
    s.command(Command::SelectDebugRow {
        part: debug::DebugPart::Variables,
        index: 1,
        open: true,
    });
    assert_eq!(
        s.request("variables")["arguments"]["variablesReference"],
        30
    );

    s.pause_at(2);
    s.answer(
        "scopes",
        json!({"scopes": [{"name": "Locals", "variablesReference": 40}]}),
    );
    s.answer(
        "variables",
        json!({"variables": [{"name": "p", "value": "{}", "variablesReference": 41}]}),
    );
    assert_eq!(
        s.request("variables")["arguments"]["variablesReference"],
        41
    );
}

#[test]
fn stepping_is_refused_while_the_program_runs() {
    let mut s = Scene::new("a\nb\n");
    s.start();
    s.act(Action::DebugStepOver);
    assert!(!s.commands().contains(&"next".to_owned()));
    s.pause_at(1);
    s.act(Action::DebugStepOver);
    assert_eq!(s.request("next")["arguments"]["threadId"], 1);
    assert_eq!(s.phase(), DebugPhase::Running);
    assert_eq!(s.ws.debug_line(Path::new(FILE)), None);
}

#[test]
fn f9_resumes_only_when_there_is_something_paused() {
    let mut s = Scene::new("a\nb\n");
    s.act(Action::ResumeProgram);
    assert_eq!(
        s.ws.tool_tab(),
        ToolTab::Claude,
        "nothing paused: its old meaning"
    );

    let mut s = Scene::new("a\nb\n");
    s.start();
    s.pause_at(1);
    s.act(Action::ResumeProgram);
    assert_eq!(s.request("continue")["arguments"]["threadId"], 1);
}

#[test]
fn run_to_cursor_is_a_breakpoint_only_until_the_next_stop() {
    let mut s = Scene::new("a\nb\nc\nd\n");
    s.start();
    s.pause_at(1);
    s.go_to(3);
    s.act(Action::DebugRunToCursor);
    assert_eq!(
        s.request("setBreakpoints")["arguments"]["breakpoints"][0]["line"],
        4
    );
    assert!(s.commands().contains(&"continue".to_owned()));
    assert!(s.ws.debug().breakpoints.is_empty(), "the user set none");
    s.pause_at(4);
    assert_eq!(
        s.request("setBreakpoints")["arguments"]["breakpoints"],
        json!([])
    );
}

#[test]
fn a_disabled_breakpoint_is_kept_but_not_sent() {
    let mut s = Scene::new("a\nb\n");
    s.go_to(1);
    s.act(Action::ToggleBreakpoint);
    s.start();
    s.answer("initialize", json!({}));
    s.adapter_event("initialized", json!({}));
    s.go_to(1);
    s.act(Action::ToggleBreakpointEnabled);
    assert_eq!(
        s.request("setBreakpoints")["arguments"]["breakpoints"],
        json!([])
    );
    assert!(!s.ws.breakpoint_at(Path::new(FILE), 1).unwrap().enabled);
}

#[test]
fn a_breakpoint_bound_later_is_recognized_by_its_id() {
    let mut s = Scene::new("a\nb\n");
    s.go_to(1);
    s.act(Action::ToggleBreakpoint);
    s.start();
    s.answer("initialize", json!({}));
    s.adapter_event("initialized", json!({}));
    s.answer(
        "setBreakpoints",
        json!({"breakpoints": [{"id": 4, "verified": false, "message": "not loaded"}]}),
    );
    let state = |s: &Scene| {
        s.ws.breakpoint_at(Path::new(FILE), 1)
            .unwrap()
            .state
            .clone()
    };
    assert_eq!(
        state(&s),
        debug::BreakpointState::Pending(Some("not loaded".to_owned()))
    );
    s.adapter_event(
        "breakpoint",
        json!({"breakpoint": {"id": 4, "verified": true}}),
    );
    assert_eq!(state(&s), debug::BreakpointState::Verified);
}

#[test]
fn saving_sends_the_lines_where_they_are_now() {
    let mut s = Scene::new("a\nb\n");
    s.go_to(1);
    s.act(Action::ToggleBreakpoint);
    s.start();
    s.answer("initialize", json!({}));
    s.adapter_event("initialized", json!({}));
    s.go_to(0);
    s.command(Command::Insert("new\n".to_owned()));
    s.act(Action::Save);
    let (doc, version) = s
        .other
        .iter()
        .rev()
        .find_map(|e| match e {
            Effect::WriteFile { doc, version, .. } => Some((*doc, *version)),
            _ => None,
        })
        .unwrap();
    s.event(Event::FileWritten {
        doc,
        version,
        disk: None,
        origin: WriteOrigin::User,
    });
    assert_eq!(
        s.request("setBreakpoints")["arguments"]["breakpoints"][0]["line"],
        3
    );
}

#[test]
fn a_missing_adapter_is_explained_instead_of_starting_anything() {
    let mut s = Scene::new("a\n");
    s.act(Action::DebugChoose);
    s.event(Event::Debug(DebugEvent::Detected {
        facts: DebugFacts {
            file: Some(PathBuf::from(FILE)),
            ..DebugFacts::default()
        },
    }));
    s.command(Command::Newline);
    assert!(s.ws.debug_session().is_none());
    assert!(s.ws.status().unwrap().text.contains("pip install debugpy"));
}

#[test]
fn nothing_starts_until_a_row_is_chosen() {
    let mut s = Scene::new("a\n");
    s.act(Action::DebugChoose);
    assert!(
        s.other
            .iter()
            .any(|e| matches!(e, Effect::Debug(DebugEffect::Detect { .. })))
    );
    assert!(
        !s.other
            .iter()
            .any(|e| matches!(e, Effect::Debug(DebugEffect::Start { .. })))
    );
}

#[test]
fn a_child_session_gets_its_own_connection_and_the_settings_it_was_given() {
    let mut s = Scene::new("a\nb\n");
    s.go_to(1);
    s.act(Action::ToggleBreakpoint);
    s.start();
    s.adapter_says(json!({
        "seq": 9,
        "type": "request",
        "command": "startDebugging",
        "arguments": {"request": "launch", "configuration": {"__pendingTargetId": "x"}},
    }));
    assert!(
        s.other
            .iter()
            .any(|e| matches!(e, Effect::Debug(DebugEffect::Connect { conn: 1, .. })))
    );
    let reply = s.sent.iter().find(|m| m["request_seq"] == 9).unwrap();
    assert_eq!(reply["success"], true);
    let seq = s.request("initialize")["seq"].clone();
    let session = s.ws.debug_session().unwrap().id;
    s.event(Event::Debug(DebugEvent::Message {
        session,
        conn: 1,
        message: json!({"type": "response", "request_seq": seq, "success": true, "body": {}}),
    }));
    assert_eq!(s.request("launch")["arguments"]["__pendingTargetId"], "x");
}

#[test]
fn an_unknown_reverse_request_is_refused_not_ignored() {
    let mut s = Scene::new("a\n");
    s.start();
    s.adapter_says(json!({
        "seq": 3,
        "type": "request",
        "command": "runInTerminal",
        "arguments": {},
    }));
    let reply = s.sent.iter().find(|m| m["request_seq"] == 3).unwrap();
    assert_eq!(reply["success"], false);
}

#[test]
fn messages_of_a_replaced_session_change_nothing() {
    let mut s = Scene::new("a\nb\n");
    s.start();
    let old = s.ws.debug_session().unwrap().id;
    s.act(Action::DebugLast);
    let new = s.ws.debug_session().unwrap().id;
    assert_ne!(old, new);
    s.event(Event::Debug(DebugEvent::Message {
        session: old,
        conn: 0,
        message: json!({"type": "event", "event": "stopped", "body": {"threadId": 1}}),
    }));
    assert_eq!(s.phase(), DebugPhase::Starting);
    s.event(Event::Debug(DebugEvent::Ended {
        session: old,
        failure: Some("gone".to_owned()),
    }));
    assert_eq!(s.phase(), DebugPhase::Starting);
}

#[test]
fn the_end_reports_the_exit_code() {
    let mut s = Scene::new("a\n");
    s.start();
    s.adapter_event("exited", json!({"exitCode": 2}));
    s.adapter_event("terminated", json!({}));
    assert!(s.commands().contains(&"disconnect".to_owned()));
    let session = s.ws.debug_session().unwrap().id;
    s.event(Event::Debug(DebugEvent::Ended {
        session,
        failure: None,
    }));
    assert_eq!(
        s.phase(),
        DebugPhase::Ended {
            note: "Program exited with code 2".to_owned()
        }
    );
}

#[test]
fn values_are_written_after_lines_that_mention_them_up_to_the_stop() {
    let mut s = Scene::new("total = 1\nsubtotal = 2\nprint(total)\nlater = total\n");
    s.start();
    s.pause_at(3);
    s.answer(
        "scopes",
        json!({"scopes": [{"name": "Locals", "variablesReference": 7}]}),
    );
    s.answer(
        "variables",
        json!({"variables": [{"name": "total", "value": "1", "variablesReference": 0}]}),
    );
    let path = Path::new(FILE);
    assert_eq!(s.ws.debug_inline_values(path, 0, "total = 1"), ["total: 1"]);
    assert!(
        s.ws.debug_inline_values(path, 1, "subtotal = 2").is_empty(),
        "a whole name only"
    );
    assert!(
        s.ws.debug_inline_values(path, 3, "later = total")
            .is_empty(),
        "below the stop"
    );
}

#[test]
fn stopping_during_the_build_means_the_program_never_starts() {
    let mut s = Scene::new("a\n");
    s.act(Action::DebugChoose);
    let adapter = AdapterCommand {
        id: AdapterId::LldbDap,
        program: "lldb-dap".to_owned(),
        args: Vec::new(),
        transport: Transport::Stdio,
    };
    s.event(Event::Debug(DebugEvent::Detected {
        facts: DebugFacts {
            adapters: [(AdapterId::LldbDap, adapter)].into(),
            has_cargo: true,
            ..DebugFacts::default()
        },
    }));
    s.command(Command::Newline);
    assert_eq!(s.phase(), DebugPhase::Building);
    s.act(Action::DebugStop);
    assert_eq!(
        s.phase(),
        DebugPhase::Ended {
            note: "Stopped".to_owned()
        }
    );
    let session = s.ws.debug_session().unwrap().id;
    s.event(Event::Debug(DebugEvent::Built {
        session,
        result: Ok(vec!["/t/app".to_owned()]),
    }));
    assert!(
        !s.other
            .iter()
            .any(|e| matches!(e, Effect::Debug(DebugEffect::Start { .. })))
    );
}

#[test]
fn a_stopped_program_is_stopped_whatever_code_it_died_with() {
    let mut s = Scene::new("a\n");
    s.start();
    s.pause_at(1);
    s.act(Action::DebugStop);
    s.adapter_event("exited", json!({"exitCode": 247}));
    s.adapter_event("terminated", json!({}));
    let session = s.ws.debug_session().unwrap().id;
    s.event(Event::Debug(DebugEvent::Ended {
        session,
        failure: None,
    }));
    assert_eq!(
        s.phase(),
        DebugPhase::Ended {
            note: "Stopped".to_owned()
        }
    );
}
