use std::io::{self, Stdout, Write, stdout};
use std::panic;
use std::sync::atomic::{AtomicBool, Ordering};

use ratatui::Terminal;
use ratatui::backend::CrosstermBackend;
use ratatui::crossterm::event::{
    DisableBracketedPaste, DisableFocusChange, DisableMouseCapture, EnableBracketedPaste,
    EnableFocusChange, EnableMouseCapture, KeyboardEnhancementFlags, PopKeyboardEnhancementFlags,
    PushKeyboardEnhancementFlags,
};
use ratatui::crossterm::execute;
use ratatui::crossterm::terminal::{
    EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    supports_keyboard_enhancement,
};

static ACTIVE: AtomicBool = AtomicBool::new(false);
static KEYBOARD_ENHANCED: AtomicBool = AtomicBool::new(false);

pub type Backend = CrosstermBackend<Stdout>;

pub struct TerminalGuard {
    terminal: Terminal<Backend>,
}

impl TerminalGuard {
    pub fn enter(report_all_keys: bool) -> io::Result<Self> {
        install_panic_hook();
        ACTIVE.store(true, Ordering::SeqCst);
        match setup(report_all_keys) {
            Ok(terminal) => Ok(Self { terminal }),
            Err(e) => {
                restore();
                Err(e)
            }
        }
    }

    pub fn terminal(&mut self) -> &mut Terminal<Backend> {
        &mut self.terminal
    }

    pub fn keyboard_enhanced(&self) -> bool {
        KEYBOARD_ENHANCED.load(Ordering::SeqCst)
    }
}

impl Drop for TerminalGuard {
    fn drop(&mut self) {
        restore();
    }
}

fn keyboard_flags(report_all_keys: bool) -> KeyboardEnhancementFlags {
    let mut flags = KeyboardEnhancementFlags::DISAMBIGUATE_ESCAPE_CODES;
    if report_all_keys {
        flags |= KeyboardEnhancementFlags::REPORT_ALL_KEYS_AS_ESCAPE_CODES
            | KeyboardEnhancementFlags::REPORT_ALTERNATE_KEYS;
    }
    flags
}

fn setup(report_all_keys: bool) -> io::Result<Terminal<Backend>> {
    enable_raw_mode()?;
    let enhanced = supports_keyboard_enhancement().unwrap_or(false);
    execute!(
        stdout(),
        EnterAlternateScreen,
        EnableBracketedPaste,
        EnableFocusChange,
        EnableMouseCapture
    )?;
    if enhanced {
        execute!(
            stdout(),
            PushKeyboardEnhancementFlags(keyboard_flags(report_all_keys))
        )?;
        KEYBOARD_ENHANCED.store(true, Ordering::SeqCst);
    }
    let mut terminal = Terminal::new(CrosstermBackend::new(stdout()))?;
    terminal.clear()?;
    Ok(terminal)
}

fn restore() {
    if !ACTIVE.swap(false, Ordering::SeqCst) {
        return;
    }
    let mut out = stdout();
    if KEYBOARD_ENHANCED.swap(false, Ordering::SeqCst) {
        let _ = execute!(out, PopKeyboardEnhancementFlags);
    }
    let _ = out.write_all(b"\x1b]22;default\x1b\\");
    let _ = execute!(
        out,
        DisableMouseCapture,
        DisableFocusChange,
        DisableBracketedPaste,
        LeaveAlternateScreen,
        ratatui::crossterm::cursor::Show
    );
    let _ = disable_raw_mode();
}

fn install_panic_hook() {
    let previous = panic::take_hook();
    panic::set_hook(Box::new(move |info| {
        restore();
        previous(info);
    }));
}
