use std::collections::HashMap;
use std::io::{Read, Write};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process_group};

use crate::core::db::{
    DbConnection, DbEffect, DbEndpoint, DbEvent, DbKind, DbOutcome, client_command, parse_csv,
    parse_tsv,
};
use crate::core::http::clean_lines;

const MAX_BYTES: u64 = 32 * 1024 * 1024;
const MAX_ERROR_CHARS: usize = 300;
const INTERRUPT_GRACE: Duration = Duration::from_millis(1500);

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
pub struct DbHost {
    table: Table,
}

impl DbHost {
    pub fn query<E>(
        &self,
        id: u64,
        connection: DbConnection,
        statement: String,
        read_only: bool,
        emit: E,
    ) where
        E: FnOnce(u64, DbOutcome) + Send + 'static,
    {
        locked(&self.table).insert(id, Running::default());
        let table = Arc::clone(&self.table);
        thread::spawn(move || {
            let outcome = run(id, &connection, &statement, read_only, &table);
            locked(&table).remove(&id);
            emit(id, outcome);
        });
    }

    pub fn run<E>(&self, effect: DbEffect, emit: E)
    where
        E: FnOnce(DbEvent) + Send + 'static,
    {
        match effect {
            DbEffect::Query {
                id,
                connection,
                statement,
                read_only,
            } => self.query(id, connection, statement, read_only, move |id, outcome| {
                emit(DbEvent::Finished { id, outcome });
            }),
            DbEffect::Cancel { id } => self.cancel(id),
        }
    }

    pub fn cancel(&self, id: u64) {
        {
            let mut table = locked(&self.table);
            let Some(entry) = table.get_mut(&id) else {
                return;
            };
            entry.stopped = true;
            let Some(group) = entry.group else {
                return;
            };
            let _ = kill_process_group(group, Signal::INT);
        }
        let table = Arc::clone(&self.table);
        thread::spawn(move || {
            thread::sleep(INTERRUPT_GRACE);
            if let Some(group) = locked(&table).get(&id).and_then(|e| e.group) {
                let _ = kill_process_group(group, Signal::KILL);
            }
        });
    }
}

impl Drop for DbHost {
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
    connection: &DbConnection,
    statement: &str,
    read_only: bool,
    table: &Table,
) -> DbOutcome {
    use std::os::unix::process::CommandExt;
    if let DbEndpoint::Sqlite { path } = &connection.endpoint
        && !Path::new(path).is_file()
    {
        return DbOutcome::Failed(format!("No database file at {}", path.display()));
    }
    let client = client_command(connection, read_only);
    let Some(program) = client
        .programs
        .iter()
        .find_map(|p| super::analysis::on_path(p))
    else {
        return DbOutcome::Failed(format!("{} is not installed", client.programs[0]));
    };
    let mut command = Command::new(program);
    command
        .args(&client.args)
        .envs(client.env.iter().map(|(k, v)| (*k, v)))
        .current_dir(std::env::temp_dir())
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0);
    if let Some(name) = &client.password_env {
        match std::env::var(name) {
            Ok(password) => {
                command.env("PGPASSWORD", password);
            }
            Err(_) => {
                return DbOutcome::Failed(format!("The variable {name} is not set"));
            }
        }
    }
    let started = Instant::now();
    let mut child = match command.spawn() {
        Ok(child) => child,
        Err(e) => return DbOutcome::Failed(format!("{} could not start: {e}", client.programs[0])),
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
        return DbOutcome::Cancelled;
    }
    let input = format!("{statement}\n;\n");
    let stdin = child.stdin.take();
    let writer = thread::spawn(move || {
        if let Some(mut pipe) = stdin {
            let _ = pipe.write_all(input.as_bytes());
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
    let mut cut = false;
    if let Some(mut pipe) = child.stdout.take() {
        let _ = (&mut pipe).take(MAX_BYTES + 1).read_to_end(&mut answer);
        cut = answer.len() as u64 > MAX_BYTES;
        if cut {
            answer.pop();
            if read_only {
                if let Some(group) = group {
                    let _ = kill_process_group(group, Signal::KILL);
                }
            } else {
                let _ = std::io::copy(&mut pipe, &mut std::io::sink());
            }
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
        return DbOutcome::Cancelled;
    }
    if !cut && !status.is_ok_and(|s| s.success()) {
        return DbOutcome::Failed(complaint(&errors, client.programs[0]));
    }
    let elapsed = u64::try_from(started.elapsed().as_millis()).unwrap_or(u64::MAX);
    DbOutcome::Done(match connection.kind() {
        DbKind::Mysql => parse_tsv(&answer, cut, elapsed),
        DbKind::Sqlite | DbKind::Postgres => parse_csv(&answer, cut, elapsed),
    })
}

fn complaint(stderr: &[u8], program: &str) -> String {
    let line = clean_lines(stderr)
        .into_iter()
        .find(|l| !l.trim().is_empty())
        .unwrap_or_else(|| format!("{program} failed"));
    line.chars().take(MAX_ERROR_CHARS).collect()
}
