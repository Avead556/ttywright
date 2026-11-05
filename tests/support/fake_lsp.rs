use std::io::{Read, Write};

use serde_json::{Value, json};
use ttywright::services::lsp::transport::{FrameDecoder, encode};

fn send(message: &Value) -> std::io::Result<()> {
    let mut out = std::io::stdout();
    out.write_all(&encode(message))?;
    out.flush()
}

fn log(method: &str) -> std::io::Result<()> {
    if let Ok(file) = std::env::var("FAKE_LSP_LOG") {
        let mut f = std::fs::File::options()
            .create(true)
            .append(true)
            .open(file)?;
        writeln!(f, "{method}")?;
    }
    Ok(())
}

fn log_details(method: &str, message: &Value) -> std::io::Result<()> {
    let params = &message["params"];
    if method == "initialize" {
        log(&format!("options={}", params["initializationOptions"]))?;
        log(&format!("pid={}", std::process::id()))?;
    }
    if method == "textDocument/didOpen" {
        let id = params["textDocument"]["languageId"].as_str();
        log(&format!("languageId={}", id.unwrap_or_default()))?;
    }
    let at = &params["position"];
    if at.is_object() {
        log(&format!("position={}:{}", at["line"], at["character"]))?;
    }
    Ok(())
}

fn initialize_reply(message: &Value) -> Value {
    let encoding = std::env::var("FAKE_LSP_ENCODING").unwrap_or_else(|_| "utf-8".to_owned());
    let mut capabilities = json!({"completionProvider": {"triggerCharacters": [".", ":"]}});
    if encoding != "none" {
        capabilities["positionEncoding"] = json!(encoding);
    }
    json!({"jsonrpc": "2.0", "id": message["id"], "result": {"capabilities": capabilities}})
}

fn publish(uri: &Value, version: &Value) -> std::io::Result<()> {
    send(&json!({
        "jsonrpc": "2.0",
        "method": "textDocument/publishDiagnostics",
        "params": {"uri": uri, "diagnostics": [{
            "range": {"start": {"line": 0, "character": 0}, "end": {"line": 0, "character": 2}},
            "severity": 1,
            "message": format!("fake error v{version}"),
        }]},
    }))
}

fn definition(message: &Value) -> Option<Value> {
    let uri = &message["params"]["textDocument"]["uri"];
    let range = |line: u32, character: u32| {
        let at = json!({"line": line, "character": character});
        json!({"start": at, "end": at})
    };
    let location = |line| json!({"uri": uri, "range": range(line, 0)});
    let result = match message["params"]["position"]["line"].as_u64() {
        Some(0) => json!([{
            "targetUri": uri,
            "targetRange": range(0, 0),
            "targetSelectionRange": range(1, 2),
        }]),
        Some(1) => json!([location(3), location(4)]),
        Some(2) => Value::Null,
        Some(3) => location(5),
        Some(4) => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": message["id"],
                "error": {"code": -32603, "message": "fake failure"},
            }));
        }
        _ => return None,
    };
    Some(json!({"jsonrpc": "2.0", "id": message["id"], "result": result}))
}

fn hover(message: &Value) -> Option<Value> {
    let result = match message["params"]["position"]["line"].as_u64() {
        Some(0) => {
            json!({"contents": {"kind": "markdown", "value": "```rust\nfn fake()\n```\n\n---\n\nFake docs"}})
        }
        Some(1) => json!({"contents": [{"language": "rust", "value": "fn old()"}, "Old docs"]}),
        Some(2) => Value::Null,
        Some(3) => json!({"contents": "Plain docs"}),
        Some(4) => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": message["id"],
                "error": {"code": -32603, "message": "fake failure"},
            }));
        }
        _ => return None,
    };
    Some(json!({"jsonrpc": "2.0", "id": message["id"], "result": result}))
}

fn completion(message: &Value) -> Option<Value> {
    let at = &message["params"]["position"];
    let edit = |text: &str| json!({"range": {"start": at, "end": at}, "newText": text});
    let result = match at["line"].as_u64() {
        Some(0) => json!({"isIncomplete": false, "items": [
            {"label": "beta()", "sortText": "2", "textEdit": edit("beta")},
            {"label": "alpha", "sortText": "1", "detail": "fn alpha()", "textEdit": edit("alpha")},
            {"label": "alpaca", "sortText": "3", "textEdit": edit("alpaca")},
            {"label": "snip", "insertTextFormat": 2, "insertText": "snip(${1:x}, $2)$0"},
            {"label": "unreadable", "kind": "not a number"},
        ]}),
        Some(1) => json!([{"label": "plain"}, {"label": "shown", "insertText": "inserted"}]),
        Some(2) => Value::Null,
        Some(3) => json!({"isIncomplete": true, "items": [{"label": "more"}]}),
        Some(4) => {
            return Some(json!({
                "jsonrpc": "2.0",
                "id": message["id"],
                "error": {"code": -32603, "message": "fake failure"},
            }));
        }
        _ => return None,
    };
    Some(json!({"jsonrpc": "2.0", "id": message["id"], "result": result}))
}

