use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Instant;

use rustix::process::{Pid, Signal, kill_process_group};

use crate::core::http::{
    HttpEffect, HttpEvent, HttpOutcome, HttpTemplate, clean_lines, curl_config, parse_env,
    parse_response, resolve,
};

#[derive(Debug, Clone, Copy)]
pub struct HttpLimits {
    pub max_seconds: u32,
    pub connect_seconds: u32,
    pub max_bytes: u64,
}

impl Default for HttpLimits {
    fn default() -> Self {
        Self {
            max_seconds: 60,
            connect_seconds: 15,
            max_bytes: 5 * 1024 * 1024,
        }
    }
}

const ENV_FILE: &str = "http-client.env";
const PRIVATE_ENV_FILE: &str = "http-client.private.env";
const MAX_ENV_BYTES: u64 = 1024 * 1024;
const MAX_ERROR_CHARS: usize = 300;

fn read_env_file(path: &Path) -> Option<String> {
    if !std::fs::symlink_metadata(path).ok()?.file_type().is_file() {
        return None;
    }
    let mut text = String::new();
    std::fs::File::open(path)
        .ok()?
        .take(MAX_ENV_BYTES)
        .read_to_string(&mut text)
        .ok()?;
    Some(text)
}

pub fn load_env(file: &Path, root: &Path) -> Vec<(String, String)> {
    for dir in file.ancestors().skip(1) {
        if !dir.starts_with(root) {
            break;
        }
        let public = read_env_file(&dir.join(ENV_FILE));
        let private = read_env_file(&dir.join(PRIVATE_ENV_FILE));
        if public.is_some() || private.is_some() {
            let mut vars = parse_env(public.as_deref().unwrap_or_default());
            vars.extend(parse_env(private.as_deref().unwrap_or_default()));
            return vars;
        }
    }
    Vec::new()
}

#[derive(Debug, Default)]
struct Running {
    group: Option<Pid>,
    stopped: bool,
}

type Table = Arc<Mutex<HashMap<u64, Running>>>;

fn locked(table: &Table) -> std::sync::MutexGuard<'_, HashMap<u64, Running>> {
    table.lock().unwrap_or_else(PoisonError::into_inner)
}

#[derive(Debug, Default)]
pub struct HttpHost {
    table: Table,
    limits: HttpLimits,
}

impl HttpHost {
    #[must_use]
    pub fn with_limits(limits: HttpLimits) -> Self {
        Self {
            table: Table::default(),
            limits,
        }
    }

    pub fn send<E>(&self, id: u64, file: PathBuf, root: PathBuf, template: HttpTemplate, emit: E)
    where
        E: FnOnce(u64, HttpOutcome) + Send + 'static,
    {
        locked(&self.table).insert(id, Running::default());
        let (table, limits) = (Arc::clone(&self.table), self.limits);
        thread::spawn(move || {
            let outcome = run(id, &file, &root, &template, limits, &table);
            locked(&table).remove(&id);
            emit(id, outcome);
        });
    }

    pub fn run<E>(&self, effect: HttpEffect, emit: E)
    where
        E: FnOnce(HttpEvent) + Send + 'static,
    {
        match effect {
            HttpEffect::Send {
                id,
                file,
                root,
                template,
            } => self.send(id, file, root, template, move |id, outcome| {
                emit(HttpEvent::Finished { id, outcome });
            }),
            HttpEffect::Cancel { id } => self.cancel(id),
        }
    }

    pub fn cancel(&self, id: u64) {
        if let Some(entry) = locked(&self.table).get_mut(&id) {
            entry.stopped = true;
            if let Some(group) = entry.group {
                let _ = kill_process_group(group, Signal::KILL);
            }
        }
    }
}

impl Drop for HttpHost {
    fn drop(&mut self) {
        for entry in locked(&self.table).values_mut() {
            entry.stopped = true;
            if let Some(group) = entry.group {
                let _ = kill_process_group(group, Signal::KILL);
            }
        }
    }
}

