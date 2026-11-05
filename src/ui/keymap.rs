use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyEventState, KeyModifiers,
};

use crate::core::{Action, Command, Focus, Motion};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Chord {
    code: KeyCode,
    ctrl: bool,
    shift: bool,
    alt: bool,
}

const fn key(code: KeyCode) -> Chord {
    Chord {
        code,
        ctrl: false,
        shift: false,
        alt: false,
    }
}

const fn ctrl(c: char) -> Chord {
    Chord {
        code: KeyCode::Char(c),
        ctrl: true,
        shift: false,
        alt: false,
    }
}

const fn ctrl_shift(c: char) -> Chord {
    Chord {
        code: KeyCode::Char(c),
        ctrl: true,
        shift: true,
        alt: false,
    }
}

const fn alt(code: KeyCode) -> Chord {
    Chord {
        code,
        ctrl: false,
        shift: false,
        alt: true,
    }
}

const fn alt_shift(code: KeyCode) -> Chord {
    Chord {
        code,
        ctrl: false,
        shift: true,
        alt: true,
    }
}

const fn shift(code: KeyCode) -> Chord {
    Chord {
        code,
        ctrl: false,
        shift: true,
        alt: false,
    }
}

const BINDINGS: &[(Chord, Action, &str)] = &[
    (ctrl_shift('a'), Action::OpenPalette, "Ctrl+Shift+A"),
    (ctrl('k'), Action::OpenPalette, "Ctrl+K"),
    (ctrl_shift('n'), Action::GotoFile, "Ctrl+Shift+N"),
    (ctrl('p'), Action::GotoFile, "Ctrl+P"),
    (ctrl('e'), Action::RecentFiles, "Ctrl+E"),
    (ctrl('n'), Action::SearchEverywhere, "Ctrl+N"),
    (ctrl('s'), Action::Save, "Ctrl+S"),
    (ctrl('w'), Action::CloseTab, "Ctrl+W"),
    (
        Chord {
            code: KeyCode::F(4),
            ctrl: true,
            shift: false,
            alt: false,
        },
        Action::CloseTab,
        "Ctrl+F4",
    ),
    (alt(KeyCode::Right), Action::NextTab, "Alt+→"),
    (alt(KeyCode::Left), Action::PreviousTab, "Alt+←"),
    (key(KeyCode::F(10)), Action::MainMenu, "F10"),
    (alt(KeyCode::F(12)), Action::TerminalToggle, "Alt+F12"),
    (alt(KeyCode::Char('3')), Action::TerminalToggle, "Alt+3"),
    (alt(KeyCode::Char('t')), Action::TerminalNew, "Alt+T"),
    (
        Chord {
            code: KeyCode::F(9),
            ctrl: true,
            shift: false,
            alt: false,
        },
        Action::RunTest,
        "Ctrl+F9",
    ),
    (key(KeyCode::F(9)), Action::ResumeProgram, "F9"),
    (shift(KeyCode::F(9)), Action::DebugLast, "Shift+F9"),
    (
        alt_shift(KeyCode::F(9)),
        Action::DebugChoose,
        "Alt+Shift+F9",
    ),
    (alt(KeyCode::F(9)), Action::DebugRunToCursor, "Alt+F9"),
    (key(KeyCode::F(8)), Action::DebugStepOver, "F8"),
    (key(KeyCode::F(7)), Action::DebugStepInto, "F7"),
    (shift(KeyCode::F(8)), Action::DebugStepOut, "Shift+F8"),
    (alt(KeyCode::F(8)), Action::DebugEvaluate, "Alt+F8"),
    (
        Chord {
            code: KeyCode::F(8),
            ctrl: true,
            shift: false,
            alt: false,
        },
        Action::ToggleBreakpoint,
        "Ctrl+F8",
    ),
    (
        Chord {
            code: KeyCode::F(8),
            ctrl: true,
            shift: true,
            alt: false,
        },
        Action::ViewBreakpoints,
        "Ctrl+Shift+F8",
    ),
    (alt(KeyCode::Char('5')), Action::DebugShow, "Alt+5"),
    (alt(KeyCode::Char('2')), Action::ClaudeOpen, "Alt+2"),
    (alt(KeyCode::Insert), Action::NewFile, "Alt+Ins"),
    (ctrl('z'), Action::Undo, "Ctrl+Z"),
    (ctrl_shift('z'), Action::Redo, "Ctrl+Shift+Z"),
    (ctrl('y'), Action::Redo, "Ctrl+Y"),
    (ctrl('f'), Action::Find, "Ctrl+F"),
    (ctrl('r'), Action::Replace, "Ctrl+R"),
    (alt(KeyCode::Char('a')), Action::ReplaceAll, "Alt+A"),
    (alt(KeyCode::Char('r')), Action::ToggleFindRegex, "Alt+R"),
    (alt(KeyCode::Char('c')), Action::ToggleFindCase, "Alt+C"),
    (alt(KeyCode::Char('w')), Action::ToggleFindWord, "Alt+W"),
    (key(KeyCode::F(3)), Action::FindNext, "F3"),
    (shift(KeyCode::F(3)), Action::FindPrevious, "Shift+F3"),
    (ctrl_shift('f'), Action::FindInProject, "Ctrl+Shift+F"),
    (ctrl('g'), Action::FindInProject, "Ctrl+G"),
    (ctrl_shift('r'), Action::ReplaceInFiles, "Ctrl+Shift+R"),
    (ctrl('l'), Action::ReplaceInFiles, "Ctrl+L"),
    (ctrl('\\'), Action::SplitVertically, "Ctrl+\\"),
    (
        Chord {
            code: KeyCode::Char('\\'),
            ctrl: true,
            shift: false,
            alt: true,
        },
        Action::SplitHorizontally,
        "Ctrl+Alt+\\",
    ),
    (ctrl('4'), Action::SplitVertically, "Ctrl+\\"),
    (
        Chord {
            code: KeyCode::Char('4'),
            ctrl: true,
            shift: false,
            alt: true,
        },
        Action::SplitHorizontally,
        "Ctrl+Alt+\\",
    ),
    (key(KeyCode::F(6)), Action::NextSplitter, "F6"),
    (alt(KeyCode::Char('1')), Action::ToggleProject, "Alt+1"),
    (ctrl('t'), Action::ToggleProject, "Ctrl+T"),
    (alt(KeyCode::Char('6')), Action::ShowProblems, "Alt+6"),
    (alt(KeyCode::Char('y')), Action::ShowLanguageServer, "Alt+Y"),
    (alt(KeyCode::Char('h')), Action::HttpRun, "Alt+H"),
    (alt(KeyCode::Char('e')), Action::DbRun, "Alt+E"),
    (alt(KeyCode::Char('d')), Action::ShowDatabase, "Alt+D"),
    (alt(KeyCode::Char('z')), Action::ShowDocker, "Alt+Z"),
    (alt(KeyCode::Char('v')), Action::ShowGitHub, "Alt+V"),
    (shift(KeyCode::Esc), Action::HideToolWindow, "Shift+Esc"),
    (
        alt_shift(KeyCode::Up),
        Action::ToolWindowTaller,
        "Alt+Shift+↑",
    ),
    (
        alt_shift(KeyCode::Down),
        Action::ToolWindowShorter,
        "Alt+Shift+↓",
    ),
    (alt(KeyCode::Char('=')), Action::ToolWindowTaller, "Alt+="),
    (alt(KeyCode::Char('-')), Action::ToolWindowShorter, "Alt+-"),
    (key(KeyCode::F(1)), Action::QuickDocumentation, "F1"),
    (ctrl('q'), Action::QuickDocumentation, "Ctrl+Q"),
    (alt(KeyCode::Char('q')), Action::QuickDocumentation, "Alt+Q"),
    (ctrl(' '), Action::Completion, "Ctrl+Space"),
    (alt(KeyCode::Char('/')), Action::Completion, "Alt+/"),
    (alt(KeyCode::F(7)), Action::FindUsages, "Alt+F7"),
    (alt(KeyCode::Char('u')), Action::FindUsages, "Alt+U"),
    (shift(KeyCode::F(6)), Action::RenameSymbol, "Shift+F6"),
    (alt(KeyCode::Char('x')), Action::RunAnything, "Alt+X"),
    (alt(KeyCode::Char('m')), Action::RenameSymbol, "Alt+M"),
    (alt(KeyCode::Char('.')), Action::MarkdownPreview, "Alt+."),
    (alt(KeyCode::Char(';')), Action::OpenInSystemViewer, "Alt+;"),
    (alt(KeyCode::Char('j')), Action::RefactorThis, "Alt+J"),
    (alt(KeyCode::Delete), Action::SafeDelete, "Alt+Delete"),
    (
        Chord {
            code: KeyCode::Char('l'),
            ctrl: true,
            shift: false,
            alt: true,
        },
        Action::ReformatCode,
        "Ctrl+Alt+L",
    ),
    (alt(KeyCode::Char('l')), Action::ReformatCode, "Alt+L"),
    (alt(KeyCode::Enter), Action::ContextActions, "Alt+Enter"),
    (
        Chord {
            code: KeyCode::Char('s'),
            ctrl: true,
            shift: false,
            alt: true,
        },
        Action::OpenSettings,
        "Ctrl+Alt+S",
    ),
    (ctrl('c'), Action::Copy, "Ctrl+C"),
    (ctrl('x'), Action::Cut, "Ctrl+X"),
    (ctrl('v'), Action::Paste, "Ctrl+V"),
    (ctrl('a'), Action::SelectAll, "Ctrl+A"),
    (alt(KeyCode::Char('9')), Action::GitLog, "Alt+9"),
    (alt(KeyCode::Char('g')), Action::GitBranches, "Alt+G"),
    (alt(KeyCode::Char('0')), Action::GitCommit, "Alt+0"),
    (ctrl_shift('k'), Action::GitPush, "Ctrl+Shift+K"),
    (
        Chord {
            code: KeyCode::F(10),
            ctrl: true,
            shift: true,
            alt: false,
        },
        Action::RunChoose,
        "Ctrl+Shift+F10",
    ),
    (
        alt_shift(KeyCode::F(10)),
        Action::RunChoose,
        "Alt+Shift+F10",
    ),
    (ctrl('j'), Action::RunChoose, "Ctrl+J"),
    (shift(KeyCode::F(10)), Action::RerunLast, "Shift+F10"),
    (
        Chord {
            code: KeyCode::F(5),
            ctrl: true,
            shift: false,
            alt: false,
        },
        Action::RerunLast,
        "Ctrl+F5",
    ),
    (ctrl('u'), Action::RerunLast, "Ctrl+U"),
    (
        Chord {
            code: KeyCode::F(2),
            ctrl: true,
            shift: false,
            alt: false,
        },
        Action::StopRun,
        "Ctrl+F2",
    ),
    (alt(KeyCode::Char('s')), Action::StopRun, "Alt+S"),
    (alt(KeyCode::Char('4')), Action::ShowRun, "Alt+4"),
    (key(KeyCode::F(4)), Action::NextRunError, "F4"),
    (alt(KeyCode::Char('n')), Action::NextRunError, "Alt+N"),
    (shift(KeyCode::F(4)), Action::PreviousRunError, "Shift+F4"),
    (alt(KeyCode::Char('p')), Action::PreviousRunError, "Alt+P"),
    (ctrl('b'), Action::GotoDefinition, "Ctrl+B"),
    (key(KeyCode::F(12)), Action::GotoDefinition, "F12"),
    (
        Chord {
            code: KeyCode::F(12),
            ctrl: true,
            shift: false,
            alt: false,
        },
        Action::FileStructure,
        "Ctrl+F12",
    ),
    (alt(KeyCode::Char('7')), Action::FileStructure, "Alt+7"),
    (
        Chord {
            code: KeyCode::Char('n'),
            ctrl: true,
            shift: true,
            alt: true,
        },
        Action::GotoSymbol,
        "Ctrl+Alt+Shift+N",
    ),
    (alt(KeyCode::Char('o')), Action::GotoSymbol, "Alt+O"),
    (
        Chord {
            code: KeyCode::Char('h'),
            ctrl: true,
            shift: false,
            alt: true,
        },
        Action::CallersOf,
        "Ctrl+Alt+H",
    ),
    (alt(KeyCode::Char('8')), Action::CallersOf, "Alt+8"),
    (key(KeyCode::F(11)), Action::ToggleBookmark, "F11"),
    (shift(KeyCode::F(11)), Action::ShowBookmarks, "Shift+F11"),
    (alt(KeyCode::Char('k')), Action::ShowBookmarks, "Alt+K"),
    (alt(KeyCode::Char('i')), Action::ShowTodos, "Alt+I"),
    (alt(KeyCode::Char('\'')), Action::ShowTests, "Alt+'"),
    (
        alt(KeyCode::Char('\\')),
        Action::ClearNotifications,
        "Alt+\\",
    ),
    (alt(KeyCode::Char('`')), Action::MacroPlay, "Alt+`"),
    (alt(KeyCode::Char('~')), Action::MacroRecord, "Alt+~"),
    (
        Chord {
            code: KeyCode::Left,
            ctrl: true,
            shift: false,
            alt: true,
        },
        Action::NavigateBack,
        "Ctrl+Alt+←",
    ),
    (alt(KeyCode::Char(',')), Action::NavigateBack, "Alt+,"),
];

