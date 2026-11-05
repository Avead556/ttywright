#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use ttywright::config::Config;
use ttywright::core::debug::{AdapterId, DebugPhase};
use ttywright::core::{Action, Command, Effect, Event, Workspace};
use ttywright::services::Services;

const WAIT: Duration = Duration::from_secs(20);

fn fake_adapter() -> String {
    let exe = std::env::current_exe().unwrap();
    let target = exe.parent().unwrap().parent().unwrap();
    target.join("examples/fake_dap").display().to_string()
}

pub struct Driver<F> {
    ws: Workspace,
    services: Services<F>,
    events: Receiver<Event>,
    dir: tempfile::TempDir,
    file: PathBuf,
    log: PathBuf,
}

fn driver(
    file_name: &str,
    adapter: AdapterId,
    words: Vec<String>,
) -> Driver<impl Fn(Event) + Clone + Send + 'static> {
    {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let file = root.join(file_name);
        std::fs::write(&file, "one\ntwo\nthree\nfour\nfive\n").unwrap();
        let log = root.join("adapter.log");
        let mut command = vec!["env".to_owned(), format!("FAKE_DAP_LOG={}", log.display())];
        command.extend(words);
        let mut config = Config::default();
        config.debug_adapters.insert(adapter, command);
        let (send, events) = channel();
        let emit = move |e| {
            let _ = send.send(e);
        };
        let services = Services::new(emit, None, &config);
        let (ws, _) = Workspace::new(root);
        Driver {
            ws,
            services,
            events,
            dir,
            file,
            log,
        }
    }
}

impl<F: Fn(Event) + Clone + Send + 'static> Driver<F> {
    fn apply(&mut self, effects: Vec<Effect>) {
        let mut queue = effects;
        while !queue.is_empty() {
            for effect in std::mem::take(&mut queue) {
                if let Some(event) = self.services.run(effect) {
                    queue.extend(self.ws.handle(event));
                }
            }
        }
    }

    fn command(&mut self, command: Command) {
        let effects = self.ws.dispatch(command);
        self.apply(effects);
    }

    fn wait(&mut self, what: &str, done: impl Fn(&Workspace) -> bool) {
        let started = Instant::now();
        while !done(&self.ws) {
            assert!(
                started.elapsed() < WAIT,
                "waiting for {what}: {:?}\n{}\nlog:\n{}",
                self.ws.debug_session().map(|s| s.phase.clone()),
                self.ws
                    .debug_session()
                    .map(|s| s.console.join("\n"))
                    .unwrap_or_default(),
                self.log(),
            );
            if let Ok(event) = self.events.recv_timeout(Duration::from_millis(100)) {
                let effects = self.ws.handle(event);
                self.apply(effects);
            }
        }
    }

    fn open_at_line(&mut self, line: usize) {
        let path = self.file.clone();
        self.command(Command::OpenFile {
            path,
            at: Some((line, 0)),
        });
    }

    fn choose_first(&mut self) {
        self.command(Command::Run(Action::DebugChoose));
        self.wait("the list", |ws| ws.picker().is_some());
        self.command(Command::Newline);
    }

    fn wait_paused(&mut self) {
        self.wait("the stop", |ws| {
            ws.debug_session().is_some_and(|s| {
                s.is_paused() && !s.frames.is_empty() && !ws.debug_var_rows().is_empty()
            })
        });
    }

    fn wait_ended(&mut self) -> String {
        self.wait("the end", |ws| {
            ws.debug_session().is_some_and(|s| !s.is_live())
        });
        match &self.ws.debug_session().unwrap().phase {
            DebugPhase::Ended { note } => note.clone(),
            other => panic!("{other:?}"),
        }
    }

    fn log(&self) -> String {
        std::fs::read_to_string(&self.log).unwrap_or_default()
    }

    fn value_of(&self, name: &str) -> Option<String> {
        self.ws
            .debug_var_rows()
            .into_iter()
            .find(|r| r.name == name)
            .map(|r| r.value)
    }

    fn root(&self) -> &Path {
        self.dir.path()
    }
}

fn stdio_driver(mode: &str) -> Driver<impl Fn(Event) + Clone + Send + 'static> {
    let words = vec![format!("FAKE_DAP_MODE={mode}"), fake_adapter()];
    driver("a.py", AdapterId::Debugpy, words)
}

