use std::io::{Read, Write};
use std::net::TcpListener;
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};
use ttywright::services::lsp::transport::{FrameDecoder, encode};

#[derive(Default)]
struct Program {
    file: String,
    line: i64,
    seq: i64,
}

fn log(text: &str) {
    if let Ok(file) = std::env::var("FAKE_DAP_LOG")
        && let Ok(mut f) = std::fs::File::options()
            .create(true)
            .append(true)
            .open(file)
    {
        let _ = writeln!(f, "{text}");
    }
}

struct Peer<W: Write> {
    out: W,
    program: Arc<Mutex<Program>>,
    is_child: bool,
}

impl<W: Write> Peer<W> {
    fn send(&mut self, message: &Value) -> std::io::Result<()> {
        self.out.write_all(&encode(message))?;
        self.out.flush()
    }

    fn next_seq(&self) -> i64 {
        let mut program = self.program.lock().unwrap_or_else(|e| e.into_inner());
        program.seq += 1;
        program.seq
    }

    fn event(&mut self, event: &str, body: Value) -> std::io::Result<()> {
        let seq = self.next_seq();
        self.send(&json!({"seq": seq, "type": "event", "event": event, "body": body}))
    }

    fn answer(&mut self, request: &Value, success: bool, body: Value) -> std::io::Result<()> {
        let seq = self.next_seq();
        self.send(&json!({
            "seq": seq,
            "type": "response",
            "request_seq": request["seq"],
            "command": request["command"],
            "success": success,
            "message": if success { "" } else { "refused by the script" },
            "body": body,
        }))
    }

    fn stop(&mut self, reason: &str) -> std::io::Result<()> {
        self.event("stopped", json!({"reason": reason, "threadId": 1}))
    }

    fn line(&self) -> (String, i64) {
        let program = self.program.lock().unwrap_or_else(|e| e.into_inner());
        (program.file.clone(), program.line)
    }

