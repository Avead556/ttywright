use crate::core::debug::DebugEvent;
use std::collections::{BTreeMap, HashMap};
use std::io::{BufRead, BufReader, Read, Write};
use std::net::{TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::Value;

use super::lsp::transport::{FrameDecoder, encode};
use super::run::{self, Guardian};
use crate::core::Event;
use crate::core::debug::{
    ADAPTERS, AdapterCommand, AdapterId, AdapterRow, DebugFacts, PORT_PLACEHOLDER, Transport,
    cargo_executables,
};

const CONNECT_WAIT: Duration = Duration::from_secs(30);
const DISCONNECT_WAIT: Duration = Duration::from_secs(2);
const TERM_WAIT: Duration = Duration::from_millis(1500);
const POLL: Duration = Duration::from_millis(50);

struct Adapter {
    conns: HashMap<u32, Sender<Vec<u8>>>,
    port: Option<u16>,
    group: u32,
    over: Arc<AtomicBool>,
}

#[derive(Default)]
pub struct DebugHost {
    adapters: HashMap<u64, Adapter>,
    launcher: Option<PathBuf>,
}

fn free_port() -> std::io::Result<u16> {
    Ok(TcpListener::bind(("127.0.0.1", 0))?.local_addr()?.port())
}

fn read_messages(
    mut reader: impl Read,
    session: u64,
    conn: u32,
    emit: &(impl Fn(Event) + Send + 'static),
) {
    let mut decoder = FrameDecoder::default();
    let mut buffer = [0u8; 16 * 1024];
    loop {
        match reader.read(&mut buffer) {
            Ok(0) | Err(_) => return,
            Ok(n) => decoder.push(&buffer[..n]),
        }
        while let Some(message) = decoder.next_message() {
            match message {
                Ok(message) => emit(Event::Debug(DebugEvent::Message {
                    session,
                    conn,
                    message,
                })),
                Err(e) => emit(Event::Debug(DebugEvent::Output {
                    session,
                    text: format!("Bad message from the adapter: {e}"),
                })),
            }
        }
    }
}

fn write_messages(mut writer: impl Write, messages: &Receiver<Vec<u8>>) {
    while let Ok(bytes) = messages.recv() {
        if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
            return;
        }
    }
}

fn connect(port: u16, over: &AtomicBool) -> Option<TcpStream> {
    let started = Instant::now();
    while started.elapsed() < CONNECT_WAIT && !over.load(Ordering::Relaxed) {
        if let Ok(stream) = TcpStream::connect(("127.0.0.1", port)) {
            return Some(stream);
        }
        thread::sleep(POLL);
    }
    None
}

fn start_tcp_conn<F>(
    session: u64,
    conn: u32,
    port: u16,
    over: Arc<AtomicBool>,
    emit: F,
) -> Sender<Vec<u8>>
where
    F: Fn(Event) + Clone + Send + 'static,
{
    let (send, receive) = channel();
    thread::spawn(move || {
        let Some(stream) = connect(port, &over) else {
            if !over.load(Ordering::Relaxed) {
                emit(Event::Debug(DebugEvent::Output {
                    session,
                    text: format!("The adapter did not open port {port}"),
                }));
            }
            return;
        };
        match stream.try_clone() {
            Ok(writer) => {
                thread::spawn(move || write_messages(writer, &receive));
            }
            Err(e) => {
                emit(Event::Debug(DebugEvent::Output {
                    session,
                    text: format!("Cannot write to the adapter: {e}"),
                }));
                return;
            }
        }
        read_messages(stream, session, conn, &emit);
    });
    send
}

impl DebugHost {
    pub fn use_launcher(&mut self, program: Option<PathBuf>) {
        self.launcher = program;
    }

    pub fn start<F>(&mut self, session: u64, adapter: &AdapterCommand, cwd: &Path, emit: F)
    where
        F: Fn(Event) + Clone + Send + 'static,
    {
        if let Err(message) = self.try_start(session, adapter, cwd, emit.clone()) {
            emit(Event::Debug(DebugEvent::Ended {
                session,
                failure: Some(message),
            }));
        }
    }

    fn try_start<F>(
        &mut self,
        session: u64,
        adapter: &AdapterCommand,
        cwd: &Path,
        emit: F,
    ) -> Result<(), String>
    where
        F: Fn(Event) + Clone + Send + 'static,
    {
        let name = &adapter.program;
        let describe = |e: &dyn std::fmt::Display| format!("Cannot start {name}: {e}");
        let port = match adapter.transport {
            Transport::Stdio => None,
            Transport::Tcp => Some(free_port().map_err(|e| describe(&e))?),
        };
        let args = adapter.args.iter().map(|arg| match port {
            Some(port) => arg.replace(PORT_PLACEHOLDER, &port.to_string()),
            None => arg.clone(),
        });
        let launcher = self.launcher.as_ref().filter(|l| l.is_file());
        let mut command = match launcher {
            Some(launcher) => {
                let mut command = Command::new(launcher);
                command.arg(run::LAUNCHER_FLAG).arg(name);
                command
            }
            None => {
                use std::os::unix::process::CommandExt as _;
                let mut command = Command::new(name);
                command.process_group(0);
                command
            }
        };
        let mut child = command
            .args(args)
            .current_dir(cwd)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .env("PYTHONUNBUFFERED", "1")
            .env("PYDEVD_DISABLE_FILE_VALIDATION", "1")
            .spawn()
            .map_err(|e| describe(&e))?;
        let group = child.id();
        let mut guardian = Guardian::start().map_err(|e| describe(&e))?;
        guardian.watch(group);
        let over = Arc::new(AtomicBool::new(false));

        if let Some(stderr) = child.stderr.take() {
            let emit = emit.clone();
            thread::spawn(move || {
                for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                    emit(Event::Debug(DebugEvent::Output {
                        session,
                        text: line,
                    }));
                }
            });
        }
        let stdin = child.stdin.take();
        let stdout = child.stdout.take();
        let first = match port {
            Some(port) => {
                if let Some(stdout) = stdout {
                    let emit = emit.clone();
                    thread::spawn(move || {
                        for line in BufReader::new(stdout).lines().map_while(Result::ok) {
                            emit(Event::Debug(DebugEvent::Output {
                                session,
                                text: line,
                            }));
                        }
                    });
                }
                start_tcp_conn(session, 0, port, Arc::clone(&over), emit.clone())
            }
            None => {
                let (send, receive) = channel();
                if let Some(stdin) = stdin {
                    thread::spawn(move || write_messages(stdin, &receive));
                }
                if let Some(stdout) = stdout {
                    let emit = emit.clone();
                    thread::spawn(move || read_messages(stdout, session, 0, &emit));
                }
                send
            }
        };
        let over_at_exit = Arc::clone(&over);
        thread::spawn(move || {
            let status = child.wait();
            over_at_exit.store(true, Ordering::Relaxed);
            run::ask_group_to_stop(group);
            guardian.dismiss();
            let failure = match status {
                Ok(status) if !status.success() => Some(format!("The adapter ended: {status}")),
                Ok(_) => None,
                Err(e) => Some(format!("The adapter is gone: {e}")),
            };
            emit(Event::Debug(DebugEvent::Ended { session, failure }));
        });
        self.adapters.insert(
            session,
            Adapter {
                conns: HashMap::from([(0, first)]),
                port,
                group,
                over,
            },
        );
        Ok(())
    }

    pub fn connect<F>(&mut self, session: u64, conn: u32, emit: F)
    where
        F: Fn(Event) + Clone + Send + 'static,
    {
        let Some(adapter) = self.adapters.get_mut(&session) else {
            return;
        };
        match adapter.port {
            Some(port) => {
                let over = Arc::clone(&adapter.over);
                let sender = start_tcp_conn(session, conn, port, over, emit);
                adapter.conns.insert(conn, sender);
            }
            None => emit(Event::Debug(DebugEvent::Output {
                session,
                text: "The adapter asked for a child session over a pipe".to_owned(),
            })),
        }
    }

    pub fn send(&self, session: u64, conn: u32, message: &Value) {
        if let Some(sender) = self
            .adapters
            .get(&session)
            .and_then(|adapter| adapter.conns.get(&conn))
        {
            let _ = sender.send(encode(message));
        }
    }

    pub fn stop(&mut self, session: u64) {
        let Some(adapter) = self.adapters.remove(&session) else {
            return;
        };
        let wait = |limit: Duration, over: &AtomicBool| {
            let started = Instant::now();
            while started.elapsed() < limit && !over.load(Ordering::Relaxed) {
                thread::sleep(POLL);
            }
        };
        thread::spawn(move || {
            let Adapter {
                group, over, conns, ..
            } = adapter;
            wait(DISCONNECT_WAIT, &over);
            drop(conns);
            if over.load(Ordering::Relaxed) {
                return;
            }
            run::ask_group_to_stop(group);
            wait(TERM_WAIT, &over);
            if !over.load(Ordering::Relaxed) {
                run::kill_group(group);
            }
        });
    }
}

