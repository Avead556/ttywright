use std::io::{Read, Write};
use std::path::Path;
use std::sync::mpsc::{Sender, channel};

use portable_pty::{ChildKiller, CommandBuilder, MasterPty, PtySize, native_pty_system};

use std::collections::HashMap;

use crate::core::terminal::{TerminalEffect, TerminalEvent};
use crate::core::{ClaudeState, Event};

const CLAUDE_NOT_FOUND: &str = "Claude Code was not found: install it from claude.com/claude-code";

fn size(rows: u16, columns: u16) -> PtySize {
    PtySize {
        rows,
        cols: columns,
        pixel_width: 0,
        pixel_height: 0,
    }
}

pub struct PtySpec<'a> {
    pub program: &'a str,
    pub args: &'a [&'a str],
    pub root: &'a Path,
    pub rows: u16,
    pub columns: u16,
    pub ide_port: Option<u16>,
}

pub struct PtyProcess {
    master: Box<dyn MasterPty + Send>,
    input: Sender<Vec<u8>>,
    killer: Box<dyn ChildKiller + Send + Sync>,
}

impl PtyProcess {
    pub fn start(
        spec: &PtySpec<'_>,
        on_output: impl Fn(Vec<u8>) + Send + 'static,
        on_end: impl FnOnce(Option<i32>) + Send + 'static,
    ) -> Result<Self, String> {
        let pair = native_pty_system()
            .openpty(size(spec.rows, spec.columns))
            .map_err(|e| e.to_string())?;
        let mut command = CommandBuilder::new(spec.program);
        command.args(spec.args);
        command.cwd(spec.root);
        command.env("TERM", "xterm-256color");
        command.env("COLORTERM", "truecolor");
        for name in [
            "TMUX",
            "TERM_PROGRAM",
            "TERM_PROGRAM_VERSION",
            "ITERM_SESSION_ID",
        ] {
            command.env_remove(name);
        }
        if let Some(port) = spec.ide_port {
            command.env("CLAUDE_CODE_SSE_PORT", port.to_string());
        }
        let mut child = pair
            .slave
            .spawn_command(command)
            .map_err(|e| e.to_string())?;
        drop(pair.slave);
        let killer = child.clone_killer();
        let mut reader = pair.master.try_clone_reader().map_err(|e| e.to_string())?;
        let mut writer = pair.master.take_writer().map_err(|e| e.to_string())?;
        let (input, typed) = channel::<Vec<u8>>();
        std::thread::spawn(move || {
            for bytes in typed {
                if writer.write_all(&bytes).is_err() || writer.flush().is_err() {
                    return;
                }
            }
        });
        std::thread::spawn(move || {
            let mut buffer = [0u8; 16 * 1024];
            loop {
                match reader.read(&mut buffer) {
                    Ok(0) | Err(_) => break,
                    Ok(n) => on_output(buffer[..n].to_vec()),
                }
            }
            let code = child
                .wait()
                .ok()
                .and_then(|status| i32::try_from(status.exit_code()).ok());
            on_end(code);
        });
        Ok(Self {
            master: pair.master,
            input,
            killer,
        })
    }

    pub fn write(&self, bytes: Vec<u8>) {
        let _ = self.input.send(bytes);
    }

    pub fn resize(&self, rows: u16, columns: u16) {
        let _ = self.master.resize(size(rows, columns));
    }
}

impl Drop for PtyProcess {
    fn drop(&mut self) {
        let _ = self.killer.kill();
    }
}

pub fn start_claude(
    root: &Path,
    rows: u16,
    columns: u16,
    ide_port: Option<u16>,
    emit: impl Fn(Event) + Clone + Send + 'static,
) -> Result<PtyProcess, String> {
    let config = std::env::current_exe()
        .ok()
        .map(|exe| super::ipc::claude_mcp_config(&exe, root));
    let with_tools;
    let args: &[&str] = match &config {
        Some(config) => {
            with_tools = ["--mcp-config", config.as_str()];
            &with_tools
        }
        None => &[],
    };
    let spec = PtySpec {
        program: "claude",
        args,
        root,
        rows,
        columns,
        ide_port,
    };
    let output = emit.clone();
    PtyProcess::start(
        &spec,
        move |bytes| output(Event::ClaudeOutput { bytes }),
        move |code| {
            emit(Event::ClaudeEnded {
                state: ClaudeState::Exited(code),
            });
        },
    )
    .map_err(|_| CLAUDE_NOT_FOUND.to_owned())
}

