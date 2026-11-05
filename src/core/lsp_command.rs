use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::document::Language;
use super::lsp::{
    LspAnswer, LspQuery, LspServerOption, LspState, PositionEncoding, RawDiagnostic, ServerChoice,
    ServerKey,
};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LspEffect {
    FindRoot {
        path: PathBuf,
        server: Language,
        project: PathBuf,
    },
    Start {
        server: ServerKey,
    },
    Restart {
        server: ServerKey,
    },
    DidOpen {
        server: ServerKey,
        language: Language,
        path: PathBuf,
        version: u64,
        text: String,
    },
    DidChange {
        server: ServerKey,
        path: PathBuf,
        version: u64,
        text: String,
    },
    DidSave {
        server: ServerKey,
        path: PathBuf,
    },
    DidClose {
        server: ServerKey,
        path: PathBuf,
    },
    ListServers,
    SelectServer(ServerChoice),
    Query {
        server: ServerKey,
        id: u64,
        path: PathBuf,
        query: LspQuery,
    },
    Cancel {
        server: ServerKey,
        id: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum LspEvent {
    RootFound {
        path: PathBuf,
        server: ServerKey,
    },
    Ready {
        server: ServerKey,
        encoding: PositionEncoding,
        completion_triggers: Vec<String>,
    },
    StateChanged {
        server: ServerKey,
        state: LspState,
    },
    Failed {
        server: ServerKey,
        reason: String,
    },
    ServerNamed {
        server: ServerKey,
        name: String,
    },
    Progress {
        server: ServerKey,
        busy: bool,
    },
    Diagnostics {
        server: ServerKey,
        path: PathBuf,
        items: Vec<RawDiagnostic>,
    },
    ChoiceNotSaved {
        message: String,
    },
    ServersListed {
        options: Vec<LspServerOption>,
    },
    Answered {
        server: ServerKey,
        id: u64,
        result: Result<LspAnswer, String>,
    },
}

impl LspEffect {
    pub const fn needs_trust(&self) -> bool {
        matches!(self, Self::FindRoot { .. } | Self::Start { .. })
    }
}
