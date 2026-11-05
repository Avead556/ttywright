use std::collections::VecDeque;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::{Duration, Instant};

use anyhow::{Context, Result};
use ratatui::crossterm::event;

use crate::config;
use crate::core::{Action, Command, Effect, Event, Focus, Workspace};
use crate::services::Services;
use crate::services::ipc;
use crate::services::paths::ProjectData;
use crate::services::user_data;
use crate::terminal::TerminalGuard;
use crate::ui::input::EscDecoder;
use crate::ui::{self, ColorDepth, Theme, UiState, keymap::Keymap, mouse};

enum Msg {
    Input(event::Event),
    InputFailed(std::io::Error),
    Service(Event),
    End,
}

const TRUST_ALL_ENV: &str = "TTYWRIGHT_TRUST_ALL";
const SWAP_DELAY: Duration = Duration::from_millis(250);
const NOTIFY_TICK: Duration = Duration::from_secs(1);
const SESSION_PERIOD: Duration = Duration::from_secs(30);

#[derive(Debug, Default)]
struct AutosaveTimer {
    stamp: u64,
    deadline: Option<Instant>,
}

impl AutosaveTimer {
    fn update(&mut self, stamp: u64, pending: bool, now: Instant, idle: Duration) {
        if stamp != self.stamp {
            self.stamp = stamp;
            self.deadline = Some(now + idle);
        }
        if !pending {
            self.deadline = None;
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum WindowKind {
    GitLog,
    Run,
}

pub struct Target {
    pub root: PathBuf,
    pub file: Option<PathBuf>,
    pub window: Option<WindowKind>,
}

impl Target {
    pub fn from_arg(arg: Option<PathBuf>, cwd: &Path) -> Self {
        let Some(arg) = arg else {
            return Self {
                root: cwd.to_path_buf(),
                file: None,
                window: None,
            };
        };
        let path = absolute(&cwd.join(arg));
        if path.is_dir() {
            return Self {
                root: path,
                file: None,
                window: None,
            };
        }
        let root = if path.starts_with(cwd) {
            cwd.to_path_buf()
        } else {
            path.parent()
                .map_or_else(|| cwd.to_path_buf(), Path::to_path_buf)
        };
        Self {
            root,
            file: Some(path),
            window: None,
        }
    }
}

fn absolute(path: &Path) -> PathBuf {
    if let Ok(p) = path.canonicalize() {
        return p;
    }
    match (
        path.parent().and_then(|p| p.canonicalize().ok()),
        path.file_name(),
    ) {
        (Some(parent), Some(name)) => parent.join(name),
        _ => path.to_path_buf(),
    }
}

pub fn run(target: Target) -> Result<()> {
    let (tx, rx) = mpsc::channel::<Msg>();
    let service_tx = tx.clone();
    let data = ProjectData::for_root(&target.root).filter(|_| target.window.is_none());
    let config_dir = config::config_dir();
    let config_file = config_dir.as_ref().map(|d| d.join("config.toml"));
    let (config, mut problems) = config::load(config_file.as_deref(), &target.root);
    let mut services = Services::new(
        move |e| {
            let _ = service_tx.send(Msg::Service(e));
        },
        data,
        &config,
    );
    services.use_launcher(std::env::current_exe().ok());
    let listener_tx = tx.clone();
    let _listener = if target.window.is_some() {
        None
    } else {
        ipc::Listener::start(
            ipc::socket_path(&target.root),
            services.run_hub(),
            services.mcp_calls(),
            move |e| {
                let _ = listener_tx.send(Msg::Service(e));
            },
        )
        .ok()
        .flatten()
    };

    if target.window.is_none() {
        services.start_ide(&target.root);
    }
    let (mut ws, effects) = match target.window {
        Some(WindowKind::GitLog) => Workspace::new_git_window(target.root),
        Some(WindowKind::Run) => Workspace::new_run_window(target.root),
        None => Workspace::new(target.root),
    };
    let vouched = std::env::var_os(TRUST_ALL_ENV).is_some_and(|v| !v.is_empty());
    ws.configure_trust(user_data::trust(ws.root()).or(vouched.then_some(true)));
    let mut effects = ws.without_untrusted(effects);
    ws.configure(config.autosave.enabled, config_file);
    ws.configure_editor(config.editor.tab_width, config.editor.soft_wrap);
    ws.configure_markdown(config.markdown_preview_on_open);
    ws.configure_run(config.run.clone());
    ws.configure_commit_checks(config.commit_checks.clone());
    ws.configure_scratch_dir(user_data::scratch_dir());
    ws.configure_notifications(config.notifications_popups, config.notifications_system);
    if !ws.is_window() {
        ws.configure_run_history(user_data::run_history());
    }
    ws.configure_debug(config.debug.clone());
    ws.configure_databases(config.databases.clone());
    if let Some(file) = target.file {
        effects.extend(ws.dispatch(Command::OpenFile {
            path: file,
            at: None,
        }));
    }
    run_effects(&mut services, &mut ws, effects);

    let mut guard =
        TerminalGuard::enter(config.double_shift).context("failed to set up the terminal")?;
    let mut bindings = config.keybindings.clone();
    if ws.is_trusted() {
        bindings.extend(config.project_keybindings.iter().cloned());
    }
    let keymap = Keymap::new(guard.keyboard_enhanced()).with_bindings(&bindings, &mut problems);
    let themes = config_dir.as_ref().map(|d| d.join("themes"));
    let theme = Theme::named(&config.theme, themes.as_deref(), &mut problems)
        .adapted(ColorDepth::detect(|name| std::env::var(name).ok()));
    let effects = ws.handle(Event::ConfigProblems { messages: problems });
    run_effects(&mut services, &mut ws, effects);
    if target.window.is_none() {
        run_effects(&mut services, &mut ws, vec![Effect::LoadPlugins]);
    }
    let autosave_idle = Duration::from_millis(config.autosave.idle_ms);
    let over_ssh =
        std::env::var_os("SSH_TTY").is_some() || std::env::var_os("SSH_CONNECTION").is_some();
    let esc_timeout = Duration::from_millis(config::esc_wait_ms(config.esc_timeout_ms, over_ssh));
    let autosave_on_focus_lost = config.autosave.enabled && config.autosave.on_focus_lost;
    let mut ui_state = UiState {
        nerd_icons: config.editor.nerd_icons,
        ..UiState::default()
    };

    let input_tx = tx.clone();
    thread::spawn(move || {
        loop {
            let msg = match event::read() {
                Ok(e) => Msg::Input(e),
                Err(e) => Msg::InputFailed(e),
            };
            let failed = matches!(msg, Msg::InputFailed(_));
            if input_tx.send(msg).is_err() || failed {
                break;
            }
        }
    });
    watch_for_the_end(tx);

    let mut decoder = EscDecoder::default();
    let mut autosave = AutosaveTimer::default();
    let mut swap_deadline: Option<Instant> = None;
    let mut notify_deadline: Option<Instant> = None;
    let mut session_due = Instant::now() + SESSION_PERIOD;
    let mut sent_viewport = 0;
    let mut sent_claude_size = None;
    let mut sent_terminal_size = None;
    let mut sent_tool_room = (0, 0, 0);
    loop {
        let drawn = guard
            .terminal()
            .draw(|frame| ui::render(frame, &ws, &mut ui_state, &theme, &keymap));
        if let Err(e) = drawn {
            finish_up(&mut services, &mut ws);
            return Err(e).context("failed to draw");
        }
        if ui_state.tool_room != sent_tool_room {
            sent_tool_room = ui_state.tool_room;
            let (rows, min, max) = sent_tool_room;
            let effects = ws.dispatch(Command::ToolRoom { rows, min, max });
            run_effects(&mut services, &mut ws, effects);
        }
        ws.merge_set_page(ui_state.page_rows);
        if let Some((rows, columns)) = ui_state.terminal_size.take()
            && sent_terminal_size != Some((rows, columns))
        {
            sent_terminal_size = Some((rows, columns));
            let effects = ws.dispatch(Command::TerminalSize { rows, columns });
            run_effects(&mut services, &mut ws, effects);
        }
        if let Some((rows, columns)) = ui_state.claude_size.take()
            && sent_claude_size != Some((rows, columns))
        {
            sent_claude_size = Some((rows, columns));
            let effects = ws.dispatch(Command::ClaudeSize { rows, columns });
            run_effects(&mut services, &mut ws, effects);
        }
        if ui_state.viewport != sent_viewport {
            sent_viewport = ui_state.viewport;
            let effects = ws.dispatch(Command::Viewport {
                text_cols: sent_viewport,
            });
            run_effects(&mut services, &mut ws, effects);
        }
        for request in std::mem::take(&mut ui_state.image_wants) {
            let effects = ws.dispatch(Command::WantImage(request));
            run_effects(&mut services, &mut ws, effects);
        }
        if ws.should_quit() {
            let effects = ws.dispatch(Command::SaveSession);
            run_effects(&mut services, &mut ws, effects);
            return Ok(());
        }
        let now = Instant::now();
        autosave.update(
            ws.edit_stamp(),
            config.autosave.enabled && ws.has_autosavable(),
            now,
            autosave_idle,
        );
        swap_deadline = match (ws.has_unswapped(), swap_deadline) {
            (false, _) => None,
            (true, None) => Some(now + SWAP_DELAY),
            (true, due) => due,
        };
        notify_deadline = match (ws.notifications().is_ticking(), notify_deadline) {
            (false, _) => None,
            (true, None) => Some(now + NOTIFY_TICK),
            (true, due) => due,
        };
        let due = [
            autosave.deadline,
            swap_deadline,
            notify_deadline,
            Some(session_due),
        ]
        .into_iter()
        .flatten()
        .min()
        .unwrap_or(session_due);
        let first = match rx.recv_timeout(due.saturating_duration_since(now)) {
            Ok(msg) => msg,
            Err(mpsc::RecvTimeoutError::Timeout) => {
                if notify_deadline == Some(due) {
                    notify_deadline = None;
                    ws.dispatch(Command::NotificationTick);
                }
                if swap_deadline == Some(due) {
                    swap_deadline = None;
                    let effects = ws.dispatch(Command::WriteSwaps);
                    run_effects(&mut services, &mut ws, effects);
                }
                if autosave.deadline == Some(due) {
                    autosave.deadline = None;
                    let effects = ws.dispatch(Command::Autosave);
                    run_effects(&mut services, &mut ws, effects);
                }
                if session_due == due {
                    session_due = due + SESSION_PERIOD;
                    let effects = ws.dispatch(Command::SaveSession);
                    run_effects(&mut services, &mut ws, effects);
                }
                continue;
            }
            Err(mpsc::RecvTimeoutError::Disconnected) => {
                anyhow::bail!("event channel closed")
            }
        };
        let mut batch: Vec<Msg> = vec![first];
        loop {
            batch.extend(rx.try_iter());
            for msg in batch.drain(..) {
                match msg {
                    Msg::Input(input) => {
                        for input in decoder.push(input) {
                            handle_input(
                                &input,
                                &mut ws,
                                &mut ui_state,
                                &keymap,
                                &mut services,
                                autosave_on_focus_lost,
                            );
                        }
                    }
                    Msg::Service(event) => {
                        let effects = ws.handle(event);
                        run_effects(&mut services, &mut ws, effects);
                    }
                    Msg::InputFailed(e) => {
                        finish_up(&mut services, &mut ws);
                        return Err(e).context("failed to read terminal input");
                    }
                    Msg::End => {
                        finish_up(&mut services, &mut ws);
                        return Ok(());
                    }
                }
            }
            if !decoder.is_pending() {
                break;
            }
            match rx.recv_timeout(esc_timeout) {
                Ok(msg) => batch.push(msg),
                Err(mpsc::RecvTimeoutError::Timeout) => {
                    for input in decoder.flush() {
                        handle_input(
                            &input,
                            &mut ws,
                            &mut ui_state,
                            &keymap,
                            &mut services,
                            autosave_on_focus_lost,
                        );
                    }
                    break;
                }
                Err(mpsc::RecvTimeoutError::Disconnected) => anyhow::bail!("event channel closed"),
            }
        }
    }
}

fn panel_input(input: &event::Event, ws: &Workspace, keymap: &Keymap) -> Option<Command> {
    let screen = ws.keys_screen()?;
    let terminal = ws.focus() == Focus::Terminal;
    let send = |bytes| {
        if terminal {
            Command::TerminalInput(bytes)
        } else {
            Command::ClaudeInput(bytes)
        }
    };
    match input {
        event::Event::Key(key) => keymap
            .leaves_terminal(*key, ws.focus())
            .or_else(|| ui::terminal::key_bytes(*key, screen.application_cursor()).map(send)),
        event::Event::Paste(text) => Some(send(ui::terminal::paste_bytes(
            text,
            screen.bracketed_paste(),
        ))),
        _ => None,
    }
}

fn handle_input<F: Fn(Event) + Clone + Send + 'static>(
    input: &event::Event,
    ws: &mut Workspace,
    ui_state: &mut UiState,
    keymap: &Keymap,
    services: &mut Services<F>,
    autosave_on_focus_lost: bool,
) {
    if ui_state.double_shift.feed(input, Instant::now()) {
        let effects = ws.dispatch(Command::Run(Action::SearchEverywhere));
        run_effects(services, ws, effects);
        return;
    }
    if !ws.overlay_open()
        && let Some(command) = panel_input(input, ws, keymap)
    {
        let effects = ws.dispatch(command);
        run_effects(services, ws, effects);
        return;
    }
    let commands = match input {
        event::Event::Mouse(m) => {
            let commands = mouse::handle(m, ui_state, ws, Instant::now());
            let pointer = mouse::pointer_at(ui_state, m.column, m.row);
            if pointer != ui_state.pointer {
                ui_state.pointer = pointer;
                let mut out = std::io::stdout();
                let _ = out.write_all(pointer.sequence().as_bytes());
                let _ = out.flush();
            }
            commands
        }
        event::Event::FocusLost if autosave_on_focus_lost => {
            vec![Command::TerminalFocus { focused: false }, Command::Autosave]
        }
        event::Event::FocusLost => vec![Command::TerminalFocus { focused: false }],
        event::Event::FocusGained => {
            vec![Command::TerminalFocus { focused: true }, Command::CheckDisk]
        }
        other => keymap
            .resolve(other, ws.focus(), ui_state.page_rows)
            .into_iter()
            .collect(),
    };
    for command in commands {
        if ui_state.preview_alone.is_some()
            && ws.focus() == Focus::Editor
            && !ws.overlay_open()
            && matches!(
                command,
                Command::Insert(_)
                    | Command::Newline
                    | Command::DeleteBackward
                    | Command::DeleteForward
            )
        {
            let effects = ws.dispatch(Command::Run(Action::MarkdownPreview));
            run_effects(services, ws, effects);
        }
        let effects = ws.dispatch(command);
        run_effects(services, ws, effects);
    }
}

#[cfg(unix)]
fn watch_for_the_end(tx: mpsc::Sender<Msg>) {
    use signal_hook::consts::{SIGHUP, SIGTERM};
    let Ok(mut signals) = signal_hook::iterator::Signals::new([SIGTERM, SIGHUP]) else {
        return;
    };
    thread::spawn(move || {
        if signals.forever().next().is_some() {
            let _ = tx.send(Msg::End);
        }
    });
}

#[cfg(not(unix))]
fn watch_for_the_end(_tx: mpsc::Sender<Msg>) {}

fn finish_up<F: Fn(Event) + Clone + Send + 'static>(
    services: &mut Services<F>,
    ws: &mut Workspace,
) {
    for command in [Command::Run(Action::Quit), Command::SaveSession] {
        let effects = ws.dispatch(command);
        run_effects(services, ws, effects);
    }
}

fn run_effects<F: Fn(Event) + Clone + Send + 'static>(
    services: &mut Services<F>,
    ws: &mut Workspace,
    effects: Vec<Effect>,
) {
    let mut queue = VecDeque::from(effects);
    while let Some(effect) = queue.pop_front() {
        if let Some(event) = services.run(effect) {
            queue.extend(ws.handle(event));
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn autosave_waits_for_quiet_after_the_last_edit() {
        let idle = Duration::from_secs(1);
        let t0 = Instant::now();
        let mut timer = AutosaveTimer::default();

        timer.update(1, true, t0, idle);
        assert_eq!(timer.deadline, Some(t0 + idle));
        let t1 = t0 + Duration::from_millis(500);
        timer.update(2, true, t1, idle);
        assert_eq!(timer.deadline, Some(t1 + idle), "an edit postpones it");
        timer.update(2, true, t1 + idle, idle);
        assert_eq!(timer.deadline, Some(t1 + idle), "no edit, no change");
    }

    #[test]
    fn autosave_does_not_rearm_after_firing_until_the_next_edit() {
        let idle = Duration::from_secs(1);
        let t0 = Instant::now();
        let mut timer = AutosaveTimer::default();
        timer.update(1, true, t0, idle);
        timer.deadline = None;
        timer.update(1, true, t0 + idle, idle);
        assert_eq!(timer.deadline, None);
        timer.update(2, false, t0 + idle, idle);
        assert_eq!(timer.deadline, None, "nothing to save");
    }

    #[test]
    fn target_resolution() {
        let dir = tempfile::tempdir().unwrap();
        let cwd = dir.path().canonicalize().unwrap();
        std::fs::create_dir(cwd.join("sub")).unwrap();
        std::fs::write(cwd.join("sub").join("a.rs"), "").unwrap();

        let t = Target::from_arg(None, &cwd);
        assert_eq!((t.root.as_path(), t.file), (cwd.as_path(), None));

        let t = Target::from_arg(Some("sub".into()), &cwd);
        assert_eq!(t.root, cwd.join("sub"));

        let t = Target::from_arg(Some("sub/a.rs".into()), &cwd);
        assert_eq!(
            (t.root.as_path(), t.file),
            (cwd.as_path(), Some(cwd.join("sub/a.rs")))
        );

        let t = Target::from_arg(Some("new.rs".into()), &cwd);
        assert_eq!(
            t.file,
            Some(cwd.join("new.rs")),
            "missing files open as new buffers"
        );
    }
}
