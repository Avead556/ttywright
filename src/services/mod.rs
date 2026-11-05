pub mod analysis;
pub mod cli;
pub mod clipboard;
pub mod coverage;
pub mod db;
pub mod debug;
pub mod duplicates;
pub mod file_op;
pub mod format;
pub mod fs;
pub mod git;
mod git_compare;
mod git_history;
pub mod git_ops;
pub mod git_stage;
pub mod history;
pub mod http;
pub mod ide;
pub mod image;
pub mod index;
pub mod ipc;
pub mod lsp;
pub mod mcp;
pub mod paths;
pub mod plugin;
pub mod replace;
pub mod run;
pub mod search;
pub mod session;
pub mod spell;
pub mod swap;
pub mod terminal;
pub mod user_data;
pub mod watcher;

use crate::core::git_command::GitEffect;
use crate::core::lsp_command::{LspEffect, LspEvent};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};
use std::thread;

use notify::RecommendedWatcher;

use self::history::{HistoryLimits, HistoryWorker};
use self::lsp::{LspHandle, LspSessions};
use self::paths::ProjectData;
use self::run::RunHost;
use self::swap::SwapWorker;
use crate::config::{Config, RustServer, ServerSettings};
use crate::core::debug::{DebugEffect, DebugEvent};
use crate::core::run_state::{RunEffect, RunEvent};
use crate::core::{
    Effect, Event, Language, LspState, ServerChoice, ServerKey, SnapshotLabel, WriteGuard,
    WriteOrigin,
};

pub struct Services<F> {
    emit: F,
    images: Option<image::Decoder>,
    watcher: Option<RecommendedWatcher>,
    swaps: Result<SwapWorker, String>,
    session: Option<session::SessionStore>,
    history: Option<HistoryWorker>,
    lsp: LspSessions,
    server_choice_file: Option<PathBuf>,
    lsp_choice_now: Option<ServerChoice>,
    rust_server: RustServer,
    server_settings: BTreeMap<Language, ServerSettings>,
    runner: RunHost,
    debugger: debug::DebugHost,
    debug_adapters: BTreeMap<crate::core::debug::AdapterId, Vec<String>>,
    run_hub: ipc::RunHub,
    mcp_calls: ipc::McpCalls,
    ide: Option<ide::IdeServer>,
    claude: Option<terminal::PtyProcess>,
    terminals: terminal::TerminalHost,
    plugins: Vec<plugin::LoadedPlugin>,
    analyses: analysis::AnalysisHost,
    http: http::HttpHost,
    duplicates: duplicates::DuplicatesHost,
    db: db::DbHost,
    index: Option<index::ProjectIndex>,
    index_file: Option<PathBuf>,
}

fn analysis_grants() -> Option<PathBuf> {
    Some(crate::config::config_dir()?.join("analysis-grants.json"))
}

fn plugin_dir() -> Option<PathBuf> {
    Some(crate::config::config_dir()?.join("plugins"))
}

