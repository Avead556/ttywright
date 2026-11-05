use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command as Process, Stdio};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process_group};
use serde::{Deserialize, Serialize};
use ttywright_plugin_host::{Reply, Request, Start, read_frame, write_frame};

use super::fs::write_atomic;
use super::run::{Guardian, LAUNCHER_FLAG};
use crate::core::plugin::clean_text;
use crate::core::{Command, Event, PluginCommand, PluginContext, PluginInfo, PluginReport};

const MANIFEST: &str = "plugin.toml";
const MODULE: &str = "plugin.wasm";
const MAX_MODULE_BYTES: u64 = 16 << 20;
const MAX_FILE_BYTES: u64 = 8 << 20;

const MAX_COMMANDS: usize = 4096;

pub const PLUGIN_HOST_FLAG: &str = "--plugin-host";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Helper {
    pub program: PathBuf,
    pub args: Vec<String>,
}

impl Helper {
    fn of_this_program() -> Option<Self> {
        if cfg!(feature = "static-modules") {
            let program = std::env::current_exe().ok()?;
            let named = program.file_stem().is_some_and(|n| n == crate::PROGRAM);
            return named.then(|| Self {
                program,
                args: vec![PLUGIN_HOST_FLAG.to_owned()],
            });
        }
        Some(Self {
            program: crate::modules::plugin_host()?,
            args: Vec::new(),
        })
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Limits {
    pub fuel: u64,
    pub memory_bytes: usize,
    pub process_time: Duration,
    pub output_bytes: usize,
    pub process_patience: Duration,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            fuel: 2_000_000_000,
            memory_bytes: 64 << 20,
            process_time: Duration::from_secs(120),
            output_bytes: 4 << 20,
            process_patience: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone)]
pub struct LoadedPlugin {
    pub info: PluginInfo,
    dir: PathBuf,
    helper: Option<Helper>,
}

impl LoadedPlugin {
    #[must_use]
    pub fn with_helper(mut self, helper: Helper) -> Self {
        self.helper = Some(helper);
        self
    }
}

#[derive(Deserialize)]
struct Manifest {
    name: String,
    #[serde(default)]
    commands: Vec<ManifestCommand>,
}

#[derive(Deserialize)]
struct ManifestCommand {
    id: String,
    title: String,
}

fn load_one(dir: &Path) -> Result<LoadedPlugin, String> {
    let text =
        std::fs::read_to_string(dir.join(MANIFEST)).map_err(|e| format!("{MANIFEST}: {e}"))?;
    let manifest: Manifest = toml::from_str(&text).map_err(|e| format!("{MANIFEST}: {e}"))?;
    let valid_name = !manifest.name.is_empty()
        && manifest
            .name
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_');
    if !valid_name {
        return Err("the name may only have letters, digits, `-` and `_`".to_owned());
    }
    let module = std::fs::metadata(dir.join(MODULE)).map_err(|e| format!("{MODULE}: {e}"))?;
    if module.len() > MAX_MODULE_BYTES {
        return Err(format!("{MODULE} is larger than {MAX_MODULE_BYTES} bytes"));
    }
    let commands = manifest
        .commands
        .into_iter()
        .map(|c| PluginCommand {
            id: c.id,
            title: clean_text(&c.title),
        })
        .collect();
    Ok(LoadedPlugin {
        info: PluginInfo {
            name: manifest.name,
            commands,
        },
        dir: dir.to_path_buf(),
        helper: Helper::of_this_program(),
    })
}

pub fn load(dir: &Path) -> (Vec<LoadedPlugin>, Vec<String>) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return (Vec::new(), Vec::new());
    };
    let mut folders: Vec<PathBuf> = entries
        .flatten()
        .map(|e| e.path())
        .filter(|p| p.is_dir())
        .collect();
    folders.sort();
    let (mut plugins, mut problems) = (Vec::<LoadedPlugin>::new(), Vec::new());
    for folder in folders {
        let label = folder
            .file_name()
            .map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        match load_one(&folder) {
            Ok(plugin) if plugins.iter().any(|p| p.info.name == plugin.info.name) => {
                problems.push(clean_text(&format!("{label}: the name is already taken")));
            }
            Ok(plugin) => plugins.push(plugin),
            Err(message) => problems.push(clean_text(&format!("Plugin {label}: {message}"))),
        }
    }
    (plugins, problems)
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Answer {
    #[serde(default)]
    commands: Vec<Command>,
    status: Option<String>,
}

fn host_read(root: &Path, name: &str) -> Option<Vec<u8>> {
    let path = root.join(name);
    std::fs::metadata(&path)
        .ok()
        .filter(|m| m.len() <= MAX_FILE_BYTES)
        .and_then(|_| std::fs::read(&path).ok())
}

fn host_write(root: &Path, name: &str, data: &[u8]) -> bool {
    let path = root.join(name);
    if let Some(parent) = path.parent()
        && std::fs::create_dir_all(parent).is_err()
    {
        return false;
    }
    write_atomic(&path, data).is_ok()
}