fn run(
    id: u64,
    file: &Path,
    root: &Path,
    template: &HttpTemplate,
    limits: HttpLimits,
    table: &Table,
) -> HttpOutcome {
    use std::os::unix::process::CommandExt;
    let request = match resolve(template, &load_env(file, root)) {
        Ok(request) => request,
        Err(message) => return HttpOutcome::Failed(message),
    };
    let Some(curl) = super::analysis::on_path("curl") else {
        return HttpOutcome::Failed("curl is not installed".to_owned());
    };
    let started = Instant::now();
    let spawned = Command::new(curl)
        .args(["-q", "-sS", "-i", "--globoff"])
        .args(["--proto", "=http,https", "--proto-redir", "=http,https"])
        .args(["--max-time", &limits.max_seconds.to_string()])
        .args(["--connect-timeout", &limits.connect_seconds.to_string()])
        .args(["--config", "-"])
        .current_dir(root)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => return HttpOutcome::Failed(format!("curl could not start: {e}")),
    };
    let group = i32::try_from(child.id()).ok().and_then(Pid::from_raw);
    let stopped_early = {
        let mut table = locked(table);
        let entry = table.entry(id).or_default();
        entry.group = group;
        entry.stopped
    };
    if stopped_early {
        let _ = child.kill();
        let _ = child.wait();
        return HttpOutcome::Cancelled;
    }
    let config = curl_config(&request);
    let stdin = child.stdin.take();
    let writer = thread::spawn(move || {
        if let Some(mut pipe) = stdin {
            let _ = pipe.write_all(config.as_bytes());
        }
    });
    let stderr = child.stderr.take();
    let errors = thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(pipe) = stderr {
            let _ = pipe.take(64 * 1024).read_to_end(&mut text);
        }
        text
    });
    let mut answer = Vec::new();
    if let Some(pipe) = child.stdout.take() {
        let _ = pipe.take(limits.max_bytes + 1).read_to_end(&mut answer);
    }
    let truncated = answer.len() as u64 > limits.max_bytes;
    if truncated {
        answer.pop();
        if let Some(group) = group {
            let _ = kill_process_group(group, Signal::KILL);
        }
    }
    let stopped = {
        let mut table = locked(table);
        let entry = table.entry(id).or_default();
        entry.group = None;
        entry.stopped
    };
    let status = child.wait();
    let _ = writer.join();
    let errors = errors.join().unwrap_or_default();
    if stopped {
        return HttpOutcome::Cancelled;
    }
    let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    if !truncated && !status.is_ok_and(|s| s.success()) {
        return HttpOutcome::Failed(complaint(&errors));
    }
    match parse_response(&answer, truncated, elapsed) {
        Ok(response) => HttpOutcome::Done(response),
        Err(message) => HttpOutcome::Failed(message),
    }
}

fn complaint(stderr: &[u8]) -> String {
    let line = clean_lines(stderr)
        .into_iter()
        .rfind(|l| !l.trim().is_empty())
        .unwrap_or_else(|| "curl failed".to_owned());
    line.chars().take(MAX_ERROR_CHARS).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn write(path: &Path, text: &str) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, text).unwrap();
    }

    #[test]
    fn the_nearest_env_is_used_and_private_values_win() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        write(&root.join(ENV_FILE), "A=root\n");
        write(&root.join("api/http-client.env"), "A=near\nB=1\n");
        write(&root.join("api/http-client.private.env"), "B=2\n");
        let env = load_env(&root.join("api/x.http"), &root);
        let get = |name: &str| {
            env.iter()
                .rev()
                .find(|(n, _)| n == name)
                .map(|(_, v)| v.as_str())
        };
        assert_eq!((get("A"), get("B")), (Some("near"), Some("2")));
        assert_eq!(load_env(&root.join("x.http"), &root)[0].1, "root");
    }

    #[test]
    fn nothing_above_the_project_is_read_and_links_are_not_followed() {
        let dir = tempfile::tempdir().unwrap();
        let top = dir.path().canonicalize().unwrap();
        write(&top.join(ENV_FILE), "A=outside\n");
        let root = top.join("project");
        std::fs::create_dir_all(&root).unwrap();
        assert!(load_env(&root.join("x.http"), &root).is_empty());
        let secret = top.join("secret");
        write(&secret, "TOKEN=x\n");
        std::os::unix::fs::symlink(&secret, root.join(ENV_FILE)).unwrap();
        assert!(load_env(&root.join("x.http"), &root).is_empty());
    }
}
