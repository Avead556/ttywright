use std::io::{self, BufRead, Write};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};

use serde_json::{Value, json};

use super::ipc;

const KNOWN_VERSIONS: [&str; 3] = ["2025-06-18", "2025-03-26", "2024-11-05"];
const INSTRUCTIONS: &str = "Tools of the ttywright editor the user is working in. read_buffer \
and get_selection see unsaved text, which files on disk do not have. propose_edit shows a diff \
to the user, and answers once they have decided, which can take a while.";
const NOT_RUNNING: &str = "ttywright is not open on this project: start it in the project's \
folder and try again";

fn tools() -> Value {
    let path = json!({ "type": "string", "description": "Absolute, or relative to the project" });
    let object = |properties: Value, required: Value| json!({ "type": "object", "properties": properties, "required": required });
    json!([
        {
            "name": "get_open_files",
            "description": "The project's folder and the files open in the editor: which is active, which have unsaved changes.",
            "annotations": { "readOnlyHint": true },
            "inputSchema": object(json!({}), json!([])),
        },
        {
            "name": "get_selection",
            "description": "The active file, the cursor and the selected text, as the user sees them now.",
            "annotations": { "readOnlyHint": true },
            "inputSchema": object(json!({}), json!([])),
        },
        {
            "name": "read_buffer",
            "description": "A file's text as the editor has it, with unsaved changes. Lines count from 1.",
            "annotations": { "readOnlyHint": true },
            "inputSchema": object(json!({
                "path": path,
                "from_line": { "type": "integer" },
                "to_line": { "type": "integer" },
            }), json!(["path"])),
        },
        {
            "name": "get_diagnostics",
            "description": "Errors and warnings from the language servers, for one file or all open ones.",
            "annotations": { "readOnlyHint": true },
            "inputSchema": object(json!({ "path": path }), json!([])),
        },
        {
            "name": "get_run_output",
            "description": "The end of the output of the program last run from the editor.",
            "annotations": { "readOnlyHint": true },
            "inputSchema": object(json!({ "last_lines": { "type": "integer" } }), json!([])),
        },
        {
            "name": "open_file",
            "description": "Shows a file to the user in the editor, at a place if given. Lines and columns count from 1.",
            "inputSchema": object(json!({
                "path": path,
                "line": { "type": "integer" },
                "column": { "type": "integer" },
            }), json!(["path"])),
        },
        {
            "name": "propose_edit",
            "description": "Proposes edits of existing project files. The user reviews a diff and accepts or rejects each file; the answer lists which. old_text must appear exactly once in the file. Nothing changes until they accept.",
            "inputSchema": object(json!({
                "summary": { "type": "string", "description": "One line on what this does and why" },
                "edits": {
                    "type": "array",
                    "items": object(json!({
                        "path": path,
                        "old_text": { "type": "string" },
                        "new_text": { "type": "string" },
                    }), json!(["path", "old_text", "new_text"])),
                },
            }), json!(["edits"])),
        },
    ])
}

fn success(id: &Value, result: Value) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "result": result })
}

fn failure(id: &Value, code: i64, message: &str) -> Value {
    json!({ "jsonrpc": "2.0", "id": id, "error": { "code": code, "message": message } })
}

pub(super) fn tool_result(result: Result<String, String>) -> Value {
    let (text, is_error) = match result {
        Ok(text) => (text, false),
        Err(text) => (text, true),
    };
    json!({ "content": [{ "type": "text", "text": text }], "isError": is_error })
}

pub(super) fn answer(message: &Value, tools: &Value, instructions: &str) -> Option<Value> {
    let id = message.get("id")?;
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    Some(match method {
        "initialize" => {
            let asked = message
                .pointer("/params/protocolVersion")
                .and_then(Value::as_str);
            let version = asked
                .filter(|v| KNOWN_VERSIONS.contains(v))
                .unwrap_or(KNOWN_VERSIONS[0]);
            success(
                id,
                json!({
                    "protocolVersion": version,
                    "capabilities": { "tools": {} },
                    "serverInfo": { "name": "ttywright", "version": env!("CARGO_PKG_VERSION") },
                    "instructions": instructions,
                }),
            )
        }
        "ping" => success(id, json!({})),
        "tools/list" => success(id, json!({ "tools": tools })),
        _ => failure(id, -32601, "Method not found"),
    })
}

