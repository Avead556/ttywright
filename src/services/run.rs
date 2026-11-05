use std::collections::HashMap;
use std::ffi::OsString;
use std::fs::File;
use std::io::{self, Read, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::process::{CommandExt, ExitStatusExt};
use std::path::{Path, PathBuf};
use std::process::{Child, ChildStdin, Command, ExitCode, ExitStatus, Stdio};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use crate::core::run::{ActiveFile, MAX_PROJECT_FILE_BYTES, RunCommand, RunFacts, RunTarget};
use crate::core::run_location::{Candidate, Named, candidates, jvm_file, tidy_path};
use crate::core::run_output::{
    MAX_LINES, MAX_TOTAL_BYTES, OutputDecoder, RunLine, RunLocation, RunOutcome, RunSpan,
};
use crate::core::run_state::RunEvent;
use crate::core::{Event, run};
use crate::services::fs::walker;

const STOP_GRACE: Duration = Duration::from_millis(1500);
const KILL_GRACE: Duration = Duration::from_secs(1);
const DRAIN_GRACE: Duration = Duration::from_millis(300);
const TICK: Duration = Duration::from_millis(10);
const FLUSH_EVERY: Duration = Duration::from_millis(30);
const GROUP_CHECK_EVERY: Duration = Duration::from_millis(50);
const CHUNK_BYTES: usize = 64 * 1024;
const CHUNKS_QUEUED: usize = 4;
const MAX_REMEMBERED_PATHS: usize = 4096;
const MAX_INDEXED_FILES: usize = 200_000;

const SESSION_WAIT: Duration = Duration::from_secs(10);
pub const LAUNCHER_FLAG: &str = "--run-in-new-session";
const COULD_NOT_EXEC: u8 = 127;

pub fn exec_in_new_session(mut args: impl Iterator<Item = OsString>) -> ExitCode {
    let Some(program) = args.next() else {
        return ExitCode::from(COULD_NOT_EXEC);
    };
    if let Err(e) = rustix::process::setsid() {
        eprintln!("cannot start a session: {e}");
        return ExitCode::from(COULD_NOT_EXEC);
    }
    let error = Command::new(&program).args(args).exec();
    eprintln!("{}: {error}", program.to_string_lossy());
    ExitCode::from(COULD_NOT_EXEC)
}

const GUARDIAN: &str = r#"
trap '' HUP INT TERM
IFS= read -r group || exit 0
IFS= read -r over && exit 0
kill -s TERM -- "-$group"
kill -s CONT -- "-$group"
i=0
while [ "$i" -lt 15 ] && kill -s 0 -- "-$group"; do
    sleep 0.1
    i=$((i + 1))
done
kill -s KILL -- "-$group"
"#;

fn signal_group(signal: &str, group: u32) -> bool {
    Command::new("sh")
        .args(["-c", r#"kill -s "$0" -- "-$1""#, signal, &group.to_string()])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .is_ok_and(|s| s.success())
}

fn group_alive(group: u32) -> bool {
    signal_group("0", group)
}

pub(super) fn ask_group_to_stop(group: u32) {
    signal_group("TERM", group);
    signal_group("CONT", group);
}

pub(super) fn kill_group(group: u32) {
    signal_group("KILL", group);
}

pub(super) struct Guardian {
    child: Child,
    stdin: Option<ChildStdin>,
}

impl Guardian {
    pub(super) fn start() -> io::Result<Self> {
        let mut child = Command::new("sh")
            .args(["-c", GUARDIAN])
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .process_group(0)
            .spawn()?;
        let stdin = child.stdin.take();
        Ok(Self { child, stdin })
    }

    pub(super) fn watch(&mut self, group: u32) {
        if let Some(stdin) = &mut self.stdin {
            let _ = writeln!(stdin, "{group}");
            let _ = stdin.flush();
        }
    }

    pub(super) fn dismiss(mut self) {
        if let Some(mut stdin) = self.stdin.take() {
            let _ = writeln!(stdin, "over");
        }
        let _ = self.child.wait();
    }
}

fn user_shell() -> PathBuf {
    std::env::var_os("SHELL")
        .filter(|s| !s.is_empty())
        .map_or_else(|| PathBuf::from("sh"), PathBuf::from)
}

fn check_program(program: &Path) -> io::Result<()> {
    let found = if program.components().count() > 1 {
        is_executable_file(program)
    } else {
        std::env::var_os("PATH").is_some_and(|path| {
            program
                .to_str()
                .is_some_and(|name| on_path(name, path.as_os_str()))
        })
    };
    if found {
        return Ok(());
    }
    let kind = if program.exists() {
        io::ErrorKind::PermissionDenied
    } else {
        io::ErrorKind::NotFound
    };
    Err(kind.into())
}

struct Output {
    reader: Box<dyn Read + Send>,
    stdout: Stdio,
    stderr: Stdio,
}

const PTY_SIZE: rustix::termios::Winsize = rustix::termios::Winsize {
    ws_row: 50,
    ws_col: 140,
    ws_xpixel: 0,
    ws_ypixel: 0,
};

fn pty_output() -> rustix::io::Result<Output> {
    use rustix::fs::{Mode, OFlags};
    use rustix::pty::{OpenptFlags, grantpt, openpt, ptsname, unlockpt};
    let master = openpt(OpenptFlags::RDWR | OpenptFlags::NOCTTY)?;
    grantpt(&master)?;
    unlockpt(&master)?;
    let name = ptsname(&master, Vec::new())?;
    let slave = rustix::fs::open(
        name.as_c_str(),
        OFlags::RDWR | OFlags::NOCTTY,
        Mode::empty(),
    )?;
    rustix::termios::tcsetwinsize(&slave, PTY_SIZE)?;
    let stderr = slave.try_clone().map_err(|_| rustix::io::Errno::MFILE)?;
    Ok(Output {
        reader: Box::new(File::from(master)),
        stdout: Stdio::from(slave),
        stderr: Stdio::from(stderr),
    })
}

fn open_output() -> io::Result<Output> {
    if let Ok(output) = pty_output() {
        return Ok(output);
    }
    let (reader, writer) = io::pipe()?;
    Ok(Output {
        reader: Box::new(reader),
        stdout: Stdio::from(writer.try_clone()?),
        stderr: Stdio::from(writer),
    })
}

fn start_program(
    target: &RunTarget,
    stdout: Stdio,
    stderr: Stdio,
    launcher: Option<&Path>,
) -> io::Result<Child> {
    if !target.cwd.is_dir() {
        return Err(io::Error::other(format!(
            "no such folder: {}",
            target.cwd.display()
        )));
    }
    let (program, args): (PathBuf, Vec<&str>) = match &target.command {
        RunCommand::Argv(argv) => {
            let Some((program, args)) = argv.split_first() else {
                return Err(io::Error::other("empty command"));
            };
            let program = if program.contains('/') {
                target.cwd.join(program)
            } else {
                PathBuf::from(program)
            };
            (program, args.iter().map(String::as_str).collect())
        }
        RunCommand::Shell(text) => (PathBuf::from("sh"), vec!["-c", text]),
        RunCommand::UserShell(text) => (user_shell(), vec!["-c", text]),
    };
    check_program(&program)?;
    let mut command = match launcher.filter(|l| l.is_file()) {
        Some(launcher) => {
            let mut command = Command::new(launcher);
            command.arg(LAUNCHER_FLAG).arg(&program);
            command
        }
        None => {
            let mut command = Command::new(&program);
            command.process_group(0);
            command
        }
    };
    command
        .args(args)
        .current_dir(&target.cwd)
        .stdin(Stdio::null())
        .stderr(stderr)
        .stdout(stdout)
        .env("TERM", "xterm-256color")
        .env("PYTHONUNBUFFERED", "1")
        .spawn()
}

struct Locator {
    cwd: PathBuf,
    root: PathBuf,
    known: HashMap<String, Option<PathBuf>>,
    by_name: Option<HashMap<String, Vec<PathBuf>>>,
}

impl Locator {
    fn new(cwd: PathBuf, root: PathBuf) -> Self {
        Self {
            cwd,
            root,
            known: HashMap::new(),
            by_name: None,
        }
    }

    fn resolve(&mut self, printed: &str) -> Option<PathBuf> {
        if let Some(known) = self.known.get(printed) {
            return known.clone();
        }
        let path = Path::new(printed);
        let found = [self.cwd.join(path), self.root.join(path)]
            .into_iter()
            .find(|p| p.is_file())
            .map(|p| tidy_path(&p));
        if self.known.len() >= MAX_REMEMBERED_PATHS {
            self.known.clear();
        }
        self.known.insert(printed.to_owned(), found.clone());
        found
    }

    fn resolve_jvm(&mut self, qualified: &str, file: &str) -> Option<PathBuf> {
        let root = &self.root;
        let by_name = self.by_name.get_or_insert_with(|| index_by_name(root));
        jvm_file(qualified, file, by_name.get(file)?).cloned()
    }

    fn place(&mut self, candidate: Candidate) -> Option<RunLocation> {
        let path = match &candidate.named {
            Named::Path(printed) => self.resolve(printed)?,
            Named::JvmFile { qualified, file } => self.resolve_jvm(qualified, file)?,
        };
        Some(RunLocation {
            path,
            line: candidate.line - 1,
            col: candidate.col.map_or(0, |c| c - 1),
            span: candidate.span,
        })
    }

    fn line(&mut self, text: String, spans: Vec<RunSpan>) -> RunLine {
        let location = candidates(&text)
            .into_iter()
            .find_map(|candidate| self.place(candidate));
        RunLine {
            text,
            location,
            spans,
        }
    }
}

fn index_by_name(root: &Path) -> HashMap<String, Vec<PathBuf>> {
    let mut by_name: HashMap<String, Vec<PathBuf>> = HashMap::new();
    let files = walker(root)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .take(MAX_INDEXED_FILES);
    for entry in files {
        let name = entry.file_name().to_string_lossy().into_owned();
        by_name.entry(name).or_default().push(entry.into_path());
    }
    by_name
}

struct Pending {
    lines: std::collections::VecDeque<(String, Vec<RunSpan>)>,
    bytes: usize,
    dropped: usize,
    sent_partial: String,
    dirty: bool,
}

impl Pending {
    fn new() -> Self {
        Self {
            lines: std::collections::VecDeque::new(),
            bytes: 0,
            dropped: 0,
            sent_partial: String::new(),
            dirty: false,
        }
    }

    fn add(&mut self, line: String, spans: Vec<RunSpan>) {
        self.bytes += line.len();
        self.lines.push_back((line, spans));
        self.dirty = true;
        while self.lines.len() > MAX_LINES || self.bytes > MAX_TOTAL_BYTES {
            let Some(old) = self.lines.pop_front() else {
                break;
            };
            self.bytes -= old.0.len();
            self.dropped += 1;
        }
    }

    fn flush(
        &mut self,
        id: u64,
        partial: String,
        locator: &mut Locator,
        emit: &impl Fn(Event),
    ) -> bool {
        if !self.dirty && partial == self.sent_partial {
            return false;
        }
        self.dirty = false;
        self.bytes = 0;
        let lines: Vec<RunLine> = self
            .lines
            .drain(..)
            .map(|(text, spans)| locator.line(text, spans))
            .collect();
        self.sent_partial.clone_from(&partial);
        emit(Event::Run(RunEvent::Output {
            id,
            lines,
            partial,
            dropped: std::mem::take(&mut self.dropped),
        }));
        true
    }
}

fn outcome_of(status: ExitStatus, stopped: bool) -> RunOutcome {
    if stopped {
        return RunOutcome::Stopped;
    }
    match (status.code(), status.signal()) {
        (Some(code), _) => RunOutcome::Exited(code),
        (None, Some(signal)) => RunOutcome::Signalled(signal),
        (None, None) => RunOutcome::Exited(-1),
    }
}

#[derive(Clone, Copy)]
enum Ending {
    None,
    Terminated { at: Instant },
    Killed { at: Instant },
}

struct Supervisor<F> {
    id: u64,
    emit: F,
    stop: Arc<AtomicBool>,
    launcher: Option<PathBuf>,
}

fn wait_for_group(child: &mut Child) {
    let deadline = Instant::now() + SESSION_WAIT;
    while Instant::now() < deadline
        && !group_alive(child.id())
        && matches!(child.try_wait(), Ok(None))
    {
        thread::sleep(Duration::from_millis(1));
    }
}

impl<F: Fn(Event)> Supervisor<F> {
    fn finish(&self, outcome: RunOutcome, started: Instant) {
        (self.emit)(Event::Run(RunEvent::Finished {
            id: self.id,
            outcome,
            duration_ms: u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX),
        }));
    }

    fn run(&self, target: &RunTarget, root: &Path) {
        let started = Instant::now();
        let failed = |reason: String| {
            self.finish(RunOutcome::FailedToStart(run::visible(&reason)), started);
        };
        let Output {
            reader,
            stdout,
            stderr,
        } = match open_output() {
            Ok(output) => output,
            Err(e) => return failed(e.to_string()),
        };
        let mut guardian = match Guardian::start() {
            Ok(guardian) => guardian,
            Err(e) => return failed(format!("no shell to supervise it: {e}")),
        };
        let child = match start_program(target, stdout, stderr, self.launcher.as_deref()) {
            Ok(child) => child,
            Err(e) => {
                guardian.dismiss();
                return failed(describe_start_error(target, &e));
            }
        };
        let mut child = child;
        if self.launcher.is_some() {
            wait_for_group(&mut child);
        }
        guardian.watch(child.id());
        let outcome = self.watch(child, reader, target, root);
        guardian.dismiss();
        self.finish(outcome, started);
    }

    fn watch(
        &self,
        mut child: Child,
        reader: Box<dyn Read + Send>,
        target: &RunTarget,
        root: &Path,
    ) -> RunOutcome {
        let group = child.id();
        let (chunks_tx, chunks) = mpsc::sync_channel(CHUNKS_QUEUED);
        thread::spawn(move || read_chunks(reader, &chunks_tx));

        let mut decoder = OutputDecoder::default();
        let mut pending = Pending::new();
        let mut locator = Locator::new(target.cwd.clone(), root.to_path_buf());
        let mut last_flush = Instant::now();
        let mut pipe_open = true;
        let mut status = None;
        let mut ending = Ending::None;
        let mut asked_to_stop = false;
        let mut group_gone_at = None;
        let mut last_check = Instant::now();
        loop {
            match chunks.recv_timeout(TICK) {
                Ok(chunk) => {
                    decoder.feed_styled(&chunk, |line, spans| pending.add(line, spans));
                }
                Err(RecvTimeoutError::Timeout) => {}
                Err(RecvTimeoutError::Disconnected) => {
                    pipe_open = false;
                    thread::sleep(TICK);
                }
            }
            if last_flush.elapsed() >= FLUSH_EVERY
                && pending.flush(self.id, decoder.partial(), &mut locator, &self.emit)
            {
                last_flush = Instant::now();
            }
            if status.is_none() {
                status = child.try_wait().ok().flatten();
            }
            if self.stop.load(Ordering::Relaxed) && !asked_to_stop {
                asked_to_stop = true;
                if group_gone_at.is_none() && status.is_none() {
                    ask_group_to_stop(group);
                    ending = Ending::Terminated { at: Instant::now() };
                }
            }
            if status.is_some() && matches!(ending, Ending::None) {
                if group_alive(group) {
                    ask_group_to_stop(group);
                } else {
                    group_gone_at = Some(Instant::now());
                }
                ending = Ending::Terminated { at: Instant::now() };
            }
            if let Ending::Terminated { at } = ending
                && group_gone_at.is_none()
                && at.elapsed() >= STOP_GRACE
            {
                signal_group("KILL", group);
                ending = Ending::Killed { at: Instant::now() };
            }
            if !matches!(ending, Ending::None)
                && group_gone_at.is_none()
                && last_check.elapsed() >= GROUP_CHECK_EVERY
            {
                last_check = Instant::now();
                if !group_alive(group) {
                    group_gone_at = Some(Instant::now());
                }
            }
            if status.is_none() && group_gone_at.is_some_and(|at| at.elapsed() >= DRAIN_GRACE) {
                let _ = child.kill();
            }
            let given_up = matches!(ending, Ending::Killed { at } if at.elapsed() >= KILL_GRACE);
            let drained =
                group_gone_at.is_some_and(|at: Instant| !pipe_open || at.elapsed() >= DRAIN_GRACE);
            if (drained && status.is_some()) || given_up {
                break;
            }
        }
        pending.flush(self.id, decoder.partial(), &mut locator, &self.emit);
        status.map_or(RunOutcome::Stopped, |s| outcome_of(s, asked_to_stop))
    }
}

fn read_chunks(mut reader: Box<dyn Read + Send>, chunks: &SyncSender<Vec<u8>>) {
    let mut buffer = vec![0; CHUNK_BYTES];
    while let Ok(n) = reader.read(&mut buffer) {
        if n == 0 || chunks.send(buffer[..n].to_vec()).is_err() {
            return;
        }
    }
}

fn describe_start_error(target: &RunTarget, error: &io::Error) -> String {
    let program = match &target.command {
        RunCommand::Argv(argv) => argv.first().map_or("", String::as_str),
        RunCommand::Shell(_) | RunCommand::UserShell(_) => "sh",
    };
    match error.kind() {
        io::ErrorKind::NotFound => format!("{program}: not found"),
        io::ErrorKind::PermissionDenied => format!("{program}: not executable"),
        _ => format!("{program}: {error}"),
    }
}

struct RunHandle {
    id: u64,
    stop: Arc<AtomicBool>,
    thread: JoinHandle<()>,
}

#[derive(Default)]
pub struct RunHost {
    runs: Vec<RunHandle>,
    launcher: Option<PathBuf>,
}

impl RunHost {
    pub fn use_launcher(&mut self, program: Option<PathBuf>) {
        self.launcher = program;
    }

    #[cfg(test)]
    pub fn start<F>(&mut self, id: u64, target: RunTarget, root: PathBuf, emit: F)
    where
        F: Fn(Event) + Send + 'static,
    {
        let replaces = self.runs.last().map(|run| run.id);
        self.start_replacing(id, target, root, replaces, emit);
    }

    pub fn start_replacing<F>(
        &mut self,
        id: u64,
        target: RunTarget,
        root: PathBuf,
        replaces: Option<u64>,
        emit: F,
    ) where
        F: Fn(Event) + Send + 'static,
    {
        let previous = replaces
            .and_then(|old| self.runs.iter().position(|run| run.id == old))
            .map(|index| self.runs.remove(index));
        self.runs.retain(|run| !run.thread.is_finished());
        let stop = Arc::new(AtomicBool::new(false));
        let supervisor = Supervisor {
            id,
            emit,
            stop: stop.clone(),
            launcher: self.launcher.clone(),
        };
        let thread = thread::spawn(move || {
            if let Some(previous) = previous {
                previous.stop.store(true, Ordering::Relaxed);
                let _ = previous.thread.join();
            }
            if supervisor.stop.load(Ordering::Relaxed) {
                supervisor.finish(RunOutcome::Stopped, Instant::now());
                return;
            }
            supervisor.run(&target, &root);
        });
        self.runs.push(RunHandle { id, stop, thread });
    }

    pub fn stop(&self, id: u64) {
        if let Some(run) = self.runs.iter().find(|run| run.id == id) {
            run.stop.store(true, Ordering::Relaxed);
        }
    }
}

impl Drop for RunHost {
    fn drop(&mut self) {
        for run in &self.runs {
            run.stop.store(true, Ordering::Relaxed);
        }
    }
}

fn is_executable_file(path: &Path) -> bool {
    path.metadata()
        .is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
}

fn read_small(path: &Path) -> Option<String> {
    let meta = path.metadata().ok()?;
    if !meta.is_file() || meta.len() > MAX_PROJECT_FILE_BYTES {
        return None;
    }
    let bytes = std::fs::read(path).ok()?;
    Some(String::from_utf8_lossy(&bytes).into_owned())
}

fn starts_with_shebang(path: &Path) -> bool {
    let is_file = path.metadata().is_ok_and(|m| m.is_file());
    let mut start = [0; 2];
    is_file
        && File::open(path)
            .and_then(|mut f| f.read_exact(&mut start))
            .is_ok()
        && start == *b"#!"
}

fn on_path(program: &str, path: &std::ffi::OsStr) -> bool {
    std::env::split_paths(path).any(|dir| is_executable_file(&dir.join(program)))
}

pub fn facts(root: &Path, file: Option<&Path>) -> RunFacts {
    let mut facts = RunFacts::default();
    for name in run::probed_files() {
        if root.join(name).is_file() {
            facts.present.insert(name.to_owned());
        }
    }
    for name in run::read_files() {
        if let Some(text) = read_small(&root.join(name)) {
            facts.texts.insert(name.to_owned(), text);
        }
    }
    if let Some(path) = std::env::var_os("PATH") {
        for program in run::probed_programs() {
            if on_path(program, &path) {
                facts.programs.insert(program.to_owned());
            }
        }
    }
    facts.file = file.map(|path| ActiveFile {
        path: path.to_path_buf(),
        executable: is_executable_file(path),
        shebang: starts_with_shebang(path),
    });
    facts
}

#[cfg(test)]
mod tests {
    use std::fs;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc::{Receiver, channel};

    use super::*;
    use crate::core::run::{RunSource, detect};

    const WAIT: Duration = Duration::from_secs(60);

    struct Run {
        events: Receiver<Event>,
        host: RunHost,
        lines: Vec<RunLine>,
        partial: String,
        dropped: usize,
        finished: Option<(RunOutcome, u64)>,
    }

    fn target(dir: &Path, script: &str) -> RunTarget {
        RunTarget {
            name: "t".to_owned(),
            command: RunCommand::Shell(script.to_owned()),
            cwd: dir.to_path_buf(),
            source: RunSource::UserConfig,
        }
    }

    impl Run {
        fn new() -> Self {
            let (_tx, events) = channel();
            Self {
                events,
                host: RunHost::default(),
                lines: Vec::new(),
                partial: String::new(),
                dropped: 0,
                finished: None,
            }
        }

        fn start(dir: &Path, script: &str) -> Self {
            let mut run = Self::new();
            run.launch(1, target(dir, script), dir);
            run
        }

        fn launch(&mut self, id: u64, target: RunTarget, root: &Path) {
            let (tx, events) = channel();
            self.events = events;
            self.finished = None;
            self.host.start(id, target, root.to_path_buf(), move |e| {
                let _ = tx.send(e);
            });
        }

        fn pump(&mut self) -> bool {
            let Ok(event) = self.events.recv_timeout(WAIT) else {
                panic!("no event in {WAIT:?}; lines so far: {:?}", self.texts());
            };
            match event {
                Event::Run(RunEvent::Output {
                    lines,
                    partial,
                    dropped,
                    ..
                }) => {
                    self.lines.extend(lines);
                    self.partial = partial;
                    self.dropped += dropped;
                    true
                }
                Event::Run(RunEvent::Finished {
                    outcome,
                    duration_ms,
                    ..
                }) => {
                    self.finished = Some((outcome, duration_ms));
                    false
                }
                other => panic!("unexpected {other:?}"),
            }
        }

        fn wait_for_line(&mut self, wanted: &str) {
            while !self.texts().iter().any(|l| l == wanted) {
                assert!(self.pump(), "ended without {wanted:?}: {:?}", self.texts());
            }
        }

        fn finish(&mut self) -> RunOutcome {
            while self.pump() {}
            self.finished.clone().map(|(o, _)| o).unwrap()
        }

        fn texts(&self) -> Vec<String> {
            self.lines.iter().map(|l| l.text.clone()).collect()
        }
    }

    fn scratch() -> (tempfile::TempDir, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().canonicalize().unwrap();
        (dir, path)
    }

    fn read_pid(file: &Path) -> u32 {
        let deadline = Instant::now() + WAIT;
        loop {
            if let Ok(text) = fs::read_to_string(file)
                && let Ok(pid) = text.trim().parse()
            {
                return pid;
            }
            assert!(
                Instant::now() < deadline,
                "{} never appeared",
                file.display()
            );
            thread::sleep(Duration::from_millis(5));
        }
    }

    fn pid_alive(pid: u32) -> bool {
        Command::new("sh")
            .args(["-c", r#"kill -s 0 "$0" 2>/dev/null"#, &pid.to_string()])
            .status()
            .is_ok_and(|s| s.success())
    }

    fn wait_gone(pid: u32) {
        let deadline = Instant::now() + WAIT;
        while pid_alive(pid) {
            assert!(Instant::now() < deadline, "pid {pid} is still alive");
            thread::sleep(Duration::from_millis(10));
        }
    }

    #[test]
    fn both_streams_arrive_in_order_with_the_exit_code() {
        let (_guard, dir) = scratch();
        let mut run = Run::start(
            &dir,
            "echo out1; echo err1 >&2; echo out2; echo err2 >&2; exit 3",
        );
        assert_eq!(run.finish(), RunOutcome::Exited(3));
        assert_eq!(run.texts(), ["out1", "err1", "out2", "err2"]);
    }

    #[test]
    fn output_arrives_while_the_program_is_still_running() {
        let (_guard, dir) = scratch();
        let gate = dir.join("gate");
        let script = format!(
            "echo before; while [ ! -e '{}' ]; do sleep 0.01; done; echo after",
            gate.display()
        );
        let mut run = Run::start(&dir, &script);
        run.wait_for_line("before");
        assert!(run.finished.is_none());
        fs::write(&gate, "").unwrap();
        run.finish();
        assert_eq!(run.texts(), ["before", "after"]);
    }

    #[test]
    fn a_prompt_without_a_newline_is_still_seen() {
        let (_guard, dir) = scratch();
        let gate = dir.join("gate");
        let script = format!(
            "printf 'name: '; while [ ! -e '{}' ]; do sleep 0.01; done",
            gate.display()
        );
        let mut run = Run::start(&dir, &script);
        while run.partial != "name: " {
            assert!(run.pump());
        }
        fs::write(&gate, "").unwrap();
        run.finish();
    }

    #[test]
    fn stdin_is_closed_so_nothing_waits_for_input() {
        let (_guard, dir) = scratch();
        let mut run = Run::start(&dir, "cat; read x; echo end");
        assert_eq!(run.finish(), RunOutcome::Exited(0));
        assert_eq!(run.texts(), ["end"]);
    }

    #[test]
    fn a_direct_command_is_not_interpreted_by_a_shell() {
        let (_guard, dir) = scratch();
        let mut run = Run::new();
        let target = RunTarget {
            command: RunCommand::Argv(vec![
                "printf".to_owned(),
                "%s\\n".to_owned(),
                "$HOME; a b".to_owned(),
            ]),
            ..target(&dir, "")
        };
        run.launch(1, target, &dir);
        run.finish();
        assert_eq!(run.texts(), ["$HOME; a b"]);
    }

    #[test]
    fn a_relative_program_means_the_commands_own_folder() {
        let (_guard, dir) = scratch();
        let script = dir.join("gradlew");
        fs::write(&script, "#!/bin/sh\necho wrapped \"$1\"\n").unwrap();
        fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
        let mut run = Run::new();
        let target = RunTarget {
            command: RunCommand::Argv(vec!["./gradlew".to_owned(), "test".to_owned()]),
            ..target(&dir, "")
        };
        run.launch(1, target, &dir);
        assert_eq!(run.finish(), RunOutcome::Exited(0));
        assert_eq!(run.texts(), ["wrapped test"]);
    }

    #[test]
    fn a_missing_program_or_folder_says_what_is_wrong() {
        let (_guard, dir) = scratch();
        let mut run = Run::new();
        let missing = RunTarget {
            command: RunCommand::Argv(vec!["no-such-program-here".to_owned()]),
            ..target(&dir, "")
        };
        run.launch(1, missing, &dir);
        assert_eq!(
            run.finish(),
            RunOutcome::FailedToStart("no-such-program-here: not found".to_owned())
        );
        let elsewhere = RunTarget {
            cwd: dir.join("nope"),
            ..target(&dir, "true")
        };
        run.launch(2, elsewhere, &dir);
        let outcome = run.finish();
        assert!(matches!(&outcome, RunOutcome::FailedToStart(m) if m.contains("nope")));
    }

    #[test]
    fn stop_reaches_children_of_children() {
        let (_guard, dir) = scratch();
        let pids = dir.join("pid");
        let script = format!(
            "(sleep 300 & echo $! > '{p}'; wait) & echo started; wait",
            p = pids.display()
        );
        let mut run = Run::start(&dir, &script);
        run.wait_for_line("started");
        let grandchild = read_pid(&pids);
        assert!(pid_alive(grandchild));
        run.host.stop(1);
        assert_eq!(run.finish(), RunOutcome::Stopped);
        wait_gone(grandchild);
    }

    #[test]
    fn a_program_ignoring_term_is_killed() {
        let (_guard, dir) = scratch();
        let mut run = Run::start(
            &dir,
            "trap '' TERM; echo started; while :; do sleep 0.05; done",
        );
        run.wait_for_line("started");
        let asked = Instant::now();
        run.host.stop(1);
        assert_eq!(run.finish(), RunOutcome::Stopped);
        assert!(asked.elapsed() >= STOP_GRACE, "gave it its chance first");
    }

    #[test]
    fn what_a_finished_program_left_behind_is_stopped_too() {
        let (_guard, dir) = scratch();
        let pid = dir.join("pid");
        let script = format!(
            "sleep 300 >/dev/null 2>&1 & echo $! > '{}'; echo done",
            pid.display()
        );
        let mut run = Run::start(&dir, &script);
        assert_eq!(run.finish(), RunOutcome::Exited(0));
        wait_gone(read_pid(&pid));
        assert_eq!(run.texts(), ["done"]);
    }

    #[test]
    fn a_daemon_holding_the_pipe_open_cannot_hold_the_run_open() {
        let (_guard, dir) = scratch();
        let (reader, writer) = io::pipe().unwrap();
        let mut outsider = Command::new("sleep")
            .arg("300")
            .stdout(writer.try_clone().unwrap())
            .process_group(0)
            .spawn()
            .unwrap();
        let mut run = Run::new();
        let (tx, events) = channel();
        run.events = events;
        let supervisor = Supervisor {
            id: 1,
            emit: move |e| {
                let _ = tx.send(e);
            },
            stop: Arc::new(AtomicBool::new(false)),
            launcher: None,
        };
        let child = start_program(
            &target(&dir, "echo mine >&2"),
            Stdio::from(writer.try_clone().unwrap()),
            Stdio::from(writer),
            None,
        )
        .unwrap();
        let t = target(&dir, "");
        let outcome = supervisor.watch(child, Box::new(reader), &t, &dir);
        assert_eq!(outcome, RunOutcome::Exited(0));
        outsider.kill().unwrap();
        outsider.wait().unwrap();
        drop(supervisor);
        while run.pump_until_closed() {}
        assert_eq!(run.texts(), ["mine"]);
    }

    impl Run {
        fn pump_until_closed(&mut self) -> bool {
            match self.events.recv_timeout(WAIT) {
                Ok(Event::Run(RunEvent::Output { lines, .. })) => {
                    self.lines.extend(lines);
                    true
                }
                _ => false,
            }
        }
    }

    #[test]
    fn huge_output_arrives_bounded_and_can_be_stopped() {
        let (_guard, dir) = scratch();
        let mut run = Run::start(&dir, "yes spam");
        let mut seen = 0;
        while seen < 3 {
            assert!(run.pump());
            seen += 1;
        }
        run.host.stop(1);
        assert_eq!(run.finish(), RunOutcome::Stopped);
        assert!(!run.lines.is_empty());
    }

    #[test]
    fn binary_garbage_and_escapes_do_not_get_through() {
        let (_guard, dir) = scratch();
        let script = r"printf 'a\033[2Jb\033]0;t\007c\001\377d\n'; head -c 4000 /dev/urandom";
        let mut run = Run::start(&dir, script);
        assert_eq!(run.finish(), RunOutcome::Exited(0));
        assert_eq!(run.lines[0].text, "abc\u{fffd}d");
        for line in &run.lines {
            assert!(
                !line
                    .text
                    .chars()
                    .any(|c| c != '\t' && run::is_unprintable(c)),
                "{:?}",
                line.text
            );
        }
    }

    #[test]
    fn asking_for_a_second_run_ends_the_first_before_it_starts() {
        let (_guard, dir) = scratch();
        let marker = dir.join("first-pid");
        let first = format!("echo $$ > '{}'; echo first; sleep 300", marker.display());
        let mut run = Run::start(&dir, &first);
        run.wait_for_line("first");
        let pid = read_pid(&marker);
        let second = format!("kill -s 0 {pid} 2>/dev/null && echo overlap || echo clean");
        run.lines.clear();
        run.launch(2, target(&dir, &second), &dir);
        assert_eq!(run.finish(), RunOutcome::Exited(0));
        assert_eq!(run.texts(), ["clean"]);
    }

    #[test]
    fn a_run_stopped_before_its_turn_never_starts() {
        let (_guard, dir) = scratch();
        let first = dir.join("first");
        let mut run = Run::start(
            &dir,
            &format!("echo $$ > '{}'; trap '' TERM; sleep 300", first.display()),
        );
        read_pid(&first);
        let marker = dir.join("second");
        run.launch(
            2,
            target(&dir, &format!("touch '{}'", marker.display())),
            &dir,
        );
        run.host.stop(2);
        assert_eq!(run.finish(), RunOutcome::Stopped);
        assert!(!marker.exists());
    }

    #[test]
    fn a_hostile_file_name_cannot_reach_the_screen_through_an_error() {
        let (_guard, dir) = scratch();
        let mut run = Run::new();
        let hostile = RunTarget {
            command: RunCommand::Argv(vec!["no-such\u{1b}[2J".to_owned()]),
            ..target(&dir, "")
        };
        run.launch(1, hostile, &dir);
        let outcome = run.finish();
        let RunOutcome::FailedToStart(text) = &outcome else {
            panic!("{outcome:?}");
        };
        assert!(!text.contains('\u{1b}'), "{text:?}");
        assert!(text.contains("not found"), "{text:?}");
    }

    #[test]
    fn dropping_the_host_stops_the_run() {
        let (_guard, dir) = scratch();
        let pid = dir.join("pid");
        let script = format!("echo $$ > '{}'; sleep 300", pid.display());
        let mut run = Run::start(&dir, &script);
        let pid = read_pid(&pid);
        run.host = RunHost::default();
        wait_gone(pid);
        run.finish();
    }

    #[test]
    fn the_guardian_stops_the_group_when_the_editor_vanishes() {
        let (_guard, dir) = scratch();
        let pid = dir.join("pid");
        let script = format!(
            "trap '' TERM; echo $$ > '{}'; while :; do sleep 0.05; done",
            pid.display()
        );
        let (_reader, writer) = io::pipe().unwrap();
        let mut guardian = Guardian::start().unwrap();
        let mut child = start_program(
            &target(&dir, &script),
            Stdio::from(writer),
            Stdio::null(),
            None,
        )
        .unwrap();
        guardian.watch(child.id());
        let pid = read_pid(&pid);
        guardian.stdin = None;
        child.wait().unwrap();
        guardian.child.wait().unwrap();
        assert!(!pid_alive(pid));
    }

    #[test]
    fn a_dismissed_guardian_leaves_the_group_alone() {
        let (_guard, dir) = scratch();
        let (_reader, writer) = io::pipe().unwrap();
        let mut guardian = Guardian::start().unwrap();
        let mut child = start_program(
            &target(&dir, "sleep 300"),
            Stdio::from(writer),
            Stdio::null(),
            None,
        )
        .unwrap();
        guardian.watch(child.id());
        guardian.dismiss();
        assert!(child.try_wait().unwrap().is_none());
        child.kill().unwrap();
        child.wait().unwrap();
    }

    fn locate(cwd: &Path, root: &Path, text: &str) -> Option<RunLocation> {
        Locator::new(cwd.to_path_buf(), root.to_path_buf())
            .line(text.to_owned(), Vec::new())
            .location
    }

    #[test]
    fn a_location_needs_the_file_to_exist() {
        let (_guard, dir) = scratch();
        fs::create_dir(dir.join("src")).unwrap();
        fs::write(dir.join("src/a.rs"), "").unwrap();
        fs::write(dir.join("12"), "").unwrap();

        let found = locate(&dir, &dir, "  --> src/a.rs:2:5").unwrap();
        assert_eq!(found.path, dir.join("src/a.rs"));
        assert_eq!((found.line, found.col), (1, 4));

        let found = locate(&dir, &dir, "./src/../src/a.rs:7: oops").unwrap();
        assert_eq!(found.path, dir.join("src/a.rs"), "tidied");
        assert_eq!((found.line, found.col), (6, 0));

        let absolute = format!("{}:1:1", dir.join("src/a.rs").display());
        assert!(locate(&dir, &dir, &absolute).is_some());

        for plain in [
            "src/missing.rs:2:5",
            "the time is 11:30:45",
            "http://localhost:3000/src/a.rs",
            "https://example.com:443/",
            "1.2.3.4:8080",
            "src:1:1",
        ] {
            assert_eq!(locate(&dir, &dir, plain), None, "{plain}");
        }
        assert!(locate(&dir, &dir, "12:30:45").is_some());
    }

    #[test]
    fn paths_are_tried_from_the_commands_folder_then_the_project() {
        let (_guard, dir) = scratch();
        fs::create_dir(dir.join("sub")).unwrap();
        fs::write(dir.join("sub/near.c"), "").unwrap();
        fs::write(dir.join("far.c"), "").unwrap();
        let cwd = dir.join("sub");
        assert_eq!(
            locate(&cwd, &dir, "near.c:1").unwrap().path,
            dir.join("sub/near.c")
        );
        assert_eq!(
            locate(&cwd, &dir, "far.c:1").unwrap().path,
            dir.join("far.c")
        );
    }

    #[test]
    fn a_stack_frame_is_found_by_searching_the_project() {
        let (_guard, dir) = scratch();
        let package = dir.join("src/main/java/com/acme");
        fs::create_dir_all(&package).unwrap();
        fs::write(package.join("Main.java"), "").unwrap();
        let found = locate(&dir, &dir, "\tat com.acme.Main.run(Main.java:12)").unwrap();
        assert_eq!(found.path, package.join("Main.java"));
        assert_eq!(found.line, 11);
        assert_eq!(
            locate(&dir, &dir, "\tat java.lang.Main.run(Main.java:1)"),
            None
        );
    }

    #[test]
    fn output_lines_carry_their_locations() {
        let (_guard, dir) = scratch();
        fs::write(dir.join("a.c"), "").unwrap();
        let mut run = Run::start(&dir, "echo plain; echo 'a.c:3:5: error'");
        run.finish();
        assert_eq!(run.lines[0].location, None);
        assert_eq!(run.lines[1].location.as_ref().unwrap().line, 2);
    }

    #[test]
    fn facts_come_from_the_disk_and_feed_detection() {
        let (_guard, dir) = scratch();
        fs::write(dir.join("go.mod"), "").unwrap();
        fs::write(dir.join("package.json"), r#"{"scripts":{"t":"x"}}"#).unwrap();
        fs::write(dir.join("yarn.lock"), "").unwrap();
        let tool = dir.join("tool");
        fs::write(&tool, "#!/bin/sh\necho hi\n").unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        let found = facts(&dir, Some(&tool));
        let commands: Vec<String> = detect(&dir, &found)
            .iter()
            .map(RunTarget::command_line)
            .collect();
        assert!(commands.contains(&"yarn run t".to_owned()), "{commands:?}");
        assert!(commands.contains(&"go run .".to_owned()), "{commands:?}");
        assert!(commands.contains(&"./tool".to_owned()), "{commands:?}");
    }

    #[test]
    fn a_planted_fifo_or_giant_does_not_stall_detection() {
        let (_guard, dir) = scratch();
        let status = Command::new("mkfifo")
            .arg(dir.join("package.json"))
            .status()
            .unwrap();
        assert!(status.success());
        let file = fs::File::create(dir.join("composer.json")).unwrap();
        file.set_len(MAX_PROJECT_FILE_BYTES + 1).unwrap();
        let found = facts(&dir, None);
        assert!(found.texts.is_empty());
        assert!(found.present.contains("composer.json"));
        assert!(!found.present.contains("package.json"));
    }

    #[test]
    fn programs_are_found_only_when_executable() {
        let (_guard, dir) = scratch();
        let plain = dir.join("plain");
        fs::write(&plain, "").unwrap();
        let tool = dir.join("tool");
        fs::write(&tool, "").unwrap();
        fs::set_permissions(&tool, fs::Permissions::from_mode(0o755)).unwrap();
        let path = dir.as_os_str();
        assert!(on_path("tool", path));
        assert!(!on_path("plain", path));
        assert!(!on_path("missing", path));
        assert_eq!(
            check_program(&plain).unwrap_err().kind(),
            io::ErrorKind::PermissionDenied
        );
        assert_eq!(
            check_program(&dir.join("missing")).unwrap_err().kind(),
            io::ErrorKind::NotFound
        );
        check_program(&tool).unwrap();
        check_program(Path::new("sh")).unwrap();
        assert!(!starts_with_shebang(&plain));
    }

    fn run_shell(
        host: &mut RunHost,
        id: u64,
        script: &str,
        dir: &Path,
        replaces: Option<u64>,
        tx: mpsc::Sender<Event>,
    ) {
        let target = RunTarget {
            name: format!("run {id}"),
            command: RunCommand::Shell(script.to_owned()),
            cwd: dir.to_path_buf(),
            source: crate::core::run::RunSource::UserConfig,
        };
        host.start_replacing(id, target, dir.to_path_buf(), replaces, move |e| {
            let _ = tx.send(e);
        });
    }

    fn output_until_finished(rx: &mpsc::Receiver<Event>, id: u64) -> Vec<RunLine> {
        let mut lines = Vec::new();
        loop {
            match rx.recv_timeout(Duration::from_secs(20)).unwrap() {
                Event::Run(RunEvent::Output {
                    id: from,
                    lines: more,
                    ..
                }) if from == id => lines.extend(more),
                Event::Run(RunEvent::Finished { id: from, .. }) if from == id => return lines,
                _ => {}
            }
        }
    }

    #[test]
    fn a_program_sees_a_terminal_and_its_colors_arrive_yet_it_has_no_way_to_our_keys() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let mut host = RunHost::default();
        let script = "test -t 1 && echo stdout-tty; test -t 2 && echo stderr-tty; \
                      test -t 0 || echo stdin-closed; stty size <&1; \
                      (exec 3</dev/tty) 2>/dev/null && echo has-dev-tty || echo no-dev-tty; \
                      printf '\\033[31mred\\033[0m\\n'";
        run_shell(&mut host, 1, script, dir.path(), None, tx);
        let lines = output_until_finished(&rx, 1);
        let texts: Vec<&str> = lines.iter().map(|l| l.text.as_str()).collect();
        assert_eq!(
            texts,
            [
                "stdout-tty",
                "stderr-tty",
                "stdin-closed",
                "50 140",
                "no-dev-tty",
                "red"
            ]
        );
        assert_eq!(lines[5].spans.len(), 1);
    }

    #[test]
    fn the_last_output_of_a_short_program_is_not_lost() {
        let dir = tempfile::tempdir().unwrap();
        for id in 0..20 {
            let (tx, rx) = mpsc::channel();
            let mut host = RunHost::default();
            run_shell(&mut host, id, "seq 1 2000", dir.path(), None, tx);
            let lines = output_until_finished(&rx, id);
            assert_eq!(lines.len(), 2000, "run {id}");
        }
    }

    #[test]
    fn runs_go_on_side_by_side_and_only_the_replaced_one_stops() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let mut host = RunHost::default();
        run_shell(
            &mut host,
            1,
            "echo one; sleep 30",
            dir.path(),
            None,
            tx.clone(),
        );
        run_shell(
            &mut host,
            2,
            "echo two; sleep 30",
            dir.path(),
            None,
            tx.clone(),
        );
        let mut seen = std::collections::BTreeSet::new();
        while seen.len() < 2 {
            if let Event::Run(RunEvent::Output { id, lines, .. }) =
                rx.recv_timeout(Duration::from_secs(20)).unwrap()
                && !lines.is_empty()
            {
                seen.insert(id);
            }
        }
        run_shell(&mut host, 3, "echo three", dir.path(), Some(1), tx);
        let mut finished = Vec::new();
        while !finished.contains(&3) {
            if let Event::Run(RunEvent::Finished { id, .. }) =
                rx.recv_timeout(Duration::from_secs(20)).unwrap()
            {
                finished.push(id);
            }
        }
        assert_eq!(finished, [1, 3], "the second is still running");
        host.stop(2);
        loop {
            if let Event::Run(RunEvent::Finished { id: 2, .. }) =
                rx.recv_timeout(Duration::from_secs(20)).unwrap()
            {
                break;
            }
        }
    }
}
