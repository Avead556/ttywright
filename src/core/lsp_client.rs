use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::document::Language;
use super::lsp::{LspState, PositionEncoding, ServerKey, ServerStatus};
use super::workspace::{Cursor, ViewId};

const MAX_FAILURE_CHARS: usize = 300;

#[derive(Debug, Clone)]
pub struct PendingQuery {
    pub server: ServerKey,
    pub id: u64,
    pub view: ViewId,
    pub version: u64,
    pub cursor: Cursor,
    pub completion: bool,
    pub quiet: bool,
}

#[derive(Debug)]
pub struct LspClient {
    pub(super) servers: BTreeMap<ServerKey, ServerStatus>,
    pub(super) pending: Option<PendingQuery>,
    roots_asked: BTreeSet<PathBuf>,
    last_asked: Option<ServerKey>,
    next_id: u64,
}

impl Default for LspClient {
    fn default() -> Self {
        Self {
            servers: BTreeMap::new(),
            pending: None,
            roots_asked: BTreeSet::new(),
            last_asked: None,
            next_id: 1,
        }
    }
}

impl LspClient {
    pub fn next_id(&mut self) -> u64 {
        let id = self.next_id;
        self.next_id += 1;
        id
    }

    pub fn asked_by(&mut self, server: &ServerKey) {
        self.last_asked = Some(server.clone());
    }

    pub fn root_asked(&self, path: &Path) -> bool {
        self.roots_asked.contains(path)
    }

    pub fn ask_root(&mut self, path: PathBuf) {
        self.roots_asked.insert(path);
    }

    pub fn root_answered(&mut self, path: &Path) {
        self.roots_asked.remove(path);
    }

    pub fn session_for_outside_file(&self, server: Language, root: &Path) -> ServerKey {
        self.last_asked
            .as_ref()
            .filter(|k| k.server == server)
            .or_else(|| self.servers.keys().find(|k| k.server == server))
            .cloned()
            .unwrap_or_else(|| ServerKey {
                server,
                root: root.to_path_buf(),
            })
    }

    pub fn drop_query_of(&mut self, server: &ServerKey) {
        if self.pending.as_ref().is_some_and(|p| p.server == *server) {
            self.pending = None;
        }
    }

    pub fn ready(
        &mut self,
        server: ServerKey,
        encoding: PositionEncoding,
        completion_triggers: Vec<String>,
    ) {
        let status = self.servers.entry(server).or_default();
        status.failure = None;
        status.state = LspState::Ready;
        status.busy = false;
        status.encoding = encoding;
        status.completion_triggers = completion_triggers;
        status.open.clear();
    }

    pub fn state_changed(&mut self, server: &ServerKey, state: LspState) {
        self.drop_query_of(server);
        if let Some(status) = self.servers.get_mut(server) {
            status.state = state;
            status.busy = false;
        }
    }

    pub fn failed(&mut self, server: &ServerKey, reason: &str) {
        if let Some(status) = self.servers.get_mut(server) {
            let shown = reason.chars().filter(|c| !c.is_control());
            status.failure = Some(shown.take(MAX_FAILURE_CHARS).collect());
        }
    }

    pub fn named(&mut self, server: &ServerKey, name: String) {
        if let Some(status) = self.servers.get_mut(server) {
            status.name = Some(name);
        }
    }

    pub fn progress(&mut self, server: &ServerKey, busy: bool) {
        if let Some(status) = self.servers.get_mut(server) {
            status.busy = busy;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn key(server: Language, root: &str) -> ServerKey {
        ServerKey {
            server,
            root: PathBuf::from(root),
        }
    }

    #[test]
    fn a_file_outside_the_project_goes_to_the_session_last_asked() {
        let mut client = LspClient::default();
        let lib = key(Language::Rust, "/lib");
        client.ready(
            key(Language::Rust, "/p"),
            PositionEncoding::Utf16,
            Vec::new(),
        );
        client.ready(lib.clone(), PositionEncoding::Utf16, Vec::new());
        client.asked_by(&lib);
        assert_eq!(
            client.session_for_outside_file(Language::Rust, Path::new("/p")),
            lib
        );
        assert_eq!(
            client.session_for_outside_file(Language::Python, Path::new("/p")),
            key(Language::Python, "/p")
        );
    }

    #[test]
    fn a_failure_shown_has_no_control_characters_and_a_limit() {
        let mut client = LspClient::default();
        let server = key(Language::Rust, "/p");
        client.ready(server.clone(), PositionEncoding::Utf16, Vec::new());
        client.failed(&server, &format!("bad\u{1b}[31m{}", "x".repeat(500)));
        let failure = client.servers[&server].failure.clone().unwrap_or_default();
        assert!(!failure.contains('\u{1b}'));
        assert_eq!(failure.chars().count(), MAX_FAILURE_CHARS);
        assert_ne!(client.next_id(), client.next_id());
    }
}