pub fn build<F>(session: u64, root: &Path, argv: &[String], emit: F) -> Result<Vec<String>, String>
where
    F: Fn(Event) + Send + 'static,
{
    use std::os::unix::process::CommandExt as _;
    let Some((program, args)) = argv.split_first() else {
        return Err("no build command".to_owned());
    };
    let mut child = Command::new(program)
        .args(args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("CARGO_TERM_COLOR", "never")
        .process_group(0)
        .spawn()
        .map_err(|e| format!("{program}: {e}"))?;
    let stderr = child.stderr.take();
    let printed = thread::spawn(move || {
        if let Some(stderr) = stderr {
            for line in BufReader::new(stderr).lines().map_while(Result::ok) {
                emit(Event::Debug(DebugEvent::Output {
                    session,
                    text: line,
                }));
            }
        }
    });
    let mut output = String::new();
    if let Some(mut stdout) = child.stdout.take() {
        let _ = stdout.read_to_string(&mut output);
    }
    let status = child.wait().map_err(|e| e.to_string())?;
    let _ = printed.join();
    if !status.success() {
        return Err(format!("{program} ended with {status}"));
    }
    Ok(cargo_executables(&output))
}

fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|dir| dir.join(program))
        .find(|candidate| candidate.is_file())
}

fn expand_script(pattern: &str) -> Option<PathBuf> {
    let home = directories::BaseDirs::new()?.home_dir().to_path_buf();
    let pattern = pattern
        .strip_prefix("~/")
        .map_or_else(|| PathBuf::from(pattern), |rest| home.join(rest));
    let text = pattern.to_str()?;
    let Some((before, after)) = text.split_once('*') else {
        return pattern.is_file().then_some(pattern);
    };
    let (dir, prefix) = before.rsplit_once('/')?;
    let (suffix, rest) = after.split_once('/').unwrap_or((after, ""));
    let mut matches: Vec<PathBuf> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(Result::ok)
        .filter(|entry| {
            let name = entry.file_name();
            let name = name.to_string_lossy();
            name.starts_with(prefix) && name.ends_with(suffix)
        })
        .map(|entry| entry.path().join(rest))
        .filter(|path| path.is_file())
        .collect();
    matches.sort();
    matches.pop()
}

