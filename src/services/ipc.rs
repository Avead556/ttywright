use std::io::{self, BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::{UnixListener, UnixStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{SyncSender, TrySendError, sync_channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde::{Deserialize, Serialize};

use super::paths::path_hash;
use crate::core::{Event, RunStreamMessage};

const READ_TIMEOUT: Duration = Duration::from_secs(1);
const HASH_CHARS: usize = 16;
const WINDOW_COLUMNS: u32 = 180;
const WINDOW_ROWS: u32 = 50;

pub const GIT_LOG_WINDOW: &str = "git-log";
pub const RUN_WINDOW: &str = "run";
const FOLLOWER_QUEUE: usize = 256;
const RECONNECT: Duration = Duration::from_secs(1);

#[derive(Debug, Default, Serialize, Deserialize)]
struct Message {
    #[serde(default)]
    open: Option<PathBuf>,
    #[serde(default)]
    at: Option<(usize, usize)>,
    #[serde(default)]
    attach_run: bool,
    #[serde(default)]
    mcp: Option<(String, serde_json::Value)>,
}

type McpAnswer = Box<dyn FnOnce(Result<String, String>) + Send>;

#[derive(Clone, Default)]
pub struct McpCalls(Arc<Mutex<(u64, std::collections::HashMap<u64, McpAnswer>)>>);

impl McpCalls {
    pub fn add(&self, answer: impl FnOnce(Result<String, String>) + Send + 'static) -> Option<u64> {
        let mut calls = self.0.lock().ok()?;
        calls.0 += 1;
        let id = calls.0;
        calls.1.insert(id, Box::new(answer));
        Some(id)
    }

    fn add_socket(&self, mut stream: UnixStream) -> Option<u64> {
        self.add(move |result| {
            if let Ok(mut line) = serde_json::to_vec(&result) {
                line.push(b'\n');
                let _ = stream.write_all(&line);
            }
        })
    }

    pub fn answer(&self, id: u64, result: Result<String, String>) {
        let answer = self.0.lock().ok().and_then(|mut calls| calls.1.remove(&id));
        if let Some(answer) = answer {
            answer(result);
        }
    }
}

pub fn call_mcp(socket: &Path, tool: &str, args: serde_json::Value) -> Result<String, String> {
    let mut stream =
        UnixStream::connect(socket).map_err(|_| "The editor is not running".to_owned())?;
    let message = Message {
        mcp: Some((tool.to_owned(), args)),
        ..Message::default()
    };
    let mut line = serde_json::to_vec(&message).map_err(|e| e.to_string())?;
    line.push(b'\n');
    stream.write_all(&line).map_err(|e| e.to_string())?;
    let mut answer = String::new();
    BufReader::new(stream)
        .read_line(&mut answer)
        .map_err(|e| e.to_string())?;
    if answer.is_empty() {
        return Err("The editor closed before answering".to_owned());
    }
    serde_json::from_str::<Result<String, String>>(&answer).map_err(|e| e.to_string())?
}

struct Follower {
    queue: SyncSender<RunStreamMessage>,
    waiting: bool,
}

#[derive(Clone, Default)]
pub struct RunHub(Arc<Mutex<Vec<Follower>>>);

impl RunHub {
    fn attach(&self, stream: UnixStream) {
        let (queue, messages) = sync_channel::<RunStreamMessage>(FOLLOWER_QUEUE);
        std::thread::spawn(move || {
            let mut stream = stream;
            for message in messages {
                let Ok(mut line) = serde_json::to_vec(&message) else {
                    continue;
                };
                line.push(b'\n');
                if stream.write_all(&line).is_err() {
                    break;
                }
            }
        });
        if let Ok(mut followers) = self.0.lock() {
            followers.push(Follower {
                queue,
                waiting: true,
            });
        }
    }

    fn deliver(&self, message: &RunStreamMessage, to_waiting: bool) {
        let Ok(mut followers) = self.0.lock() else {
            return;
        };
        followers.retain_mut(|follower| {
            if follower.waiting != to_waiting {
                return true;
            }
            follower.waiting = false;
            !matches!(
                follower.queue.try_send(message.clone()),
                Err(TrySendError::Full(_) | TrySendError::Disconnected(_))
            )
        });
    }

    pub fn send(&self, message: &RunStreamMessage) {
        self.deliver(message, false);
    }

    pub fn start(&self, message: &RunStreamMessage) {
        self.deliver(message, true);
    }
}

pub fn socket_path(root: &Path) -> PathBuf {
    let dir = std::env::var_os("XDG_RUNTIME_DIR").map_or_else(std::env::temp_dir, PathBuf::from);
    let hash = path_hash(root);
    dir.join(format!("ttywright-{}.sock", &hash[..HASH_CHARS]))
}

pub struct Listener {
    path: PathBuf,
}

impl Listener {
    pub fn start(
        path: PathBuf,
        hub: RunHub,
        calls: McpCalls,
        emit: impl Fn(Event) + Send + 'static,
    ) -> io::Result<Option<Self>> {
        if UnixStream::connect(&path).is_ok() {
            return Ok(None);
        }
        match std::fs::remove_file(&path) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => return Err(e),
            _ => {}
        }
        let listener = UnixListener::bind(&path)?;
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o600))?;
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                match read_message(stream) {
                    Some(Incoming::Open { path, at }) => emit(Event::ExternalOpen { path, at }),
                    Some(Incoming::AttachRun(stream)) => {
                        let _ = stream.set_read_timeout(None);
                        hub.attach(stream);
                        emit(Event::RunWatcherAttached);
                    }
                    Some(Incoming::Mcp { stream, tool, args }) => {
                        let _ = stream.set_read_timeout(None);
                        if let Some(id) = calls.add_socket(stream) {
                            emit(Event::McpCalled { id, tool, args });
                        }
                    }
                    None => {}
                }
            }
        });
        Ok(Some(Self { path }))
    }
}

