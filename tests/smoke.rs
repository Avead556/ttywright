#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::io::{Read, Write};
use std::path::Path;
use std::process::Command;
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};

use portable_pty::{Child, CommandBuilder, MasterPty, PtySize, native_pty_system};

const WAIT: Duration = Duration::from_secs(60);

struct Editor {
    child: Box<dyn Child + Send + Sync>,
    writer: Box<dyn Write + Send>,
    output: Receiver<Vec<u8>>,
    seen: Vec<u8>,
    _master: Box<dyn MasterPty + Send>,
}

impl Editor {
    fn start(home: &Path, project: &Path, file: &str) -> Self {
        Self::start_with(home, project, &[file])
    }

    fn start_with(home: &Path, project: &Path, args: &[&str]) -> Self {
        Self::start_env(home, project, args, &[])
    }

    fn start_env(home: &Path, project: &Path, args: &[&str], env: &[(&str, String)]) -> Self {
        let program = Path::new(env!("CARGO_BIN_EXE_ttyw"));
        Self::start_program(program, home, project, args, env)
    }

    fn start_program(
        program: &Path,
        home: &Path,
        project: &Path,
        args: &[&str],
        env: &[(&str, String)],
    ) -> Self {
        let pair = native_pty_system()
            .openpty(PtySize {
                rows: 30,
                cols: 100,
                pixel_width: 0,
                pixel_height: 0,
            })
            .unwrap();
        let mut command = CommandBuilder::new(program);
        command.args(args);
        command.cwd(project);
        command.env("HOME", home);
        command.env("XDG_DATA_HOME", home.join("data"));
        command.env("XDG_CONFIG_HOME", home.join("config"));
        command.env("TERM", "xterm-256color");
        command.env("TTYWRIGHT_TRUST_ALL", "1");
        for (name, value) in env {
            command.env(name, value);
        }
        let child = pair.slave.spawn_command(command).unwrap();
        drop(pair.slave);
        let mut reader = pair.master.try_clone_reader().unwrap();
        let writer = pair.master.take_writer().unwrap();
        let (tx, output) = channel();
        std::thread::spawn(move || {
            let mut chunk = [0; 65536];
            while let Ok(n) = reader.read(&mut chunk) {
                if n == 0 || tx.send(chunk[..n].to_vec()).is_err() {
                    break;
                }
            }
        });
        Self {
            child,
            writer,
            output,
            seen: Vec::new(),
            _master: pair.master,
        }
    }

    fn shows_within(&mut self, wanted: &[u8], time: std::time::Duration, keys: &[u8]) -> bool {
        self.send(keys);
        let deadline = Instant::now() + time;
        while let Ok(chunk) = self
            .output
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        {
            self.seen.extend(chunk);
            if self.seen.windows(wanted.len()).any(|w| w == wanted) {
                self.seen.clear();
                return true;
            }
        }
        false
    }

    fn wait_all(&mut self, wanted: &[&[u8]]) {
        let mut left: Vec<&[u8]> = wanted.to_vec();
        while let Some(next) = left.first().copied() {
            let seen = self.wait_for(next);
            left.retain(|w| !seen.windows(w.len()).any(|part| part == *w));
        }
    }

    fn wait_for(&mut self, wanted: &[u8]) -> Vec<u8> {
        let deadline = Instant::now() + WAIT;
        loop {
            if self.seen.windows(wanted.len()).any(|w| w == wanted) {
                return std::mem::take(&mut self.seen);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            let chunk = self.output.recv_timeout(left).unwrap_or_else(|_| {
                panic!(
                    "never saw {:?} in {:?}",
                    String::from_utf8_lossy(wanted),
                    String::from_utf8_lossy(&self.seen)
                )
            });
            if chunk.windows(3).any(|w| w == b"\x1b[c") {
                self.send(b"\x1b[?62c");
            }
            if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                self.send(b"\x1b[1;1R");
            }
            self.seen.extend(chunk);
        }
    }

    fn send(&mut self, bytes: &[u8]) {
        self.writer.write_all(bytes).unwrap();
        self.writer.flush().unwrap();
    }

    fn wait_exit(&mut self) -> bool {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Some(status) = self.child.try_wait().unwrap() {
                return status.success();
            }
            assert!(Instant::now() < deadline, "the editor did not exit");
            std::thread::sleep(Duration::from_millis(20));
        }
    }

    fn kill_hard(&mut self) {
        let pid = self.child.process_id().unwrap().to_string();
        assert!(
            Command::new("kill")
                .args(["-9", &pid])
                .status()
                .unwrap()
                .success()
        );
        let _ = self.child.wait();
    }
}

const QUIT: &[u8] = b"\x11";
const ENTER_ALT_SCREEN: &[u8] = b"\x1b[?1049h";
const LEAVE_ALT_SCREEN: &[u8] = b"\x1b[?1049l";

#[test]
fn it_starts_draws_saves_and_gives_the_terminal_back() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("a.txt"), "hello\n").unwrap();
    let mut editor = Editor::start(home.path(), project.path(), "a.txt");
    editor.wait_for(ENTER_ALT_SCREEN);
    editor.wait_for(b"hello");
    editor.send(b"Z");
    editor.wait_for(b"Z");
    editor.send(QUIT);
    editor.wait_for(LEAVE_ALT_SCREEN);
    assert!(editor.wait_exit(), "a clean exit status");
    assert_eq!(
        std::fs::read_to_string(project.path().join("a.txt")).unwrap(),
        "Zhello\n"
    );
}

#[test]
fn a_kill_right_after_an_edit_appears_loses_nothing() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let file = project.path().join("a.txt");
    std::fs::write(&file, "base\n").unwrap();

    let mut expected = String::from("base\n");
    for mark in ["Q", "W", "E"] {
        let mut editor = Editor::start(home.path(), project.path(), "a.txt");
        editor.wait_for(b"base");
        editor.send(b"\x1b[1;5H");
        editor.send(mark.as_bytes());
        editor.wait_for(mark.as_bytes());
        editor.kill_hard();
        expected.insert_str(0, mark);
        assert_ne!(
            std::fs::read_to_string(&file).unwrap(),
            expected,
            "the file must not already have it, or this tests nothing"
        );
    }

    let mut editor = Editor::start(home.path(), project.path(), "a.txt");
    editor.wait_for(b"base");
    editor.send(QUIT);
    assert!(editor.wait_exit());
    assert_eq!(std::fs::read_to_string(&file).unwrap(), expected);
}

fn git(dir: &Path, args: &[&str]) {
    let status = Command::new("git")
        .current_dir(dir)
        .args(args)
        .env("GIT_CONFIG_NOSYSTEM", "1")
        .env("HOME", dir)
        .output()
        .unwrap();
    assert!(status.status.success(), "git {args:?}");
}

#[test]
fn a_git_window_shows_the_log_and_opens_a_file_in_the_main_window() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(project.path()).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "T"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("shown.txt"), "text in the file\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "the only commit"]);

    let mut main = Editor::start_with(home.path(), &root, &[]);
    main.wait_for(ENTER_ALT_SCREEN);
    main.wait_for(b"Project");
    let mut window = Editor::start_with(home.path(), &root, &["--window", "git-log"]);
    window.wait_for(b"the only commit");
    window.wait_for(b"shown.txt");
    window.send(b"\t");
    window.send(b"\r");
    window.wait_for(b"Opened in the main window");
    main.wait_for(b"text in the file");
    window.send(QUIT);
    assert!(window.wait_exit());
    main.send(QUIT);
    assert!(main.wait_exit());
}

#[test]
fn a_git_window_says_so_when_there_is_no_main_window() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = std::fs::canonicalize(project.path()).unwrap();
    git(&root, &["init", "-q", "-b", "main"]);
    git(&root, &["config", "user.name", "T"]);
    git(&root, &["config", "user.email", "t@example.com"]);
    git(&root, &["config", "commit.gpgsign", "false"]);
    std::fs::write(root.join("a.txt"), "x\n").unwrap();
    git(&root, &["add", "-A"]);
    git(&root, &["commit", "-qm", "alone"]);
    let mut window = Editor::start_with(home.path(), &root, &["--window", "git-log"]);
    window.wait_for(b"alone");
    window.wait_for(b"a.txt");
    window.send(b"\t");
    window.send(b"\r");
    window.wait_for(b"The main window is not running");
    window.send(QUIT);
    assert!(window.wait_exit());
}

fn runnable(project: &Path, body: &str) -> std::path::PathBuf {
    let pid_file = project.join("pid");
    std::fs::write(
        project.join("job.sh"),
        format!("echo $$ > '{}'\n{body}\n", pid_file.display()),
    )
    .unwrap();
    pid_file
}

fn read_pid(file: &Path) -> String {
    let deadline = Instant::now() + WAIT;
    loop {
        if let Ok(text) = std::fs::read_to_string(file)
            && !text.trim().is_empty()
        {
            return text.trim().to_owned();
        }
        assert!(Instant::now() < deadline, "the script never started");
        std::thread::sleep(Duration::from_millis(10));
    }
}