pub struct Keymap {
    distinguishes_shift: bool,
    user: Vec<(Chord, Option<Action>, String)>,
}

fn parse_chord(text: &str) -> Option<Chord> {
    let lower = text.to_ascii_lowercase();
    let mut parts: Vec<&str> = lower.split('+').map(str::trim).collect();
    let name = parts.pop()?;
    let code = match name {
        "enter" | "return" => KeyCode::Enter,
        "esc" | "escape" => KeyCode::Esc,
        "tab" => KeyCode::Tab,
        "space" => KeyCode::Char(' '),
        "plus" => KeyCode::Char('+'),
        "backspace" => KeyCode::Backspace,
        "delete" => KeyCode::Delete,
        "left" => KeyCode::Left,
        "right" => KeyCode::Right,
        "up" => KeyCode::Up,
        "down" => KeyCode::Down,
        "home" => KeyCode::Home,
        "end" => KeyCode::End,
        "pageup" => KeyCode::PageUp,
        "pagedown" => KeyCode::PageDown,
        _ => match name.strip_prefix('f').and_then(|n| n.parse::<u8>().ok()) {
            Some(n) if (1..=12).contains(&n) => KeyCode::F(n),
            _ => {
                let mut chars = name.chars();
                let c = chars.next()?;
                if chars.next().is_some() {
                    return None;
                }
                KeyCode::Char(c)
            }
        },
    };
    let mut chord = key(code);
    for modifier in parts {
        match modifier {
            "ctrl" | "control" => chord.ctrl = true,
            "shift" => chord.shift = true,
            "alt" | "option" | "meta" => chord.alt = true,
            _ => return None,
        }
    }
    Some(chord)
}

