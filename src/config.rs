use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use toml::{Table, Value};

use crate::core::db::{DbConnection, DbEndpoint, DbServer};
use crate::core::debug::{AdapterId, ConfiguredDebug, adapter_for_config_name};
use crate::core::{ConfiguredRun, Language};
use crate::lang::languages::{self, LANGUAGES};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Autosave {
    pub enabled: bool,
    pub idle_ms: u64,
    pub on_focus_lost: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct History {
    pub max_age_days: u64,
    pub max_size_mb: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RustServer {
    pub command: Option<String>,
    pub args: Vec<String>,
    pub auto_download: bool,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerSettings {
    pub command: Option<String>,
    pub args: Option<Vec<String>>,
    pub enabled: bool,
}

impl Default for ServerSettings {
    fn default() -> Self {
        Self {
            command: None,
            args: None,
            enabled: true,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct EditorSettings {
    pub tab_width: usize,
    pub soft_wrap: bool,
    pub nerd_icons: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Config {
    pub theme: String,
    pub editor: EditorSettings,
    pub autosave: Autosave,
    pub markdown_preview_on_open: bool,
    pub notifications_popups: bool,
    pub notifications_system: bool,
    pub commit_checks: crate::core::CommitChecks,
    pub history: History,
    pub esc_timeout_ms: Option<u64>,
    pub double_shift: bool,
    pub rust_server: RustServer,
    pub servers: BTreeMap<Language, ServerSettings>,
    pub keybindings: Vec<(String, String)>,
    pub project_keybindings: Vec<(String, String)>,
    pub run: Vec<ConfiguredRun>,
    pub debug_adapters: BTreeMap<AdapterId, Vec<String>>,
    pub debug: Vec<ConfiguredDebug>,
    pub databases: Vec<DbConnection>,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            theme: "darcula".to_owned(),
            commit_checks: crate::core::CommitChecks::default(),
            editor: EditorSettings {
                tab_width: 4,
                soft_wrap: false,
                nerd_icons: false,
            },
            markdown_preview_on_open: true,
            notifications_popups: true,
            notifications_system: true,
            autosave: Autosave {
                enabled: true,
                idle_ms: 1000,
                on_focus_lost: true,
            },
            history: History {
                max_age_days: 14,
                max_size_mb: 256,
            },
            esc_timeout_ms: None,
            double_shift: false,
            rust_server: RustServer {
                command: None,
                args: Vec::new(),
                auto_download: true,
                enabled: true,
            },
            servers: BTreeMap::new(),
            keybindings: Vec::new(),
            project_keybindings: Vec::new(),
            run: Vec::new(),
            debug_adapters: BTreeMap::new(),
            databases: Vec::new(),
            debug: Vec::new(),
        }
    }
}

pub fn esc_wait_ms(configured: Option<u64>, over_ssh: bool) -> u64 {
    configured.unwrap_or(if over_ssh { 75 } else { 25 })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Trust {
    User,
    Project,
}

struct Reader<'a> {
    source: &'a str,
    warnings: &'a mut Vec<String>,
}

impl Reader<'_> {
    fn warn(&mut self, message: String) {
        self.warnings.push(format!("{}: {message}", self.source));
    }

    fn read_bool(&mut self, table: &Table, key: &str, target: &mut bool) {
        match table.get(key) {
            None => {}
            Some(Value::Boolean(value)) => *target = *value,
            Some(_) => self.warn(format!("{key} must be true or false")),
        }
    }

    fn read_number(&mut self, table: &Table, key: &str, range: (u64, u64), target: &mut u64) {
        let in_range = |n: &i64| u64::try_from(*n).is_ok_and(|n| n >= range.0 && n <= range.1);
        match table.get(key) {
            None => {}
            Some(Value::Integer(n)) if in_range(n) => {
                *target = u64::try_from(*n).unwrap_or(*target);
            }
            Some(_) => self.warn(format!(
                "{key} must be a number from {} to {}",
                range.0, range.1
            )),
        }
    }

    fn check_keys(&mut self, table: &Table, section: &str, known: &[&str]) {
        for key in table.keys().filter(|k| !known.contains(&k.as_str())) {
            let name = if section.is_empty() {
                key.clone()
            } else {
                format!("{section}.{key}")
            };
            self.warn(format!("unknown setting {name}"));
        }
    }

    fn section<'t>(&mut self, table: &'t Table, name: &str) -> Option<&'t Table> {
        match table.get(name) {
            None => None,
            Some(Value::Table(section)) => Some(section),
            Some(_) => {
                self.warn(format!("{name} must be a section"));
                None
            }
        }
    }

    fn read_command(
        &mut self,
        table: &Table,
        section: &str,
        command: &mut Option<String>,
        args: &mut Option<Vec<String>>,
    ) {
        match table.get("command") {
            None => {}
            Some(Value::String(text)) if !text.is_empty() => *command = Some(text.clone()),
            Some(_) => self.warn(format!("{section}.command must be text")),
        }
        match table.get("args") {
            None => {}
            Some(Value::Array(list)) if list.iter().all(Value::is_str) => {
                *args = Some(
                    list.iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect(),
                );
            }
            Some(_) => self.warn(format!("{section}.args must be a list of text")),
        }
    }

    fn read_rust_server(&mut self, table: &Table, config: &mut RustServer) {
        self.check_keys(
            table,
            "lsp.rust",
            &["command", "args", "auto_download", "enabled"],
        );
        let mut args = None;
        self.read_command(table, "lsp.rust", &mut config.command, &mut args);
        if let Some(args) = args {
            config.args = args;
        }
        self.read_bool(table, "auto_download", &mut config.auto_download);
        self.read_bool(table, "enabled", &mut config.enabled);
    }

    fn read_servers(&mut self, section: &Table, config: &mut Config) {
        for (name, value) in section {
            let Some(server) = languages::server_for_config_name(name) else {
                let known: Vec<_> = LANGUAGES
                    .iter()
                    .filter(|r| r.lsp.is_some())
                    .map(|r| r.config_name)
                    .collect();
                self.warn(format!(
                    "unknown setting lsp.{name} (known: {})",
                    known.join(", ")
                ));
                continue;
            };
            let full = format!("lsp.{name}");
            let Value::Table(table) = value else {
                self.warn(format!("{full} must be a section"));
                continue;
            };
            if server.provisioned {
                self.read_rust_server(table, &mut config.rust_server);
                continue;
            }
            self.check_keys(table, &full, &["command", "args", "enabled"]);
            if config.servers.contains_key(&server.id) {
                let names = server.config_names().join(", ");
                self.warn(format!(
                    "{full}: {names} are one server, so these settings add to each other"
                ));
            }
            let settings = config.servers.entry(server.id).or_default();
            let (mut command, mut args) = (settings.command.take(), settings.args.take());
            self.read_command(table, &full, &mut command, &mut args);
            let settings = config.servers.entry(server.id).or_default();
            settings.command = command;
            settings.args = args;
            let mut enabled = settings.enabled;
            self.read_bool(table, "enabled", &mut enabled);
            config.servers.entry(server.id).or_default().enabled = enabled;
        }
    }

    fn read_debug(&mut self, table: &Table, config: &mut Config) {
        if let Some(section) = self.section(table, "debug_adapter") {
            for (name, value) in section {
                let id = adapter_for_config_name(name);
                let words = value.get("command").and_then(Value::as_array).map(|c| {
                    c.iter()
                        .filter_map(|w| w.as_str().map(str::to_owned))
                        .collect::<Vec<_>>()
                });
                match (id, words) {
                    (Some(id), Some(words)) if !words.is_empty() => {
                        config.debug_adapters.insert(id, words);
                    }
                    (None, _) => self.warn(format!("unknown setting debug_adapter.{name}")),
                    _ => self.warn(format!(
                        "debug_adapter.{name} needs command = [\"program\", \"argument\"]"
                    )),
                }
            }
        }
        let Some(entries) = table.get("debug") else {
            return;
        };
        let Value::Array(entries) = entries else {
            return self.warn("debug must be a list of [[debug]] sections".to_owned());
        };
        for entry in entries {
            let name = entry.get("name").and_then(Value::as_str);
            let adapter = entry.get("adapter").and_then(Value::as_str);
            let (Some(name), Some(adapter)) = (name, adapter) else {
                self.warn("each [[debug]] needs a name and an adapter".to_owned());
                continue;
            };
            let Some(adapter) = adapter_for_config_name(adapter) else {
                self.warn(format!("debug {name}: unknown adapter {adapter}"));
                continue;
            };
            let request = entry
                .get("request")
                .and_then(Value::as_str)
                .unwrap_or("launch");
            if request != "launch" && request != "attach" {
                self.warn(format!("debug {name}: request must be launch or attach"));
                continue;
            }
            let Ok(settings) =
                serde_json::to_value(entry.get("settings").unwrap_or(&Value::Table(Table::new())))
            else {
                self.warn(format!("debug {name}: settings cannot be used"));
                continue;
            };
            config.debug.push(ConfiguredDebug {
                name: name.to_owned(),
                adapter,
                request: request.to_owned(),
                settings,
                cwd: entry.get("cwd").and_then(Value::as_str).map(str::to_owned),
            });
        }
    }

    fn read_databases(&mut self, value: &Value, config: &mut Config) {
        let Value::Array(entries) = value else {
            return self.warn("database must be a list of [[database]] sections".to_owned());
        };
        for entry in entries {
            let Value::Table(entry) = entry else {
                self.warn("database must be a list of [[database]] sections".to_owned());
                continue;
            };
            if entry.contains_key("password") {
                self.warn(
                    "database: password is not read from a file like this; \
                     use password_env or defaults_file"
                        .to_owned(),
                );
                continue;
            }
            self.check_keys(
                entry,
                "database",
                &[
                    "name",
                    "kind",
                    "path",
                    "host",
                    "port",
                    "user",
                    "database",
                    "password_env",
                    "defaults_file",
                ],
            );
            match read_database(entry) {
                Ok(connection) => config.databases.push(connection),
                Err(message) => self.warn(format!("database: {message}")),
            }
        }
    }

    fn read_run(&mut self, value: &Value, trust: Trust, config: &mut Config) {
        let Value::Array(entries) = value else {
            self.warn("run must be a list of [[run]] sections".to_owned());
            return;
        };
        for entry in entries {
            let Value::Table(entry) = entry else {
                self.warn("run must be a list of [[run]] sections".to_owned());
                continue;
            };
            self.check_keys(entry, "run", &["name", "command", "cwd", "env"]);
            let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_owned);
            let (Some(name), Some(command)) = (
                text("name").filter(|n| !n.is_empty()),
                text("command").filter(|c| !c.trim().is_empty()),
            ) else {
                self.warn("each [[run]] needs a name and a command".to_owned());
                continue;
            };
            if entry.get("cwd").is_some_and(|c| !c.is_str()) {
                self.warn(format!("run {name}: cwd must be text"));
                continue;
            }
            let mut env = Vec::new();
            match entry.get("env") {
                None => {}
                Some(Value::Table(table)) => {
                    for (key, value) in table {
                        let valid = key.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
                            && !key.starts_with(|c: char| c.is_ascii_digit())
                            && !key.is_empty();
                        match value.as_str().filter(|_| valid) {
                            Some(value) => env.push((key.clone(), value.to_owned())),
                            None => self.warn(format!(
                                "run {name}: env {} must be a name and text",
                                crate::core::run::visible(key)
                            )),
                        }
                    }
                }
                Some(_) => {
                    self.warn(format!("run {name}: env must be a table"));
                    continue;
                }
            }
            let command = crate::core::run::with_env(&command, &env);
            config.run.push(ConfiguredRun {
                name,
                command,
                cwd: text("cwd"),
                from_project: trust == Trust::Project,
            });
        }
    }

    fn read_editor(&mut self, table: &Table, config: &mut EditorSettings) {
        self.check_keys(table, "editor", &["tab_width", "soft_wrap", "nerd_icons"]);
        self.read_bool(table, "soft_wrap", &mut config.soft_wrap);
        self.read_bool(table, "nerd_icons", &mut config.nerd_icons);
        let mut tab_width = config.tab_width as u64;
        self.read_number(table, "tab_width", (1, 16), &mut tab_width);
        config.tab_width = usize::try_from(tab_width).unwrap_or(config.tab_width);
    }

    fn read(&mut self, table: &Table, trust: Trust, config: &mut Config) {
        self.check_keys(
            table,
            "",
            &[
                "theme",
                "editor",
                "autosave",
                "history",
                "input",
                "lsp",
                "keybindings",
                "run",
                "debug",
                "debug_adapter",
                "database",
                "commit_checks",
                "markdown",
                "notifications",
            ],
        );
        match table.get("theme") {
            None => {}
            Some(Value::String(theme)) => config.theme.clone_from(theme),
            Some(_) => self.warn("theme must be text".to_owned()),
        }
        if let Some(section) = self.section(table, "editor") {
            self.read_editor(section, &mut config.editor);
        }
        if let Some(section) = self.section(table, "markdown") {
            self.check_keys(section, "markdown", &["preview_on_open"]);
            self.read_bool(
                section,
                "preview_on_open",
                &mut config.markdown_preview_on_open,
            );
        }
        if let Some(section) = self.section(table, "notifications") {
            self.check_keys(section, "notifications", &["popups", "system"]);
            self.read_bool(section, "popups", &mut config.notifications_popups);
            self.read_bool(section, "system", &mut config.notifications_system);
        }
        if let Some(section) = self.section(table, "autosave") {
            self.check_keys(
                section,
                "autosave",
                &["enabled", "idle_ms", "on_focus_lost"],
            );
            let autosave = &mut config.autosave;
            self.read_bool(section, "enabled", &mut autosave.enabled);
            self.read_number(section, "idle_ms", (100, 600_000), &mut autosave.idle_ms);
            self.read_bool(section, "on_focus_lost", &mut autosave.on_focus_lost);
        }
        if let Some(section) = self.section(table, "history") {
            self.check_keys(section, "history", &["max_age_days", "max_size_mb"]);
            let history = &mut config.history;
            self.read_number(
                section,
                "max_age_days",
                (1, 3650),
                &mut history.max_age_days,
            );
            self.read_number(
                section,
                "max_size_mb",
                (1, 1_048_576),
                &mut history.max_size_mb,
            );
        }
        if let Some(section) = self.section(table, "input") {
            self.check_keys(section, "input", &["esc_timeout_ms", "double_shift"]);
            self.read_bool(section, "double_shift", &mut config.double_shift);
            let mut wait = config.esc_timeout_ms.unwrap_or(0);
            self.read_number(section, "esc_timeout_ms", (5, 2000), &mut wait);
            config.esc_timeout_ms = Some(wait).filter(|w| *w > 0);
        }
        if let Some(section) = self.section(table, "lsp") {
            if trust == Trust::Project {
                self.warn("lsp is ignored here: a project must not choose programs to run".into());
            } else {
                self.read_servers(section, config);
            }
        }
        if let Some(run) = table.get("run") {
            self.read_run(run, trust, config);
        }
        if let Some(section) = self.section(table, "commit_checks") {
            if trust == Trust::Project {
                self.warn(
                    "commit_checks is ignored here: a project must not choose what runs".into(),
                );
            } else {
                self.check_keys(section, "commit_checks", &["todo", "problems", "command"]);
                self.read_bool(section, "todo", &mut config.commit_checks.todo);
                self.read_bool(section, "problems", &mut config.commit_checks.problems);
                match section.get("command") {
                    None => {}
                    Some(Value::String(command)) if !command.trim().is_empty() => {
                        config.commit_checks.command = Some(command.clone());
                    }
                    Some(_) => self.warn("command must be text".to_owned()),
                }
            }
        }
        if let Some(databases) = table.get("database") {
            if trust == Trust::Project {
                self.warn(
                    "database is ignored here: a project must not choose where queries go".into(),
                );
            } else {
                self.read_databases(databases, config);
            }
        }
        if table.contains_key("debug") || table.contains_key("debug_adapter") {
            if trust == Trust::Project {
                self.warn(
                    "debug is ignored here: a project must not choose programs to run".into(),
                );
            } else {
                self.read_debug(table, config);
            }
        }
        if let Some(section) = self.section(table, "keybindings") {
            for (chord, action) in section {
                match action {
                    Value::String(action) => {
                        match trust {
                            Trust::Project => &mut config.project_keybindings,
                            Trust::User => &mut config.keybindings,
                        }
                        .push((chord.clone(), action.clone()));
                    }
                    _ => self.warn(format!("keybindings.{chord} must be an action name")),
                }
            }
        }
    }
}

fn read_file(file: &Path, trust: Trust, config: &mut Config, warnings: &mut Vec<String>) {
    let Ok(text) = std::fs::read_to_string(file) else {
        return;
    };
    let source = match trust {
        Trust::User => "config.toml",
        Trust::Project => ".ttywright/config.toml",
    };
    read_text(&text, source, trust, config, warnings);
}

fn read_text(
    text: &str,
    source: &str,
    trust: Trust,
    config: &mut Config,
    warnings: &mut Vec<String>,
) {
    let mut reader = Reader { source, warnings };
    match text.parse::<Table>() {
        Ok(table) => reader.read(&table, trust, config),
        Err(e) => {
            let first = e.message().lines().next().unwrap_or("invalid TOML");
            reader.warn(first.to_owned());
        }
    }
}

pub fn load(user_file: Option<&Path>, project_root: &Path) -> (Config, Vec<String>) {
    let mut config = Config::default();
    let mut warnings = Vec::new();
    if let Some(file) = user_file {
        read_file(file, Trust::User, &mut config, &mut warnings);
    }
    let project_file = project_root.join(".ttywright/config.toml");
    read_file(&project_file, Trust::Project, &mut config, &mut warnings);
    (config, warnings)
}

fn absolute_path(text: &str) -> Result<PathBuf, String> {
    let path = match text.strip_prefix("~/") {
        Some(rest) => std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join(rest))
            .ok_or("HOME is not set")?,
        None => PathBuf::from(text),
    };
    if path.is_absolute() {
        Ok(path)
    } else {
        Err(format!("{text} must be an absolute path or start with ~/"))
    }
}