fn wait_gone(pid: &str) {
    let deadline = Instant::now() + WAIT;
    while Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
    {
        assert!(Instant::now() < deadline, "process {pid} was left behind");
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn run_open_file(editor: &mut Editor) {
    editor.send(b"\n");
    editor.wait_for(b"Run job.sh");
    editor.send(b"\r");
}

#[test]
fn a_program_runs_and_no_escape_of_its_output_reaches_the_terminal() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    runnable(
        project.path(),
        r"printf 'to-out\n'; printf 'to-err\n' >&2; printf '\033]0;PWN%s\007\033[?1049lvisible-end\n' ED; exit 7",
    );
    let mut editor = Editor::start(home.path(), project.path(), "job.sh");
    editor.wait_for(b"job.sh");
    run_open_file(&mut editor);
    let shown = editor.wait_for(b"Exit code 7");
    let has = |needle: &[u8]| shown.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"to-out") && has(b"to-err") && has(b"visible-end"));
    assert!(!has(b"PWNED"), "a title was set");
    assert!(!has(b"\x1b]0;"), "a title was set");
    assert!(
        !has(b"\x1b[?1049l"),
        "the program left the alternate screen"
    );
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

#[test]
fn stop_leaves_no_child_behind() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let child_pid = project.path().join("child");
    let pid_file = runnable(
        project.path(),
        &format!(
            "sleep 300 &\necho $! > '{}'\ntrap '' TERM\nwhile :; do sleep 0.05; done",
            child_pid.display()
        ),
    );
    let mut editor = Editor::start(home.path(), project.path(), "job.sh");
    editor.wait_for(b"job.sh");
    run_open_file(&mut editor);
    let script = read_pid(&pid_file);
    let child = read_pid(&child_pid);
    editor.wait_for(b"Running");
    editor.send(b"\x1b4");
    editor.wait_for(b"s stop");
    editor.send(b"s");
    editor.wait_for(b"r rerun");
    wait_gone(&script);
    wait_gone(&child);
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

#[test]
fn quitting_stops_what_is_running_and_does_not_wait_for_it() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let pid_file = runnable(project.path(), "trap '' TERM\nwhile :; do sleep 0.05; done");
    let mut editor = Editor::start(home.path(), project.path(), "job.sh");
    editor.wait_for(b"job.sh");
    run_open_file(&mut editor);
    let script = read_pid(&pid_file);
    editor.send(QUIT);
    assert!(editor.wait_exit());
    wait_gone(&script);
}

#[test]
fn a_killed_editor_still_leaves_no_child_behind() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let pid_file = runnable(project.path(), "while :; do sleep 0.05; done");
    let mut editor = Editor::start(home.path(), project.path(), "job.sh");
    editor.wait_for(b"job.sh");
    run_open_file(&mut editor);
    let script = read_pid(&pid_file);
    editor.kill_hard();
    wait_gone(&script);
}

#[test]
fn opening_a_project_with_a_hostile_config_runs_nothing_until_y() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let marker = project.path().join("ran");
    std::fs::create_dir(project.path().join(".ttywright")).unwrap();
    std::fs::write(
        project.path().join(".ttywright/config.toml"),
        format!(
            "[[run]]\nname = \"setup\"\ncommand = \"touch '{}'\"\n",
            marker.display()
        ),
    )
    .unwrap();
    std::fs::write(project.path().join("a.txt"), "text").unwrap();
    let mut editor = Editor::start(home.path(), project.path(), "a.txt");
    editor.wait_for(b"a.txt");
    editor.send(b"\n");
    editor.wait_for(b"[project config] setup");
    editor.send(b"\r");
    editor.wait_for(b"any other key");
    assert!(!marker.exists(), "ran without being asked");
    editor.send(b"\r");
    editor.wait_for(b"Not run");
    assert!(!marker.exists(), "Enter is not yes");
    editor.send(b"\n");
    editor.wait_for(b"[project config] setup");
    editor.send(b"\r");
    editor.wait_for(b"any other key");
    editor.send(b"y");
    editor.wait_for(b"Exit code 0");
    assert!(marker.exists());
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

#[test]
fn a_program_cannot_reach_the_terminal_through_dev_tty() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    runnable(
        project.path(),
        "printf 'RAW-%s\n' TTY > /dev/tty && printf 'ope%s\n' ned\nread x < /dev/tty\nprintf 'went-%s\n' on",
    );
    let mut editor = Editor::start(home.path(), project.path(), "job.sh");
    editor.wait_for(b"job.sh");
    run_open_file(&mut editor);
    let shown = editor.wait_for(b"Exit code");
    let has = |needle: &[u8]| shown.windows(needle.len()).any(|w| w == needle);
    assert!(has(b"went-on"), "a read from it must not stop the program");
    assert!(!has(b"opened"), "it could open the terminal");
    assert!(!has(b"RAW-TTY"), "it wrote on the terminal");
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

#[test]
fn a_run_window_says_what_it_waits_for_and_quits_cleanly() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let mut window = Editor::start_with(home.path(), project.path(), &["--window", "run"]);
    window.wait_for(b"not running");
    window.send(QUIT);
    assert!(window.wait_exit());
}

#[test]
fn plugins_run_from_the_users_folder_and_cannot_freeze_the_editor() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("a.txt"), "abc\n").unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/hello");
    let hello = wat::parse_file(source.join("plugin.wat")).unwrap();
    let spin = wat::parse_str(
        r#"(module (memory (export "memory") 2)
             (func (export "alloc") (param i32) (result i32) (i32.const 4096))
             (func (export "handle") (param i32 i32) (result i64)
               (loop $l (br $l)) (i64.const 0)))"#,
    )
    .unwrap();
    let mark = home.path().join("mark.txt");
    let request = format!(
        r#"{{\"program\":\"sh\",\"args\":[\"-c\",\"[ -r /dev/tty ] && : > /dev/tty && echo tty >> {m} ; echo ran >> {m}\"]}}"#,
        m = mark.display()
    );
    let tool = wat::parse_str(format!(
        r#"(module
             (import "idea" "run" (func $run (param i32 i32) (result i64)))
             (memory (export "memory") 2)
             (global $next (mut i32) (i32.const 4096))
             (func (export "alloc") (param i32) (result i32)
               (local $at i32)
               (local.set $at (global.get $next))
               (global.set $next (i32.add (local.get $at) (local.get 0)))
               (local.get $at))
             (data (i32.const 16) "{request}")
             (data (i32.const 2048) "{{\"commands\":[{{\"Run\":\"Save\"}},{{\"Run\":\"Quit\"}}]}}")
             (func (export "handle") (param i32 i32) (result i64)
               (drop (call $run (i32.const 16) (i32.const {len})))
               (i64.const {answer})))"#,
        len = request.replace('\\', "").len(),
        answer = (2048_i64 << 32) | 44,
    ))
    .unwrap();
    let install = |folder: &Path, name: &str, title: &str, wasm: &[u8]| {
        let dir = folder.join(name);
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::write(
            dir.join("plugin.toml"),
            format!("name = \"{name}\"\n[[commands]]\nid = \"say\"\ntitle = \"{title}\"\n"),
        )
        .unwrap();
        std::fs::write(dir.join("plugin.wasm"), wasm).unwrap();
    };
    let user_folders = [
        home.path()
            .join("Library/Application Support/ttywright/plugins"),
        home.path().join("config/ttywright/plugins"),
    ];
    for folder in &user_folders {
        install(folder, "hello", "Say Hello", &hello);
        install(folder, "spin", "Spin Forever", &spin);
        install(folder, "tool", "Run Tool And Quit", &tool);
    }
    install(
        &project.path().join(".ttywright/plugins"),
        "evil",
        "Evil Thing",
        &hello,
    );

    let mut editor = Editor::start(home.path(), project.path(), "a.txt");
    editor.wait_for(b"abc");

    editor.send(b"\x0bPlugin");
    let shown = editor.wait_for(b"Say Hello");
    assert!(!String::from_utf8_lossy(&shown).contains("Evil"));
    editor.send(b" Say\r");
    editor.wait_for(b"Sai");

    editor.send(b"\x0bPlugin Spin\r");
    std::thread::sleep(Duration::from_millis(100));
    editor.send(b"Z");
    let typed = editor.wait_for(b"Z");
    assert!(
        !String::from_utf8_lossy(&typed).contains("too long"),
        "typing answered while it ran"
    );
    editor.wait_for(b"too long");

    editor.send(b"\x0bPlugin Run Tool\r");
    assert!(editor.wait_exit(), "the plugin's quit is a real quit");
    assert_eq!(
        std::fs::read_to_string(&mark).unwrap(),
        "ran\n",
        "it ran, and without the editor's terminal"
    );
    let text = std::fs::read_to_string(project.path().join("a.txt")).unwrap();
    assert!(text.contains("hello from a plugin"), "{text}");
}