#[derive(Deserialize)]
pub(super) struct RunRequest {
    pub(super) program: String,
    #[serde(default)]
    pub(super) args: Vec<String>,
    pub(super) stdin: Option<String>,
    pub(super) cwd: Option<PathBuf>,
    #[serde(skip)]
    pub(super) env: Vec<(String, String)>,
}

#[derive(Debug, Default, Serialize)]
pub(super) struct RunResult {
    pub(super) code: Option<i32>,
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) timed_out: bool,
    pub(super) truncated: bool,
    pub(super) error: Option<String>,
}

fn failed_run(error: String) -> RunResult {
    RunResult {
        error: Some(error),
        ..RunResult::default()
    }
}

type Collected = Arc<Mutex<Vec<u8>>>;

fn collect(
    mut from: impl Read + Send + 'static,
    limit: usize,
    over: Arc<AtomicBool>,
) -> (Collected, JoinHandle<()>) {
    let kept = Collected::default();
    let into = kept.clone();
    let reader = thread::spawn(move || {
        let mut chunk = [0_u8; 64 * 1024];
        while let Ok(n @ 1..) = from.read(&mut chunk) {
            let mut kept = into.lock().unwrap_or_else(PoisonError::into_inner);
            let room = limit.saturating_sub(kept.len());
            kept.extend_from_slice(&chunk[..n.min(room)]);
            if n > room {
                over.store(true, Ordering::Relaxed);
                return;
            }
        }
    });
    (kept, reader)
}

fn collected(kept: &Collected) -> String {
    String::from_utf8_lossy(&kept.lock().unwrap_or_else(PoisonError::into_inner)).into_owned()
}

pub(super) fn run_program(
    request: RunRequest,
    root: &Path,
    deadline: Instant,
    limit: usize,
    launcher: Option<&Path>,
) -> RunResult {
    if Instant::now() >= deadline {
        return RunResult {
            timed_out: true,
            ..RunResult::default()
        };
    }
    let mut process = match launcher {
        Some(launcher) => {
            let mut process = Process::new(launcher);
            process.arg(LAUNCHER_FLAG).arg(&request.program);
            process
        }
        None => {
            use std::os::unix::process::CommandExt as _;
            let mut process = Process::new(&request.program);
            process.process_group(0);
            process
        }
    };
    let cwd = request
        .cwd
        .map_or_else(|| root.to_path_buf(), |c| root.join(c));
    let spawned = process
        .args(&request.args)
        .envs(request.env.iter().map(|(k, v)| (k, v)))
        .current_dir(cwd)
        .stdin(if request.stdin.is_some() {
            Stdio::piped()
        } else {
            Stdio::null()
        })
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => return failed_run(format!("{}: {e}", request.program)),
    };
    let group = Pid::from_child(&child);
    let guardian = Guardian::start().ok().map(|mut guardian| {
        guardian.watch(child.id());
        guardian
    });
    if let (Some(mut pipe), Some(text)) = (child.stdin.take(), request.stdin) {
        thread::spawn(move || {
            let _ = pipe.write_all(text.as_bytes());
        });
    }
    let over = Arc::new(AtomicBool::new(false));
    let mut readers = Vec::new();
    let mut streams = Vec::new();
    if let Some(pipe) = child.stdout.take() {
        let (kept, reader) = collect(pipe, limit, over.clone());
        streams.push(kept);
        readers.push(reader);
    }
    if let Some(pipe) = child.stderr.take() {
        let (kept, reader) = collect(pipe, limit, over.clone());
        streams.push(kept);
        readers.push(reader);
    }
    let mut result = RunResult::default();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Err(e) => {
                result.error = Some(e.to_string());
                break None;
            }
            Ok(None) => {}
        }
        if over.load(Ordering::Relaxed) {
            result.truncated = true;
            break None;
        }
        if Instant::now() >= deadline {
            result.timed_out = true;
            break None;
        }
        thread::sleep(Duration::from_millis(5));
    };
    let _ = kill_process_group(group, Signal::KILL);
    if status.is_none() {
        let _ = child.kill();
        let _ = child.wait();
    }
    if let Some(guardian) = guardian {
        guardian.dismiss();
    }
    result.code = status.and_then(|s| s.code());
    let patience = Instant::now() + Duration::from_millis(500);
    while Instant::now() < patience && !readers.iter().all(JoinHandle::is_finished) {
        thread::sleep(Duration::from_millis(5));
    }
    result.truncated |= over.load(Ordering::Relaxed);
    if let [stdout, stderr] = streams.as_slice() {
        result.stdout = collected(stdout);
        result.stderr = collected(stderr);
    }
    result
}