fn read_database(entry: &Table) -> Result<DbConnection, String> {
    let text = |key: &str| entry.get(key).and_then(Value::as_str).map(str::to_owned);
    let name = text("name").ok_or("each [[database]] needs a name")?;
    let in_name = |message: String| format!("{name}: {message}");
    let server = || -> Result<DbServer, String> {
        let port = match entry.get("port") {
            None => None,
            Some(value) => Some(
                value
                    .as_integer()
                    .and_then(|p| u16::try_from(p).ok())
                    .ok_or("port must be a number up to 65535")?,
            ),
        };
        Ok(DbServer {
            host: text("host"),
            port,
            user: text("user"),
            database: text("database"),
        })
    };
    let kind = text("kind").ok_or_else(|| in_name("kind is missing".to_owned()))?;
    let endpoint = match kind.as_str() {
        "sqlite" => {
            let path = text("path").ok_or_else(|| in_name("path is missing".to_owned()))?;
            DbEndpoint::Sqlite {
                path: absolute_path(&path).map_err(in_name)?,
            }
        }
        "postgres" => DbEndpoint::Postgres {
            server: server().map_err(in_name)?,
            password_env: text("password_env"),
        },
        "mysql" => DbEndpoint::Mysql {
            server: server().map_err(in_name)?,
            defaults_file: text("defaults_file")
                .map(|f| absolute_path(&f))
                .transpose()
                .map_err(in_name)?,
        },
        other => {
            return Err(in_name(format!(
                "unknown kind {other} (known: sqlite, postgres, mysql)"
            )));
        }
    };
    Ok(DbConnection { name, endpoint })
}

