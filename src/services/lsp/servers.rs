use std::path::Path;

use serde_json::Value;

use super::{LspConfig, ServerCommand, choice, command_name};
use crate::config::{RustServer, ServerSettings};
use crate::core::{Language, ServerChoice, ServerKey};
use crate::lang::languages::{Candidate, ServerSpec};
use crate::services::paths;

const STORAGE: &str = "{storage}";

pub fn root_of(server: Language, file: &Path, project: &Path) -> std::path::PathBuf {
    let exists = |path: &Path| path.exists();
    let has_line_starting = |path: &Path, start: &str| {
        std::fs::read_to_string(path)
            .is_ok_and(|text| text.lines().any(|l| l.trim_start().starts_with(start)))
    };
    server
        .server()
        .and_then(|spec| spec.find_root(file, project, &exists, &has_line_starting))
        .unwrap_or_else(|| project.to_path_buf())
}

fn ready_to_run(candidate: &Candidate, command: String, storage: &str) -> ServerCommand {
    ServerCommand {
        command,
        args: candidate
            .args
            .iter()
            .map(|arg| arg.replace(STORAGE, storage))
            .collect(),
        initialization: candidate
            .initialization
            .and_then(|json| serde_json::from_str(json).ok())
            .map(|value| fill_storage(value, storage)),
    }
}

fn fill_storage(value: Value, storage: &str) -> Value {
    match value {
        Value::String(text) => Value::String(text.replace(STORAGE, storage)),
        Value::Array(list) => {
            Value::Array(list.into_iter().map(|v| fill_storage(v, storage)).collect())
        }
        Value::Object(map) => Value::Object(
            map.into_iter()
                .map(|(k, v)| (k, fill_storage(v, storage)))
                .collect(),
        ),
        other => other,
    }
}

fn storage_for(command: &str, root: &Path) -> String {
    let dir = paths::server_storage(command, root);
    let _ = std::fs::create_dir_all(&dir);
    dir.to_string_lossy().into_owned()
}

fn from_table(spec: &ServerSpec, root: &Path) -> Vec<ServerCommand> {
    spec.candidates
        .iter()
        .map(|c| ready_to_run(c, c.command.to_owned(), &storage_for(c.command, root)))
        .collect()
}

fn from_settings(
    spec: &ServerSpec,
    settings: &ServerSettings,
    command: &str,
    root: &Path,
) -> ServerCommand {
    let name = command_name(command);
    let known = spec.candidates.iter().find(|c| c.command == name);
    let mut ready = known.map_or_else(
        || ServerCommand {
            command: command.to_owned(),
            args: Vec::new(),
            initialization: None,
        },
        |c| ready_to_run(c, command.to_owned(), &storage_for(c.command, root)),
    );
    if let Some(args) = &settings.args {
        ready.args.clone_from(args);
    }
    ready
}

