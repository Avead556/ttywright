#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::process::Command as Process;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use ttywright::config::Config;
use ttywright::core::debug::DebugPhase;
use ttywright::core::{Action, Command, Effect, Event, Workspace};
use ttywright::services::Services;

const WAIT: Duration = Duration::from_secs(120);

struct Driver<F> {
    ws: Workspace,
    services: Services<F>,
    events: Receiver<Event>,
}

fn driver(root: &Path) -> Driver<impl Fn(Event) + Clone + Send + 'static> {
    let (send, events) = channel();
    let emit = move |e| {
        let _ = send.send(e);
    };
    let services = Services::new(emit, None, &Config::default());
    let (ws, _) = Workspace::new(root.to_path_buf());
    Driver {
        ws,
        services,
        events,
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

    fn console(&self) -> String {
        self.ws
            .debug_session()
            .map(|s| s.console.join("\n"))
            .unwrap_or_default()
    }

    fn wait(&mut self, what: &str, done: impl Fn(&Workspace) -> bool) {
        let started = Instant::now();
        while !done(&self.ws) {
            assert!(
                started.elapsed() < WAIT,
                "waiting for {what}\nstate: {:?}\nconsole:\n{}",
                self.ws.debug_session().map(|s| s.phase.clone()),
                self.console()
            );
            if let Ok(event) = self.events.recv_timeout(Duration::from_millis(100)) {
                let effects = self.ws.handle(event);
                self.apply(effects);
            }
        }
    }

    fn value(&self, name: &str) -> Option<String> {
        self.ws
            .debug_var_rows()
            .into_iter()
            .find(|r| r.name == name)
            .map(|r| r.value)
    }

    fn the_usual(
        &mut self,
        file: &Path,
        stop_line: usize,
        variable: &str,
        value: &str,
        printed: &str,
    ) {
        self.command(Command::OpenFile {
            path: file.to_path_buf(),
            at: Some((stop_line, 0)),
        });
        self.command(Command::Run(Action::ToggleBreakpoint));
        self.command(Command::Run(Action::DebugChoose));
        self.wait("the list", |ws| ws.picker().is_some());
        self.command(Command::Newline);

        self.wait("the breakpoint", |ws| {
            ws.debug_line(file) == Some(stop_line)
        });
        self.wait("the value", |ws| {
            ws.debug_var_rows().iter().any(|r| r.name == variable)
        });
        let seen = self.value(variable).unwrap();
        assert!(seen.contains(value), "{variable} = {seen}");
        println!("stopped at {}, {variable} = {seen}", stop_line + 1);

        self.command(Command::Run(Action::DebugStepOver));
        self.wait("the step", |ws| {
            let line = ws.debug_line(file);
            line.is_some_and(|l| l > stop_line)
        });
        println!("stepped to {:?}", self.ws.debug_line(file).map(|l| l + 1));

        self.command(Command::Run(Action::ResumeProgram));
        self.wait("the end", |ws| {
            ws.debug_session().is_some_and(|s| !s.is_live())
        });
        let console = self.console();
        println!("{console}");
        assert!(console.contains(printed), "no {printed} in:\n{console}");
        let DebugPhase::Ended { note } = &self.ws.debug_session().unwrap().phase else {
            panic!("not ended");
        };
        println!("ended: {note}");
        assert_eq!(self.ws.debug_line(file), None);
    }
}

fn project() -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    (dir, root)
}

#[test]
#[ignore = "needs debugpy on PATH's python3"]
fn python_with_debugpy() {
    let (_dir, root) = project();
    let file = root.join("a.py");
    std::fs::write(
        &file,
        "total = 40\ntotal = total + 2\nprint('the answer', total)\n",
    )
    .unwrap();
    let mut d = driver(&root);
    d.the_usual(&file, 1, "total", "40", "the answer 42");
}

#[test]
#[ignore = "needs dlv on PATH"]
fn go_with_delve() {
    let (_dir, root) = project();
    std::fs::write(root.join("go.mod"), "module x\n\ngo 1.21\n").unwrap();
    let file = root.join("main.go");
    std::fs::write(
        &file,
        "package main\n\nimport \"fmt\"\n\nfunc main() {\n\ttotal := 40\n\ttotal = total + 2\n\tfmt.Println(\"the answer\", total)\n}\n",
    )
    .unwrap();
    let mut d = driver(&root);
    d.the_usual(&file, 6, "total", "40", "the answer 42");
}

#[test]
#[ignore = "needs lldb-dap or codelldb"]
fn rust_builds_and_stops() {
    let (_dir, root) = project();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"sample\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::create_dir_all(root.join("src")).unwrap();
    let file = root.join("src/main.rs");
    std::fs::write(
        &file,
        "fn main() {\n    let total = 40;\n    let answer = total + 2;\n    println!(\"the answer {answer}\");\n}\n",
    )
    .unwrap();
    let mut d = driver(&root);
    d.the_usual(&file, 2, "total", "40", "the answer 42");
}

#[test]
#[ignore = "needs lldb-dap or codelldb, and cc"]
fn c_through_a_configured_entry() {
    let (_dir, root) = project();
    let file = root.join("a.c");
    std::fs::write(
        &file,
        "#include <stdio.h>\nint main(void) {\n    int total = 40;\n    total = total + 2;\n    printf(\"the answer %d\\n\", total);\n    return 0;\n}\n",
    )
    .unwrap();
    let exe = root.join("a.out");
    let built = Process::new("cc")
        .args(["-g", "-O0", "-o"])
        .arg(&exe)
        .arg(&file)
        .status()
        .unwrap();
    assert!(built.success());
    let mut d = driver(&root);
    d.ws.configure_debug(vec![ttywright::core::debug::ConfiguredDebug {
        name: "the C program".to_owned(),
        adapter: ttywright::core::debug::AdapterId::LldbDap,
        request: "launch".to_owned(),
        settings: serde_json::json!({"program": exe}),
        cwd: None,
    }]);
    d.the_usual(&file, 3, "total", "40", "the answer 42");
}