#[test]
fn analyze_project_runs_nothing_until_y_and_shows_the_findings() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    let bin = home.path().join("bin");
    std::fs::create_dir_all(&bin).unwrap();
    let mark = home.path().join("ran");
    let ruff = bin.join("ruff");
    std::fs::write(
        &ruff,
        format!(
            "#!/bin/sh\necho x >> {}\necho '[{{\"code\":\"F401\",\"filename\":\"a.py\",\"location\":{{\"row\":2,\"column\":1}},\"message\":\"unused import\"}}]'\nexit 1\n",
            mark.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&ruff, std::fs::Permissions::from_mode(0o755)).unwrap();
    std::fs::write(project_path.join("ruff.toml"), "").unwrap();
    std::fs::write(project_path.join("a.py"), "one\ntwo\n").unwrap();
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let env = [("PATH", path)];

    let mut editor = Editor::start_env(home.path(), &project_path, &["a.py"], &env);
    editor.wait_for(b"one");
    assert!(!mark.exists(), "opening a project runs nothing");
    editor.send(b"\x0bAnalyze Project\r");
    editor.wait_for(b"These can run code");
    assert!(!mark.exists(), "the question comes first");
    editor.send(b"n");
    editor.wait_for(b"Not run");
    assert!(!mark.exists());

    editor.send(b"\x0bAnalyze Project\r");
    editor.wait_for(b"These can run code");
    editor.send(b"y");
    let shown = editor.wait_for(b"[ruff]");
    assert!(String::from_utf8_lossy(&shown).contains("unused import"));
    assert_eq!(std::fs::read_to_string(&mark).unwrap().lines().count(), 1);
    editor.send(b"\x11");
    assert!(editor.wait_exit());

    let mut editor = Editor::start_env(home.path(), &project_path, &["a.py"], &env);
    editor.wait_for(b"one");
    assert_eq!(
        std::fs::read_to_string(&mark).unwrap().lines().count(),
        1,
        "approval does not mean running by itself"
    );
    editor.send(b"\x0bAnalyze Project\r");
    editor.wait_for(b"unused import");
    assert_eq!(std::fs::read_to_string(&mark).unwrap().lines().count(), 2);
    editor.send(b"\x11");
    assert!(editor.wait_exit());
}

fn files_named(dir: &Path, ending: &str) -> Vec<std::path::PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.is_dir() {
            found.extend(files_named(&path, ending));
        } else if path.to_string_lossy().ends_with(ending) {
            found.push(path);
        }
    }
    found
}

#[test]
fn go_to_symbol_finds_a_name_in_a_closed_file_and_the_index_is_kept_for_next_time() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("a.txt"), "abc\n").unwrap();
    let lib = project.path().join("lib.py");
    std::fs::write(&lib, "import os\n\ndef find_me_here():\n    pass\n").unwrap();
    let mut editor = Editor::start(home.path(), project.path(), "a.txt");
    editor.wait_for(b"a.txt");
    std::thread::sleep(Duration::from_millis(800));

    editor.send(b"\x1bo");
    editor.wait_for(b"Go to Symbol");
    editor.send(b"find_me");
    editor.wait_for(b"lib.py:3");
    editor.send(b"\r");
    editor.wait_for(b"Python");
    editor.send(b"X");
    editor.send(b"\x13");

    editor.send(b"\x1b7");
    editor.wait_for(b"File Structure");
    editor.send(b"\x1b");

    editor.send(b"\x11");
    assert!(editor.wait_exit());
    assert_eq!(
        std::fs::read_to_string(&lib).unwrap(),
        "import os\n\ndef Xfind_me_here():\n    pass\n",
        "the cursor went to the name"
    );
    assert_eq!(
        files_named(project.path(), ".bin").len(),
        1,
        "kept for next time, in the project's own folder"
    );
    assert!(files_named(home.path(), ".bin").is_empty());

    let mut editor = Editor::start(home.path(), project.path(), "a.txt");
    editor.wait_for(b"a.txt");
    editor.send(b"\x1bo");
    editor.wait_for(b"Go to Symbol");
    editor.send(b"Xfind");
    editor.wait_for(b"lib.py:3");
    editor.send(b"\x1b");
    editor.send(b"\x11");
    assert!(editor.wait_exit());
}

#[test]
fn find_in_files_answers_with_the_index_and_sees_a_file_changed_outside() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("a.txt"), "abc\n").unwrap();
    let other = project.path().join("other.txt");
    std::fs::write(&other, "nothing here\n").unwrap();
    let mut editor = Editor::start(home.path(), project.path(), "a.txt");
    editor.wait_for(b"a.txt");
    std::thread::sleep(Duration::from_millis(800));

    std::fs::write(&other, "qqmarkerqq here\n").unwrap();
    editor.send(b"\x07");
    editor.send(b"qqmarkerqq\r");
    editor.wait_for(b"other.txt");
    editor.send(b"\x11");
    assert!(editor.wait_exit());
}

#[test]
fn an_untrusted_project_runs_nothing_of_its_own_until_y() {
    use std::os::unix::fs::PermissionsExt;
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    let mark = home.path().join("ran");
    let hook = home.path().join("hook.sh");
    std::fs::write(&hook, format!("#!/bin/sh\necho x >> {}\n", mark.display())).unwrap();
    std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
    let server = home.path().join("server.sh");
    let server_mark = home.path().join("server-ran");
    std::fs::write(
        &server,
        format!("#!/bin/sh\necho x >> {}\nsleep 30\n", server_mark.display()),
    )
    .unwrap();
    std::fs::set_permissions(&server, std::fs::Permissions::from_mode(0o755)).unwrap();
    assert!(
        Command::new("git")
            .args(["init", "-q"])
            .current_dir(&project_path)
            .status()
            .unwrap()
            .success()
    );
    assert!(
        Command::new("git")
            .args(["config", "core.fsmonitor", hook.to_str().unwrap()])
            .current_dir(&project_path)
            .status()
            .unwrap()
            .success()
    );
    std::fs::write(project_path.join("a.rs"), "fn visible_text() {}\n").unwrap();
    std::fs::create_dir(project_path.join(".ttywright")).unwrap();
    std::fs::write(
        project_path.join(".ttywright/config.toml"),
        "[keybindings]\n\"Alt+Z\" = \"project.trust\"\n",
    )
    .unwrap();
    let env = [
        ("TTYWRIGHT_TRUST_ALL", String::new()),
        ("TTYWRIGHT_RUST_LSP", server.display().to_string()),
    ];
    let settle = || std::thread::sleep(Duration::from_millis(700));

    let mut editor = Editor::start_env(home.path(), &project_path, &["a.rs"], &env);
    editor.wait_for(b"Trust This Project?");
    editor.send(b"\r");
    settle();
    assert!(!mark.exists(), "Enter is not yes; git ran");
    assert!(!server_mark.exists(), "a server ran");
    editor.send(b"n");
    editor.wait_for(b"Not trusted:");
    settle();
    editor.send(QUIT);
    assert!(editor.wait_exit());

    let mut editor = Editor::start_env(home.path(), &project_path, &["a.rs"], &env);
    editor.wait_for(b"untrusted");
    settle();
    assert!(
        !String::from_utf8_lossy(&editor.seen).contains("Trust This Project?"),
        "asked only once"
    );
    assert!(!mark.exists());
    editor.send(b"\x0bAnalyze Project\r");
    editor.wait_for(b"Analyze Project runs this project");
    editor.send(b"\x1b");
    settle();
    editor.send(b"\x0bTrust This Project\r");
    editor.wait_for(b"Trust This Project?");
    editor.send(b"y");
    let deadline = Instant::now() + WAIT;
    while !(mark.exists() && server_mark.exists()) {
        assert!(
            Instant::now() < deadline,
            "git and the server start after y"
        );
        std::thread::sleep(Duration::from_millis(50));
    }
    editor.send(QUIT);
    assert!(editor.wait_exit());

    let mut editor = Editor::start_env(home.path(), &project_path, &["a.rs"], &env);
    let seen = editor.wait_for(b"visible_text");
    settle();
    editor.send(QUIT);
    editor.wait_exit();
    let rest = String::from_utf8_lossy(&seen);
    assert!(!rest.contains("Trust This Project?"), "{rest}");
    assert!(!rest.contains("untrusted"), "remembered: {rest}");
}

#[test]
fn a_separate_window_of_an_unknown_project_does_not_hang_on_the_question() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    let env = [("TTYWRIGHT_TRUST_ALL", String::new())];
    let args = ["--window", "git-log", project_path.to_str().unwrap()];
    let mut editor = Editor::start_env(home.path(), &project_path, &args, &env);
    editor.wait_for(b"trusted");
    assert!(!String::from_utf8_lossy(&editor.seen).contains("Trust This Project?"));
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