pub fn config_for(
    key: &ServerKey,
    choice: &ServerChoice,
    rust: &RustServer,
    settings: Option<&ServerSettings>,
) -> Option<LspConfig> {
    let spec = key.server.server()?;
    if spec.provisioned {
        if !rust.enabled {
            return None;
        }
        let mut config = choice::config_for(choice, key.root.clone(), rust);
        config.server = key.server;
        return Some(config);
    }
    let settings = settings.cloned().unwrap_or_default();
    if !settings.enabled {
        return None;
    }
    let mut config = LspConfig::new(String::new(), key.root.clone());
    config.server = key.server;
    if let Some(command) = &settings.command {
        let ready = from_settings(spec, &settings, command, &key.root);
        config.name = Some(command_name(command));
        config.command = ready.command;
        config.args = ready.args;
        config.initialization = ready.initialization;
    } else {
        config.candidates = from_table(spec, &key.root);
    }
    Some(config)
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use serde_json::json;

    use super::*;
    use crate::lang::languages::{GO_SERVER, LANGUAGES, PHP_SERVER};

    fn key(server: Language) -> ServerKey {
        ServerKey {
            server,
            root: PathBuf::from("/nonexistent/project"),
        }
    }

    fn rust() -> RustServer {
        RustServer {
            command: None,
            args: Vec::new(),
            auto_download: true,
            enabled: true,
        }
    }

    fn config(server: Language, settings: Option<ServerSettings>) -> Option<LspConfig> {
        config_for(
            &key(server),
            &ServerChoice::Automatic,
            &rust(),
            settings.as_ref(),
        )
    }

    #[test]
    fn a_server_with_no_settings_is_looked_for_in_the_tables_order() {
        let python = config(Language::Python, None).unwrap();
        let found: Vec<_> = python
            .candidates
            .iter()
            .map(|c| c.command.as_str())
            .collect();
        assert_eq!(found, ["pyright-langserver", "pylsp"]);
        assert_eq!(python.candidates[0].args, ["--stdio"]);
        assert_eq!(python.server, Language::Python);
    }

    #[test]
    fn folders_are_filled_in_where_a_server_needs_one() {
        let java = config(Language::Java, None).unwrap();
        assert_eq!(java.candidates[0].args[0], "-data");
        assert!(!java.candidates[0].args[1].contains('{'));
        let php = config(Language::Php, None).unwrap();
        let options = php.candidates[0].initialization.as_ref().unwrap();
        let storage = options["storagePath"].as_str().unwrap();
        assert!(storage.contains("intelephense"), "{storage}");
        assert!(!storage.contains('{'));
    }

    #[test]
    fn a_folder_with_quotes_stays_valid_json() {
        let ready = ready_to_run(&PHP_SERVER.candidates[0], "x".into(), "/a\"b");
        let options = ready.initialization.unwrap();
        assert_eq!(options["storagePath"], json!("/a\"b"));
    }

    #[test]
    fn the_users_command_needs_no_finding_and_keeps_what_the_table_knows() {
        let settings = ServerSettings {
            command: Some("/opt/bin/intelephense".to_owned()),
            ..ServerSettings::default()
        };
        let php = config(Language::Php, Some(settings)).unwrap();
        assert!(php.candidates.is_empty());
        assert_eq!(php.command, "/opt/bin/intelephense");
        assert_eq!(php.args, ["--stdio"]);
        assert!(php.initialization.is_some());
        assert_eq!(php.name.as_deref(), Some("intelephense"));
        assert!(!php.check_first, "not all servers answer --version");
    }

    #[test]
    fn the_users_arguments_win_and_an_unknown_program_gets_none_of_the_tables() {
        let settings = ServerSettings {
            command: Some("/opt/bin/gopls".to_owned()),
            args: Some(vec!["serve".to_owned()]),
            enabled: true,
        };
        assert_eq!(
            config(Language::Go, Some(settings)).unwrap().args,
            ["serve"]
        );
        let other = ServerSettings {
            command: Some("mine".to_owned()),
            ..ServerSettings::default()
        };
        let php = config(Language::Php, Some(other)).unwrap();
        assert!(php.args.is_empty());
        assert!(php.initialization.is_none());
    }

    #[test]
    fn enabled_false_means_no_server() {
        let off = ServerSettings {
            enabled: false,
            ..ServerSettings::default()
        };
        assert!(config(Language::Go, Some(off)).is_none());
        let mut no_rust = rust();
        no_rust.enabled = false;
        let none = config_for(
            &key(Language::Rust),
            &ServerChoice::Automatic,
            &no_rust,
            None,
        );
        assert!(none.is_none());
    }

    #[test]
    fn rust_goes_on_through_its_own_automatic_choice() {
        let config = config(Language::Rust, None).unwrap();
        assert!(config.candidates.is_empty());
        assert!(
            config.check_first || std::env::var_os("TTYWRIGHT_RUST_LSP").is_some(),
            "ADR-0008"
        );
        assert_eq!(config.server, Language::Rust);
        assert!(GO_SERVER.candidates.iter().all(|c| !c.install.is_empty()));
    }

    #[test]
    fn every_servers_row_renders_with_no_placeholder_left() {
        for row in LANGUAGES {
            let Some(spec) = row.language.server() else {
                continue;
            };
            for candidate in spec.candidates {
                let ready = ready_to_run(candidate, candidate.command.to_owned(), "/s");
                assert!(!ready.args.iter().any(|a| a.contains('{')), "{}", row.name);
                let options = ready
                    .initialization
                    .map(|o| o.to_string())
                    .unwrap_or_default();
                assert!(!options.contains(STORAGE), "{}", row.name);
            }
        }
    }
}
