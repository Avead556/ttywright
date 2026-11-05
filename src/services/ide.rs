use std::collections::VecDeque;
use std::io::{self, ErrorKind, Read};
use std::net::{SocketAddr, TcpListener, TcpStream};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, Sender, channel};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::{Value, json};
use tungstenite::handshake::server::{ErrorResponse, Request, Response};
use tungstenite::http::{HeaderValue, StatusCode};
use tungstenite::{Message, WebSocket};

use super::ipc::McpCalls;
use super::mcp;
use crate::core::Event;

const POLL: Duration = Duration::from_millis(15);
const AUTH_HEADER: &str = "x-claude-code-ide-authorization";
const INSTRUCTIONS: &str = "The ttywright editor the user is working in.";

fn ide_tools() -> Value {
    let empty = json!({ "type": "object", "properties": {} });
    let path = |name: &str| json!({ "type": "object", "properties": { name: { "type": "string" } }, "required": [name] });
    json!([
        {
            "name": "openDiff",
            "description": "Shows an edit for the user to accept or reject.",
            "inputSchema": {
                "type": "object",
                "properties": {
                    "old_file_path": { "type": "string" },
                    "new_file_path": { "type": "string" },
                    "new_file_contents": { "type": "string" },
                    "tab_name": { "type": "string" },
                },
                "required": ["new_file_path", "new_file_contents", "tab_name"],
            },
        },
        { "name": "close_tab", "description": "Closes a diff.", "inputSchema": path("tab_name") },
        { "name": "closeAllDiffTabs", "description": "Closes all diffs.", "inputSchema": empty },
        { "name": "openFile", "description": "Shows a file.", "inputSchema": path("filePath") },
        { "name": "getDiagnostics", "description": "Errors and warnings from the language servers.", "inputSchema": { "type": "object", "properties": { "uri": { "type": "string" } } } },
        { "name": "getCurrentSelection", "description": "The active file and selection.", "inputSchema": empty },
        { "name": "getOpenEditors", "description": "Open files.", "inputSchema": empty },
        { "name": "getWorkspaceFolders", "description": "The project's folder.", "inputSchema": empty },
    ])
}

fn content(tool: &str, result: Result<String, String>) -> Value {
    match result {
        Ok(text) if tool == "openDiff" => {
            let items: Vec<String> = serde_json::from_str(&text).unwrap_or_default();
            let items: Vec<Value> = items
                .into_iter()
                .map(|text| json!({ "type": "text", "text": text }))
                .collect();
            json!({ "content": items, "isError": false })
        }
        other => mcp::tool_result(other),
    }
}

#[allow(clippy::result_large_err)]
fn accept_handshake(
    token: &str,
    request: &Request,
    mut response: Response,
) -> Result<Response, ErrorResponse> {
    let refuse = || {
        let mut refused = ErrorResponse::new(None);
        *refused.status_mut() = StatusCode::FORBIDDEN;
        refused
    };
    let auth = request
        .headers()
        .get(AUTH_HEADER)
        .and_then(|v| v.to_str().ok());
    if auth != Some(token) || request.headers().contains_key("origin") {
        return Err(refuse());
    }
    response
        .headers_mut()
        .insert("sec-websocket-protocol", HeaderValue::from_static("mcp"));
    Ok(response)
}

fn random_token() -> io::Result<String> {
    let mut bytes = [0u8; 16];
    std::fs::File::open("/dev/urandom")?.read_exact(&mut bytes)?;
    Ok(bytes.iter().map(|b| format!("{b:02x}")).collect())
}

fn lock_dir() -> Option<PathBuf> {
    let config = std::env::var_os("CLAUDE_CONFIG_DIR")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| Path::new(&home).join(".claude")))?;
    Some(config.join("ide"))
}

fn write_lock(path: &Path, root: &Path, token: &str) -> io::Result<()> {
    use std::io::Write;
    use std::os::unix::fs::OpenOptionsExt;
    if let Some(dir) = path.parent() {
        std::fs::create_dir_all(dir)?;
    }
    let lock = json!({
        "pid": std::process::id(),
        "workspaceFolders": [root],
        "ideName": "ttywright",
        "transport": "ws",
        "runningInWindows": false,
        "authToken": token,
    });
    std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(true)
        .mode(0o600)
        .open(path)?
        .write_all(lock.to_string().as_bytes())
}

#[derive(Clone, Default)]
struct Clients(Arc<Mutex<Vec<Sender<String>>>>);

impl Clients {
    fn add(&self) -> Option<(Sender<String>, Receiver<String>)> {
        let (tx, rx) = channel();
        self.0.lock().ok()?.push(tx.clone());
        Some((tx, rx))
    }

    fn send(&self, text: &str) {
        if let Ok(mut clients) = self.0.lock() {
            clients.retain(|client| client.send(text.to_owned()).is_ok());
        }
    }
}

pub struct IdeServer {
    port: u16,
    lock: PathBuf,
    clients: Clients,
}