#[test]
fn a_whole_session_from_a_breakpoint_to_the_exit_code() {
    let mut d = stdio_driver("normal");
    d.open_at_line(2);
    d.command(Command::Run(Action::ToggleBreakpoint));
    d.choose_first();
    d.wait_paused();

    let file = d.file.clone();
    assert_eq!(d.ws.debug_line(&file), Some(2));
    let frames = &d.ws.debug_session().unwrap().frames;
    assert!(frames[0].exists);
    assert!(!frames[1].exists);
    d.wait("the locals", |ws| {
        ws.debug_var_rows().iter().any(|r| r.name == "count")
    });
    assert_eq!(d.value_of("count").as_deref(), Some("3"));

    let at =
        d.ws.debug_var_rows()
            .iter()
            .position(|r| r.name == "point")
            .unwrap();
    d.command(Command::SelectDebugRow {
        part: ttywright::core::debug::DebugPart::Variables,
        index: at,
        open: true,
    });
    d.wait("a field", |ws| {
        ws.debug_var_rows().iter().any(|r| r.name == "x")
    });
    assert_eq!(d.value_of("x").as_deref(), Some("7"));

    d.command(Command::Run(Action::DebugStepOver));
    d.wait("the next line", |ws| ws.debug_line(&file) == Some(3));
    d.wait("new values", |ws| {
        ws.debug_var_rows()
            .iter()
            .any(|r| r.name == "count" && r.value == "4")
    });
    d.wait("the field again", |ws| {
        ws.debug_var_rows().iter().any(|r| r.name == "x")
    });

    d.command(Command::Run(Action::DebugAddWatch));
    d.command(Command::Insert("a+b".to_owned()));
    d.command(Command::Newline);
    d.wait("the watch", |ws| {
        ws.debug_var_rows()
            .iter()
            .any(|r| r.name == "a+b" && r.value == "<a+b>")
    });
    d.command(Command::Run(Action::DebugEvaluate));
    d.command(Command::Insert("broken".to_owned()));
    d.command(Command::Newline);
    d.wait("the refusal", |ws| {
        ws.debug_session()
            .unwrap()
            .console
            .iter()
            .any(|l| l.starts_with("broken:"))
    });

    d.command(Command::Run(Action::ResumeProgram));
    assert_eq!(d.wait_ended(), "Program exited with code 3");
    assert_eq!(d.ws.debug_line(&file), None);
    let console = &d.ws.debug_session().unwrap().console;
    assert!(console.iter().any(|l| l == "started"));
    assert!(console.iter().any(|l| l == "finished"));

    let log = d.log();
    let order = |word: &str| {
        log.find(word)
            .unwrap_or_else(|| panic!("no {word} in {log}"))
    };
    assert!(order("initialize") < order("launch"));
    assert!(order("setBreakpoints") < order("configurationDone"));
    assert!(log.contains(&format!("\"program\":\"{}\"", file.display())));
    assert!(log.contains("\"line\":3"));
    assert!(log.contains("disconnect {"));
    assert!(log.contains("\"terminateDebuggee\":true"));
    assert!(d.root().is_dir());
}

#[test]
fn a_refused_launch_says_why() {
    let mut d = stdio_driver("refuse-launch");
    d.open_at_line(0);
    d.choose_first();
    let note = d.wait_ended();
    assert!(note.contains("refused by the script"), "{note}");
}

#[test]
fn stop_ends_a_paused_program() {
    let mut d = stdio_driver("normal");
    d.open_at_line(1);
    d.command(Command::Run(Action::ToggleBreakpoint));
    d.choose_first();
    d.wait_paused();
    d.command(Command::Run(Action::DebugStop));
    assert_eq!(d.wait_ended(), "Stopped");
}

#[test]
fn an_adapter_that_ignores_everything_is_still_stopped() {
    let words = vec!["sleep".to_owned(), "1000".to_owned()];
    let mut d = driver("a.py", AdapterId::Debugpy, words);
    d.open_at_line(0);
    d.choose_first();
    d.wait("the start", |ws| ws.debug_session().is_some());
    d.command(Command::Run(Action::DebugStop));
    d.wait_ended();
}

#[test]
fn a_program_that_cannot_start_says_so() {
    let mut d = driver(
        "a.py",
        AdapterId::Debugpy,
        vec!["/no/such/adapter".to_owned()],
    );
    d.open_at_line(0);
    d.choose_first();
    let note = d.wait_ended();
    assert!(note.contains("adapter"), "{note}");
}

#[test]
fn a_listening_adapter_is_reached_on_a_port_of_its_own() {
    let words = vec![fake_adapter(), "--port".to_owned(), "{port}".to_owned()];
    let mut d = driver("a.js", AdapterId::JsDebug, words);
    d.open_at_line(1);
    d.command(Command::Run(Action::ToggleBreakpoint));
    d.choose_first();
    d.wait_paused();
    let file = d.file.clone();
    assert_eq!(d.ws.debug_line(&file), Some(1));
}

#[test]
fn a_child_session_is_where_the_program_stops() {
    let words = vec![
        "FAKE_DAP_MODE=child".to_owned(),
        fake_adapter(),
        "--port".to_owned(),
        "{port}".to_owned(),
    ];
    let mut d = driver("a.js", AdapterId::JsDebug, words);
    d.open_at_line(1);
    d.command(Command::Run(Action::ToggleBreakpoint));
    d.choose_first();
    d.wait_paused();
    d.command(Command::Run(Action::DebugStepOver));
    let file = d.file.clone();
    d.wait("the step in the child", |ws| {
        ws.debug_line(&file) == Some(2)
    });
    let log = d.log();
    assert!(log.contains("\"__child\":true"), "{log}");
    assert_eq!(log.matches("\ninitialize").count() + 1, 2, "{log}");
    d.command(Command::Run(Action::DebugStop));
    d.wait_ended();
}