#[test]
fn run_anything_runs_what_is_typed_in_the_project_even_untrusted() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    let env = [
        ("TTYWRIGHT_TRUST_ALL", String::new()),
        ("SHELL", "/bin/sh".to_owned()),
    ];
    let mut editor = Editor::start_env(home.path(), &project_path, &[], &env);
    editor.wait_for(b"Trust This Project?");
    editor.send(b"n");
    editor.wait_for(b"Not trusted:");
    editor.send(b"\x1bx");
    editor.wait_for(b"In your shell, in");
    editor.send(b"pwd > typed-here; echo sum-$((1+1))\r");
    editor.wait_for(b"sum-2");
    let written = std::fs::read_to_string(project_path.join("typed-here")).unwrap();
    assert_eq!(written.trim(), project_path.display().to_string());
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

#[test]
fn a_scratch_file_is_kept_outside_the_project_and_comes_back_from_the_chooser() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    let mut editor = Editor::start_with(home.path(), &project_path, &[]);
    editor.wait_for(b"Navigate");
    editor.send(b"\x0bNew Scratch File\r");
    editor.wait_for(b"Markdown");
    editor.send(b"Markdown\r");
    editor.wait_for(b"scratch-1.md");
    editor.send(b"kept-in-scratch");
    editor.wait_for(b"kept-in-scratch");
    editor.send(QUIT);
    assert!(editor.wait_exit());
    assert_eq!(
        std::fs::read_dir(&project_path)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_name() != ".ttywright")
            .count(),
        0,
        "nothing in the project"
    );

    let mut editor = Editor::start_with(home.path(), &project_path, &[]);
    editor.wait_for(b"Navigate");
    editor.send(b"\x0bScratches\r");
    editor.wait_for(b"scratch-1.md");
    editor.send(b"\r");
    editor.wait_for(b"kept-in-scratch");
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

#[test]
fn todo_and_bookmark_panels_list_jump_and_remove() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    std::fs::write(project_path.join("a.txt"), "first\nsecond\n").unwrap();
    std::fs::write(project_path.join("b.txt"), "x\ny\n// FIXME later-on\n").unwrap();
    let mut editor = Editor::start(home.path(), &project_path, "a.txt");
    editor.wait_for(b"first");
    editor.send(b"\x1bi");
    editor.wait_for(b"later-on");
    editor.send(b"\r");
    editor.wait_for(b"b.txt");
    editor.send(b"\x0bToggle Bookmark\r");
    editor.wait_for(b"Bookmark set");
    editor.send(b"\x1bk");
    editor.wait_for(b"Enter go");
    editor.send(b"d");
    editor.wait_for(b"Bookmark removed");
    editor.send(QUIT);
    assert!(editor.wait_exit());
    assert!(
        !project_path
            .join("b.txt")
            .metadata()
            .unwrap()
            .permissions()
            .readonly(),
        "still the same file"
    );
    let text = std::fs::read_to_string(project_path.join("b.txt")).unwrap();
    assert_eq!(
        text, "x\ny\n// FIXME later-on\n",
        "`d` removed a bookmark, not text"
    );
}

#[test]
fn comparing_with_a_branch_changelists_and_a_failing_check_on_a_real_repository() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let path = project.path().canonicalize().unwrap();
    git(&path, &["init", "-q", "-b", "main"]);
    git(&path, &["config", "user.email", "t@example.com"]);
    git(&path, &["config", "user.name", "T"]);
    std::fs::write(path.join("a.txt"), "one\n").unwrap();
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-qm", "first-on-main"]);
    git(&path, &["checkout", "-qb", "feature"]);
    std::fs::write(path.join("b.txt"), "added-on-feature\n").unwrap();
    git(&path, &["add", "-A"]);
    git(&path, &["commit", "-qm", "second-on-feature"]);
    std::fs::write(path.join("a.txt"), "one\n// TODO left-behind\n").unwrap();
    git(&path, &["config", "commit.gpgsign", "false"]);
    for folder in ["config", "Library/Application Support"] {
        let config = home.path().join(folder).join("ttywright");
        std::fs::create_dir_all(&config).unwrap();
        std::fs::write(
            config.join("config.toml"),
            "[commit_checks]\ntodo = true\ncommand = \"echo check-said-no; exit 4\"\n",
        )
        .unwrap();
    }

    let mut editor = Editor::start_with(home.path(), &path, &[]);
    editor.wait_for(b"Navigate");
    editor.send(b"\x0bCompare with Branch\r");
    editor.wait_for(b"main");
    editor.send(b"main\r");
    editor.wait_all(&[b"second-on-feature", b"b.txt"]);

    editor.send(b"c");
    editor.wait_for(b"TICKED");
    editor.send(b"e");
    editor.wait_for(b"Move to Changelist");
    editor.send(b"mine\r");
    editor.wait_for(b"mine");
    editor.send(b"j");
    editor.wait_for(b"Changelist mine: 1 ticked");
    editor.send(b"\t");
    editor.send(b"msg-with-checks\r");
    editor.wait_all(&[b"left-behind", b"check-said-no"]);
    let log = |path: &Path| {
        let out = std::process::Command::new("git")
            .args(["log", "--format=%s"])
            .current_dir(path)
            .output()
            .unwrap();
        String::from_utf8_lossy(&out.stdout).into_owned()
    };
    assert!(!log(&path).contains("msg-with-checks"), "not yet");
    editor.send(b"y");
    editor.wait_for(b"msg-with-checks");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while !log(&path).contains("msg-with-checks") {
        assert!(
            std::time::Instant::now() < deadline,
            "the commit never came"
        );
        std::thread::sleep(std::time::Duration::from_millis(50));
    }
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

#[test]
fn a_coverage_report_marks_the_lines_and_an_edit_makes_it_stale() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let path = project.path().canonicalize().unwrap();
    std::fs::write(path.join("a.txt"), "ran-line\nnot-run-line\n").unwrap();
    std::thread::sleep(std::time::Duration::from_millis(20));
    std::fs::write(
        path.join("lcov.info"),
        "SF:a.txt\nDA:1,3\nDA:2,0\nend_of_record\nSF:/etc/passwd\nDA:1,1\nend_of_record\n",
    )
    .unwrap();
    let mut editor = Editor::start(home.path(), &path, "a.txt");
    editor.wait_for(b"ran-line");
    editor.send(b"\x0bLoad Coverage Report\r");
    editor.wait_for(b"Load Coverage Report");
    editor.send(b"\r");
    editor.wait_all(&["✓ ".as_bytes(), "✗".as_bytes(), b"cov 50%"]);
    editor.send(b"x");
    editor.wait_for(b"cov stale");
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