impl Drop for Listener {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Debug)]
enum Incoming {
    Open {
        path: PathBuf,
        at: Option<(usize, usize)>,
    },
    AttachRun(UnixStream),
    Mcp {
        stream: UnixStream,
        tool: String,
        args: serde_json::Value,
    },
}

fn read_message(stream: UnixStream) -> Option<Incoming> {
    let _ = stream.set_read_timeout(Some(READ_TIMEOUT));
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).ok()?;
    let message: Message = serde_json::from_str(&line).ok()?;
    if message.attach_run {
        return Some(Incoming::AttachRun(reader.into_inner()));
    }
    if let Some((tool, args)) = message.mcp {
        return Some(Incoming::Mcp {
            stream: reader.into_inner(),
            tool,
            args,
        });
    }
    Some(Incoming::Open {
        path: message.open?,
        at: message.at,
    })
}

pub fn send_open(socket: &Path, path: PathBuf, at: Option<(usize, usize)>) -> Result<(), String> {
    let mut stream =
        UnixStream::connect(socket).map_err(|_| "The main window is not running".to_owned())?;
    let message = Message {
        open: Some(path),
        at,
        ..Message::default()
    };
    let mut line = serde_json::to_vec(&message).map_err(|e| e.to_string())?;
    line.push(b'\n');
    stream.write_all(&line).map_err(|e| e.to_string())
}

pub fn follow_run(socket: PathBuf, emit: impl Fn(Event) + Send + 'static) {
    std::thread::spawn(move || {
        loop {
            match attach(&socket) {
                Some(stream) => {
                    emit(Event::RunStreamState { connected: true });
                    let mut reader = BufReader::new(stream);
                    let mut line = String::new();
                    loop {
                        line.clear();
                        match reader.read_line(&mut line) {
                            Ok(n) if n > 0 => {}
                            _ => break,
                        }
                        if let Ok(message) = serde_json::from_str(&line) {
                            emit(Event::RunStream(message));
                        }
                    }
                    emit(Event::RunStreamState { connected: false });
                }
                None => emit(Event::RunStreamState { connected: false }),
            }
            std::thread::sleep(RECONNECT);
        }
    });
}