fn host_run(request: &[u8], calls: &Calls<'_>) -> Vec<u8> {
    let deadline = calls
        .process_deadline
        .get()
        .unwrap_or_else(|| Instant::now() + calls.process_time);
    calls.process_deadline.set(Some(deadline));
    let result = match serde_json::from_slice::<RunRequest>(request) {
        Ok(request) => run_program(
            request,
            calls.root,
            deadline,
            calls.output_bytes,
            calls.launcher,
        ),
        Err(e) => failed_run(format!("bad request: {e}")),
    };
    serde_json::to_vec(&result).unwrap_or_default()
}

struct Calls<'a> {
    root: &'a Path,
    process_time: Duration,
    process_deadline: std::cell::Cell<Option<Instant>>,
    output_bytes: usize,
    launcher: Option<&'a Path>,
}

fn serve_process(
    pipe_in: &mut impl Read,
    pipe_out: &mut impl Write,
    calls: &Calls<'_>,
) -> std::io::Result<Result<Vec<u8>, String>> {
    loop {
        let (request, bytes): (Request, Vec<u8>) = read_frame(pipe_in)?;
        let result = match request {
            Request::Done { error: Some(error) } => return Ok(Err(error)),
            Request::Done { error: None } => return Ok(Ok(bytes)),
            Request::ReadFile { name } => host_read(calls.root, &name),
            Request::WriteFile { name } => host_write(calls.root, &name, &bytes).then(Vec::new),
            Request::Run => Some(host_run(&bytes, calls)),
        };
        let ok = result.is_some();
        write_frame(pipe_out, &Reply { ok }, &result.unwrap_or_default())?;
    }
}

fn kill_if_still_there_after(pid: u32, patience: Duration) -> Arc<AtomicBool> {
    let done = Arc::new(AtomicBool::new(false));
    let seen = done.clone();
    thread::spawn(move || {
        let deadline = Instant::now() + patience;
        while !seen.load(Ordering::Relaxed) {
            if Instant::now() >= deadline {
                if let Some(pid) = Pid::from_raw(pid.cast_signed()) {
                    let _ = rustix::process::kill_process(pid, Signal::KILL);
                }
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
    });
    done
}

fn call(
    helper: &Helper,
    module: &[u8],
    input: &[u8],
    limits: Limits,
    calls: &Calls<'_>,
) -> Result<Vec<u8>, String> {
    let mut child = Process::new(&helper.program)
        .args(&helper.args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .spawn()
        .map_err(|e| format!("the plugin process cannot start: {e}"))?;
    let (Some(mut pipe_out), Some(mut pipe_in)) = (child.stdin.take(), child.stdout.take()) else {
        return Err("the plugin process has no pipes".to_owned());
    };
    let start = Start {
        fuel: limits.fuel,
        memory_bytes: limits.memory_bytes,
        input_len: input.len(),
    };
    let patience = limits.process_time + limits.process_patience;
    let watch = kill_if_still_there_after(child.id(), patience);
    let bytes = [input, module].concat();
    let answer = write_frame(&mut pipe_out, &start, &bytes)
        .and_then(|()| serve_process(&mut pipe_in, &mut pipe_out, calls));
    drop(pipe_out);
    drop(pipe_in);
    let status = child.wait();
    watch.store(true, Ordering::Relaxed);
    answer.unwrap_or_else(|_| {
        let how = status.map_or_else(|e| e.to_string(), |s| s.to_string());
        Err(format!("the plugin process stopped unexpectedly ({how})"))
    })
}

pub fn run(
    plugin: &LoadedPlugin,
    command: &str,
    root: &Path,
    context: PluginContext,
    limits: Limits,
    launcher: Option<&Path>,
) -> Result<PluginReport, String> {
    let helper = plugin
        .helper
        .as_ref()
        .ok_or("the plugin process is not available")?;
    let module_path = plugin.dir.join(MODULE);
    let size = std::fs::metadata(&module_path)
        .map_err(|e| e.to_string())?
        .len();
    if size > MAX_MODULE_BYTES {
        return Err(format!("{MODULE} is larger than {MAX_MODULE_BYTES} bytes"));
    }
    let module = std::fs::read(module_path).map_err(|e| e.to_string())?;
    let input = serde_json::to_vec(&Event::PluginInvoked {
        plugin: plugin.info.name.clone(),
        command: command.to_owned(),
        context,
    })
    .map_err(|e| e.to_string())?;
    let calls = Calls {
        root,
        process_time: limits.process_time,
        process_deadline: std::cell::Cell::new(None),
        output_bytes: limits.output_bytes,
        launcher,
    };
    let answer = call(helper, &module, &input, limits, &calls)?;
    let answer: Answer =
        serde_json::from_slice(&answer).map_err(|e| format!("its answer is malformed: {e}"))?;
    if answer.commands.len() > MAX_COMMANDS {
        return Err(format!("it answered more than {MAX_COMMANDS} commands"));
    }
    Ok(PluginReport {
        commands: answer.commands,
        status: answer.status,
    })
}