#[test]
fn structural_search_and_replace_change_code_not_comments_and_duplicates_are_listed() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let path = project.path().canonicalize().unwrap();
    let text = "fn a() {\n    foo( 1,\n       2);\n    // foo(3, 4)\n}\n";
    std::fs::write(path.join("a.rs"), text).unwrap();
    let body = "fn copied(a: i32) -> i32 {\n let t = a + 1;\n if t > 2 { println!(\"{}\", t); return t - 1; }\n for i in 0..t { println!(\"{}\", i + a); }\n t * a + 3\n}\n";
    std::fs::write(path.join("c1.rs"), body).unwrap();
    std::fs::write(path.join("c2.rs"), body).unwrap();

    let mut editor = Editor::start_with(home.path(), &path, &[]);
    editor.wait_for(b"Navigate");
    editor.send(b"\x0bReplace in Files\r");
    editor.wait_for(b"Replace");
    editor.send(b"\x0bToggle Structural\r");
    editor.wait_for(b"[$X$]");
    editor.send(b"foo($A$, $B$)\r");
    editor.wait_for(b"foo( 1,");
    editor.send(b"\tbar($B$, $A$)\r");
    editor.wait_for(b"bar(2, 1)");
    assert_eq!(
        std::fs::read_to_string(path.join("a.rs")).unwrap(),
        text,
        "only a preview"
    );
    editor.send(b"y");
    let wanted = "fn a() {\n    bar(2, 1);\n    // foo(3, 4)\n}\n";
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
    while std::fs::read_to_string(path.join("a.rs")).unwrap() != wanted {
        assert!(std::time::Instant::now() < deadline, "never replaced");
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    editor.send(b"\x1b");
    editor.send(b"\x1b");
    editor.send(b"\x0bLocate Duplicates\r");
    editor.wait_all(&[b"2 places", b"c1.rs:1-6"]);
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

fn shows_white(parser: &vt100::Parser) -> bool {
    let screen = parser.screen();
    let (rows, cols) = screen.size();
    (0..rows).any(|y| {
        (0..cols).any(|x| {
            screen.cell(y, x).is_some_and(|c| {
                matches!(c.bgcolor(), vt100::Color::Idx(231) | vt100::Color::Idx(15))
                    || matches!(c.bgcolor(), vt100::Color::Rgb(r, g, b) if r > 240 && g > 240 && b > 240)
            })
        })
    })
}

#[test]
#[ignore = "needs rust-analyzer on PATH"]
fn real_rust_analyzer_through_the_ui_rename_refactor_this_and_safe_delete() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let path = project.path().canonicalize().unwrap();
    std::fs::create_dir(path.join("src")).unwrap();
    std::fs::write(
        path.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    std::fs::write(path.join("src/main.rs"), "fn helper() -> i32 {\n    3\n}\n\n/// Docs.\nfn unused() {}\n\nfn main() {\n    println!(\"{}\", helper());\n}\n").unwrap();
    let mut editor = Editor::start_env(
        home.path(),
        &path,
        &["src/main.rs"],
        &[("TTYWRIGHT_RUST_LSP", "rust-analyzer".to_owned())],
    );
    editor.wait_for(b"helper");
    let read = || std::fs::read_to_string(path.join("src/main.rs")).unwrap();
    editor.send(b"\x1b[C\x1b[C\x1b[C");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(120);
    while !editor.shows_within(b"Safe Delete", std::time::Duration::from_secs(1), b"\x1bj") {
        assert!(
            std::time::Instant::now() < deadline,
            "the server never answered"
        );
    }
    editor.send(b"\x1b");
    std::thread::sleep(std::time::Duration::from_millis(200));
    editor.send(b"\x1bm");
    editor.wait_for(b"Rename Symbol");
    editor.send(b"\x7f\x7f\x7f\x7f\x7f\x7fassist\r");
    editor.wait_for(b"Apply Refactoring");
    assert!(read().contains("fn helper"), "only a preview");
    editor.send(b"y");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while !(read().contains("fn assist") && read().contains("assist()")) {
        assert!(
            std::time::Instant::now() < deadline,
            "not renamed: {}",
            read()
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    editor.send(b"\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B");
    editor.send(b"\x1b[3;3~");
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    while read().contains("unused") {
        assert!(
            std::time::Instant::now() < deadline,
            "not deleted: {}",
            read()
        );
        std::thread::sleep(std::time::Duration::from_millis(100));
    }
    assert!(!read().contains("Docs"), "{}", read());
    assert!(read().contains("fn main"));
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

fn write_tool(bin: &Path, name: &str, body: &str) {
    use std::os::unix::fs::PermissionsExt;
    std::fs::create_dir_all(bin).unwrap();
    let tool = bin.join(name);
    std::fs::write(&tool, body).unwrap();
    std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o755)).unwrap();
}

#[test]
fn docker_lists_cleans_hostile_names_and_removes_only_after_y() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    std::fs::write(project_path.join("a.txt"), "one\n").unwrap();
    let bin = home.path().join("bin");
    let log = home.path().join("docker-log");
    write_tool(
        &bin,
        "docker",
        &format!(
            r#"#!/bin/sh
echo "$PWD|$*" >> {log}
case "$1" in
  compose) echo '{{"services":{{"web":{{"image":"nginx","environment":{{"KEY":"topsecret"}}}}}}}}' ;;
  ps) printf '%s\n' '{{"ID":"abc123","Names":"evil\u001b]0;pwned\u0007name","Image":"nginx","State":"running","Status":"Up 1 minute"}}' ;;
  images) echo '{{"ID":"f00d","Repository":"nginx","Tag":"1","Size":"9MB"}}' ;;
  logs) echo 'log-line-from-container' ;;
esac
"#,
            log = log.display()
        ),
    );
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let env = [("PATH", path)];
    let mut editor = Editor::start_env(home.path(), &project_path, &["a.txt"], &env);
    editor.wait_for(b"one");
    assert!(!log.exists(), "nothing runs until the tab opens");
    editor.send(b"\x1bz");
    let shown = editor.wait_for(b"nginx");
    assert!(!String::from_utf8_lossy(&shown).contains("topsecret"));
    editor.send(b"\t");
    let shown = editor.wait_for(b"minute");
    assert!(
        !shown.windows(4).any(|w| w == b"\x1b]0;"),
        "the title sequence does not reach the terminal as one"
    );
    editor.send(b"d");
    editor.wait_for(b"abc123");
    editor.send(b"\r");
    editor.wait_for(b"Not run");
    let said = std::fs::read_to_string(&log).unwrap();
    assert!(!said.contains("rm abc123"), "Enter is not y");
    editor.send(b"d");
    editor.wait_for(b"abc123");
    editor.send(b"y");
    editor.wait_for(b"done");
    let said = std::fs::read_to_string(&log).unwrap();
    assert!(said.contains("rm abc123"), "{said}");
    let project_dir = project_path.display().to_string();
    for line in said.lines() {
        let (cwd, args) = line.split_once('|').unwrap();
        assert_eq!(
            args.starts_with("compose"),
            cwd.ends_with(&project_dir),
            "only compose runs in the project: {line}"
        );
    }
    editor.send(b"l");
    editor.wait_for(b"log-line-from-container");
    editor.send(b"\x11");
    assert!(editor.wait_exit());
}

#[test]
fn docker_in_an_untrusted_project_never_reads_the_compose_file() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    std::fs::write(project_path.join("a.txt"), "one\n").unwrap();
    let bin = home.path().join("bin");
    let log = home.path().join("docker-log");
    write_tool(
        &bin,
        "docker",
        &format!("#!/bin/sh\necho \"$*\" >> {}\n", log.display()),
    );
    let path = format!("{}:{}", bin.display(), std::env::var("PATH").unwrap());
    let env = [("PATH", path), ("TTYWRIGHT_TRUST_ALL", String::new())];
    let mut editor = Editor::start_env(home.path(), &project_path, &["a.txt"], &env);
    editor.wait_for(b"Trust This Project?");
    editor.send(b"n");
    editor.wait_for(b"Not trusted:");
    editor.send(b"\x1bz");
    editor.wait_for(b"trusted");
    let deadline = Instant::now() + WAIT;
    let said = loop {
        let said = std::fs::read_to_string(&log).unwrap_or_default();
        if said.contains("images") || Instant::now() > deadline {
            break said;
        }
        std::thread::sleep(Duration::from_millis(50));
    };
    editor.send(b"\x11");
    editor.wait_exit();
    assert!(!said.contains("compose"), "{said}");
    assert!(said.contains("ps -a"), "{said}");
}

#[test]
#[ignore = "needs docker on PATH"]
fn real_docker_lists_a_compose_file_and_says_plainly_if_the_daemon_is_down() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    std::fs::write(project_path.join("a.txt"), "one\n").unwrap();
    std::fs::write(
        project_path.join("compose.yaml"),
        "services:\n  realweb:\n    image: nginx:1\n    environment:\n      KEY: topsecret\n",
    )
    .unwrap();
    let config = std::path::PathBuf::from(std::env::var("HOME").unwrap()).join(".docker");
    let env = [("DOCKER_CONFIG", config.display().to_string())];
    let mut editor = Editor::start_env(home.path(), &project_path, &["a.txt"], &env);
    editor.wait_for(b"one");
    editor.send(b"\x1bz");
    let shown = editor.wait_for(b"realweb");
    assert!(!String::from_utf8_lossy(&shown).contains("topsecret"));
    editor.send(b"\t");
    let daemon = std::process::Command::new("docker")
        .args(["version", "--format", "{{.Server.Version}}"])
        .output()
        .unwrap();
    if !daemon.status.success() {
        editor.wait_for(b"running");
    }
    editor.send(b"\x11");
    assert!(editor.wait_exit());
}

fn fake_gh(home: &Path) -> Vec<(&'static str, String)> {
    let bin = home.join("bin");
    let log = home.join("gh-log");
    let stdin = home.join("gh-stdin");
    write_tool(
        &bin,
        "gh",
        &format!(
            r#"#!/bin/sh
echo "$*" >> {log}
case "$1 $2" in
  "api user") echo '{{"login":"tester","email":"private@example.com"}}' ;;
  "repo view") echo '{{"nameWithOwner":"owner/name","defaultBranchRef":{{"name":"main"}}}}' ;;
  "pr list") echo '[{{"number":7,"title":"Evil\u001b]0;pwned\u0007title","state":"OPEN","author":{{"login":"mallory"}},"headRefName":"fix"}}]' ;;
  "issue list") echo '[]' ;;
  "pr view") echo '{{"number":7,"title":"Evil title","state":"OPEN","author":{{"login":"mallory"}},"body":"body-of-pr\u001b[2J","url":"u","headRefName":"fix","headRefOid":"abc123","baseRefName":"main","statusCheckRollup":[{{"name":"lint","conclusion":"SUCCESS"}}]}}' ;;
  "pr diff") printf 'diff --git a/x.rs b/x.rs\n--- a/x.rs\n+++ b/x.rs\n@@ -1 +1 @@\n-old-line\n+new-line\n' ;;
  "pr comment") cat > {stdin} ;;
  "pr merge") echo 'ghp_secrettoken123 merge failed' >&2; exit 1 ;;
  api*) echo '[]' ;;
esac
"#,
            log = log.display(),
            stdin = stdin.display()
        ),
    );
    vec![("PATH", format!("{}:/usr/bin:/bin", bin.display()))]
}