impl Keymap {
    pub fn new(distinguishes_shift: bool) -> Self {
        Self {
            distinguishes_shift,
            user: Vec::new(),
        }
    }

    pub fn with_bindings(
        mut self,
        bindings: &[(String, String)],
        warnings: &mut Vec<String>,
    ) -> Self {
        for (text, name) in bindings {
            let Some(chord) = parse_chord(text) else {
                warnings.push(format!("keybindings: {text} is not a key combination"));
                continue;
            };
            let action = match name.as_str() {
                "none" => None,
                id => match Action::from_id(id) {
                    Some(action) => Some(action),
                    None => {
                        warnings.push(format!("keybindings: unknown action {id}"));
                        continue;
                    }
                },
            };
            self.user.push((chord, action, text.clone()));
        }
        self
    }

    pub fn label(&self, action: Action) -> Option<&str> {
        let chosen = self.user.iter().find(|(_, a, _)| *a == Some(action));
        if let Some((_, _, label)) = chosen {
            return Some(label);
        }
        if action == Action::Quit {
            return Some(if self.distinguishes_shift {
                "Ctrl+Shift+Q"
            } else {
                "Ctrl+Q"
            });
        }
        let taken = |chord: &Chord| self.user.iter().any(|(c, _, _)| c == chord);
        let mut bound = BINDINGS
            .iter()
            .filter(|(c, a, _)| *a == action && !taken(c));
        let first = bound.clone().next();
        let deliverable = |(c, _, _): &&(Chord, Action, &str)| !(c.alt || c.ctrl && c.shift);
        let preferred = if self.distinguishes_shift {
            first
        } else {
            bound.find(deliverable).or(first)
        };
        preferred.map(|(_, _, l)| *l)
    }