pub fn config_dir() -> Option<PathBuf> {
    let dirs = directories::ProjectDirs::from("", "", "ttywright")?;
    Some(dirs.config_dir().to_path_buf())
}

#[cfg(test)]
mod tests {
    #[test]
    fn a_debug_entry_and_an_adapter_are_read_from_the_users_own_file() {
        let text = r#"
[debug_adapter.php]
command = ["node", "/opt/phpDebug.js"]

[[debug]]
name = "Docker"
adapter = "php"
[debug.settings]
port = 9003
pathMappings = { "/var/www" = "/home/me/site" }
"#;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        std::fs::write(&file, text).unwrap();
        let mut config = Config::default();
        let mut warnings = Vec::new();
        read_file(&file, Trust::User, &mut config, &mut warnings);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(
            config.debug_adapters[&AdapterId::PhpDebug],
            ["node", "/opt/phpDebug.js"]
        );
        assert_eq!(config.debug[0].name, "Docker");
        assert_eq!(config.debug[0].request, "launch");
        assert_eq!(config.debug[0].settings["port"], 9003);
    }

    #[test]
    fn a_project_cannot_choose_what_the_debugger_runs() {
        let text = "[debug_adapter.python]\ncommand = [\"./evil\"]\n[[debug]]\nname = \"x\"\nadapter = \"python\"\n";
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("config.toml");
        std::fs::write(&file, text).unwrap();
        let mut config = Config::default();
        let mut warnings = Vec::new();
        read_file(&file, Trust::Project, &mut config, &mut warnings);
        assert!(config.debug_adapters.is_empty());
        assert!(config.debug.is_empty());
        assert!(
            warnings.iter().any(|w| w.contains("debug is ignored")),
            "{warnings:?}"
        );
    }