#[derive(Default)]
pub struct TerminalHost {
    running: HashMap<u32, PtyProcess>,
}

impl TerminalHost {
    pub fn run(
        &mut self,
        effect: TerminalEffect,
        ide_port: Option<u16>,
        emit: impl Fn(Event) + Clone + Send + 'static,
    ) -> Option<Event> {
        match effect {
            TerminalEffect::Start {
                id,
                root,
                rows,
                columns,
                command,
                project_command: _,
            } => match start_shell(id, &root, rows, columns, ide_port, &command, emit) {
                Ok(process) => {
                    self.running.insert(id, process);
                    None
                }
                Err(message) => Some(Event::Terminal(TerminalEvent::Ended {
                    id,
                    failure: Some(message),
                })),
            },
            TerminalEffect::Write { id, bytes } => {
                if let Some(process) = self.running.get(&id) {
                    process.write(bytes);
                }
                None
            }
            TerminalEffect::Resize { id, rows, columns } => {
                if let Some(process) = self.running.get(&id) {
                    process.resize(rows, columns);
                }
                None
            }
            TerminalEffect::Close { id } => {
                self.running.remove(&id);
                None
            }
        }
    }
}

pub fn start_shell(
    id: u32,
    root: &Path,
    rows: u16,
    columns: u16,
    ide_port: Option<u16>,
    command: &[String],
    emit: impl Fn(Event) + Clone + Send + 'static,
) -> Result<PtyProcess, String> {
    let shell = std::env::var("SHELL").unwrap_or_else(|_| "/bin/sh".to_owned());
    let login = ["-l".to_owned()];
    let (program, args) = match command.split_first() {
        Some((program, args)) => (program, args),
        None => (&shell, &login[..]),
    };
    let args: Vec<&str> = args.iter().map(String::as_str).collect();
    let spec = PtySpec {
        program,
        args: &args,
        root,
        rows,
        columns,
        ide_port,
    };
    let output = emit.clone();
    PtyProcess::start(
        &spec,
        move |bytes| output(Event::Terminal(TerminalEvent::Output { id, bytes })),
        move |_| {
            emit(Event::Terminal(TerminalEvent::Ended { id, failure: None }));
        },
    )
    .map_err(|e| format!("Could not start {program}: {e}"))
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;
    use std::time::Duration;

    use super::*;

    #[test]
    fn a_program_runs_on_a_real_tty_at_the_given_size_and_ending_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = channel();
        let ended = tx.clone();
        let spec = PtySpec {
            program: "cat",
            args: &[],
            root: dir.path(),
            rows: 10,
            columns: 40,
            ide_port: Some(7),
        };
        let process = PtyProcess::start(
            &spec,
            move |bytes| {
                let _ = tx.send(Some(bytes));
            },
            move |_| {
                let _ = ended.send(None);
            },
        )
        .unwrap();
        process.write(b"hello\n".to_vec());
        let mut seen = Vec::new();
        while !String::from_utf8_lossy(&seen).contains("hello") {
            let bytes = rx.recv_timeout(Duration::from_secs(20)).unwrap().unwrap();
            seen.extend_from_slice(&bytes);
        }
        process.resize(20, 60);
        process.write(vec![4]);
        while rx.recv_timeout(Duration::from_secs(20)).unwrap().is_some() {}
    }

    #[test]
    fn a_missing_program_is_reported() {
        let dir = tempfile::tempdir().unwrap();
        let spec = PtySpec {
            program: "ttywright-no-such-program",
            args: &[],
            root: dir.path(),
            rows: 10,
            columns: 40,
            ide_port: None,
        };
        assert!(PtyProcess::start(&spec, |_| {}, |_| {}).is_err());
    }
}