impl<F: Fn(Event) + Clone + Send + 'static> Services<F> {
    fn plugins_load(&mut self) {
        let (plugins, problems) =
            plugin_dir().map_or_else(Default::default, |dir| plugin::load(&dir));
        let plugins_info = plugins.iter().map(|p| p.info.clone()).collect();
        self.plugins = plugins;
        (self.emit)(Event::PluginsLoaded {
            plugins: plugins_info,
            problems,
        });
    }

    pub fn new(emit: F, data: Option<ProjectData>, config: &Config) -> Self {
        let limits = HistoryLimits {
            max_age_secs: config.history.max_age_days * 24 * 60 * 60,
            max_size_bytes: config.history.max_size_mb * 1024 * 1024,
        };
        let swaps = data
            .as_ref()
            .ok_or_else(|| "no data directory".to_owned())
            .and_then(|d| {
                SwapWorker::start(d.swap_dir.clone(), emit.clone()).map_err(|e| e.to_string())
            });
        let history = data
            .as_ref()
            .map(|d| HistoryWorker::start(d.history_dir.clone(), limits, emit.clone()));
        Self {
            emit,
            images: None,
            watcher: None,
            swaps,
            server_choice_file: data.as_ref().map(|d| d.server_choice_file.clone()),
            index_file: data.as_ref().map(|d| d.index_file.clone()),
            index: None,
            session: data.map(|d| d.session),
            history,
            lsp: LspSessions::default(),
            lsp_choice_now: None,
            rust_server: config.rust_server.clone(),
            server_settings: config.servers.clone(),
            runner: RunHost::default(),
            debugger: debug::DebugHost::default(),
            debug_adapters: config.debug_adapters.clone(),
            run_hub: ipc::RunHub::default(),
            mcp_calls: ipc::McpCalls::default(),
            ide: None,
            claude: None,
            terminals: terminal::TerminalHost::default(),
            plugins: Vec::new(),
            analyses: analysis::AnalysisHost::default(),
            http: http::HttpHost::default(),
            duplicates: duplicates::DuplicatesHost::default(),
            db: db::DbHost::default(),
        }
    }

    pub fn start_ide(&mut self, root: &std::path::Path) {
        self.ide = ide::IdeServer::start(root, self.mcp_calls.clone(), self.emit.clone()).ok();
    }

    pub fn mcp_calls(&self) -> ipc::McpCalls {
        self.mcp_calls.clone()
    }

    pub fn run_hub(&self) -> ipc::RunHub {
        self.run_hub.clone()
    }

    pub fn use_launcher(&mut self, program: Option<PathBuf>) {
        self.debugger.use_launcher(program.clone());
        self.runner.use_launcher(program);
    }

    fn server_choice(&self) -> ServerChoice {
        self.lsp_choice_now.clone().unwrap_or_else(|| {
            self.server_choice_file
                .as_deref()
                .map(lsp::choice::load)
                .unwrap_or_default()
        })
    }

    fn start_lsp(&mut self, key: ServerKey) -> Option<Event> {
        let config = lsp::servers::config_for(
            &key,
            &self.server_choice(),
            &self.rust_server,
            self.server_settings.get(&key.server),
        );
        let state = |state| {
            Some(Event::Lsp(LspEvent::StateChanged {
                server: key.clone(),
                state,
            }))
        };
        let Some(config) = config else {
            return state(LspState::Disabled);
        };
        match LspHandle::start(config, self.emit.clone()) {
            Ok(handle) => {
                self.lsp.insert(key, handle);
                None
            }
            Err(_) => state(LspState::Crashed),
        }
    }

    pub fn run(&mut self, effect: Effect) -> Option<Event> {
        let send = self.emit.clone();
        match effect {
            Effect::ReadFile { path, at } => {
                let event = fs::read(path, at);
                if let Event::FileRead {
                    path,
                    result: Ok(Some(bytes)),
                    ..
                } = &event
                    && !crate::core::image_art::is_image(path, bytes)
                {
                    self.snapshot(path, bytes, SnapshotLabel::Opened);
                }
                Some(event)
            }
            Effect::WriteFile {
                doc,
                version,
                path,
                bytes,
                guard,
                origin,
            } => {
                if guard == WriteGuard::Overwrite {
                    self.snapshot_file(&path, SnapshotLabel::BeforeOverwrite);
                }
                let event = fs::write(doc, version, &path, &bytes, guard, origin);
                match &event {
                    Event::FileWritten { .. } => {
                        let label = match origin {
                            WriteOrigin::User => SnapshotLabel::Save,
                            WriteOrigin::Auto => SnapshotLabel::Autosave,
                        };
                        self.snapshot(&path, &bytes, label);
                    }
                    Event::WriteConflict { .. } => {
                        self.snapshot_file(&path, SnapshotLabel::ExternalChange);
                    }
                    _ => {}
                }
                Some(event)
            }
            Effect::CheckFile { doc, path, known } => {
                let event = fs::check(doc, &path, known);
                if let Some(Event::FileChanged {
                    result: Ok(Some(bytes)),
                    ..
                }) = &event
                {
                    self.snapshot(&path, bytes, SnapshotLabel::ExternalChange);
                }
                event
            }
            Effect::ReloadFile { doc, path } => {
                let event = fs::reload(doc, &path);
                if let Event::FileReloaded {
                    result: Ok(Some(bytes)),
                    ..
                } = &event
                {
                    self.snapshot(&path, bytes, SnapshotLabel::ExternalChange);
                }
                Some(event)
            }
            Effect::Snapshot { path, bytes, label } => {
                self.snapshot(&path, &bytes, label);
                None
            }
            Effect::ListHistory { doc, path } => match &self.history {
                Some(history) => {
                    history.list(doc, path);
                    None
                }
                None => Some(Event::HistoryFailed {
                    message: "no data directory".to_owned(),
                }),
            },
            Effect::ReadHistoryVersion { hash } => {
                if let Some(history) = &self.history {
                    history.read(hash);
                }
                None
            }
            Effect::ListDir { path } => Some(fs::list_dir(path)),
            Effect::McpReadFiles { id, call, paths } => {
                thread::spawn(move || send(mcp_read_files(id, call, paths)));
                None
            }
            Effect::McpReply { id, result } => {
                let calls = self.mcp_calls.clone();
                thread::spawn(move || calls.answer(id, result));
                None
            }
            Effect::OpenClaude { root } => {
                thread::spawn(move || {
                    send(Event::ClaudeOpened {
                        result: ipc::open_claude(&root),
                    });
                });
                None
            }
            Effect::ClaudeStart {
                root,
                rows,
                columns,
            } => {
                let port = self.ide.as_ref().map(ide::IdeServer::port);
                match terminal::start_claude(&root, rows, columns, port, send) {
                    Ok(process) => {
                        self.claude = Some(process);
                        None
                    }
                    Err(message) => Some(Event::ClaudeEnded {
                        state: crate::core::ClaudeState::Failed(message),
                    }),
                }
            }
            Effect::Terminal(effect) => {
                let port = self.ide.as_ref().map(ide::IdeServer::port);
                self.terminals.run(effect, port, send)
            }
            Effect::ClaudeWrite { bytes } => {
                if let Some(process) = &self.claude {
                    process.write(bytes);
                }
                None
            }
            Effect::ClaudeResize { rows, columns } => {
                if let Some(process) = &self.claude {
                    process.resize(rows, columns);
                }
                None
            }
            Effect::IdeNotify { method, params } => {
                if let Some(ide) = &self.ide {
                    ide.notify(&method, &params);
                }
                None
            }
            Effect::FormatWithTool {
                doc,
                version,
                path,
                root,
                text,
            } => {
                thread::spawn(move || {
                    send(Event::Formatted {
                        doc,
                        version,
                        result: format::format_text(&path, &root, &text),
                    });
                });
                None
            }
            Effect::ScratchCreate { extension } => Some(Event::ScratchCreated {
                result: user_data::scratch_dir()
                    .ok_or_else(|| "this system has no data folder".to_owned())
                    .and_then(|dir| user_data::create_scratch(&dir, &extension)),
            }),
            Effect::ScratchList => Some(Event::ScratchesListed {
                paths: user_data::scratch_dir()
                    .map_or_else(Vec::new, |dir| user_data::list_scratches(&dir)),
            }),
            Effect::SaveRunHistory { commands } => {
                let _ = user_data::save_run_history(&commands);
                None
            }
            Effect::SaveTrust { root, trusted } => {
                user_data::save_trust(&root, trusted)
                    .err()
                    .map(|message| Event::ConfigProblems {
                        messages: vec![format!("The answer was not remembered: {message}")],
                    })
            }
            Effect::SystemNotify { title, text } => {
                thread::spawn(move || system_notify(&title, &text));
                None
            }
            Effect::SpellCheck {
                path,
                text,
                language,
            } => {
                thread::spawn(move || send(spell::check_event(path, &text, language)));
                None
            }
            Effect::SpellAddWord { word } => {
                thread::spawn(move || {
                    let result = spell::add_word(&word);
                    send(Event::WordAdded { word, result });
                });
                None
            }
            Effect::ReadDependencies { root } => {
                thread::spawn(move || send(read_dependencies(&root)));
                None
            }
            Effect::OpenImage { root, request } => {
                thread::spawn(move || {
                    send(Event::ImageOpened {
                        result: image::open_outside(&root, &request),
                    });
                });
                None
            }
            Effect::OpenProject { dir } => {
                thread::spawn(move || {
                    send(Event::ProjectOpened {
                        result: ipc::open_project(&dir),
                    });
                });
                None
            }
            Effect::Analysis(effect) => {
                self.analyses
                    .run(effect, analysis_grants(), move |e| send(Event::Analysis(e)));
                None
            }
            Effect::Http(effect) => {
                self.http.run(effect, move |event| send(Event::Http(event)));
                None
            }
            Effect::Db(effect) => {
                self.db.run(effect, move |event| send(Event::Db(event)));
                None
            }
            Effect::Cli(crate::core::cli::CliEffect::Run {
                id,
                tool,
                root,
                args,
                input,
                in_project,
                write,
            }) => {
                let launcher = std::env::current_exe()
                    .ok()
                    .filter(|exe| exe.file_name().is_some_and(|n| n == crate::PROGRAM));
                thread::spawn(move || {
                    let outcome = cli::run(cli::CliRequest {
                        tool,
                        args,
                        input,
                        root: &root,
                        in_project,
                        write,
                        launcher: launcher.as_deref(),
                    });
                    send(Event::Cli(crate::core::cli::CliEvent::Finished {
                        id,
                        tool,
                        outcome,
                    }));
                });
                None
            }
            Effect::LoadGrammar { language } => {
                thread::spawn(move || {
                    send(Event::GrammarLoaded {
                        language,
                        error: crate::lang::syntax::load_module(language).err(),
                    });
                });
                None
            }
            Effect::LoadPlugins => {
                self.plugins_load();
                None
            }
            Effect::RunPlugin {
                plugin,
                command,
                root,
                context,
            } => {
                let found = self.plugins.iter().find(|p| p.info.name == plugin).cloned();
                let launcher = std::env::current_exe()
                    .ok()
                    .filter(|exe| exe.file_name().is_some_and(|n| n == crate::PROGRAM));
                thread::spawn(move || {
                    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
                        let found = found.ok_or_else(|| "it is gone".to_owned())?;
                        plugin::run(
                            &found,
                            &command,
                            &root,
                            context,
                            plugin::Limits::default(),
                            launcher.as_deref(),
                        )
                    }))
                    .unwrap_or_else(|_| Err("crashed the plugin runtime".to_owned()));
                    send(Event::PluginDone { plugin, result });
                });
                None
            }
            Effect::FileOp(op) => {
                thread::spawn(move || send(file_op::run(op)));
                None
            }
            Effect::WatchProject { root } => match watcher::watch(&root, send) {
                Ok(watcher) => {
                    self.watcher = Some(watcher);
                    None
                }
                Err(e) => Some(Event::WatchFailed {
                    message: e.to_string(),
                }),
            },
            Effect::WriteSwap {
                path,
                base,
                contents,
            } => {
                if let Ok(swaps) = &self.swaps {
                    swaps.write(path, base, contents);
                }
                None
            }
            Effect::JournalAppend {
                path,
                disk,
                start,
                changes,
            } => {
                let swaps = self.swaps.as_ref().ok()?;
                swaps
                    .journal()
                    .append(&path, disk, start.as_deref(), &changes)
                    .err()
                    .map(|e| Event::SwapFailed {
                        message: e.to_string(),
                    })
            }
            Effect::RemoveJournal { path } => {
                if let Ok(swaps) = &self.swaps {
                    let _ = swaps.journal().remove(&path);
                }
                None
            }
            Effect::RemoveSwap { path } => {
                if let Ok(swaps) = &self.swaps {
                    swaps.remove(path);
                }
                None
            }
            Effect::RecoverSwaps => match &self.swaps {
                Ok(swaps) => Some(Event::SwapsFound {
                    swaps: swaps.recover(),
                }),
                Err(message) => Some(Event::SwapFailed {
                    message: message.clone(),
                }),
            },
            Effect::Lsp(LspEffect::FindRoot {
                path,
                server,
                project,
            }) => {
                let root = lsp::servers::root_of(server, &path, &project);
                Some(Event::Lsp(LspEvent::RootFound {
                    path,
                    server: ServerKey { server, root },
                }))
            }
            Effect::Lsp(LspEffect::Start { server }) => self.start_lsp(server),
            Effect::Git(GitEffect::Status { root }) => {
                thread::spawn(move || send(git::status_event(&root)));
                None
            }
            Effect::Git(GitEffect::Log { root }) => {
                thread::spawn(move || send(git::log_event(&root)));
                None
            }
            Effect::Git(GitEffect::CommitFiles { root, hash }) => {
                thread::spawn(move || send(git::commit_files_event(&root, hash)));
                None
            }
            Effect::Git(GitEffect::Diff { id, root, source }) => {
                thread::spawn(move || send(git::diff_event(&root, id, &source)));
                None
            }
            Effect::Git(GitEffect::Commit {
                root,
                message,
                paths,
                untracked,
                amend,
            }) => {
                thread::spawn(move || {
                    send(git::commit_event(
                        &root, &message, &paths, &untracked, amend,
                    ));
                });
                None
            }
            Effect::Git(GitEffect::ReadLastCommit { root }) => {
                thread::spawn(move || send(git::last_commit_event(&root)));
                None
            }
            Effect::Git(GitEffect::Rollback {
                root,
                file,
                absolute,
            }) => {
                if let Ok(Some(bytes)) = fs::read_if_exists(&absolute) {
                    self.snapshot(&absolute, &bytes, SnapshotLabel::BeforeRevert);
                }
                thread::spawn(move || send(git::rollback_event(&root, &file)));
                None
            }
            Effect::Git(GitEffect::ListBranches { root }) => {
                thread::spawn(move || send(git::branches_event(&root)));
                None
            }
            Effect::Git(GitEffect::Switch {
                root,
                branch,
                create,
            }) => {
                thread::spawn(move || send(git::switch_event(&root, &branch, create)));
                None
            }
            Effect::Git(GitEffect::Base { root, path }) => {
                thread::spawn(move || send(git::base_event(&root, path)));
                None
            }
            Effect::Git(GitEffect::Remote { root, op }) => {
                thread::spawn(move || send(git::remote_event(&root, op)));
                None
            }
            Effect::Git(GitEffect::Hunks { id, root, file }) => {
                thread::spawn(move || send(git_stage::hunks_event(&root, id, &file)));
                None
            }
            Effect::Git(GitEffect::ApplyHunk {
                root,
                file,
                hunk,
                lines,
            }) => {
                thread::spawn(move || {
                    send(git_stage::apply_event(
                        &root,
                        &file,
                        &hunk,
                        lines.as_deref(),
                    ));
                });
                None
            }
            Effect::Git(GitEffect::StageFiles { root, files, stage }) => {
                thread::spawn(move || send(git_stage::stage_files_event(&root, &files, stage)));
                None
            }
            Effect::Git(GitEffect::CommitStaged {
                root,
                message,
                amend,
                staged_id,
            }) => {
                thread::spawn(move || {
                    send(git_stage::commit_staged_event(
                        &root, &message, amend, &staged_id,
                    ));
                });
                None
            }
            Effect::Git(GitEffect::Op { root, op, keep }) => {
                for path in &keep {
                    self.snapshot_file(path, SnapshotLabel::BeforeRevert);
                }
                thread::spawn(move || send(git_ops::op_event(&root, op)));
                None
            }
            Effect::Git(GitEffect::ListStashes { root }) => {
                thread::spawn(move || send(git_ops::stashes_event(&root)));
                None
            }
            Effect::Duplicates(effect) => {
                self.duplicates.run(effect, send);
                None
            }
            Effect::DecodeImage { root, request } => {
                let emit = self.emit.clone();
                self.images
                    .get_or_insert_with(|| {
                        image::Decoder::start(move |request, result| {
                            emit(Event::ImageDecoded { request, result });
                        })
                    })
                    .ask(root, request);
                None
            }
            Effect::Coverage(crate::core::coverage::CoverageEffect::Read { root, file }) => {
                thread::spawn(move || send(coverage::read_event(&root, file.as_deref())));
                None
            }
            Effect::Git(GitEffect::Compare { root, base, path }) => {
                thread::spawn(move || send(git_compare::compare_event(&root, base, path)));
                None
            }
            Effect::Git(GitEffect::CommitChecks {
                root,
                staged,
                paths,
                untracked,
                todo,
                command,
            }) => {
                thread::spawn(move || {
                    send(git_compare::checks_event(
                        &root,
                        staged,
                        &paths,
                        &untracked,
                        todo,
                        command.as_deref(),
                    ));
                });
                None
            }
            Effect::Git(GitEffect::FileHistory { root, path }) => {
                thread::spawn(move || send(git_history::file_history_event(&root, path)));
                None
            }
            Effect::Git(GitEffect::Blame {
                root,
                doc,
                path,
                version,
            }) => {
                thread::spawn(move || send(git_history::blame_event(&root, doc, &path, version)));
                None
            }
            Effect::OpenInMainWindow { root, path, at } => {
                thread::spawn(move || {
                    send(Event::OpenedInMainWindow {
                        result: ipc::send_open(&ipc::socket_path(&root), path, at),
                    });
                });
                None
            }
            Effect::OpenGitWindow { root } => {
                thread::spawn(move || {
                    send(Event::GitWindowOpened {
                        result: ipc::open_window(&root, ipc::GIT_LOG_WINDOW),
                    });
                });
                None
            }
            Effect::OpenRunWindow { root } => {
                thread::spawn(move || {
                    send(Event::RunWindowOpened {
                        result: ipc::open_window(&root, ipc::RUN_WINDOW),
                    });
                });
                None
            }
            Effect::RunFollow { root } => {
                ipc::follow_run(ipc::socket_path(&root), send);
                None
            }
            Effect::RunStreamSend(message) => {
                self.run_hub.send(&message);
                None
            }
            Effect::RunStreamStart(message) => {
                self.run_hub.start(&message);
                None
            }
            Effect::Run(RunEffect::Detect { root, file }) => {
                Some(Event::Run(RunEvent::FactsRead {
                    facts: run::facts(&root, file.as_deref()),
                }))
            }
            Effect::Run(RunEffect::Start {
                id,
                target,
                root,
                replaces,
            }) => {
                self.runner
                    .start_replacing(id, target, root, replaces, send);
                None
            }
            Effect::Run(RunEffect::Stop { id }) => {
                self.runner.stop(id);
                None
            }
            Effect::Debug(DebugEffect::Detect { root, file }) => {
                let configured = self.debug_adapters.clone();
                thread::spawn(move || {
                    send(Event::Debug(DebugEvent::Detected {
                        facts: debug::detect(&root, file, &configured),
                    }));
                });
                None
            }
            Effect::Debug(DebugEffect::Build {
                session,
                root,
                argv,
            }) => {
                thread::spawn(move || {
                    let result = debug::build(session, &root, &argv, send.clone());
                    send(Event::Debug(DebugEvent::Built { session, result }));
                });
                None
            }
            Effect::Debug(DebugEffect::Start {
                session,
                adapter,
                cwd,
            }) => {
                self.debugger.start(session, &adapter, &cwd, send);
                None
            }
            Effect::Debug(DebugEffect::Connect { session, conn }) => {
                self.debugger.connect(session, conn, send);
                None
            }
            Effect::Debug(DebugEffect::Send {
                session,
                conn,
                message,
            }) => {
                self.debugger.send(session, conn, &message);
                None
            }
            Effect::Debug(DebugEffect::Stop { session }) => {
                self.debugger.stop(session);
                None
            }
            Effect::Debug(DebugEffect::ProbeFiles {
                session,
                generation,
                paths,
            }) => Some(Event::Debug(DebugEvent::FilesProbed {
                session,
                generation,
                existing: paths.into_iter().filter(|p| p.is_file()).collect(),
            })),
            Effect::WriteClipboard { text } => {
                clipboard::copy(&text);
                None
            }
            Effect::ReadClipboard => {
                thread::spawn(move || {
                    send(Event::ClipboardRead {
                        text: clipboard::paste(),
                    });
                });
                None
            }
            Effect::Lsp(LspEffect::ListServers) => {
                let current = self.server_choice();
                thread::spawn(move || {
                    send(Event::Lsp(LspEvent::ServersListed {
                        options: lsp::choice::list_options(&current),
                    }));
                });
                None
            }
            Effect::Lsp(LspEffect::SelectServer(choice)) => {
                if let Some(file) = &self.server_choice_file
                    && let Err(e) = lsp::choice::save(file, &choice)
                {
                    send(Event::Lsp(LspEvent::ChoiceNotSaved {
                        message: e.to_string(),
                    }));
                }
                self.lsp_choice_now = Some(choice);
                let rust: Vec<ServerKey> = self.lsp.keys_of(Language::Rust);
                self.lsp.shut_down(|key| key.server == Language::Rust);
                rust.into_iter().find_map(|key| self.start_lsp(key))
            }
            Effect::Lsp(LspEffect::Restart { server }) => {
                if let Some(lsp) = self.lsp.get(&server) {
                    lsp.restart();
                }
                None
            }
            Effect::Lsp(LspEffect::DidOpen {
                server,
                language,
                path,
                version,
                text,
            }) => {
                if let Some(lsp) = self.lsp.get(&server) {
                    let id = language.language_id().unwrap_or_default();
                    lsp.did_open(path, id, version, text);
                }
                None
            }
            Effect::Lsp(LspEffect::DidChange {
                server,
                path,
                version,
                text,
            }) => {
                if let Some(lsp) = self.lsp.get(&server) {
                    lsp.did_change(path, version, text);
                }
                None
            }
            Effect::Lsp(LspEffect::DidSave { server, path }) => {
                if let Some(lsp) = self.lsp.get(&server) {
                    lsp.did_save(path);
                }
                None
            }
            Effect::Lsp(LspEffect::DidClose { server, path }) => {
                if let Some(lsp) = self.lsp.get(&server) {
                    lsp.did_close(path);
                }
                None
            }
            Effect::Lsp(LspEffect::Query {
                server,
                id,
                path,
                query,
            }) => {
                if let Some(lsp) = self.lsp.get(&server) {
                    lsp.query(id, path, query);
                }
                None
            }
            Effect::Lsp(LspEffect::Cancel { server, id }) => {
                if let Some(lsp) = self.lsp.get(&server) {
                    lsp.cancel(id);
                }
                None
            }
            Effect::LoadSession => {
                let event = self.session.as_ref().and_then(session::SessionStore::load);
                if let Some(Event::SessionLoaded { files, .. }) = &event {
                    for file in files {
                        if let Ok(Some(bytes)) = &file.result {
                            self.snapshot(&file.path, bytes, SnapshotLabel::Opened);
                        }
                    }
                }
                event
            }
            Effect::WriteSession(saved) => self.session.as_ref().and_then(|store| {
                store.save(&saved).err().map(|e| Event::SessionSaveFailed {
                    message: e.to_string(),
                })
            }),
            Effect::PlanProjectReplace {
                id,
                files,
                query,
                options,
                replacement,
            } => {
                thread::spawn(move || {
                    send(replace::plan(id, &files, &query, options, &replacement));
                });
                None
            }
            Effect::ApplyProjectReplace {
                id,
                files,
                query,
                options,
                replacement,
            } => {
                let recorder = self.history.as_ref().and_then(HistoryWorker::recorder);
                thread::spawn(move || {
                    let snapshot = |path: &Path, bytes: &[u8], label| match &recorder {
                        Some(recorder) => recorder.record(path, bytes, label),
                        None => Err("Local History is not available".to_owned()),
                    };
                    send(replace::apply(
                        id,
                        &files,
                        &query,
                        options,
                        &replacement,
                        snapshot,
                    ));
                });
                None
            }
            Effect::IndexProject { root } => {
                let whole_home = directories::BaseDirs::new()
                    .is_some_and(|dirs| dirs.home_dir() == root || root.parent().is_none());
                if !whole_home {
                    let emit = self.emit.clone();
                    let cache = self.index_file.clone();
                    self.index = Some(index::ProjectIndex::start(
                        root,
                        cache,
                        index::IndexLimits::default(),
                        emit,
                    ));
                }
                None
            }
            Effect::RescanIndex => {
                if let Some(index) = &self.index {
                    index.rescan();
                }
                None
            }
            Effect::QuerySymbols { id, query, limit } => {
                if let Some(reader) = self.index.as_ref().map(index::ProjectIndex::reader) {
                    thread::spawn(move || {
                        let symbols = reader.symbols(&query, limit);
                        send(Event::SymbolsFound { id, symbols });
                    });
                }
                None
            }
            Effect::FileSymbols {
                path,
                version,
                language,
                text,
            } => {
                thread::spawn(move || {
                    let symbols = crate::lang::symbols::extract(language, &text)
                        .into_iter()
                        .map(|s| crate::core::LspSymbol {
                            name: s.name,
                            kind: s.kind.to_owned(),
                            depth: s.depth,
                            start: s.start,
                        })
                        .collect();
                    send(Event::FileSymbolsRead {
                        path,
                        version,
                        symbols,
                    });
                });
                None
            }
            Effect::IndexFiles { root } => {
                thread::spawn(move || send(search::index_files(&root)));
                None
            }
            Effect::SearchProject {
                id,
                root,
                query,
                options,
            } => {
                let reader = self.index.as_ref().map(index::ProjectIndex::reader);
                thread::spawn(move || {
                    send(search::search_with(
                        id,
                        &root,
                        &query,
                        options,
                        reader.as_ref(),
                    ));
                });
                None
            }
        }
    }

    fn snapshot(&self, path: &Path, bytes: &[u8], label: SnapshotLabel) {
        if let Some(history) = &self.history {
            history.record(path.to_path_buf(), bytes.to_vec(), label);
        }
    }

    fn snapshot_file(&self, path: &Path, label: SnapshotLabel) {
        if let Ok(Some(bytes)) = fs::read_if_exists(path) {
            self.snapshot(path, &bytes, label);
        }
    }
}