fn has_module(python: &str, module: &str) -> bool {
    Command::new(python)
        .args(["-I", "-c", &format!("import {module}")])
        .current_dir("/")
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn xcode_tool(program: &str) -> Option<String> {
    if !cfg!(target_os = "macos") {
        return None;
    }
    let output = Command::new("xcrun")
        .args(["-f", program])
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .output()
        .ok()
        .filter(|o| o.status.success())?;
    let path = String::from_utf8(output.stdout).ok()?.trim().to_owned();
    Path::new(&path).is_file().then_some(path)
}

fn command_from(row: &AdapterRow, words: &[String]) -> Option<AdapterCommand> {
    let (program, args) = words.split_first()?;
    Some(AdapterCommand {
        id: row.id,
        program: program.clone(),
        args: args.to_vec(),
        transport: row.transport,
    })
}

fn find_adapter(row: &AdapterRow) -> Option<AdapterCommand> {
    for words in row.commands {
        let words: Vec<String> = words.iter().map(|w| (*w).to_owned()).collect();
        if on_path(&words[0]).is_none() {
            continue;
        }
        if row.needs_module.is_some_and(|m| !has_module(&words[0], m)) {
            continue;
        }
        return command_from(row, &words);
    }
    if row.id == AdapterId::LldbDap
        && let Some(path) = xcode_tool("lldb-dap")
    {
        return command_from(row, &[path]);
    }
    if on_path("node").is_some() {
        for script in row.node_scripts {
            if let Some(script) = expand_script(script) {
                let mut words = vec!["node".to_owned(), script.display().to_string()];
                words.extend(row.script_args.iter().map(|a| (*a).to_owned()));
                return command_from(row, &words);
            }
        }
    }
    None
}

pub fn detect(
    root: &Path,
    file: Option<PathBuf>,
    configured: &BTreeMap<AdapterId, Vec<String>>,
) -> DebugFacts {
    let adapters = ADAPTERS
        .iter()
        .filter_map(|row| {
            let found = match configured.get(&row.id) {
                Some(words) => command_from(row, words),
                None => find_adapter(row),
            };
            Some((row.id, found?))
        })
        .collect();
    let project_python = [".venv/bin/python", "venv/bin/python"]
        .iter()
        .map(|p| root.join(p))
        .find(|p| p.is_file());
    DebugFacts {
        adapters,
        has_cargo: root.join("Cargo.toml").is_file(),
        project_python,
        file,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_planted_module_in_the_project_is_not_what_finds_debugpy() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("planted_dbg.py"), "").unwrap();
        assert!(!has_module("python3", "planted_dbg"));
    }

    #[test]
    fn a_configured_command_wins_and_the_table_decides_the_transport() {
        let configured = BTreeMap::from([(
            AdapterId::Delve,
            vec!["/my/dlv".to_owned(), "dap".to_owned()],
        )]);
        let dir = tempfile::tempdir().unwrap();
        let facts = detect(dir.path(), None, &configured);
        let found = &facts.adapters[&AdapterId::Delve];
        assert_eq!(found.program, "/my/dlv");
        assert_eq!(found.transport, Transport::Tcp);
    }

    #[test]
    fn the_projects_own_python_and_cargo_are_noticed() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("Cargo.toml"), "").unwrap();
        std::fs::create_dir_all(dir.path().join(".venv/bin")).unwrap();
        std::fs::write(dir.path().join(".venv/bin/python"), "").unwrap();
        let facts = detect(dir.path(), None, &BTreeMap::new());
        assert!(facts.has_cargo);
        assert_eq!(
            facts.project_python,
            Some(dir.path().join(".venv/bin/python"))
        );
    }

    #[test]
    fn a_versioned_folder_is_found_by_its_pattern() {
        let dir = tempfile::tempdir().unwrap();
        let script = dir.path().join("x.php-debug-1.2.3/out/phpDebug.js");
        std::fs::create_dir_all(script.parent().unwrap()).unwrap();
        std::fs::write(&script, "").unwrap();
        let pattern = format!("{}/x.php-debug-*/out/phpDebug.js", dir.path().display());
        assert_eq!(expand_script(&pattern), Some(script));
    }
}