#[test]
fn github_shows_hostile_text_clean_and_posts_only_what_the_screen_showed_after_y() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    std::fs::write(project_path.join("a.txt"), "one\n").unwrap();
    let env = fake_gh(home.path());
    let log = home.path().join("gh-log");
    let stdin = home.path().join("gh-stdin");
    let logged = || std::fs::read_to_string(&log).unwrap_or_default();
    let mut editor = Editor::start_env(home.path(), &project_path, &["a.txt"], &env);
    editor.wait_for(b"one");
    assert_eq!(logged(), "", "nothing is asked until the tab opens");
    editor.send(b"\x1bv");
    let shown = editor.wait_for(b"mallory");
    assert!(
        !shown.windows(4).any(|w| w == b"\x1b]0;"),
        "a title cannot set the window's title"
    );
    assert!(!String::from_utf8_lossy(&shown).contains("private@example.com"));

    editor.send(b"\r");
    editor.wait_for(b"review:");
    editor.send(b"\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B\x1b[B");
    editor.wait_for(b"body-of-pr");
    editor.send(b"f");
    editor.wait_for(b"new-line");
    editor.send(b"\x1b");
    std::thread::sleep(Duration::from_millis(200));

    editor.send(b"c");
    editor.wait_for(b"Comment");
    editor.send(b"my-comment");
    editor.wait_for(b"my-comment");
    editor.send(b"\r");
    let shown = editor.wait_for(b"tester");
    let shown = String::from_utf8_lossy(&shown);
    assert!(shown.contains("owner/name"), "says where: {shown}");
    assert!(!logged().contains("pr comment"), "not before y");
    editor.send(b"\r");
    editor.wait_for(b"Not");
    assert!(!logged().contains("pr comment"), "Enter is not y");

    editor.send(b"c");
    editor.wait_for(b"Comment");
    editor.send(b"my-comment");
    editor.wait_for(b"my-comment");
    editor.send(b"\r");
    editor.wait_for(b"tester");
    editor.send(b"y");
    editor.wait_for(b"done");
    assert!(
        logged().contains("pr comment 7 -R owner/name --body-file -"),
        "{}",
        logged()
    );
    assert_eq!(std::fs::read_to_string(&stdin).unwrap(), "my-comment");

    editor.send(b"m");
    editor.wait_for(b"continue");
    editor.send(b"y");
    editor.wait_for(b"Type");
    assert!(!logged().contains("pr merge"), "y alone does not merge");
    editor.send(b"7\r");
    let shown = editor.wait_for(b"failed");
    assert!(
        logged().contains("pr merge 7 --merge -R owner/name --match-head-commit abc123"),
        "{}",
        logged()
    );
    assert!(
        !String::from_utf8_lossy(&shown).contains("secrettoken"),
        "a token never reaches the screen"
    );
    editor.send(b"\x11");
    assert!(editor.wait_exit());
}

#[test]
fn github_in_an_untrusted_project_runs_no_gh_at_all() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    std::fs::write(project_path.join("a.txt"), "one\n").unwrap();
    let mut env = fake_gh(home.path());
    env.push(("TTYWRIGHT_TRUST_ALL", String::new()));
    let mut editor = Editor::start_env(home.path(), &project_path, &["a.txt"], &env);
    editor.wait_for(b"Trust This Project?");
    editor.send(b"n");
    editor.wait_for(b"Not trusted:");
    editor.send(b"\x1bv");
    editor.wait_for(b"gh reads");
    std::thread::sleep(Duration::from_millis(300));
    assert!(!home.path().join("gh-log").exists(), "gh ran");
    editor.send(b"\x1b");
    editor.send(b"\x11");
    assert!(editor.wait_exit());
}

#[test]
#[ignore = "needs gh on PATH"]
fn real_gh_without_a_login_says_so_plainly() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    std::fs::write(project_path.join("a.txt"), "one\n").unwrap();
    git(&project_path, &["init", "-q"]);
    git(
        &project_path,
        &["remote", "add", "origin", "https://github.com/cli/cli"],
    );
    let env = [
        (
            "GH_CONFIG_DIR",
            home.path().join("gh").display().to_string(),
        ),
        ("GH_TOKEN", String::new()),
        ("GITHUB_TOKEN", String::new()),
    ];
    let mut editor = Editor::start_env(home.path(), &project_path, &["a.txt"], &env);
    editor.wait_for(b"one");
    editor.send(b"\x1bv");
    editor.wait_for(b"logged");
    editor.send(b"\x11");
    assert!(editor.wait_exit());
}

#[test]
#[ignore = "needs gh on PATH, logged in, and the network"]
fn real_gh_reads_a_public_repository_and_a_pull_request() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let project_path = project.path().canonicalize().unwrap();
    std::fs::write(project_path.join("a.txt"), "one\n").unwrap();
    git(&project_path, &["init", "-q"]);
    git(
        &project_path,
        &["remote", "add", "origin", "https://github.com/cli/cli"],
    );
    let real_home = std::env::var("HOME").unwrap();
    let env = [
        ("GH_CONFIG_DIR", format!("{real_home}/.config/gh")),
        ("HOME", real_home),
    ];
    let mut editor = Editor::start_env(home.path(), &project_path, &["a.txt"], &env);
    editor.wait_for(b"one");
    editor.send(b"\x1bv");
    editor.wait_for(b"cli/cli");
    editor.wait_for(b"#");
    editor.send(b"\r");
    editor.wait_for(b"review:");
    editor.send(b"f");
    editor.wait_for(b"Esc back");
    editor.send(b"\x1b");
    editor.send(b"\x1b");
    editor.send(b"\x11");
    editor.wait_exit();
}

fn copy_tree(from: &Path, to: &Path) {
    let done = Command::new("cp")
        .arg("-R")
        .arg(from)
        .arg(to)
        .status()
        .unwrap();
    assert!(done.success());
}

fn moved_release_folder(into: &Path) -> std::path::PathBuf {
    let dist = Path::new(env!("CARGO_MANIFEST_DIR")).join("dist");
    let built = std::fs::read_dir(dist)
        .unwrap()
        .flatten()
        .find(|e| e.file_name().to_string_lossy().starts_with("ttywright-"))
        .expect("scripts/build-release.sh has been run")
        .path();
    let folder = into.join("somewhere else");
    copy_tree(&built, &folder);
    folder
}

fn install_hello(home: &Path) {
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/hello");
    for base in ["Library/Application Support/ttywright", "config/ttywright"] {
        let dir = home.join(base).join("plugins/hello");
        std::fs::create_dir_all(&dir).unwrap();
        std::fs::copy(source.join("plugin.toml"), dir.join("plugin.toml")).unwrap();
        let wasm = wat::parse_file(source.join("plugin.wat")).unwrap();
        std::fs::write(dir.join("plugin.wasm"), wasm).unwrap();
    }
}

const COLOURED_FN: &[u8] = b"fn\x1b[";

#[test]
#[ignore = "needs scripts/build-release.sh"]
fn a_moved_release_folder_works_and_takes_no_code_from_the_project() {
    let (home, place, project) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let folder = moved_release_folder(place.path());
    install_hello(home.path());
    std::fs::write(project.path().join("a.rs"), "fn main() {}\n").unwrap();
    let mark = project.path().join("planted-ran");
    let planted = project.path().join("lib");
    std::fs::create_dir_all(planted.join("grammars")).unwrap();
    std::fs::write(planted.join("grammars/rust.dylib"), "not a library").unwrap();
    std::fs::write(planted.join("grammars/rust.so"), "not a library").unwrap();
    let script = planted.join("ttywright-plugins");
    std::fs::write(&script, format!("#!/bin/sh\ntouch {}\n", mark.display())).unwrap();
    Command::new("chmod")
        .arg("+x")
        .arg(&script)
        .status()
        .unwrap();

    let link = place.path().join("linked");
    std::os::unix::fs::symlink(folder.join("ttywright"), &link).unwrap();
    let path_with_planted = format!("{}:/usr/bin:/bin", planted.display());
    let mut editor = Editor::start_program(
        &link,
        home.path(),
        project.path(),
        &["a.rs"],
        &[("PATH", path_with_planted)],
    );
    editor.wait_for(COLOURED_FN);
    editor.send(b"\x0bPlugin Say\r");
    editor.wait_for(b"Said hello");
    assert!(!mark.exists(), "nothing of the project's ran");
    editor.send(QUIT);
    assert!(editor.wait_exit());
    let text = std::fs::read_to_string(project.path().join("a.rs")).unwrap();
    assert!(text.contains("hello from a plugin"), "{text}");
}