const MCP_MAX_FILE: u64 = 16 * 1024 * 1024;

fn mcp_read_files(id: u64, call: crate::core::mcp::McpCall, paths: Vec<PathBuf>) -> Event {
    let may_be_new = matches!(call, crate::core::mcp::McpCall::OpenDiff { .. });
    let read = |path: &PathBuf| {
        if may_be_new && !path.exists() {
            return Ok(String::new());
        }
        let size = std::fs::metadata(path)
            .map_err(|e| format!("{}: {e}", path.display()))?
            .len();
        if size > MCP_MAX_FILE {
            return Err(format!("{} is too large", path.display()));
        }
        std::fs::read_to_string(path).map_err(|e| format!("{}: {e}", path.display()))
    };
    let disk = paths.iter().map(|p| (p.clone(), read(p))).collect();
    Event::McpFilesRead { id, call, disk }
}

const MAX_MANIFEST_BYTES: u64 = 2 * 1024 * 1024;

fn read_dependencies(root: &Path) -> Event {
    let mut dependencies = Vec::new();
    let mut problems = Vec::new();
    for file in crate::core::deps::MANIFESTS {
        let path = root.join(file);
        let Ok(meta) = std::fs::symlink_metadata(&path) else {
            continue;
        };
        if !meta.is_file() || meta.len() > MAX_MANIFEST_BYTES {
            problems.push(format!("{file} was not read"));
            continue;
        }
        let parsed = std::fs::read_to_string(&path)
            .map_err(|e| format!("{file}: {e}"))
            .and_then(|text| crate::core::deps::parse(file, &text));
        match parsed {
            Ok(found) => dependencies.extend(found),
            Err(problem) => problems.push(problem),
        }
    }
    Event::DependenciesRead {
        dependencies,
        problems,
    }
}

fn system_notify(title: &str, text: &str) {
    if std::env::var_os("SSH_CONNECTION").is_some() {
        return;
    }
    let mut command = if cfg!(target_os = "macos") {
        let mut c = std::process::Command::new("osascript");
        c.args([
            "-e",
            "on run argv",
            "-e",
            "display notification (item 2 of argv) with title (item 1 of argv)",
            "-e",
            "end run",
            "--",
        ]);
        c
    } else {
        let mut c = std::process::Command::new("notify-send");
        c.arg("--");
        c
    };
    let _ = command
        .args([title, text])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
}