fn references(message: &Value) -> Option<Value> {
    let uri = &message["params"]["textDocument"]["uri"];
    let at = |line: u32| {
        let start = json!({"line": line, "character": 1});
        json!({"uri": uri, "range": {"start": start, "end": start}})
    };
    let result = match message["params"]["position"]["line"].as_u64() {
        Some(0) => json!([at(2), at(1)]),
        Some(2) => Value::Null,
        _ => return None,
    };
    Some(json!({"jsonrpc": "2.0", "id": message["id"], "result": result}))
}

fn formatting(message: &Value) -> Value {
    let uri = message["params"]["textDocument"]["uri"]
        .as_str()
        .unwrap_or_default();
    if uri.contains("bad") {
        return json!({
            "jsonrpc": "2.0",
            "id": message["id"],
            "error": {"code": -32603, "message": "rustfmt failed"},
        });
    }
    let start = json!({"line": 0, "character": 0});
    let result = if uri.contains("clean") {
        Value::Null
    } else {
        json!([{"range": {"start": start, "end": start}, "newText": "// formatted\n"}])
    };
    json!({"jsonrpc": "2.0", "id": message["id"], "result": result})
}

fn code_actions(message: &Value) -> Value {
    let uri = message["params"]["textDocument"]["uri"]
        .as_str()
        .unwrap_or_default();
    let start = json!({"line": 0, "character": 0});
    let edit = json!({"range": {"start": start, "end": start}, "newText": "// fixed\n"});
    let result = if message["params"]["range"]["start"]["line"] == 2 {
        Value::Null
    } else {
        json!([{"title": "Fake fix", "kind": "quickfix", "edit": {"changes": {uri: [edit]}}}])
    };
    json!({"jsonrpc": "2.0", "id": message["id"], "result": result})
}

fn should_crash(mode: &str) -> std::io::Result<bool> {
    match mode {
        "crash-on-open" => Ok(true),
        "crash-once" => {
            let marker = std::env::var("FAKE_LSP_MARKER").unwrap_or_default();
            if std::path::Path::new(&marker).exists() {
                Ok(false)
            } else {
                std::fs::write(marker, "")?;
                Ok(true)
            }
        }
        _ => Ok(false),
    }
}

fn main() -> std::io::Result<()> {
    let mode = std::env::var("FAKE_LSP_MODE").unwrap_or_else(|_| "normal".to_owned());
    let mut decoder = FrameDecoder::default();
    let mut stdin = std::io::stdin();
    let mut chunk = [0; 65536];
    let mut pending_initialize = None;
    loop {
        let n = stdin.read(&mut chunk)?;
        if n == 0 {
            return Ok(());
        }
        decoder.push(&chunk[..n]);
        while let Some(message) = decoder.next_message() {
            let Ok(message) = message else { continue };
            let method = message["method"].as_str().unwrap_or_default().to_owned();
            log(&method)?;
            log_details(&method, &message)?;
            match method.as_str() {
                "initialize" if mode == "hang-init" => {}
                "initialize" if mode == "asks-first" => {
                    pending_initialize = Some(message);
                    send(
                        &json!({"jsonrpc": "2.0", "id": "ask", "method": "window/showMessageRequest",
                        "params": {"type": 3, "message": "hello"}}),
                    )?;
                }
                "" if message["id"] == "ask" => {
                    if let Some(request) = pending_initialize.take() {
                        send(&initialize_reply(&request))?;
                    }
                }
                "initialize" => send(&initialize_reply(&message))?,
                "initialized" if mode == "garbage" => {
                    for _ in 0..10 {
                        std::io::stdout().write_all(b"Content-Length: 1\r\n\r\n{")?;
                    }
                    std::io::stdout().flush()?;
                }
                "textDocument/didOpen" => {
                    if should_crash(&mode)? {
                        std::process::exit(1);
                    }
                    let doc = &message["params"]["textDocument"];
                    publish(&doc["uri"], &doc["version"])?;
                }
                "textDocument/didChange" => {
                    let doc = &message["params"]["textDocument"];
                    publish(&doc["uri"], &doc["version"])?;
                }
                "textDocument/definition" => {
                    if let Some(reply) = definition(&message) {
                        send(&reply)?;
                    }
                }
                "textDocument/hover" => {
                    if let Some(reply) = hover(&message) {
                        send(&reply)?;
                    }
                }
                "textDocument/completion" => {
                    if let Some(reply) = completion(&message) {
                        send(&reply)?;
                    }
                }
                "textDocument/references" => {
                    if let Some(reply) = references(&message) {
                        send(&reply)?;
                    }
                }
                "textDocument/formatting" => send(&formatting(&message))?,
                "textDocument/codeAction" => send(&code_actions(&message))?,
                "shutdown" if mode == "ignore-shutdown" => {
                    std::thread::sleep(std::time::Duration::from_secs(3600));
                }
                "shutdown" => {
                    send(&json!({"jsonrpc": "2.0", "id": message["id"], "result": null}))?
                }
                "exit" => return Ok(()),
                _ => {}
            }
        }
    }
}