    fn handle(&mut self, request: &Value) -> std::io::Result<bool> {
        let command = request["command"].as_str().unwrap_or("");
        let args = &request["arguments"];
        match command {
            "launch" | "setBreakpoints" | "disconnect" => log(&format!("{command} {args}")),
            _ => log(command),
        }
        let mode = std::env::var("FAKE_DAP_MODE").unwrap_or_default();
        match command {
            "initialize" => {
                self.answer(
                    request,
                    true,
                    json!({
                        "supportsConfigurationDoneRequest": true,
                        "exceptionBreakpointFilters": [
                            {"filter": "raised", "label": "Raised", "default": false},
                            {"filter": "uncaught", "label": "Uncaught", "default": true},
                        ],
                    }),
                )?;
                self.event("initialized", json!({}))?;
            }
            "launch" => {
                if mode == "refuse-launch" {
                    self.answer(request, false, json!({}))?;
                } else if mode == "child" && !self.is_child {
                    let seq = self.next_seq();
                    self.send(&json!({
                        "seq": seq,
                        "type": "request",
                        "command": "startDebugging",
                        "arguments": {"request": "launch", "configuration": {"__child": true}},
                    }))?;
                    self.answer(request, true, json!({}))?;
                } else {
                    self.answer(request, true, json!({}))?;
                }
            }
            "setBreakpoints" => {
                let lines: Vec<i64> = args["breakpoints"]
                    .as_array()
                    .map(|b| b.iter().filter_map(|b| b["line"].as_i64()).collect())
                    .unwrap_or_default();
                if let Some(first) = lines.first() {
                    let mut program = self.program.lock().unwrap_or_else(|e| e.into_inner());
                    program.line = *first;
                    program.file = args["source"]["path"].as_str().unwrap_or("").to_owned();
                }
                let answers: Vec<Value> = lines
                    .iter()
                    .enumerate()
                    .map(|(n, line)| json!({"id": n + 100, "verified": true, "line": line}))
                    .collect();
                self.answer(request, true, json!({"breakpoints": answers}))?;
            }
            "configurationDone" => {
                self.answer(request, true, json!({}))?;
                let runs_here = mode != "child" || self.is_child;
                if runs_here {
                    self.event(
                        "output",
                        json!({"category": "stdout", "output": "started\n"}),
                    )?;
                    self.stop("breakpoint")?;
                }
            }
            "threads" => self.answer(
                request,
                true,
                json!({"threads": [{"id": 1, "name": "main"}]}),
            )?,
            "stackTrace" => {
                let (file, line) = self.line();
                self.answer(
                    request,
                    true,
                    json!({"stackFrames": [
                        {"id": 11, "name": "inner", "line": line, "column": 1,
                         "source": {"path": file}},
                        {"id": 12, "name": "outer", "line": 1, "column": 1,
                         "source": {"path": "/nowhere/lib.c"}},
                    ]}),
                )?;
            }
            "scopes" => self.answer(
                request,
                true,
                json!({"scopes": [
                    {"name": "Locals", "variablesReference": 21, "expensive": false},
                    {"name": "Globals", "variablesReference": 22, "expensive": true},
                ]}),
            )?,
            "variables" => {
                let variables = match args["variablesReference"].as_i64() {
                    Some(21) => {
                        let (_, line) = self.line();
                        json!([
                            {"name": "count", "value": line.to_string(), "type": "int",
                             "variablesReference": 0},
                            {"name": "point", "value": "{...}", "type": "Point",
                             "variablesReference": 31},
                        ])
                    }
                    Some(31) => json!([{"name": "x", "value": "7", "variablesReference": 0}]),
                    _ => json!([{"name": "G", "value": "1", "variablesReference": 0}]),
                };
                self.answer(request, true, json!({"variables": variables}))?;
            }
            "evaluate" => {
                let expression = args["expression"].as_str().unwrap_or("");
                if expression == "broken" {
                    self.answer(request, false, json!({}))?;
                } else {
                    self.answer(
                        request,
                        true,
                        json!({"result": format!("<{expression}>"), "variablesReference": 0}),
                    )?;
                }
            }
            "next" | "stepIn" | "stepOut" => {
                self.answer(request, true, json!({}))?;
                self.program.lock().unwrap_or_else(|e| e.into_inner()).line += 1;
                self.stop("step")?;
            }
            "continue" => {
                self.answer(request, true, json!({}))?;
                self.event(
                    "output",
                    json!({"category": "stdout", "output": "finished\n"}),
                )?;
                self.event("exited", json!({"exitCode": 3}))?;
                self.event("terminated", json!({}))?;
            }
            "disconnect" => {
                self.answer(request, true, json!({}))?;
                return Ok(false);
            }
            _ => self.answer(request, true, json!({}))?,
        }
        Ok(true)
    }
}

fn serve(
    mut input: impl Read,
    out: impl Write,
    program: Arc<Mutex<Program>>,
    is_child: bool,
) -> std::io::Result<()> {
    let mut peer = Peer {
        out,
        program,
        is_child,
    };
    let mut decoder = FrameDecoder::default();
    let mut buffer = [0u8; 8192];
    loop {
        let n = input.read(&mut buffer)?;
        if n == 0 {
            return Ok(());
        }
        decoder.push(&buffer[..n]);
        while let Some(message) = decoder.next_message() {
            let Ok(message) = message else { continue };
            if message["type"] == "response" {
                continue;
            }
            if !peer.handle(&message)? {
                return Ok(());
            }
        }
    }
}

fn main() -> std::io::Result<()> {
    let program = Arc::new(Mutex::new(Program::default()));
    let args: Vec<String> = std::env::args().collect();
    let port = args
        .iter()
        .position(|a| a == "--port")
        .and_then(|i| args.get(i + 1));
    let Some(port) = port else {
        return serve(std::io::stdin(), std::io::stdout(), program, false);
    };
    let listener = TcpListener::bind(format!("127.0.0.1:{port}"))?;
    let mut first = true;
    for stream in listener.incoming() {
        let stream = stream?;
        let reader = stream.try_clone()?;
        let is_child = !first;
        first = false;
        let program = Arc::clone(&program);
        let handle = std::thread::spawn(move || serve(reader, stream, program, is_child));
        if !is_child {
            std::thread::spawn(move || {
                let _ = handle.join();
                std::process::exit(0);
            });
        }
    }
    Ok(())
}