fn attach(socket: &Path) -> Option<UnixStream> {
    let mut stream = UnixStream::connect(socket).ok()?;
    let message = Message {
        attach_run: true,
        ..Message::default()
    };
    let mut line = serde_json::to_vec(&message).ok()?;
    line.push(b'\n');
    stream.write_all(&line).ok()?;
    Some(stream)
}

fn shell_quote(text: &str) -> String {
    format!("'{}'", text.replace('\'', r"'\''"))
}

fn applescript_quote(text: &str) -> String {
    format!("\"{}\"", text.replace('\\', r"\\").replace('"', "\\\""))
}

fn window_command(exe: &Path, root: &Path, kind: &str) -> String {
    format!(
        "cd {} && exec {} --window {kind}",
        shell_quote(&root.display().to_string()),
        shell_quote(&exe.display().to_string())
    )
}

fn opener(
    tmux: bool,
    term_program: Option<&str>,
    macos: bool,
    command: &str,
) -> Option<(&'static str, Vec<String>)> {
    if tmux {
        return Some(("tmux", vec!["new-window".to_owned(), command.to_owned()]));
    }
    let scripts = if term_program == Some("iTerm.app") {
        vec![
            format!(
                "tell application \"iTerm\" to create window with default profile command {}",
                applescript_quote(&format!("sh -c {}", shell_quote(command)))
            ),
            format!(
                "tell application \"iTerm\" to tell current session of current window to \
                 set columns to {WINDOW_COLUMNS}"
            ),
            format!(
                "tell application \"iTerm\" to tell current session of current window to \
                 set rows to {WINDOW_ROWS}"
            ),
        ]
    } else if macos {
        vec![
            format!(
                "tell application \"Terminal\" to do script {}",
                applescript_quote(command)
            ),
            format!(
                "tell application \"Terminal\" to set number of columns of front window to \
                 {WINDOW_COLUMNS}"
            ),
            format!(
                "tell application \"Terminal\" to set number of rows of front window to \
                 {WINDOW_ROWS}"
            ),
            "tell application \"Terminal\" to activate".to_owned(),
        ]
    } else {
        return None;
    };
    let args = scripts
        .into_iter()
        .flat_map(|script| ["-e".to_owned(), script])
        .collect();
    Some(("osascript", args))
}

pub fn open_window(root: &Path, kind: &str) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    run_in_new_window(&window_command(&exe, root, kind))
}

pub fn claude_mcp_config(exe: &Path, root: &Path) -> String {
    serde_json::json!({
        "mcpServers": {
            "ttywright": { "command": exe, "args": ["mcp", root] },
        },
    })
    .to_string()
}

fn claude_command(exe: &Path, root: &Path) -> String {
    format!(
        "cd {} && exec claude --mcp-config {}",
        shell_quote(&root.display().to_string()),
        shell_quote(&claude_mcp_config(exe, root))
    )
}

pub fn open_project(dir: &Path) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    run_in_new_window(&format!(
        "cd {} && exec {}",
        shell_quote(&dir.display().to_string()),
        shell_quote(&exe.display().to_string())
    ))
}

pub fn open_claude(root: &Path) -> Result<(), String> {
    let on_path = std::env::var_os("PATH")
        .is_some_and(|path| std::env::split_paths(&path).any(|dir| dir.join("claude").is_file()));
    if !on_path {
        return Err("Claude Code was not found: install it from claude.com/claude-code".to_owned());
    }
    let exe = std::env::current_exe().map_err(|e| e.to_string())?;
    run_in_new_window(&claude_command(&exe, root))
}