#[test]
#[ignore = "needs scripts/build-release.sh"]
fn a_missing_or_damaged_module_costs_only_its_own_feature() {
    let (home, place, project) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let folder = moved_release_folder(place.path());
    install_hello(home.path());
    std::fs::write(project.path().join("a.rs"), "fn main() {}\n").unwrap();
    std::fs::write(project.path().join("b.toml"), "a = 1\n").unwrap();
    let extension = std::env::consts::DLL_EXTENSION;
    let grammars = folder.join("lib/grammars");
    std::fs::remove_file(grammars.join(format!("rust.{extension}"))).unwrap();
    std::fs::write(grammars.join(format!("toml.{extension}")), "garbage").unwrap();
    std::fs::remove_file(folder.join("lib/ttywright-plugins")).unwrap();

    let mut editor = Editor::start_program(
        &folder.join("ttywright"),
        home.path(),
        project.path(),
        &["a.rs"],
        &[],
    );
    editor.wait_for(b"No colours for Rust");
    editor.send(b"\x0bPlugin Say\r");
    editor.wait_for(b"cannot start");
    editor.send(b"x");
    editor.send(QUIT);
    assert!(editor.wait_exit(), "the editor went on as usual");
    let text = std::fs::read_to_string(project.path().join("a.rs")).unwrap();
    assert!(text.starts_with('x'), "{text}");

    let mut editor = Editor::start_program(
        &folder.join("ttywright"),
        home.path(),
        project.path(),
        &["b.toml"],
        &[],
    );
    editor.wait_for(b"No colours for TOML");
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

#[test]
#[ignore = "needs scripts/build-release.sh"]
fn the_server_of_a_release_folder_runs_from_its_own_file_and_only_needed_grammars_load() {
    let (home, place, project) = (
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
        tempfile::tempdir().unwrap(),
    );
    let folder = moved_release_folder(place.path());
    let file = |name: &str, text: &str| {
        std::fs::create_dir_all(project.path().join("src")).unwrap();
        std::fs::write(project.path().join(name), text).unwrap();
    };
    file(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    file("src/main.rs", "fn main() {}\n");
    let mut editor = Editor::start_program(
        &folder.join("ttywright"),
        home.path(),
        project.path(),
        &["src/main.rs"],
        &[("PATH", "/usr/bin:/bin".to_owned())],
    );
    editor.wait_for(COLOURED_FN);
    let server = folder.join("lib/rust-analyzer");
    let deadline = Instant::now() + WAIT;
    let running = || {
        Command::new("pgrep")
            .args(["-f", &server.display().to_string()])
            .output()
            .is_ok_and(|o| !o.stdout.is_empty())
    };
    while !running() {
        assert!(Instant::now() < deadline, "the server did not start");
        std::thread::sleep(Duration::from_millis(100));
    }
    let pid = editor.child.process_id().unwrap().to_string();
    let open = Command::new("lsof")
        .args(["-p", &pid])
        .output()
        .unwrap()
        .stdout;
    let open = String::from_utf8_lossy(&open);
    assert!(open.contains("grammars/rust."), "{open}");
    assert!(!open.contains("grammars/php."), "only what is met: {open}");
    assert!(!open.contains("wasmtime"));
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

fn untrusted_env() -> Vec<(&'static str, String)> {
    vec![("TTYWRIGHT_TRUST_ALL", String::new())]
}

fn wait_file(path: &Path, wanted: &str) {
    let deadline = Instant::now() + WAIT;
    loop {
        let now = std::fs::read_to_string(path).unwrap_or_default();
        if now == wanted {
            return;
        }
        assert!(Instant::now() < deadline, "the file still holds {now:?}");
        std::thread::sleep(Duration::from_millis(50));
    }
}

#[test]
fn text_typed_after_the_trust_question_reaches_the_disk_however_it_was_answered() {
    for answer in [&b"y"[..], b"n", b"\x1b"] {
        for by_ctrl_s in [true, false] {
            let home = tempfile::tempdir().unwrap();
            let project = tempfile::tempdir().unwrap();
            let file = project.path().join("a.txt");
            std::fs::write(&file, "hello\n").unwrap();
            let mut editor =
                Editor::start_env(home.path(), project.path(), &["a.txt"], &untrusted_env());
            editor.wait_for(b"Trust This");
            editor.send(answer);
            std::thread::sleep(Duration::from_millis(300));
            editor.send(b"Z");
            if by_ctrl_s {
                editor.send(b"\x13");
            }
            wait_file(&file, "Zhello\n");
            editor.kill_hard();
        }
    }
}

#[test]
fn a_terminate_or_hangup_saves_the_text_and_the_tabs() {
    for signal in ["-TERM", "-HUP"] {
        let home = tempfile::tempdir().unwrap();
        let project = tempfile::tempdir().unwrap();
        let file = project.path().join("a.txt");
        std::fs::write(&file, "hello\n").unwrap();
        let mut editor = Editor::start(home.path(), project.path(), "a.txt");
        editor.wait_for(b"hello");
        editor.send(b"Z");
        editor.wait_for(b"Z");
        let pid = editor.child.process_id().unwrap().to_string();
        assert!(
            Command::new("kill")
                .args([signal, &pid])
                .status()
                .unwrap()
                .success()
        );
        assert!(editor.wait_exit(), "{signal}: a clean exit status");
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "Zhello\n");

        let mut again = Editor::start_with(home.path(), project.path(), &[]);
        again.wait_for(b"hello");
        again.send(b"Y");
        wait_file(&file, "ZYhello\n");
        again.send(QUIT);
        assert!(again.wait_exit());
        assert_eq!(std::fs::read_to_string(&file).unwrap(), "ZYhello\n");
    }
}

#[test]
fn after_a_clean_quit_the_tab_returns_and_typing_is_saved() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let file = project.path().join("a.txt");
    std::fs::write(&file, "hello\n").unwrap();
    let mut editor = Editor::start(home.path(), project.path(), "a.txt");
    editor.wait_for(b"hello");
    editor.send(QUIT);
    assert!(editor.wait_exit());
    let mut again = Editor::start_with(home.path(), project.path(), &[]);
    again.wait_for(b"hello");
    again.send(b"Q\x13");
    wait_file(&file, "Qhello\n");
    again.send(QUIT);
    assert!(again.wait_exit());
}

fn screen_of(editor: &mut Editor, wanted: &str, keys: &[u8]) -> vt100::Parser {
    editor.send(keys);
    let deadline = Instant::now() + WAIT;
    let mut parser = vt100::Parser::new(30, 100, 0);
    let mut fed = 0;
    loop {
        while let Ok(chunk) = editor.output.try_recv() {
            if chunk.windows(3).any(|w| w == b"\x1b[c") {
                editor.send(b"\x1b[?62c");
            }
            if chunk.windows(4).any(|w| w == b"\x1b[6n") {
                editor.send(b"\x1b[1;1R");
            }
            editor.seen.extend(chunk);
        }
        parser.process(&editor.seen[fed..]);
        fed = editor.seen.len();
        if parser.screen().contents().contains(wanted) {
            std::thread::sleep(Duration::from_millis(200));
            while let Ok(chunk) = editor.output.try_recv() {
                parser.process(&chunk);
            }
            return parser;
        }
        assert!(
            Instant::now() < deadline,
            "never showed {wanted:?}:\n{}",
            parser.screen().contents()
        );
        std::thread::sleep(Duration::from_millis(20));
    }
}

fn cell_of(screen: &vt100::Screen, text: &str, after_column: u16) -> vt100::Cell {
    let (rows, cols) = screen.size();
    for row in 0..rows {
        let line = screen.contents_between(row, 0, row, cols);
        let chars: Vec<char> = line.chars().collect();
        let wanted: Vec<char> = text.chars().collect();
        for start in usize::from(after_column)..chars.len().saturating_sub(wanted.len()) {
            if chars[start..].starts_with(&wanted) {
                return screen.cell(row, start as u16).unwrap().clone();
            }
        }
    }
    panic!("{text:?} is not on the screen:\n{}", screen.contents());
}

fn gradient_png(path: &Path) {
    image::RgbImage::from_fn(64, 32, |x, _| image::Rgb([if x < 32 { 255 } else { 0 }; 3]))
        .save(path)
        .unwrap();
}

#[test]
fn a_markdown_file_opens_rendered_with_real_bold_and_italic_on_the_screen() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(
        project.path().join("a.md"),
        "# Heading\n\nplain **thick** and *slanted* end\n",
    )
    .unwrap();
    let mut editor = Editor::start(home.path(), project.path(), "a.md");
    let parser = screen_of(&mut editor, "plain thick and slanted end", b"");
    let screen = parser.screen();
    assert!(
        screen.contents().contains("**thick**"),
        "the text beside it"
    );
    let thick = cell_of(screen, "thick and", 0);
    assert!(thick.bold(), "bold on the screen");
    let slanted = cell_of(screen, "slanted end", 0);
    assert!(slanted.italic(), "italic on the screen");
    assert!(!cell_of(screen, "plain thick", 0).bold());
    assert!(screen.contents().contains("═══"), "the heading's rule");
    let parser = screen_of(&mut editor, "Preview", b"\x1b.");
    assert!(
        !parser.screen().contents().contains("plain thick and"),
        "{}",
        parser.screen().contents()
    );
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

fn pictures_work_in(program: &Path) {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path();
    gradient_png(&root.join("half.png"));
    image::RgbImage::from_fn(64, 32, |x, _| image::Rgb([if x < 32 { 255 } else { 0 }; 3]))
        .save(root.join("half.jpg"))
        .unwrap();
    std::fs::write(
        root.join("a.md"),
        "![the png](half.png)\n\n![the jpeg](half.jpg)\n\n![web](https://x.test/a.png)\n\n![none](none.png)\n",
    )
    .unwrap();
    let trust = [("TTYWRIGHT_TRUST_ALL", "1".to_owned())];

    let mut editor = Editor::start_program(program, home.path(), root, &["a.md"], &trust);
    let parser = screen_of(&mut editor, "the png", b"");
    std::thread::sleep(Duration::from_millis(400));
    let text = parser.screen().contents();
    assert!(text.contains("the png"), "{text}");
    assert!(!text.contains("decoding image") || shows_white(&parser));
    editor.send(QUIT);
    assert!(editor.wait_exit());

    std::fs::write(
        root.join("b.md"),
        "![web](https://x.test/a.png)\n\n![none](none.png)\n",
    )
    .unwrap();
    let mut editor = Editor::start_program(program, home.path(), root, &["b.md"], &trust);
    let parser = screen_of(&mut editor, "file not found", b"");
    assert!(parser.screen().contents().contains("https://x.test/a.png"));
    editor.send(QUIT);
    assert!(editor.wait_exit());

    let before = std::fs::read(root.join("half.png")).unwrap();
    let mut editor = Editor::start_program(program, home.path(), root, &["half.png"], &trust);
    let parser = screen_of(&mut editor, "64×32", b"");
    assert!(shows_white(&parser), "the light half has its color");
    editor.send(b"x\x13");
    std::thread::sleep(Duration::from_millis(1500));
    editor.send(QUIT);
    assert!(editor.wait_exit());
    assert_eq!(std::fs::read(root.join("half.png")).unwrap(), before);

    let mut editor = Editor::start_program(program, home.path(), root, &["half.jpg"], &trust);
    let parser = screen_of(&mut editor, "64×32", b"");
    assert!(shows_white(&parser), "the light half has its color");
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

#[test]
fn pictures_show_as_characters_in_a_preview_and_in_their_own_tab() {
    pictures_work_in(Path::new(env!("CARGO_BIN_EXE_ttyw")));
}

#[test]
fn a_damaged_picture_says_so_and_a_narrow_window_shows_the_preview_alone() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    std::fs::write(project.path().join("bad.png"), b"\x89PNG\r\n\x1a\ngarbage").unwrap();
    let mut editor = Editor::start(home.path(), project.path(), "bad.png");
    screen_of(&mut editor, "cannot show it", b"");
    editor.send(QUIT);
    assert!(editor.wait_exit());

    std::fs::write(project.path().join("a.md"), "plain **thick** end\n").unwrap();
    let mut editor = Editor::start(home.path(), project.path(), "a.md");
    screen_of(&mut editor, "plain thick end", b"");
    editor
        ._master
        .resize(PtySize {
            rows: 30,
            cols: 45,
            pixel_width: 0,
            pixel_height: 0,
        })
        .unwrap();
    let file = project.path().join("a.md");
    std::thread::sleep(Duration::from_millis(500));
    let parser = screen_of(&mut editor, "Z", b"Z");
    let shown = parser.screen().contents();
    assert!(
        shown.contains("Zplain **thick**"),
        "the text is back:\n{shown}"
    );
    wait_file(&file, "Zplain **thick** end\n");
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

#[test]
#[ignore = "needs the folder from scripts/build-release.sh"]
fn pictures_work_in_the_release_folder() {
    let into = tempfile::tempdir().unwrap();
    let program = moved_release_folder(into.path());
    pictures_work_in(&program.join("ttywright"));
}

#[test]
fn a_picture_goes_to_the_system_viewer_and_nothing_else_does() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    let bin = tempfile::tempdir().unwrap();
    let marker = bin.path().join("opened");
    for name in ["open", "xdg-open"] {
        let script = bin.path().join(name);
        std::fs::write(
            &script,
            format!("#!/bin/sh\necho \"$1\" >> '{}'\n", marker.display()),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
    }
    image::RgbImage::from_pixel(8, 8, image::Rgb([255, 255, 255]))
        .save(root.join("real.png"))
        .unwrap();
    std::fs::write(root.join("evil.command"), "#!/bin/sh\n").unwrap();
    std::fs::write(root.join("a.md"), "![x](evil.command)\n").unwrap();
    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let env = [("PATH", path), ("TTYWRIGHT_TRUST_ALL", "1".to_owned())];

    let mut editor = Editor::start_env(home.path(), &root, &["a.md"], &env);
    let parser = screen_of(&mut editor, "not a picture", b"\x1b;");
    assert!(parser.screen().contents().contains("not a picture"));
    assert!(!marker.exists(), "never handed to the opener");
    editor.send(QUIT);
    assert!(editor.wait_exit());

    let mut editor = Editor::start_env(home.path(), &root, &["real.png"], &env);
    screen_of(&mut editor, "8×8", b"");
    editor.send(b"o");
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !marker.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let opened = std::fs::read_to_string(&marker).unwrap_or_default();
    assert_eq!(opened.trim(), root.join("real.png").display().to_string());
    editor.send(QUIT);
    assert!(editor.wait_exit());
}

#[test]
fn the_panel_starts_claude_with_the_editors_tools() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    std::fs::write(root.join("a.txt"), "x\n").unwrap();
    let bin = tempfile::tempdir().unwrap();
    let marker = bin.path().join("args");
    let script = bin.path().join("claude");
    std::fs::write(
        &script,
        format!(
            "#!/bin/sh\nprintf '%s\\n' \"$@\" > '{}'\nsleep 5\n",
            marker.display()
        ),
    )
    .unwrap();
    std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755)).unwrap();
    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let env = [("PATH", path), ("TTYWRIGHT_TRUST_ALL", "1".to_owned())];
    let mut editor = Editor::start_env(home.path(), &root, &["a.txt"], &env);
    editor.wait_for(b"a.txt");
    editor.send(b"\x1b2");
    let deadline = std::time::Instant::now() + Duration::from_secs(10);
    while !marker.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let args = std::fs::read_to_string(&marker).unwrap_or_default();
    let mut lines = args.lines();
    assert_eq!(lines.next(), Some("--mcp-config"), "{args}");
    let config: serde_json::Value = serde_json::from_str(lines.next().unwrap_or("")).unwrap();
    let server = &config["mcpServers"]["ttywright"];
    assert_eq!(server["args"][0], "mcp");
    assert_eq!(server["args"][1], root.display().to_string());
    editor.send(b"\x1b\x5b\x32\x30\x7e");
    drop(editor);
}