    pub fn resolve(&self, event: &Event, focus: Focus, page_rows: usize) -> Option<Command> {
        match event {
            Event::Key(key) if key.kind != KeyEventKind::Release => {
                self.key(*key, focus, page_rows)
            }
            Event::Paste(text) => Some(Command::Insert(text.clone())),
            _ => None,
        }
    }

    pub fn leaves_terminal(&self, key: KeyEvent, focus: Focus) -> Option<Command> {
        if key.kind == KeyEventKind::Release {
            return None;
        }
        if focus == Focus::Terminal && key.modifiers == KeyModifiers::ALT {
            let action = match key.code {
                KeyCode::Char('w') => Some(Action::TerminalCloseTab),
                KeyCode::PageUp => Some(Action::TerminalPrevious),
                KeyCode::PageDown => Some(Action::TerminalNext),
                _ => None,
            };
            if let Some(action) = action {
                return Some(Command::Run(action));
            }
        }
        let leaves = match key.code {
            KeyCode::F(9 | 10) => key.modifiers.is_empty(),
            KeyCode::F(12) => key.modifiers == KeyModifiers::ALT,
            KeyCode::Char('t') => key.modifiers == KeyModifiers::ALT,
            KeyCode::Char(c) if c.is_ascii_digit() => key.modifiers == KeyModifiers::ALT,
            KeyCode::Char('q') => key.modifiers == KeyModifiers::CONTROL,
            KeyCode::Esc => key.modifiers == KeyModifiers::SHIFT,
            _ => false,
        };
        if leaves {
            self.key(key, focus, 1)
        } else {
            None
        }
    }