    use super::*;

    fn parse(text: &str, trust: Trust) -> (Config, Vec<String>) {
        let mut config = Config::default();
        let mut warnings = Vec::new();
        read_text(text, "f", trust, &mut config, &mut warnings);
        (config, warnings)
    }

    #[test]
    fn run_commands_are_read_and_marked_with_their_source() {
        let text = r#"
            [[run]]
            name = "serve"
            command = "php -S localhost:8000"
            cwd = "public"
            [[run]]
            name = "lint"
            command = "eslint ."
        "#;
        let (config, warnings) = parse(text, Trust::User);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.run.len(), 2);
        assert_eq!(config.run[0].name, "serve");
        assert_eq!(config.run[0].command, "php -S localhost:8000");
        assert_eq!(config.run[0].cwd.as_deref(), Some("public"));
        assert_eq!(config.run[1].cwd, None);
        assert!(config.run.iter().all(|r| !r.from_project));

        let (config, warnings) = parse(text, Trust::Project);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(config.run.iter().all(|r| r.from_project), "never trusted");
    }

    #[test]
    fn bad_run_entries_warn_and_are_left_out() {
        for text in [
            "run = 3",
            "run = [1]",
            "[[run]]\nname = \"x\"",
            "[[run]]\ncommand = \"x\"",
            "[[run]]\nname = \"\"\ncommand = \"x\"",
            "[[run]]\nname = \"x\"\ncommand = \"  \"",
            "[[run]]\nname = \"x\"\ncommand = 3",
            "[[run]]\nname = \"x\"\ncommand = \"y\"\ncwd = 3",
            "[[run]]\nname = \"x\"\ncommand = \"y\"\nenv = \"z\"",
        ] {
            let (config, warnings) = parse(text, Trust::User);
            assert!(config.run.is_empty(), "{text}");
            assert!(!warnings.is_empty(), "{text}");
        }
        let (config, warnings) = parse(
            "[[run]]\nname = \"x\"\ncommand = \"y\"\nother = \"z\"",
            Trust::User,
        );
        assert_eq!(config.run.len(), 1);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn every_setting_is_read() {
        let (config, warnings) = parse(
            r#"
            theme = "light"
            [editor]
            tab_width = 2
            soft_wrap = true
            nerd_icons = true
            [autosave]
            enabled = false
            idle_ms = 2500
            on_focus_lost = false
            [history]
            max_age_days = 30
            max_size_mb = 64
            [input]
            esc_timeout_ms = 200
            double_shift = true
            [lsp.rust]
            command = "ra"
            args = ["--x"]
            auto_download = false
            [keybindings]
            "ctrl+j" = "lsp.definition"
            "#,
            Trust::User,
        );
        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(config.theme, "light");
        assert_eq!(config.editor.tab_width, 2);
        assert!(config.editor.soft_wrap);
        assert!(config.editor.nerd_icons);
        assert!(!config.autosave.enabled && !config.autosave.on_focus_lost);
        assert_eq!(config.autosave.idle_ms, 2500);
        assert_eq!(config.history.max_age_days, 30);
        assert_eq!(config.history.max_size_mb, 64);
        assert_eq!(config.esc_timeout_ms, Some(200));
        assert!(config.double_shift);
        assert_eq!(config.rust_server.command.as_deref(), Some("ra"));
        assert_eq!(config.rust_server.args, ["--x"]);
        assert!(!config.rust_server.auto_download);
        assert_eq!(
            config.keybindings,
            [("ctrl+j".to_owned(), "lsp.definition".to_owned())]
        );
    }

    #[test]
    fn a_bad_value_warns_and_keeps_the_default_and_the_rest() {
        let (config, warnings) = parse(
            "theme = 3\n[autosave]\nidle_ms = 1\nenabled = false\n[history]\nmax_size_mb = \"x\"",
            Trust::User,
        );
        assert_eq!(config.theme, "darcula");
        assert_eq!(config.autosave.idle_ms, 1000);
        assert!(!config.autosave.enabled, "the good value next to it counts");
        assert_eq!(config.history.max_size_mb, 256);
        assert_eq!(warnings.len(), 3, "{warnings:?}");
        assert!(warnings[1].contains("idle_ms"));
    }

    #[test]
    fn unknown_settings_are_named() {
        let (_, warnings) = parse(
            "colour = 1\n[autosave]\nidle = 1\n[lsp.rust]\nx = 1",
            Trust::User,
        );
        let joined = warnings.join("\n");
        assert!(joined.contains("unknown setting colour"), "{joined}");
        assert!(joined.contains("unknown setting autosave.idle"), "{joined}");
        assert!(joined.contains("unknown setting lsp.rust.x"), "{joined}");
    }

    #[test]
    fn broken_toml_means_all_defaults_and_one_warning() {
        let (config, warnings) = parse("theme = = \"x\"", Trust::User);
        assert_eq!(config, Config::default());
        assert_eq!(warnings.len(), 1);
        assert!(warnings[0].starts_with("f: "));
    }

    #[test]
    fn a_project_cannot_name_a_database_and_a_user_can() {
        let dir = tempfile::tempdir().unwrap();
        let user = dir.path().join("config.toml");
        let project = dir.path().join("p");
        std::fs::create_dir_all(project.join(".ttywright")).unwrap();
        std::fs::write(
            project.join(".ttywright/config.toml"),
            "[[database]]\nname = \"evil\"\nkind = \"sqlite\"\npath = \"/x\"\n",
        )
        .unwrap();
        std::fs::write(
            &user,
            r#"
            [[database]]
            name = "local"
            kind = "sqlite"
            path = "/data/a.db"
            [[database]]
            name = "prod"
            kind = "postgres"
            host = "h"
            port = 5433
            password_env = "PROD_PW"
            [[database]]
            name = "plain"
            kind = "mysql"
            password = "hunter2"
            [[database]]
            name = "relative"
            kind = "sqlite"
            path = "a.db"
            "#,
        )
        .unwrap();
        let (config, warnings) = load(Some(&user), &project);
        let names: Vec<_> = config.databases.iter().map(|d| d.name.as_str()).collect();
        assert_eq!(names, ["local", "prod"]);
        let joined = warnings.join("\n");
        assert!(
            joined.contains("must not choose where queries go"),
            "{joined}"
        );
        assert!(joined.contains("password_env"), "{joined}");
        assert!(!joined.contains("hunter2"), "{joined}");
        assert!(joined.contains("absolute"), "{joined}");
    }

    #[test]
    fn a_project_cannot_choose_a_program_to_run() {
        let text = "theme = \"light\"\n[lsp.rust]\ncommand = \"./evil\"\nauto_download = false";
        let (config, warnings) = parse(text, Trust::Project);
        assert_eq!(config.rust_server.command, None);
        assert!(config.rust_server.auto_download);
        assert_eq!(config.theme, "light", "the rest of its file still counts");
        assert!(warnings[0].contains("must not choose programs"));
    }

    #[test]
    fn the_project_file_goes_on_top_of_the_users() {
        let dir = tempfile::tempdir().expect("temp dir");
        let user = dir.path().join("user.toml");
        std::fs::write(&user, "theme = \"light\"\n[autosave]\nidle_ms = 500").expect("writes");
        let project = dir.path().join(".ttywright");
        std::fs::create_dir(&project).expect("creates");
        std::fs::write(project.join("config.toml"), "[autosave]\nidle_ms = 700").expect("writes");
        let (config, warnings) = load(Some(&user), dir.path());
        assert!(warnings.is_empty());
        assert_eq!(config.theme, "light");
        assert_eq!(config.autosave.idle_ms, 700);
    }

    #[test]
    fn esc_waits_less_locally_than_over_ssh_and_the_setting_wins() {
        assert_eq!(esc_wait_ms(None, false), 25);
        assert_eq!(esc_wait_ms(None, true), 75);
        assert_eq!(esc_wait_ms(Some(200), false), 200);
        assert_eq!(esc_wait_ms(Some(5), true), 5);
        let (config, warnings) = parse("[input]\nesc_timeout_ms = 1", Trust::User);
        assert_eq!(config.esc_timeout_ms, None, "out of range");
        assert_eq!(warnings.len(), 1);
    }

    #[test]
    fn commit_checks_are_off_by_default_and_never_from_a_project() {
        assert!(!Config::default().commit_checks.any());
        let text = "[commit_checks]\ntodo = true\nproblems = true\ncommand = \"make check\"";
        let (config, warnings) = parse(text, Trust::User);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert!(config.commit_checks.todo && config.commit_checks.problems);
        assert_eq!(config.commit_checks.command.as_deref(), Some("make check"));
        let (config, warnings) = parse(text, Trust::Project);
        assert!(!config.commit_checks.any());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    #[test]
    fn tab_width_defaults_to_4_and_rejects_what_would_divide_by_zero() {
        assert_eq!(Config::default().editor.tab_width, 4);
        assert!(!Config::default().editor.soft_wrap);
        for bad in ["0", "17", "-1", "\"x\""] {
            let (config, warnings) = parse(&format!("[editor]\ntab_width = {bad}"), Trust::User);
            assert_eq!(config.editor.tab_width, 4, "{bad}");
            assert_eq!(warnings.len(), 1, "{bad}: {warnings:?}");
        }
        let (config, _) = parse("[editor]\ntab_width = 8", Trust::Project);
        assert_eq!(config.editor.tab_width, 8, "a project may set it");
    }

    #[test]
    fn no_files_mean_defaults_without_warnings() {
        let dir = tempfile::tempdir().expect("temp dir");
        let (config, warnings) = load(Some(&dir.path().join("none.toml")), dir.path());
        assert_eq!(config, Config::default());
        assert!(warnings.is_empty());
    }

    #[test]
    fn each_language_can_set_its_server_and_javascript_means_typescripts() {
        let text = r#"
            [lsp.go]
            command = "/opt/gopls"
            args = ["serve"]
            [lsp.php]
            enabled = false
            [lsp.javascript]
            command = "mine"
        "#;
        let (config, warnings) = parse(text, Trust::User);
        assert!(warnings.is_empty(), "{warnings:?}");
        let go = &config.servers[&Language::Go];
        assert_eq!(go.command.as_deref(), Some("/opt/gopls"));
        assert_eq!(go.args.as_deref(), Some(&["serve".to_owned()][..]));
        assert!(go.enabled);
        assert!(!config.servers[&Language::Php].enabled);
        let shared = &config.servers[&Language::TypeScript];
        assert_eq!(shared.command.as_deref(), Some("mine"));
    }

    #[test]
    fn rust_keeps_its_settings_and_gains_enabled() {
        let text =
            "[lsp.rust]\ncommand = \"ra\"\nargs = [\"a\"]\nauto_download = false\nenabled = false";
        let (config, warnings) = parse(text, Trust::User);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(config.rust_server.command.as_deref(), Some("ra"));
        assert_eq!(config.rust_server.args, ["a"]);
        assert!(!config.rust_server.auto_download);
        assert!(!config.rust_server.enabled);
    }

    #[test]
    fn what_is_wrong_in_a_server_section_is_a_warning() {
        let text = "[lsp.go]\nauto_download = false\nargs = 1\ncommand = 2\nenabled = \"no\"\n[lsp.toml]\n[lsp.cobol]\n[lsp]\npython = 1";
        let (_, warnings) = parse(text, Trust::User);
        let joined = warnings.join("\n");
        for expected in [
            "unknown setting lsp.go.auto_download",
            "lsp.go.args must be a list of text",
            "lsp.go.command must be text",
            "enabled must be true or false",
            "unknown setting lsp.toml",
            "unknown setting lsp.cobol",
            "lsp.python must be a section",
        ] {
            assert!(joined.contains(expected), "{expected} in {joined}");
        }
    }

    #[test]
    fn a_second_section_of_one_server_is_pointed_out() {
        let text = "[lsp.javascript]\ncommand = \"a\"\n[lsp.typescript]\nargs = [\"b\"]";
        let (config, warnings) = parse(text, Trust::User);
        assert!(warnings.join("").contains("are one server"), "{warnings:?}");
        let shared = &config.servers[&Language::TypeScript];
        assert_eq!(shared.command.as_deref(), Some("a"));
        assert_eq!(shared.args.as_deref(), Some(&["b".to_owned()][..]));
    }

    #[test]
    fn a_project_cannot_choose_the_command_of_any_server() {
        let text = "[lsp.go]\ncommand = \"./evil\"\n[lsp.php]\nargs = [\"x\"]";
        let (config, warnings) = parse(text, Trust::Project);
        assert!(config.servers.is_empty(), "{:?}", config.servers);
        assert!(
            warnings[0].contains("a project must not choose"),
            "{warnings:?}"
        );
    }

    #[test]
    fn a_run_s_env_becomes_part_of_the_command_shown() {
        let (config, warnings) = parse(
            "[[run]]\nname = \"a\"\ncommand = \"go\"\nenv = { MODE = \"it's\", \"BAD-NAME\" = \"x\" }\n",
            Trust::User,
        );
        assert_eq!(config.run[0].command, r"export MODE='it'\''s'; go");
        assert!(
            warnings.iter().any(|w| w.contains("BAD-NAME")),
            "{warnings:?}"
        );
    }
}