#[test]
fn a_finished_run_notifies_in_the_corner_and_through_the_system() {
    let home = tempfile::tempdir().unwrap();
    let project = tempfile::tempdir().unwrap();
    let root = project.path().canonicalize().unwrap();
    std::fs::write(root.join("a.txt"), "x\n").unwrap();
    let bin = tempfile::tempdir().unwrap();
    let marker = bin.path().join("told");
    for name in ["osascript", "notify-send"] {
        let script = bin.path().join(name);
        std::fs::write(
            &script,
            format!(
                "#!/bin/sh\nprintf '%s\\n' \"$@\" >> '{}'\n",
                marker.display()
            ),
        )
        .unwrap();
        std::fs::set_permissions(&script, std::os::unix::fs::PermissionsExt::from_mode(0o755))
            .unwrap();
    }
    let path = format!(
        "{}:{}",
        bin.path().display(),
        std::env::var("PATH").unwrap_or_default()
    );
    let env = [("PATH", path), ("TTYWRIGHT_TRUST_ALL", "1".to_owned())];
    let mut editor = Editor::start_env(home.path(), &root, &["a.txt"], &env);
    editor.wait_for(b"a.txt");
    editor.send(b"\x1bx");
    editor.wait_for(b"Run Anything");
    editor.send(b"sleep 1; exit 3");
    std::thread::sleep(Duration::from_millis(200));
    editor.send(b"\r");
    std::thread::sleep(Duration::from_millis(300));
    editor.send(b"\x1b[O");
    let parser = screen_of(&mut editor, "Exit code 3", b"");
    assert!(
        parser.screen().contents().contains("Run Output"),
        "its action"
    );
    let deadline = std::time::Instant::now() + Duration::from_secs(5);
    while !marker.exists() && std::time::Instant::now() < deadline {
        std::thread::sleep(Duration::from_millis(50));
    }
    let told = std::fs::read_to_string(&marker).unwrap_or_default();
    assert!(told.contains("Exit code 3"), "{told}");
    editor.send(QUIT);
    assert!(editor.wait_exit());
}