    fn key(&self, key: KeyEvent, focus: Focus, page_rows: usize) -> Option<Command> {
        let ctrl = key.modifiers.contains(KeyModifiers::CONTROL);
        let alt = key.modifiers.contains(KeyModifiers::ALT);
        let shift = key.modifiers.contains(KeyModifiers::SHIFT)
            || matches!(key.code, KeyCode::Char(c) if c.is_uppercase());
        let code = match key.code {
            KeyCode::Char(c) if ctrl || alt => KeyCode::Char(c.to_ascii_lowercase()),
            other => other,
        };
        let chord = Chord {
            code,
            ctrl,
            shift,
            alt,
        };
        if let Some((_, action, _)) = self.user.iter().find(|(c, _, _)| *c == chord) {
            return action.map(Command::Run);
        }
        if ctrl && code == KeyCode::Char('q') && (shift || !self.distinguishes_shift) {
            return Some(Command::Run(Action::Quit));
        }
        if let Some((_, action, _)) = BINDINGS.iter().find(|(c, _, _)| *c == chord) {
            return Some(Command::Run(*action));
        }
        let rows = page_rows.max(1);
        let command = match key.code {
            KeyCode::Enter if shift && focus == Focus::FindBar => {
                Command::Run(Action::FindPrevious)
            }
            KeyCode::Char(c) if !ctrl && !alt && key.state.contains(KeyEventState::CAPS_LOCK) => {
                Command::Insert(c.to_uppercase().collect())
            }
            KeyCode::Char(c) if !ctrl && !alt => Command::Insert(c.to_string()),
            KeyCode::Char('b') if alt && !ctrl => Command::Move(Motion::WordLeft),
            KeyCode::Char('f') if alt && !ctrl => Command::Move(Motion::WordRight),
            KeyCode::Left if ctrl => Command::Move(Motion::WordLeft),
            KeyCode::Right if ctrl => Command::Move(Motion::WordRight),
            KeyCode::Enter => Command::Newline,
            KeyCode::Esc => Command::Escape,
            KeyCode::Tab => Command::Indent,
            KeyCode::BackTab => Command::Outdent,
            KeyCode::Backspace => Command::DeleteBackward,
            KeyCode::Delete => Command::DeleteForward,
            KeyCode::Left => Command::Move(Motion::Left),
            KeyCode::Right => Command::Move(Motion::Right),
            KeyCode::Up => Command::Move(Motion::Up),
            KeyCode::Down => Command::Move(Motion::Down),
            KeyCode::Home if ctrl => Command::Move(Motion::DocStart),
            KeyCode::End if ctrl => Command::Move(Motion::DocEnd),
            KeyCode::Home => Command::Move(Motion::LineStart),
            KeyCode::End => Command::Move(Motion::LineEnd),
            KeyCode::PageUp => Command::Move(Motion::PageUp { rows }),
            KeyCode::PageDown => Command::Move(Motion::PageDown { rows }),
            _ => return None,
        };
        let command = match command {
            Command::Move(motion) if shift && focus == Focus::Editor => Command::Select(motion),
            other => other,
        };
        Some(command)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn press(code: KeyCode, modifiers: KeyModifiers) -> Event {
        Event::Key(KeyEvent::new(code, modifiers))
    }

    fn resolve(km: &Keymap, code: KeyCode, modifiers: KeyModifiers) -> Option<Command> {
        km.resolve(&press(code, modifiers), Focus::Editor, 10)
    }

    #[test]
    fn plain_and_shifted_chars_insert_text() {
        let km = Keymap::new(true);
        assert_eq!(
            resolve(&km, KeyCode::Char('a'), KeyModifiers::NONE),
            Some(Command::Insert("a".into()))
        );
        assert_eq!(
            resolve(&km, KeyCode::Char('A'), KeyModifiers::SHIFT),
            Some(Command::Insert("A".into()))
        );
    }

    #[test]
    fn ctrl_shift_chords_match_whatever_case_the_terminal_sends() {
        let km = Keymap::new(true);
        let cs = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        for c in ['a', 'A'] {
            assert_eq!(
                resolve(&km, KeyCode::Char(c), cs),
                Some(Command::Run(Action::OpenPalette))
            );
        }
        assert_eq!(
            resolve(&km, KeyCode::Char('z'), cs),
            Some(Command::Run(Action::Redo))
        );
        assert_eq!(
            resolve(&km, KeyCode::Char('z'), KeyModifiers::CONTROL),
            Some(Command::Run(Action::Undo))
        );
    }

    #[test]
    fn quick_documentation_never_takes_quit_from_legacy_terminals() {
        let hover = Some(Command::Run(Action::QuickDocumentation));
        let ctrl_q = |km: &Keymap| resolve(km, KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(ctrl_q(&Keymap::new(true)), hover);
        assert_eq!(
            ctrl_q(&Keymap::new(false)),
            Some(Command::Run(Action::Quit))
        );
        let km = Keymap::new(false);
        assert_eq!(resolve(&km, KeyCode::F(1), KeyModifiers::NONE), hover);
        assert_eq!(resolve(&km, KeyCode::Char('q'), KeyModifiers::ALT), hover);
        assert_eq!(km.label(Action::QuickDocumentation), Some("F1"));
    }

    #[test]
    fn completion_has_a_chord_macos_leaves_alone() {
        let km = Keymap::new(false);
        let run = Some(Command::Run(Action::Completion));
        assert_eq!(resolve(&km, KeyCode::Char(' '), KeyModifiers::CONTROL), run);
        assert_eq!(resolve(&km, KeyCode::Char('/'), KeyModifiers::ALT), run);
    }

    #[test]
    fn usages_and_formatting_have_their_idea_chords_and_a_spare() {
        let km = Keymap::new(false);
        let usages = Some(Command::Run(Action::FindUsages));
        let format = Some(Command::Run(Action::ReformatCode));
        assert_eq!(resolve(&km, KeyCode::F(7), KeyModifiers::ALT), usages);
        assert_eq!(resolve(&km, KeyCode::Char('u'), KeyModifiers::ALT), usages);
        let ctrl_alt = KeyModifiers::CONTROL | KeyModifiers::ALT;
        assert_eq!(resolve(&km, KeyCode::Char('l'), ctrl_alt), format);
        assert_eq!(resolve(&km, KeyCode::Char('l'), KeyModifiers::ALT), format);
    }

    #[test]
    fn alt_enter_shows_context_actions_and_plain_enter_stays_a_newline() {
        let km = Keymap::new(false);
        assert_eq!(
            resolve(&km, KeyCode::Enter, KeyModifiers::ALT),
            Some(Command::Run(Action::ContextActions))
        );
        assert_eq!(
            resolve(&km, KeyCode::Enter, KeyModifiers::NONE),
            Some(Command::Newline)
        );
    }

    #[test]
    fn shift_selects_in_the_editor_and_moves_elsewhere() {
        let km = Keymap::new(true);
        let press = |code, focus| {
            let event = Event::Key(KeyEvent::new(code, KeyModifiers::SHIFT));
            km.resolve(&event, focus, 10)
        };
        assert_eq!(
            press(KeyCode::Left, Focus::Editor),
            Some(Command::Select(Motion::Left))
        );
        assert_eq!(
            press(KeyCode::End, Focus::Editor),
            Some(Command::Select(Motion::LineEnd))
        );
        assert_eq!(
            press(KeyCode::Left, Focus::FindBar),
            Some(Command::Move(Motion::Left))
        );
        let word = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        let event = Event::Key(KeyEvent::new(KeyCode::Right, word));
        assert_eq!(
            km.resolve(&event, Focus::Editor, 10),
            Some(Command::Select(Motion::WordRight))
        );
    }

    #[test]
    fn caps_lock_reported_as_a_state_still_types_capitals() {
        let km = Keymap::new(true);
        let mut key = KeyEvent::new(KeyCode::Char('a'), KeyModifiers::NONE);
        key.state = KeyEventState::CAPS_LOCK;
        assert_eq!(
            km.resolve(&Event::Key(key), Focus::Editor, 1),
            Some(Command::Insert("A".into()))
        );
    }

    #[test]
    fn new_navigation_chords_work_without_the_kitty_protocol() {
        let km = Keymap::new(false);
        for (c, action) in [('e', Action::RecentFiles), ('n', Action::SearchEverywhere)] {
            assert_eq!(
                resolve(&km, KeyCode::Char(c), KeyModifiers::CONTROL),
                Some(Command::Run(action))
            );
            assert!(km.label(action).is_some());
        }
    }

    #[test]
    fn find_options_have_chords_of_their_own() {
        let km = Keymap::new(false);
        for (c, action) in [
            ('r', Action::ToggleFindRegex),
            ('c', Action::ToggleFindCase),
            ('w', Action::ToggleFindWord),
        ] {
            assert_eq!(
                resolve(&km, KeyCode::Char(c), KeyModifiers::ALT),
                Some(Command::Run(action))
            );
        }
    }

    #[test]
    fn replace_in_files_has_a_chord_for_legacy_terminals() {
        let run = Some(Command::Run(Action::ReplaceInFiles));
        let cs = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert_eq!(resolve(&Keymap::new(true), KeyCode::Char('r'), cs), run);
        let legacy = Keymap::new(false);
        assert_eq!(
            resolve(&legacy, KeyCode::Char('l'), KeyModifiers::CONTROL),
            run
        );
        assert_eq!(legacy.label(Action::ReplaceInFiles), Some("Ctrl+L"));
        assert_eq!(
            resolve(&legacy, KeyCode::Char('r'), KeyModifiers::CONTROL),
            Some(Command::Run(Action::Replace)),
            "what Ctrl+Shift+R becomes there, as the compat notes say"
        );
    }

    #[test]
    fn the_tool_window_resizes_with_arrows_or_with_plain_keys() {
        let km = Keymap::new(false);
        let chord = KeyModifiers::ALT | KeyModifiers::SHIFT;
        let taller = Some(Command::Run(Action::ToolWindowTaller));
        let shorter = Some(Command::Run(Action::ToolWindowShorter));
        assert_eq!(resolve(&km, KeyCode::Up, chord), taller);
        assert_eq!(resolve(&km, KeyCode::Down, chord), shorter);
        assert_eq!(resolve(&km, KeyCode::Char('='), KeyModifiers::ALT), taller);
        assert_eq!(resolve(&km, KeyCode::Char('-'), KeyModifiers::ALT), shorter);
        assert_eq!(
            resolve(&km, KeyCode::Up, KeyModifiers::SHIFT),
            Some(Command::Select(Motion::Up))
        );
    }

    #[test]
    fn no_two_default_bindings_share_a_chord() {
        for (i, (chord, _, label)) in BINDINGS.iter().enumerate() {
            let again = BINDINGS[i + 1..].iter().find(|(c, _, _)| c == chord);
            assert!(again.is_none(), "{label} is bound twice");
        }
    }

    #[test]
    fn replacing_and_tab_have_their_keys() {
        let km = Keymap::new(false);
        assert_eq!(
            resolve(&km, KeyCode::Char('r'), KeyModifiers::CONTROL),
            Some(Command::Run(Action::Replace))
        );
        assert_eq!(
            resolve(&km, KeyCode::Char('a'), KeyModifiers::ALT),
            Some(Command::Run(Action::ReplaceAll))
        );
        assert_eq!(
            resolve(&km, KeyCode::Tab, KeyModifiers::NONE),
            Some(Command::Indent)
        );
        assert_eq!(
            resolve(&km, KeyCode::BackTab, KeyModifiers::SHIFT),
            Some(Command::Outdent)
        );
    }

    #[test]
    fn running_has_intellij_chords_and_ones_every_terminal_can_send() {
        let cs = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        let km = Keymap::new(false);
        for (code, modifiers, action) in [
            (KeyCode::F(10), cs, Action::RunChoose),
            (
                KeyCode::F(10),
                KeyModifiers::ALT | KeyModifiers::SHIFT,
                Action::RunChoose,
            ),
            (KeyCode::Char('j'), KeyModifiers::CONTROL, Action::RunChoose),
            (KeyCode::F(10), KeyModifiers::SHIFT, Action::RerunLast),
            (KeyCode::F(5), KeyModifiers::CONTROL, Action::RerunLast),
            (KeyCode::Char('u'), KeyModifiers::CONTROL, Action::RerunLast),
            (KeyCode::F(2), KeyModifiers::CONTROL, Action::StopRun),
            (KeyCode::Char('s'), KeyModifiers::ALT, Action::StopRun),
            (KeyCode::Char('4'), KeyModifiers::ALT, Action::ShowRun),
            (KeyCode::F(4), KeyModifiers::NONE, Action::NextRunError),
            (KeyCode::Char('n'), KeyModifiers::ALT, Action::NextRunError),
            (KeyCode::F(4), KeyModifiers::SHIFT, Action::PreviousRunError),
            (
                KeyCode::Char('p'),
                KeyModifiers::ALT,
                Action::PreviousRunError,
            ),
        ] {
            assert_eq!(resolve(&km, code, modifiers), Some(Command::Run(action)));
        }
        assert_eq!(km.label(Action::RunChoose), Some("Ctrl+J"));
        assert_eq!(
            Keymap::new(true).label(Action::RunChoose),
            Some("Ctrl+Shift+F10")
        );
        assert_eq!(
            resolve(&km, KeyCode::F(4), KeyModifiers::CONTROL),
            Some(Command::Run(Action::CloseTab))
        );
    }

    #[test]
    fn git_has_its_tool_window_chords() {
        let km = Keymap::new(true);
        for (c, action) in [('9', Action::GitLog), ('0', Action::GitCommit)] {
            assert_eq!(
                resolve(&km, KeyCode::Char(c), KeyModifiers::ALT),
                Some(Command::Run(action))
            );
        }
        let chord = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert_eq!(
            resolve(&km, KeyCode::Char('k'), chord),
            Some(Command::Run(Action::GitPush))
        );
    }

    #[test]
    fn navigation_has_chords_every_terminal_can_send() {
        let km = Keymap::new(false);
        for (code, modifiers, action) in [
            (
                KeyCode::Char('b'),
                KeyModifiers::CONTROL,
                Action::GotoDefinition,
            ),
            (KeyCode::F(12), KeyModifiers::NONE, Action::GotoDefinition),
            (KeyCode::Char(','), KeyModifiers::ALT, Action::NavigateBack),
            (
                KeyCode::Left,
                KeyModifiers::CONTROL | KeyModifiers::ALT,
                Action::NavigateBack,
            ),
        ] {
            assert_eq!(resolve(&km, code, modifiers), Some(Command::Run(action)));
        }
    }

    #[test]
    fn quit_needs_shift_only_when_the_terminal_can_report_it() {
        let ctrl_q = |km: &Keymap| resolve(km, KeyCode::Char('q'), KeyModifiers::CONTROL);
        assert_eq!(
            ctrl_q(&Keymap::new(true)),
            Some(Command::Run(Action::QuickDocumentation))
        );
        assert_eq!(
            ctrl_q(&Keymap::new(false)),
            Some(Command::Run(Action::Quit))
        );
        let cs = KeyModifiers::CONTROL | KeyModifiers::SHIFT;
        assert_eq!(
            resolve(&Keymap::new(true), KeyCode::Char('Q'), cs),
            Some(Command::Run(Action::Quit))
        );
    }

    #[test]
    fn alt_one_toggles_project_and_paste_inserts() {
        let km = Keymap::new(false);
        assert_eq!(
            resolve(&km, KeyCode::Char('1'), KeyModifiers::ALT),
            Some(Command::Run(Action::ToggleProject))
        );
        assert_eq!(
            km.resolve(&Event::Paste("a\nb".into()), Focus::Editor, 1),
            Some(Command::Insert("a\nb".into()))
        );
    }

    #[test]
    fn option_arrows_of_terminal_app_move_by_word_instead_of_opening_search() {
        let km = Keymap::new(false);
        for (c, motion) in [('b', Motion::WordLeft), ('f', Motion::WordRight)] {
            assert_eq!(
                resolve(&km, KeyCode::Char(c), KeyModifiers::ALT),
                Some(Command::Move(motion))
            );
        }
    }

    #[test]
    fn ctrl_arrows_move_by_word_and_alt_arrows_keep_switching_tabs() {
        let km = Keymap::new(true);
        assert_eq!(
            resolve(&km, KeyCode::Right, KeyModifiers::CONTROL),
            Some(Command::Move(Motion::WordRight))
        );
        assert_eq!(
            resolve(&km, KeyCode::Left, KeyModifiers::CONTROL),
            Some(Command::Move(Motion::WordLeft))
        );
        assert_eq!(
            resolve(&km, KeyCode::Right, KeyModifiers::ALT),
            Some(Command::Run(Action::NextTab))
        );
    }

    #[test]
    fn project_and_project_search_have_chords_without_alt_or_shift() {
        let km = Keymap::new(false);
        for (c, action) in [('t', Action::ToggleProject), ('g', Action::FindInProject)] {
            assert_eq!(
                resolve(&km, KeyCode::Char(c), KeyModifiers::CONTROL),
                Some(Command::Run(action))
            );
        }
    }

    #[test]
    fn labels_show_a_chord_the_terminal_can_deliver() {
        let legacy = Keymap::new(false);
        assert_eq!(legacy.label(Action::ToggleProject), Some("Ctrl+T"));
        assert_eq!(legacy.label(Action::FindInProject), Some("Ctrl+G"));
        assert_eq!(legacy.label(Action::OpenPalette), Some("Ctrl+K"));
        assert_eq!(legacy.label(Action::NextTab), Some("Alt+→"));
        let modern = Keymap::new(true);
        assert_eq!(modern.label(Action::ToggleProject), Some("Alt+1"));
        assert_eq!(modern.label(Action::FindInProject), Some("Ctrl+Shift+F"));
    }

    #[test]
    fn shift_enter_goes_back_only_in_the_find_bar() {
        let km = Keymap::new(true);
        let ev = press(KeyCode::Enter, KeyModifiers::SHIFT);
        assert_eq!(
            km.resolve(&ev, Focus::FindBar, 1),
            Some(Command::Run(Action::FindPrevious))
        );
        assert_eq!(km.resolve(&ev, Focus::Editor, 1), Some(Command::Newline));
    }

    #[test]
    fn every_action_except_rarely_used_ones_has_a_label() {
        let km = Keymap::new(true);
        assert_eq!(km.label(Action::OpenPalette), Some("Ctrl+Shift+A"));
        assert_eq!(km.label(Action::Quit), Some("Ctrl+Shift+Q"));
        assert_eq!(km.label(Action::SaveAll), None);
    }

    fn with(bindings: &[(&str, &str)]) -> (Keymap, Vec<String>) {
        let owned: Vec<_> = bindings
            .iter()
            .map(|(c, a)| ((*c).to_owned(), (*a).to_owned()))
            .collect();
        let mut warnings = Vec::new();
        (
            Keymap::new(true).with_bindings(&owned, &mut warnings),
            warnings,
        )
    }

    #[test]
    fn a_user_binding_runs_its_action_and_labels_it() {
        let (km, warnings) = with(&[("Ctrl+Alt+J", "lsp.definition")]);
        assert!(warnings.is_empty());
        let chord = KeyModifiers::CONTROL | KeyModifiers::ALT;
        assert_eq!(
            resolve(&km, KeyCode::Char('j'), chord),
            Some(Command::Run(Action::GotoDefinition))
        );
        assert_eq!(km.label(Action::GotoDefinition), Some("Ctrl+Alt+J"));
        assert_eq!(
            resolve(&km, KeyCode::Char('b'), KeyModifiers::CONTROL),
            Some(Command::Run(Action::GotoDefinition)),
            "the default keeps working too"
        );
    }

    #[test]
    fn a_user_binding_wins_over_a_default_even_quit() {
        let legacy = Keymap::new(false);
        let mut warnings = Vec::new();
        let km = legacy.with_bindings(
            &[("ctrl+q".to_owned(), "lsp.hover".to_owned())],
            &mut warnings,
        );
        assert_eq!(
            resolve(&km, KeyCode::Char('q'), KeyModifiers::CONTROL),
            Some(Command::Run(Action::QuickDocumentation))
        );
    }

    #[test]
    fn none_frees_a_chord_and_drops_its_label() {
        let (km, _) = with(&[("ctrl+k", "none")]);
        assert_eq!(
            resolve(&km, KeyCode::Char('k'), KeyModifiers::CONTROL),
            None
        );
        let km = Keymap {
            distinguishes_shift: false,
            ..km
        };
        assert_eq!(km.label(Action::OpenPalette), Some("Ctrl+Shift+A"));
    }

    #[test]
    fn named_keys_and_modifiers_parse() {
        let (km, warnings) = with(&[
            ("f5", "lsp.hover"),
            ("alt+enter", "lsp.hover"),
            ("ctrl+space", "lsp.hover"),
        ]);
        assert!(warnings.is_empty());
        let hover = Some(Command::Run(Action::QuickDocumentation));
        assert_eq!(resolve(&km, KeyCode::F(5), KeyModifiers::NONE), hover);
        assert_eq!(resolve(&km, KeyCode::Enter, KeyModifiers::ALT), hover);
        assert_eq!(
            resolve(&km, KeyCode::Char(' '), KeyModifiers::CONTROL),
            hover
        );
    }

    #[test]
    fn bad_bindings_warn_and_change_nothing() {
        let (km, warnings) = with(&[
            ("ctrl+nonsense", "lsp.hover"),
            ("hyper+a", "lsp.hover"),
            ("f13", "lsp.hover"),
            ("ctrl+d", "no.such.action"),
        ]);
        assert_eq!(warnings.len(), 4, "{warnings:?}");
        assert_eq!(
            resolve(&km, KeyCode::Char('d'), KeyModifiers::CONTROL),
            None
        );
    }

    #[test]
    fn a_terminal_keeps_nearly_every_key_for_the_shell() {
        let km = Keymap::new(true);
        let chord = |code, modifiers| KeyEvent::new(code, modifiers);
        let leaves = |code, modifiers, focus| km.leaves_terminal(chord(code, modifiers), focus);
        let t = Focus::Terminal;
        for (code, modifiers) in [
            (KeyCode::Esc, KeyModifiers::NONE),
            (KeyCode::Char('c'), KeyModifiers::CONTROL),
            (KeyCode::Char('r'), KeyModifiers::CONTROL),
            (KeyCode::Char('t'), KeyModifiers::CONTROL),
            (KeyCode::Char('b'), KeyModifiers::ALT),
            (KeyCode::F(3), KeyModifiers::NONE),
        ] {
            assert_eq!(leaves(code, modifiers, t), None, "{code:?}");
        }
        let run = |action| Some(Command::Run(action));
        assert_eq!(
            leaves(KeyCode::F(12), KeyModifiers::ALT, t),
            run(Action::TerminalToggle)
        );
        assert_eq!(
            leaves(KeyCode::Char('t'), KeyModifiers::ALT, t),
            run(Action::TerminalNew)
        );
        assert_eq!(
            leaves(KeyCode::Char('w'), KeyModifiers::ALT, t),
            run(Action::TerminalCloseTab)
        );
        assert_eq!(
            leaves(KeyCode::PageDown, KeyModifiers::ALT, t),
            run(Action::TerminalNext)
        );
        assert_eq!(
            leaves(KeyCode::Char('w'), KeyModifiers::ALT, Focus::Claude),
            None,
            "only tabs of the terminal"
        );
    }
}