fn run_in_new_window(command: &str) -> Result<(), String> {
    let term = std::env::var("TERM_PROGRAM").ok();
    let tmux = std::env::var_os("TMUX").is_some();
    let by_hand = || format!("Run in another terminal: {command}");
    let macos = cfg!(target_os = "macos");
    let (program, args) = opener(tmux, term.as_deref(), macos, command).ok_or_else(by_hand)?;
    let status = std::process::Command::new(program)
        .args(args)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status()
        .map_err(|_| by_hand())?;
    if status.success() {
        Ok(())
    } else {
        Err(by_hand())
    }
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;

    use super::*;

    #[test]
    fn a_message_reaches_the_listener_and_the_socket_goes_with_it() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        let (tx, rx) = channel();
        let listener = Listener::start(
            socket.clone(),
            RunHub::default(),
            McpCalls::default(),
            move |e| {
                let _ = tx.send(e);
            },
        )
        .unwrap()
        .expect("nobody else listens");
        let mode = std::fs::metadata(&socket).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600, "only this user");
        send_open(&socket, PathBuf::from("/a b/ы.rs"), Some((3, 4))).unwrap();
        let event = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(
            event,
            Event::ExternalOpen {
                path: PathBuf::from("/a b/ы.rs"),
                at: Some((3, 4)),
            }
        );
        drop(listener);
        assert!(!socket.exists());
    }

    #[test]
    fn a_second_main_window_does_not_take_the_socket_and_a_dead_ones_is_replaced() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        let first = Listener::start(
            socket.clone(),
            RunHub::default(),
            McpCalls::default(),
            |_| {},
        )
        .unwrap();
        assert!(first.is_some());
        assert!(
            Listener::start(
                socket.clone(),
                RunHub::default(),
                McpCalls::default(),
                |_| {}
            )
            .unwrap()
            .is_none()
        );
        std::mem::forget(first);
        let stale = dir.path().join("stale.sock");
        drop(UnixListener::bind(&stale).unwrap());
        assert!(stale.exists(), "what a crash leaves");
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        let mut listener = None;
        while listener.is_none() && std::time::Instant::now() < deadline {
            listener = Listener::start(
                stale.clone(),
                RunHub::default(),
                McpCalls::default(),
                |_| {},
            )
            .unwrap();
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(listener.is_some());
    }

    #[test]
    fn garbage_on_the_socket_is_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        let (tx, rx) = channel();
        let _listener = Listener::start(
            socket.clone(),
            RunHub::default(),
            McpCalls::default(),
            move |e| {
                let _ = tx.send(e);
            },
        )
        .unwrap();
        UnixStream::connect(&socket)
            .unwrap()
            .write_all(b"not json\n")
            .unwrap();
        send_open(&socket, PathBuf::from("/ok"), None).unwrap();
        let event = rx.recv_timeout(Duration::from_secs(10)).unwrap();
        assert_eq!(
            event,
            Event::ExternalOpen {
                path: PathBuf::from("/ok"),
                at: None,
            }
        );
    }

    #[test]
    fn with_no_main_window_sending_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let result = send_open(&dir.path().join("none.sock"), PathBuf::from("/x"), None);
        assert_eq!(result, Err("The main window is not running".to_owned()));
    }

    #[test]
    fn a_short_socket_path_differs_by_project() {
        let a = socket_path(Path::new("/p/a"));
        assert_ne!(a, socket_path(Path::new("/p/b")));
        assert_eq!(a, socket_path(Path::new("/p/a")));
    }

    #[test]
    fn quotes_in_paths_cannot_break_out_of_the_command() {
        let command = window_command(
            Path::new("/my apps/i't"),
            Path::new("/a'b; rm -rf ~"),
            GIT_LOG_WINDOW,
        );
        assert_eq!(
            command,
            r"cd '/a'\''b; rm -rf ~' && exec '/my apps/i'\''t' --window git-log"
        );
        let (program, args) = opener(false, Some("Apple_Terminal"), true, "say \"hi\"\\").unwrap();
        assert_eq!(program, "osascript");
        assert_eq!(
            args[1],
            r#"tell application "Terminal" to do script "say \"hi\"\\""#
        );
    }

    #[test]
    fn each_terminal_has_its_way_and_a_mac_always_has_terminal_app() {
        assert_eq!(opener(true, None, true, "c").unwrap().0, "tmux");
        let iterm = opener(false, Some("iTerm.app"), true, "c").unwrap().1;
        assert!(iterm[1].contains("iTerm"));
        assert!(iterm.iter().any(|a| a.contains("set columns to 180")));
        assert!(iterm.iter().any(|a| a.contains("set rows to 50")));
        for term in [Some("JetBrains-JediTerm"), Some("ghostty"), None] {
            let args = opener(false, term, true, "c").unwrap().1;
            assert!(args[1].contains("\"Terminal\""), "{args:?}");
            assert!(
                args.iter()
                    .any(|a| a.contains("columns of front window to 180"))
            );
            assert!(
                args.iter()
                    .any(|a| a.contains("rows of front window to 50"))
            );
            assert!(
                args[args.len() - 1].contains("activate"),
                "brought to the front"
            );
        }
        assert!(
            opener(false, Some("kitty"), false, "c").is_none(),
            "elsewhere, no"
        );
    }

    #[test]
    fn a_sender_gone_before_the_message_is_read_still_gets_it_delivered() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        let listener = UnixListener::bind(&socket).unwrap();
        send_open(&socket, PathBuf::from("/gone"), None).unwrap();
        let (stream, _) = listener.accept().unwrap();
        assert!(matches!(
            read_message(stream),
            Some(Incoming::Open { path, at: None }) if path == Path::new("/gone")
        ));
    }

    fn output(id: u64, text: &str) -> RunStreamMessage {
        RunStreamMessage::Output {
            id,
            lines: vec![crate::core::RunLine {
                spans: Vec::new(),
                text: text.to_owned(),
                location: None,
            }],
            partial: String::new(),
            dropped: 0,
        }
    }

    fn next(rx: &std::sync::mpsc::Receiver<Event>) -> RunStreamMessage {
        loop {
            match rx.recv_timeout(Duration::from_secs(10)).unwrap() {
                Event::RunStream(message) => return message,
                Event::RunStreamState { .. } => {}
                other => panic!("{other:?}"),
            }
        }
    }

    #[test]
    fn a_follower_gets_the_snapshot_first_then_the_stream_in_order() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        let hub = RunHub::default();
        let (main_tx, main_rx) = channel();
        let _listener =
            Listener::start(socket.clone(), hub.clone(), McpCalls::default(), move |e| {
                let _ = main_tx.send(e);
            })
            .unwrap()
            .unwrap();
        hub.send(&output(1, "before anyone"));

        let (tx, rx) = channel();
        follow_run(socket.clone(), move |e| {
            let _ = tx.send(e);
        });
        assert_eq!(
            main_rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            Event::RunWatcherAttached
        );
        hub.send(&output(1, "covered by the snapshot"));
        hub.start(&RunStreamMessage::Snapshot(None));
        hub.send(&output(1, "after"));
        hub.send(&RunStreamMessage::Finished {
            id: 1,
            outcome: crate::core::RunOutcome::Exited(3),
            duration_ms: 9,
        });
        assert_eq!(next(&rx), RunStreamMessage::Snapshot(None));
        assert_eq!(next(&rx), output(1, "after"));
        assert!(matches!(
            next(&rx),
            RunStreamMessage::Finished { id: 1, .. }
        ));
    }

    #[test]
    fn a_follower_comes_back_when_the_main_window_does() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s.sock");
        let (tx, rx) = channel();
        follow_run(socket.clone(), move |e| {
            let _ = tx.send(e);
        });
        assert_eq!(
            rx.recv_timeout(Duration::from_secs(10)).unwrap(),
            Event::RunStreamState { connected: false },
            "no main window yet"
        );
        let hub = RunHub::default();
        let (main_tx, main_rx) = channel();
        let _listener = Listener::start(socket, hub.clone(), McpCalls::default(), move |e| {
            let _ = main_tx.send(e);
        })
        .unwrap()
        .unwrap();
        main_rx.recv_timeout(Duration::from_secs(15)).unwrap();
        hub.start(&RunStreamMessage::Snapshot(None));
        assert_eq!(next(&rx), RunStreamMessage::Snapshot(None));
    }

    #[test]
    fn a_follower_that_stops_reading_is_cut_off_and_never_holds_the_sender_up() {
        let hub = RunHub::default();
        let (ours, theirs) = UnixStream::pair().unwrap();
        hub.attach(ours);
        hub.start(&RunStreamMessage::Snapshot(None));
        let big = "x".repeat(16 * 1024);
        let start = std::time::Instant::now();
        for _ in 0..(FOLLOWER_QUEUE * 4) {
            hub.send(&output(1, &big));
        }
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "{:?}",
            start.elapsed()
        );
        assert!(
            hub.0.lock().unwrap().is_empty(),
            "dropped, to come back later"
        );
        drop(theirs);
    }

    #[test]
    fn a_window_that_went_away_is_forgotten() {
        let hub = RunHub::default();
        let (ours, theirs) = UnixStream::pair().unwrap();
        hub.attach(ours);
        hub.start(&RunStreamMessage::Snapshot(None));
        drop(theirs);
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while !hub.0.lock().unwrap().is_empty() && std::time::Instant::now() < deadline {
            hub.send(&output(1, "x"));
            std::thread::sleep(Duration::from_millis(5));
        }
        assert!(hub.0.lock().unwrap().is_empty());
    }

    #[test]
    fn the_run_window_command_names_its_kind() {
        let command = window_command(Path::new("/x"), Path::new("/p"), RUN_WINDOW);
        assert!(command.ends_with("--window run"), "{command}");
    }

    #[test]
    fn claude_starts_in_the_project_with_this_editor_s_tools_and_quotes_survive() {
        let command = claude_command(Path::new("/bin/idea terminal"), Path::new("/my project"));
        assert!(
            command.starts_with("cd '/my project' && exec claude --mcp-config '"),
            "{command}"
        );
        let config = command
            .split("--mcp-config '")
            .nth(1)
            .unwrap()
            .trim_end_matches('\'');
        let value: serde_json::Value = serde_json::from_str(config).unwrap();
        let server = &value["mcpServers"]["ttywright"];
        assert_eq!(server["command"], "/bin/idea terminal");
        assert_eq!(server["args"], serde_json::json!(["mcp", "/my project"]));
    }

    #[test]
    fn a_tool_call_goes_to_the_editor_and_its_answer_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s");
        let calls = McpCalls::default();
        let (tx, rx) = channel();
        let _listener =
            Listener::start(socket.clone(), RunHub::default(), calls.clone(), move |e| {
                let _ = tx.send(e);
            })
            .unwrap()
            .unwrap();
        let client = std::thread::spawn(move || {
            call_mcp(&socket, "read_buffer", serde_json::json!({ "path": "a" }))
        });
        let Event::McpCalled { id, tool, args } = rx.recv().unwrap() else {
            panic!("not a call");
        };
        assert_eq!(
            (tool.as_str(), &args["path"]),
            ("read_buffer", &serde_json::json!("a"))
        );
        calls.answer(id, Err("no such file".to_owned()));
        assert_eq!(client.join().unwrap(), Err("no such file".to_owned()));
    }

    #[test]
    fn an_editor_closing_before_it_answers_is_an_error_not_a_hang() {
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("s");
        let listener = UnixListener::bind(&socket).unwrap();
        let editor = std::thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            let mut line = String::new();
            BufReader::new(stream).read_line(&mut line).unwrap();
        });
        let answer = call_mcp(&socket, "x", serde_json::json!({}));
        editor.join().unwrap();
        assert!(answer.unwrap_err().contains("closed"));
    }
}