impl IdeServer {
    pub fn start(
        root: &Path,
        calls: McpCalls,
        emit: impl Fn(Event) + Clone + Send + 'static,
    ) -> io::Result<Self> {
        let listener = TcpListener::bind(SocketAddr::from(([127, 0, 0, 1], 0)))?;
        let port = listener.local_addr()?.port();
        let token = random_token()?;
        let dir = lock_dir().ok_or_else(|| io::Error::other("no home folder"))?;
        let lock = dir.join(format!("{port}.lock"));
        write_lock(&lock, root, &token)?;
        let clients = Clients::default();
        let accepting = clients.clone();
        std::thread::spawn(move || {
            for stream in listener.incoming().flatten() {
                let (token, calls, emit, clients) = (
                    token.clone(),
                    calls.clone(),
                    emit.clone(),
                    accepting.clone(),
                );
                std::thread::spawn(move || serve_client(stream, &token, &calls, &emit, &clients));
            }
        });
        Ok(Self {
            port,
            lock,
            clients,
        })
    }

    pub fn port(&self) -> u16 {
        self.port
    }

    pub fn notify(&self, method: &str, params: &Value) {
        let message = json!({ "jsonrpc": "2.0", "method": method, "params": params });
        self.clients.send(&message.to_string());
    }
}

impl Drop for IdeServer {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.lock);
    }
}

fn serve_client(
    stream: TcpStream,
    token: &str,
    calls: &McpCalls,
    emit: &(impl Fn(Event) + Clone + Send + 'static),
    clients: &Clients,
) {
    let handshake =
        |request: &Request, response: Response| accept_handshake(token, request, response);
    let Ok(mut socket) = tungstenite::accept_hdr(stream, handshake) else {
        return;
    };
    let Some((out, queue)) = clients.add() else {
        return;
    };
    let _ = socket.get_ref().set_read_timeout(Some(POLL));
    let mut connected = false;
    loop {
        match socket.read() {
            Ok(Message::Text(text)) => {
                let message: Value = serde_json::from_str(text.as_str()).unwrap_or(Value::Null);
                connected |= handle_message(&message, calls, &out, emit);
            }
            Ok(Message::Close(_)) | Err(tungstenite::Error::ConnectionClosed) => break,
            Ok(_) => {}
            Err(tungstenite::Error::Io(e))
                if matches!(e.kind(), ErrorKind::WouldBlock | ErrorKind::TimedOut) => {}
            Err(_) => break,
        }
        if !drain(&mut socket, &queue) {
            break;
        }
    }
    if connected {
        emit(Event::IdeDisconnected);
    }
}

fn drain(socket: &mut WebSocket<TcpStream>, queue: &Receiver<String>) -> bool {
    let waiting: VecDeque<String> = queue.try_iter().collect();
    for text in waiting {
        if socket.send(Message::text(text)).is_err() {
            return false;
        }
    }
    true
}

fn handle_message(
    message: &Value,
    calls: &McpCalls,
    out: &Sender<String>,
    emit: &impl Fn(Event),
) -> bool {
    let method = message.get("method").and_then(Value::as_str).unwrap_or("");
    if method == "ide_connected" {
        emit(Event::IdeConnected);
        return true;
    }
    if method != "tools/call" {
        if let Some(reply) = mcp::answer(message, &ide_tools(), INSTRUCTIONS) {
            let _ = out.send(reply.to_string());
        }
        return false;
    }
    let (Some(request), Some(tool)) = (
        message.get("id").cloned(),
        message.pointer("/params/name").and_then(Value::as_str),
    ) else {
        return false;
    };
    let tool = tool.to_owned();
    let args = message
        .pointer("/params/arguments")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let out = out.clone();
    let answering = tool.clone();
    let id = calls.add(move |result| {
        let reply = json!({
            "jsonrpc": "2.0",
            "id": request,
            "result": content(&answering, result),
        });
        let _ = out.send(reply.to_string());
    });
    if let Some(id) = id {
        emit(Event::McpCalled { id, tool, args });
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;

    fn request(auth: Option<&str>, origin: bool) -> Request {
        let mut builder = Request::builder().uri("/");
        if let Some(auth) = auth {
            builder = builder.header(AUTH_HEADER, auth);
        }
        if origin {
            builder = builder.header("origin", "https://evil.example");
        }
        builder.body(()).unwrap()
    }

    #[test]
    fn only_the_right_token_and_no_browser_gets_in() {
        let ok = accept_handshake("t", &request(Some("t"), false), Response::new(())).unwrap();
        assert_eq!(ok.headers()["sec-websocket-protocol"], "mcp");
        assert!(accept_handshake("t", &request(Some("x"), false), Response::new(())).is_err());
        assert!(accept_handshake("t", &request(None, false), Response::new(())).is_err());
        assert!(accept_handshake("t", &request(Some("t"), true), Response::new(())).is_err());
    }

    #[test]
    fn an_accepted_diff_comes_back_as_claude_code_expects() {
        let reply = content(
            "openDiff",
            Ok(serde_json::to_string(&["FILE_SAVED", "new text"]).unwrap()),
        );
        assert_eq!(reply["content"][0]["text"], "FILE_SAVED");
        assert_eq!(reply["content"][1]["text"], "new text");
        assert_eq!(
            content("close_tab", Ok("x".into()))["content"][0]["text"],
            "x"
        );
    }

    #[test]
    fn the_lock_file_is_private_and_says_where_to_connect() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("ide/123.lock");
        write_lock(&path, Path::new("/proj"), "secret").unwrap();
        let mode = std::fs::metadata(&path).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
        let lock: Value = serde_json::from_slice(&std::fs::read(&path).unwrap()).unwrap();
        assert_eq!(lock["transport"], "ws");
        assert_eq!(lock["authToken"], "secret");
        assert_eq!(lock["workspaceFolders"][0], "/proj");
    }
}