fn find_socket(start: &Path) -> Option<PathBuf> {
    start
        .ancestors()
        .map(ipc::socket_path)
        .find(|socket| std::os::unix::net::UnixStream::connect(socket).is_ok())
}

fn call_tool(start: &Path, message: &Value) -> Value {
    let name = message
        .pointer("/params/name")
        .and_then(Value::as_str)
        .unwrap_or("");
    let args = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let result = find_socket(start)
        .ok_or_else(|| NOT_RUNNING.to_owned())
        .and_then(|socket| ipc::call_mcp(&socket, name, args));
    tool_result(result)
}

fn write_line(out: &Mutex<io::Stdout>, value: &Value) {
    if let Ok(mut out) = out.lock() {
        let _ = writeln!(out, "{value}");
        let _ = out.flush();
    }
}

pub fn serve(start: PathBuf) {
    let start = start.canonicalize().unwrap_or(start);
    let start = Arc::new(start);
    let out = Arc::new(Mutex::new(io::stdout()));
    for line in io::stdin().lock().lines() {
        let Ok(line) = line else {
            return;
        };
        let Ok(message) = serde_json::from_str::<Value>(&line) else {
            write_line(&out, &failure(&Value::Null, -32700, "Parse error"));
            continue;
        };
        let is_call = message.get("method").and_then(Value::as_str) == Some("tools/call");
        match message.get("id").cloned().filter(|_| is_call) {
            Some(id) => {
                let (start, out) = (Arc::clone(&start), Arc::clone(&out));
                std::thread::spawn(move || {
                    write_line(&out, &success(&id, call_tool(&start, &message)));
                });
            }
            None => {
                if let Some(reply) = answer(&message, &tools(), INSTRUCTIONS) {
                    write_line(&out, &reply);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn it_introduces_itself_in_the_client_s_version_when_it_knows_it() {
        let ask = |version: &str| {
            answer(
                &json!({
                    "id": 1,
                    "method": "initialize",
                    "params": { "protocolVersion": version },
                }),
                &tools(),
                INSTRUCTIONS,
            )
            .unwrap()
        };
        let reply = ask("2024-11-05");
        assert_eq!(reply["result"]["protocolVersion"], "2024-11-05");
        assert_eq!(reply["result"]["serverInfo"]["name"], "ttywright");
        assert!(reply["result"]["capabilities"]["tools"].is_object());
        assert_eq!(
            ask("1999-01-01")["result"]["protocolVersion"],
            KNOWN_VERSIONS[0]
        );
    }

    #[test]
    fn notifications_get_no_answer_and_unknown_methods_an_error() {
        assert_eq!(
            answer(
                &json!({ "method": "notifications/initialized" }),
                &tools(),
                INSTRUCTIONS
            ),
            None
        );
        assert_eq!(
            answer(
                &json!({ "id": 2, "method": "ping" }),
                &tools(),
                INSTRUCTIONS
            )
            .unwrap()["result"],
            json!({})
        );
        let reply = answer(
            &json!({ "id": 3, "method": "resources/list" }),
            &tools(),
            INSTRUCTIONS,
        )
        .unwrap();
        assert_eq!(reply["error"]["code"], -32601);
    }

    #[test]
    fn each_listed_tool_is_one_the_editor_understands() {
        let list = tools();
        let names: Vec<&str> = list
            .as_array()
            .unwrap()
            .iter()
            .map(|t| t["name"].as_str().unwrap())
            .collect();
        assert_eq!(names.len(), 7);
        let args =
            json!({ "path": "a", "edits": [{ "path": "a", "old_text": "x", "new_text": "y" }] });
        for name in names {
            let parsed = crate::core::mcp::McpCall::parse(name, &args, Path::new("/p"));
            assert!(parsed.is_ok(), "{name}: {parsed:?}");
        }
    }

    #[test]
    fn a_tool_failure_is_a_result_the_model_can_read() {
        let reply = tool_result(Err("no".to_owned()));
        assert_eq!(reply["isError"], true);
        assert_eq!(reply["content"][0]["text"], "no");
    }

    #[test]
    fn with_no_editor_open_a_call_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let reply = call_tool(
            dir.path(),
            &json!({ "params": { "name": "get_selection" } }),
        );
        assert_eq!(reply["isError"], true);
        assert!(
            reply["content"][0]["text"]
                .as_str()
                .unwrap()
                .contains("not open")
        );
    }
}
