use crate::core::git_command::GitEvent;
use crate::core::lsp_command::{LspEffect, LspEvent};
use std::path::PathBuf;

use ratatui::Terminal;
use ratatui::backend::TestBackend;

use super::*;
use crate::core::debug::{DebugEffect, DebugEvent};
use crate::core::run_state::{RunEffect, RunEvent};
use crate::core::{
    Action, Command, DirEntry, Effect, Event, Focus, Language, LspAnswer, LspCompletion, LspState,
    Motion, PositionEncoding, RawDiagnostic, ServerKey, Severity,
};

fn rust_server() -> ServerKey {
    server_for(Language::Rust)
}

fn server_for(server: Language) -> ServerKey {
    ServerKey {
        server,
        root: PathBuf::from("/proj"),
    }
}

struct Screen {
    ws: Workspace,
    ui: UiState,
    keymap: Keymap,
    clicks: u64,
}

impl Screen {
    fn new() -> Self {
        let (ws, _) = Workspace::new(PathBuf::from("/proj"));
        Self {
            ws,
            ui: UiState::default(),
            keymap: Keymap::new(true),
            clicks: 0,
        }
    }

    fn open(&mut self, name: &str, text: &str) {
        let path = PathBuf::from("/proj").join(name);
        self.ws.dispatch(Command::OpenFile {
            path: path.clone(),
            at: None,
        });
        let effects = self.ws.handle(Event::FileRead {
            path,
            at: None,
            disk: None,
            result: Ok(Some(text.as_bytes().to_vec())),
        });
        self.answer_roots(effects);
    }

    fn answer_roots(&mut self, effects: Vec<Effect>) {
        for effect in effects {
            if let Effect::Lsp(LspEffect::FindRoot {
                path,
                server,
                project,
            }) = effect
            {
                let root = project;
                self.ws.handle(Event::Lsp(LspEvent::RootFound {
                    path,
                    server: ServerKey { server, root },
                }));
            }
        }
    }

    fn run(&mut self, command: Command) {
        let effects = self.ws.dispatch(command);
        assert!(
            effects.iter().all(|e| matches!(
                e,
                Effect::Lsp(LspEffect::FindRoot { .. })
                    | Effect::QuerySymbols { .. }
                    | Effect::Lsp(LspEffect::Start { .. })
                    | Effect::Lsp(LspEffect::DidOpen { .. })
                    | Effect::Lsp(LspEffect::DidChange { .. })
                    | Effect::JournalAppend { .. }
            )),
            "test commands need no I/O: {effects:?}"
        );
    }

    fn draw(&mut self, width: u16, height: u16) -> Vec<String> {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| render(f, &self.ws, &mut self.ui, &Theme::DARCULA, &self.keymap))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        (0..height)
            .map(|y| {
                (0..width)
                    .map(|x| buf[(x, y)].symbol())
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect()
    }
}

fn find_row<'a>(rows: &'a [String], needle: &str) -> &'a str {
    rows.iter().find(|r| r.contains(needle)).map_or_else(
        || panic!("no row contains {needle:?}:\n{}", rows.join("\n")),
        String::as_str,
    )
}

#[test]
fn main_screen_has_title_tree_tabs_code_and_status() {
    let mut s = Screen::new();
    s.ws.handle(Event::DirListed {
        path: PathBuf::from("/proj"),
        result: Ok(vec![
            DirEntry {
                name: "src".into(),
                path: "/proj/src".into(),
                is_dir: true,
            },
            DirEntry {
                name: "Cargo.toml".into(),
                path: "/proj/Cargo.toml".into(),
                is_dir: false,
            },
        ]),
    });
    s.open("main.rs", "fn main() {\n}\n");
    let rows = s.draw(100, 20);
    assert!(
        rows[0]
            .starts_with(" File  Edit  View  Navigate  Code  Git  Run  Debug  Claude  Help   proj"),
        "{}",
        rows[0]
    );
    assert!(rows[1].starts_with("───┬"), "{}", rows[1]);
    assert!(rows[2].starts_with(" ◫ │ Project"), "{}", rows[2]);
    assert!(rows[2].contains("│ main.rs ×"), "{}", rows[2]);
    assert!(find_row(&rows, "▸ src").contains("▸ src"));
    assert!(find_row(&rows, "Cargo.toml").contains("Cargo.toml"));
    assert!(find_row(&rows, "1 │ fn main() {").ends_with("1 │ fn main() {"));
    let status = &rows[19];
    assert!(status.starts_with(" proj › main.rs"), "{status}");
    assert!(
        status.ends_with("1:1  LF  UTF-8  4 spaces  Rust  ⠋"),
        "{status}"
    );
}

#[test]
fn empty_editor_shows_shortcut_hints() {
    let mut s = Screen::new();
    let rows = s.draw(100, 20);
    assert!(find_row(&rows, "Go to File").contains("Ctrl+Shift+N"));
    assert!(find_row(&rows, "Find Action").contains("Ctrl+Shift+A"));
}

#[test]
fn modified_tab_shows_a_dot() {
    let mut s = Screen::new();
    s.open("a.rs", "");
    s.open("b.rs", "");
    s.run(Command::Insert("x".into()));
    let rows = s.draw(100, 20);
    assert!(rows[2].ends_with("│ a.rs  b.rs ●"), "{}", rows[2]);
}

#[test]
fn vertical_split_shows_two_editors_side_by_side() {
    let mut s = Screen::new();
    s.open("a.rs", "let x = 1;");
    s.run(Command::Run(Action::SplitVertically));
    let rows = s.draw(120, 20);
    assert_eq!(rows[2].matches("a.rs").count(), 2, "{}", rows[2]);
    assert_eq!(find_row(&rows, "let x").matches("let x = 1;").count(), 2);
}

#[test]
fn palette_lists_actions_with_shortcuts() {
    let mut s = Screen::new();
    s.run(Command::Run(Action::OpenPalette));
    s.run(Command::Insert("split".into()));
    let rows = s.draw(120, 30);
    assert!(find_row(&rows, "╭ Actions").contains('╮'));
    assert!(find_row(&rows, "❯ split").contains("❯ split"));
    let line = find_row(&rows, "Split Vertically");
    assert!(
        line.contains("Window") && line.contains("Ctrl+\\"),
        "{line}"
    );
}

#[test]
fn find_bar_counts_matches() {
    let mut s = Screen::new();
    s.open("a.rs", "foo foo foo");
    s.run(Command::Run(Action::Find));
    s.run(Command::Insert("foo".into()));
    s.run(Command::Run(Action::FindNext));
    let rows = s.draw(100, 20);
    assert!(find_row(&rows, "❯ foo").contains("❯ foo   2/3"));
}

#[test]
fn find_in_files_tool_window_shows_grouped_results() {
    let mut s = Screen::new();
    s.run(Command::Run(Action::FindInProject));
    s.run(Command::Insert("needle".into()));
    let fx = s.ws.dispatch(Command::Newline);
    let [crate::core::Effect::SearchProject { id, .. }] = fx.as_slice() else {
        panic!("{fx:?}");
    };
    s.ws.handle(Event::SearchFinished {
        id: *id,
        matches: vec![crate::core::SearchMatch {
            path: "/proj/src/lib.rs".into(),
            line: 41,
            col: 4,
            len: 6,
            text: "    let needle = 1;".into(),
        }],
        truncated: false,
    });
    s.run(Command::Move(Motion::Down));
    let rows = s.draw(100, 30);
    assert!(find_row(&rows, "Find   1 matches in 1 files").contains("Find"));
    assert!(
        find_row(&rows, "┴").starts_with("   ├"),
        "tool window joins the stripe"
    );
    assert!(find_row(&rows, "src/lib.rs").contains("▪ src/lib.rs  1"));
    assert!(find_row(&rows, "let needle").contains("▌     42  let needle = 1;"));
}

#[test]
fn tiny_terminal_shows_placeholder() {
    let mut s = Screen::new();
    let rows = s.draw(30, 8);
    assert_eq!(rows[3].trim(), "Terminal is too small");
    assert_eq!(rows[4].trim(), "Need at least 40×10, now 30×8");
}

#[test]
fn narrow_terminal_hides_the_project_panel() {
    let mut s = Screen::new();
    s.open("a.rs", "x");
    let rows = s.draw(70, 12);
    assert!(!rows[2].contains("Project"), "{}", rows[2]);
}

#[test]
fn ensure_visible_moves_minimally() {
    assert_eq!(ensure_visible((10, 0), (12, 3), (5, 20)), (10, 0));
    assert_eq!(ensure_visible((10, 0), (4, 3), (5, 20)), (4, 0));
    assert_eq!(ensure_visible((10, 0), (15, 3), (5, 20)), (11, 0));
    assert_eq!(ensure_visible((0, 0), (0, 25), (5, 20)), (0, 6));
}

#[test]
fn local_history_panel_lists_versions_and_shows_the_diff() {
    let mut s = Screen::new();
    s.open("a.rs", "new line\n");
    let doc = s.ws.active().unwrap().0.doc;
    let entry = |label, timestamp| crate::core::HistoryEntry {
        timestamp,
        path: PathBuf::from("/proj/a.rs"),
        hash: "h".to_owned(),
        label,
    };
    s.ws.handle(Event::HistoryListed {
        doc,
        entries: vec![
            entry(crate::core::SnapshotLabel::Autosave, 1000),
            entry(crate::core::SnapshotLabel::Opened, 400),
        ],
        now: 1060,
    });
    let loading = s.draw(100, 24).join("\n");
    assert!(loading.contains("Local History — a.rs"));
    assert!(loading.contains("1 min ago") && loading.contains("Autosave"));
    assert!(loading.contains("Loading"));

    s.ws.handle(Event::HistoryVersion {
        hash: "h".to_owned(),
        result: Ok(b"old line\n".to_vec()),
    });
    let screen = s.draw(100, 24).join("\n");
    assert!(screen.contains("- old line"));
    assert!(screen.contains("+ new line"));
    assert!(screen.contains("R revert"));
    assert!(!screen.contains("Project"), "the panel covers the screen");
}

#[test]
fn local_history_panel_absorbs_clicks_and_fits_small_screens() {
    let mut s = Screen::new();
    s.open("a.rs", "x\n");
    let doc = s.ws.active().unwrap().0.doc;
    s.ws.handle(Event::HistoryListed {
        doc,
        entries: vec![crate::core::HistoryEntry {
            timestamp: 1,
            path: PathBuf::from("/proj/a.rs"),
            hash: "h".to_owned(),
            label: crate::core::SnapshotLabel::Save,
        }],
        now: 1,
    });
    s.draw(40, 10);
    use super::super::mouse::Hit;
    assert_eq!(s.ui.hits.at(39, 8), Some(&Hit::History));
    assert_eq!(s.ui.hits.at(1, 2), Some(&Hit::HistoryEntry { index: 0 }));
}

#[test]
fn diff_tint_covers_the_whole_row() {
    let mut s = Screen::new();
    s.open("a.rs", "new\n");
    let doc = s.ws.active().unwrap().0.doc;
    s.ws.handle(Event::HistoryListed {
        doc,
        entries: vec![crate::core::HistoryEntry {
            timestamp: 1,
            path: PathBuf::from("/proj/a.rs"),
            hash: "h".to_owned(),
            label: crate::core::SnapshotLabel::Save,
        }],
        now: 1,
    });
    s.ws.handle(Event::HistoryVersion {
        hash: "h".to_owned(),
        result: Ok(b"old\n".to_vec()),
    });
    let theme = Theme::DARCULA;
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    terminal
        .draw(|f| render(f, &s.ws, &mut s.ui, &theme, &s.keymap))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let y = (0..24)
        .find(|y| {
            (0..100)
                .map(|x| buffer[(x, *y)].symbol())
                .collect::<String>()
                .contains("- old")
        })
        .unwrap();
    assert_eq!(buffer[(99, y)].bg, theme.diff_removed.bg.unwrap());
    assert_eq!(buffer[(40, y)].bg, theme.diff_removed.bg.unwrap());
    assert_eq!(buffer[(99, y + 1)].bg, theme.diff_added.bg.unwrap());
}

impl Screen {
    fn diagnose(&mut self, name: &str, severity: Severity, message: &str) {
        self.ws.handle(Event::Lsp(LspEvent::Diagnostics {
            server: rust_server(),
            path: PathBuf::from("/proj").join(name),
            items: vec![RawDiagnostic {
                severity,
                message: message.to_owned(),
                start: (0, 0),
                end: (0, 2),
            }],
        }));
    }
}

#[test]
fn status_bar_shows_problem_counts_and_server_state() {
    let mut s = Screen::new();
    s.open("a.rs", "fn main() {}\n");
    let status = |s: &mut Screen| s.draw(100, 24).last().cloned().unwrap();
    s.ws.handle(Event::Lsp(LspEvent::Ready {
        server: rust_server(),
        encoding: PositionEncoding::Utf8,
        completion_triggers: vec![".".to_owned()],
    }));
    assert!(status(&mut s).ends_with('✓'));
    s.diagnose("a.rs", Severity::Error, "boom");
    assert!(status(&mut s).contains("✖ 1"));
    s.ws.handle(Event::Lsp(LspEvent::StateChanged {
        server: rust_server(),
        state: LspState::NotFound,
    }));
    assert!(status(&mut s).contains("rust-analyzer not found"));
    s.ws.handle(Event::Lsp(LspEvent::StateChanged {
        server: rust_server(),
        state: LspState::Crashed,
    }));
    assert!(status(&mut s).contains("rust-analyzer crashed"));
}

#[test]
fn diagnostic_is_underlined_in_its_severity_color() {
    let mut s = Screen::new();
    s.open("a.rs", "fn main() {}\n");
    s.diagnose("a.rs", Severity::Error, "boom");
    s.ws.handle(Event::Lsp(LspEvent::Ready {
        server: rust_server(),
        encoding: PositionEncoding::Utf8,
        completion_triggers: vec![".".to_owned()],
    }));
    s.diagnose("a.rs", Severity::Error, "boom");
    let _ = s.ws.dispatch(Command::Move(Motion::Right));
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    let theme = Theme::DARCULA;
    terminal
        .draw(|f| render(f, &s.ws, &mut s.ui, &theme, &s.keymap))
        .unwrap();
    let buffer = terminal.backend().buffer();
    let (x, y) = (0..24)
        .find_map(|y| {
            let line: String = (0..100).map(|x| buffer[(x, y)].symbol()).collect();
            line.find("fn main").map(|i| {
                let chars = line[..i].chars().count();
                (u16::try_from(chars).unwrap(), y)
            })
        })
        .unwrap();
    let cell = &buffer[(x, y)];
    assert_eq!(cell.fg, theme.error);
    assert!(cell.modifier.contains(ratatui::style::Modifier::UNDERLINED));
    assert!(
        !buffer[(x + 3, y)]
            .modifier
            .contains(ratatui::style::Modifier::UNDERLINED),
        "the rest of the line is untouched"
    );
}

#[test]
fn problems_panel_lists_each_problem_with_its_place() {
    let mut s = Screen::new();
    s.open("a.rs", "x\n");
    s.diagnose("a.rs", Severity::Warning, "unused thing\nsecond line");
    let _ = s.ws.dispatch(Command::Run(Action::ShowProblems));
    let screen = s.draw(100, 24).join("\n");
    assert!(screen.contains("Problems"));
    assert!(screen.contains("▲ unused thing"));
    assert!(screen.contains("a.rs:1"));
    assert!(!screen.contains("second line"));
}

#[test]
fn linter_findings_show_their_source_and_the_question_shows_the_whole_command() {
    use crate::core::analysis::{
        AnalysisCommand, AnalysisOutcome, AnalysisPlan, AnalysisTool, Finding,
    };
    let mut s = Screen::new();
    s.open("a.rs", "x\n");
    s.ws.handle(Event::Analysis(
        crate::core::analysis::AnalysisEvent::Finished {
            tool: AnalysisTool::Clippy,
            outcome: AnalysisOutcome::Found(vec![Finding {
                path: PathBuf::from("/proj/a.rs"),
                line: 0,
                col: 0,
                severity: Severity::Warning,
                message: "needless borrow".to_owned(),
            }]),
        },
    ));
    let _ = s.ws.dispatch(Command::Run(Action::ShowProblems));
    let screen = s.draw(100, 24).join("\n");
    assert!(screen.contains("needless borrow"));
    assert!(screen.contains("[clippy] a.rs:1"), "{screen}");

    let _ = s.ws.dispatch(Command::Run(Action::AnalyzeProject));
    s.ws.handle(Event::Analysis(
        crate::core::analysis::AnalysisEvent::Planned {
            plan: AnalysisPlan {
                commands: vec![AnalysisCommand {
                    tool: AnalysisTool::Eslint,
                    program: PathBuf::from("/proj/node_modules/.bin/eslint"),
                    args: vec!["--format".to_owned(), "json".to_owned()],
                    from_project: true,
                }],
                missing: Vec::new(),
                approved: false,
            },
        },
    ));
    let screen = s.draw(100, 24).join("\n");
    assert!(
        screen.contains("/proj/node_modules/.bin/eslint --format json"),
        "{screen}"
    );
    assert!(screen.contains("part of the project"), "{screen}");
}

#[test]
fn empty_problems_panel_says_so() {
    let mut s = Screen::new();
    s.open("a.rs", "x\n");
    let _ = s.ws.dispatch(Command::Run(Action::ShowProblems));
    assert!(s.draw(100, 24).join("\n").contains("No problems found"));
}

impl Screen {
    fn hover(&mut self, text: &str, cursor_line: usize, markdown: &str) {
        self.open("a.rs", text);
        self.ws.handle(Event::Lsp(LspEvent::Ready {
            server: rust_server(),
            encoding: PositionEncoding::Utf8,
            completion_triggers: vec![".".to_owned()],
        }));
        for _ in 0..cursor_line {
            self.ws.dispatch(Command::Move(Motion::Down));
        }
        let asked = self.ws.dispatch(Command::Run(Action::QuickDocumentation));
        let id = asked
            .iter()
            .find_map(|e| match e {
                Effect::Lsp(LspEffect::Query { id, .. }) => Some(*id),
                _ => None,
            })
            .unwrap();
        self.ws.handle(Event::Lsp(LspEvent::Answered {
            server: rust_server(),
            id,
            result: Ok(LspAnswer::Hover(Some(markdown.to_owned()))),
        }));
    }
}

#[test]
fn hover_appears_above_the_cursor_and_keeps_its_line_visible() {
    let mut s = Screen::new();
    s.hover(
        &"code\n".repeat(15),
        10,
        "```rust\nfn shown()\n```\n\nSome docs",
    );
    let rows = s.draw(100, 24);
    let doc = rows.iter().position(|r| r.contains("fn shown()")).unwrap();
    let cursor_row = s.ui.editor_cursor.unwrap().y as usize;
    assert!(doc < cursor_row, "{}", rows.join("\n"));
    assert!(rows[cursor_row].contains("code"), "{}", rows.join("\n"));
    assert!(find_row(&rows, "Some docs").contains('│'));
}

#[test]
fn hover_goes_below_a_cursor_at_the_top() {
    let mut s = Screen::new();
    s.hover("first\nsecond\n", 0, "docs");
    let rows = s.draw(100, 24);
    let doc = rows.iter().position(|r| r.contains("docs")).unwrap();
    let cursor_row = s.ui.editor_cursor.unwrap().y as usize;
    assert!(doc > cursor_row, "{}", rows.join("\n"));
    assert!(rows[cursor_row].contains("first"));
}

#[test]
fn long_hover_scrolls_and_shows_where_it_is() {
    let mut s = Screen::new();
    let docs: Vec<String> = (0..30).map(|n| format!("row{n:02}")).collect();
    s.hover("x\n", 0, &docs.join("\n"));
    let rows = s.draw(100, 24);
    assert!(find_row(&rows, "↑↓ 12/30").contains("↑↓"));
    assert!(rows.iter().any(|r| r.contains("row00")));
    assert!(!rows.iter().any(|r| r.contains("row12")));
    s.ws.dispatch(Command::Move(Motion::Down));
    let rows = s.draw(100, 24);
    assert!(!rows.iter().any(|r| r.contains("row00")));
    assert!(rows.iter().any(|r| r.contains("row12")));
}

#[test]
fn hover_never_panics_in_tiny_terminals() {
    for (w, h) in [(40, 10), (41, 11), (60, 12), (200, 60)] {
        let mut s = Screen::new();
        s.hover("x\n", 0, &"long words here ".repeat(40));
        s.draw(w, h);
    }
}

impl Screen {
    fn popup(&mut self, text: &str, items: Vec<LspCompletion>) {
        self.open("a.rs", text);
        self.ws.handle(Event::Lsp(LspEvent::Ready {
            server: rust_server(),
            encoding: PositionEncoding::Utf8,
            completion_triggers: Vec::new(),
        }));
        let asked = self.ws.dispatch(Command::Run(Action::Completion));
        let id = asked
            .iter()
            .find_map(|e| match e {
                Effect::Lsp(LspEffect::Query { id, .. }) => Some(*id),
                _ => None,
            })
            .unwrap();
        self.ws.handle(Event::Lsp(LspEvent::Answered {
            server: rust_server(),
            id,
            result: Ok(LspAnswer::Completion {
                items,
                incomplete: false,
            }),
        }));
    }
}

fn offered(label: &str, detail: Option<&str>) -> LspCompletion {
    LspCompletion {
        additional: Vec::new(),
        resolve: None,
        label: label.to_owned(),
        detail: detail.map(str::to_owned),
        filter: label.to_owned(),
        sort: label.to_owned(),
        insert: label.to_owned(),
        start: None,
    }
}

#[test]
fn completion_shows_labels_with_details_under_the_cursor() {
    let mut s = Screen::new();
    s.popup(
        "code\nmore\n",
        vec![offered("alpha", Some("fn alpha()")), offered("beta", None)],
    );
    let rows = s.draw(100, 24);
    let cursor_row = s.ui.editor_cursor.unwrap().y as usize;
    let row = rows.iter().position(|r| r.contains("alpha")).unwrap();
    assert!(row > cursor_row, "{}", rows.join("\n"));
    assert!(rows[row].contains("fn alpha()"), "{}", rows[row]);
    assert!(rows[row + 1].contains("beta"), "{}", rows.join("\n"));
    assert!(rows[cursor_row].contains("code"));
}

#[test]
fn completion_opens_above_a_cursor_at_the_bottom() {
    let mut s = Screen::new();
    s.popup(&"code\n".repeat(40), vec![offered("alpha", None)]);
    for _ in 0..18 {
        s.ws.dispatch(Command::Escape);
    }
    let asked = {
        s.open("a.rs", "");
        s.ws.dispatch(Command::Move(Motion::DocEnd));
        s.ws.dispatch(Command::Run(Action::Completion))
    };
    let id = asked
        .iter()
        .find_map(|e| match e {
            Effect::Lsp(LspEffect::Query { id, .. }) => Some(*id),
            _ => None,
        })
        .unwrap();
    s.ws.handle(Event::Lsp(LspEvent::Answered {
        server: rust_server(),
        id,
        result: Ok(LspAnswer::Completion {
            items: vec![offered("alpha", None)],
            incomplete: false,
        }),
    }));
    let rows = s.draw(100, 24);
    let cursor_row = s.ui.editor_cursor.unwrap().y as usize;
    let row = rows.iter().position(|r| r.contains("alpha")).unwrap();
    assert!(row < cursor_row, "{}", rows.join("\n"));
}

#[test]
fn a_long_completion_list_keeps_the_selection_visible() {
    let mut s = Screen::new();
    let items: Vec<_> = (0..30)
        .map(|n| offered(&format!("item{n:02}"), None))
        .collect();
    s.popup("", items);
    for _ in 0..15 {
        s.ws.dispatch(Command::Move(Motion::Down));
    }
    let rows = s.draw(100, 24);
    assert!(
        rows.iter().any(|r| r.contains("item15")),
        "{}",
        rows.join("\n")
    );
    assert!(!rows.iter().any(|r| r.contains("item00")));
}

#[test]
fn completion_never_panics_in_tiny_terminals() {
    for (w, h) in [(40, 10), (41, 11), (60, 12), (200, 60)] {
        let mut s = Screen::new();
        s.popup(
            "x",
            vec![offered(&"long".repeat(40), Some(&"detail".repeat(40)))],
        );
        s.draw(w, h);
    }
}

#[test]
fn the_find_panel_says_when_it_shows_usages() {
    let mut s = Screen::new();
    s.open("a.rs", "helper\n");
    s.ws.handle(Event::Lsp(LspEvent::Ready {
        server: rust_server(),
        encoding: PositionEncoding::Utf8,
        completion_triggers: Vec::new(),
    }));
    let asked = s.ws.dispatch(Command::Run(Action::FindUsages));
    let id = asked
        .iter()
        .find_map(|e| match e {
            Effect::Lsp(LspEffect::Query { id, .. }) => Some(*id),
            _ => None,
        })
        .unwrap();
    s.ws.handle(Event::Lsp(LspEvent::Answered {
        server: rust_server(),
        id,
        result: Ok(LspAnswer::References(vec![crate::core::LspUsage {
            location: crate::core::LspLocation {
                path: PathBuf::from("/proj/a.rs"),
                start: (0, 0),
            },
            disk_line: "helper".to_owned(),
        }])),
    }));
    let rows = s.draw(100, 24);
    assert!(find_row(&rows, "1 usage of helper in 1 file").contains("Find"));
}

#[test]
fn no_cell_keeps_the_terminals_own_background_in_a_colored_theme() {
    let mut s = Screen::new();
    s.open("a.rs", "one\ntwo\n");
    s.ws.dispatch(Command::Run(Action::FindInProject));
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|f| render(f, &s.ws, &mut s.ui, &Theme::LIGHT, &s.keymap))
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    for y in 0..30 {
        for x in 0..100 {
            assert_ne!(
                buf[(x, y)].bg,
                ratatui::style::Color::Reset,
                "cell {x},{y} has no background"
            );
        }
    }
}

#[test]
fn the_selection_is_tinted_only_where_it_is() {
    let mut s = Screen::new();
    s.open("a.txt", "abcd\nefgh\n");
    s.ws.dispatch(Command::Run(Action::SelectAll));
    s.ws.dispatch(Command::Move(Motion::DocStart));
    s.ws.dispatch(Command::Move(Motion::Right));
    for _ in 0..2 {
        s.ws.dispatch(Command::Select(Motion::Right));
    }
    let mut terminal = Terminal::new(TestBackend::new(100, 24)).unwrap();
    terminal
        .draw(|f| render(f, &s.ws, &mut s.ui, &Theme::DARCULA, &s.keymap))
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    let row = (0..24)
        .find(|&y| {
            (0..100)
                .map(|x| buf[(x, y)].symbol())
                .collect::<String>()
                .contains("abcd")
        })
        .unwrap();
    let start = (0..100).find(|&x| buf[(x, row)].symbol() == "a").unwrap();
    let selected = Theme::DARCULA.selected.bg;
    let bgs: Vec<_> = (0..4).map(|i| buf[(start + i, row)].bg).map(Some).collect();
    assert_ne!(bgs[0], selected, "a");
    assert_eq!(bgs[1], selected, "b");
    assert_eq!(bgs[2], selected, "c");
    assert_ne!(bgs[3], selected, "d");
}

#[test]
fn replacing_shows_two_rows_and_puts_the_cursor_in_the_chosen_field() {
    let mut s = Screen::new();
    s.open("a.txt", "foo foo\n");
    s.ws.dispatch(Command::Run(Action::Replace));
    s.ws.dispatch(Command::Insert("foo".into()));
    s.ws.dispatch(Command::Indent);
    s.ws.dispatch(Command::Insert("bar".into()));
    let rows = s.draw(100, 24);
    let query = rows
        .iter()
        .position(|r| r.contains("❯ foo") && r.contains("1/2"));
    let replace = rows.iter().position(|r| r.contains("❯ bar"));
    assert_eq!(replace, query.map(|q| q + 1), "{}", rows.join("\n"));
    assert!(find_row(&rows, "❯ bar").contains("Alt+A all"));
}

#[test]
fn plain_find_has_one_row() {
    let mut s = Screen::new();
    s.open("a.txt", "foo\n");
    s.ws.dispatch(Command::Run(Action::Find));
    let rows = s.draw(100, 24);
    assert!(!rows.iter().any(|r| r.contains("replace")));
}

#[test]
fn the_log_draws_its_graph_refs_subject_author_and_hash() {
    use crate::core::{GitChange, GitCommit, GitFile};
    let mut s = Screen::new();
    s.open("main.rs", "fn main() {}\n");
    s.ws.dispatch(Command::Run(Action::GitLog));
    let commit = |hash: &str, parents: &[&str], refs: &[&str], subject: &str| GitCommit {
        hash: hash.to_owned(),
        parents: parents.iter().map(|p| (*p).to_owned()).collect(),
        author: "Ann".to_owned(),
        time: 100,
        refs: refs
            .iter()
            .map(|r| crate::core::GitRef {
                name: (*r).to_owned(),
                kind: crate::core::GitRefKind::Local,
            })
            .collect(),
        subject: subject.to_owned(),
    };
    s.ws.handle(Event::Git(GitEvent::LogRead {
        result: Ok(vec![
            commit("1234567890", &["b", "a"], &["main"], "merge it"),
            commit("b", &["base"], &[], "on main"),
            commit("a", &["base"], &["feature"], "on feature"),
            commit("base", &[], &[], "start"),
        ]),
        now: 7300,
    }));
    s.ws.handle(Event::Git(GitEvent::CommitFilesRead {
        hash: "1234567890".into(),
        result: Ok(vec![GitFile::new("src/x.rs".into(), GitChange::Modified)]),
    }));
    let rows = s.draw(120, 24);
    let top = find_row(&rows, "merge it");
    assert!(
        find_row(&rows, "fn main").contains("fn main"),
        "the editor stays visible"
    );
    assert!(top.contains("●─╮"), "{top}");
    assert!(top.contains("[main]"), "{top}");
    assert!(top.contains("Ann"), "{top}");
    assert!(top.contains("2 h ago"), "{top}");
    assert!(top.contains("12345678"), "{top}");
    assert!(!top.contains("123456789"), "a short hash");
    assert!(find_row(&rows, "on main").contains("● │"));
    assert!(find_row(&rows, "on feature").contains("│ ●"));
    assert!(find_row(&rows, "start").contains("●─╯"));
    assert!(find_row(&rows, "on feature").contains("[feature]"));
    assert!(find_row(&rows, "src/x.rs").contains('M'));
}

#[test]
fn the_commit_view_shows_ticks_status_message_and_counts() {
    use crate::core::{GitChange, GitFile};
    let mut s = Screen::new();
    s.ws.dispatch(Command::Run(Action::GitCommit));
    s.ws.handle(Event::Git(GitEvent::StatusRead {
        branch: Some("main".into()),
        top: None,
        files: Ok(vec![
            GitFile::new("m.rs".into(), GitChange::Modified),
            GitFile::new("new.rs".into(), GitChange::Untracked),
        ]),
        state: crate::core::GitRepoState::default(),
    }));
    let rows = s.draw(100, 24);
    assert!(find_row(&rows, "m.rs").contains("[x]  M"));
    assert!(find_row(&rows, "new.rs").contains("[ ]  ?"));
    assert!(find_row(&rows, "TICKED").contains("(1)"));
    assert!(rows[0].contains("⎇ main"), "{}", rows[0]);
}

#[test]
fn the_title_bar_names_the_branch_only_in_a_repository() {
    let mut s = Screen::new();
    assert!(!s.draw(100, 20)[0].contains('⎇'));
    s.ws.handle(Event::Git(GitEvent::StatusRead {
        branch: Some("feature/x".into()),
        top: None,
        files: Ok(Vec::new()),
        state: crate::core::GitRepoState::default(),
    }));
    assert!(s.draw(100, 20)[0].contains("⎇ feature/x"));
}

#[test]
fn the_git_panel_never_panics_on_small_screens() {
    for (w, h) in [(40, 10), (41, 11), (60, 12), (200, 60)] {
        for action in [Action::GitLog, Action::GitCommit] {
            let mut s = Screen::new();
            s.ws.dispatch(Command::Run(action));
            s.draw(w, h);
        }
    }
}

#[test]
fn the_left_stripe_has_a_git_icon_under_project() {
    let mut s = Screen::new();
    let rows = s.draw(100, 24);
    assert!(rows[2].contains('◫'), "{}", rows[2]);
    assert!(rows[3].contains('⎇'), "{}", rows[3]);
    let hit = s.ui.hits.at(1, 3);
    assert!(
        matches!(hit, Some(crate::ui::mouse::Hit::StripeGit)),
        "{hit:?}"
    );
}

#[test]
fn the_gutter_marks_lines_that_differ_from_the_last_commit() {
    let mut s = Screen::new();
    s.open("a.txt", "one\nTWO\nnew\nfour\n");
    s.ws.handle(Event::Git(GitEvent::BaseRead {
        path: PathBuf::from("/proj/a.txt"),
        text: Some("one\ntwo\nfour\n".into()),
    }));
    let rows = s.draw(100, 24);
    assert!(find_row(&rows, "one").contains("│ one"));
    assert!(find_row(&rows, "TWO").contains("┃ TWO"));
    let mut s = Screen::new();
    s.open("b.txt", "a\nc\n");
    s.ws.handle(Event::Git(GitEvent::BaseRead {
        path: PathBuf::from("/proj/b.txt"),
        text: Some("a\nb\nc\n".into()),
    }));
    let rows = s.draw(100, 24);
    assert!(
        rows.iter().any(|r| r.contains("▁ a")),
        "{}",
        rows.join("\n")
    );
}

#[test]
fn a_file_not_in_the_last_commit_has_no_marks_and_an_edit_updates_them() {
    let mut s = Screen::new();
    s.open("a.txt", "one\n");
    let rows = s.draw(100, 24);
    assert!(find_row(&rows, "one").contains("│ one"));
    s.ws.handle(Event::Git(GitEvent::BaseRead {
        path: PathBuf::from("/proj/a.txt"),
        text: Some("one\n".into()),
    }));
    assert!(find_row(&s.draw(100, 24), "one").contains("│ one"));
    s.ws.dispatch(Command::Insert("X".into()));
    assert!(find_row(&s.draw(100, 24), "Xone").contains("┃ Xone"));
}

#[test]
fn a_many_line_message_grows_its_field_and_amend_shows() {
    let mut s = Screen::new();
    s.ws.dispatch(Command::Run(Action::GitCommit));
    s.ws.dispatch(Command::Indent);
    for text in ["subject", "\n", "body"] {
        s.ws.dispatch(Command::Insert(text.into()));
    }
    let rows = s.draw(100, 24);
    assert!(find_row(&rows, "❯ subject").contains("subject"));
    assert!(
        rows.iter().any(|r| r.contains("  body")),
        "{}",
        rows.join(
            "
"
        )
    );
}

#[test]
fn a_git_window_fills_the_screen_with_the_log() {
    let (ws, _) = Workspace::new_git_window(PathBuf::from("/proj"));
    let mut s = Screen {
        ws,
        ui: UiState::default(),
        keymap: Keymap::new(true),
        clicks: 0,
    };
    s.ws.handle(Event::Git(GitEvent::LogRead {
        result: Ok(vec![crate::core::GitCommit {
            hash: "abc".into(),
            parents: Vec::new(),
            author: "Ann".into(),
            time: 0,
            refs: Vec::new(),
            subject: "only commit".into(),
        }]),
        now: 0,
    }));
    let rows = s.draw(100, 24);
    assert!(rows[0].contains("Commit  Log"), "{}", rows[0]);
    assert!(rows[1].contains("only commit"));
    assert!(
        !rows.iter().any(|r| r.contains("Project")),
        "no main window parts"
    );
}

#[test]
fn a_narrow_log_keeps_commits_and_files() {
    let mut s = Screen::new();
    s.ws.dispatch(Command::Run(Action::GitLog));
    s.ws.handle(Event::Git(GitEvent::LogRead {
        result: Ok(vec![crate::core::GitCommit {
            hash: "abc".into(),
            parents: Vec::new(),
            author: "Ann".into(),
            time: 0,
            refs: Vec::new(),
            subject: "subject here".into(),
        }]),
        now: 0,
    }));
    assert!(find_row(&s.draw(70, 24), "subject here").contains('●'));
}

#[test]
fn a_long_subject_stops_short_of_the_author() {
    let mut s = Screen::new();
    s.ws.dispatch(Command::Run(Action::GitLog));
    s.ws.handle(Event::Git(GitEvent::LogRead {
        result: Ok(vec![crate::core::GitCommit {
            hash: "abcdef123".into(),
            parents: Vec::new(),
            author: "Ann".into(),
            time: 0,
            refs: Vec::new(),
            subject: "a very long subject that goes on and on and on and on and on".into(),
        }]),
        now: 0,
    }));
    let rows = s.draw(120, 24);
    let row = find_row(&rows, "a very long");
    assert!(row.contains("… Ann") || row.contains(" Ann"), "{row}");
    assert!(!row.contains("…Ann"), "{row}");
}

#[test]
fn the_log_tells_refs_authors_hashes_and_merges_apart_by_color() {
    use crate::core::{GitCommit, GitRef, GitRefKind};
    let mut s = Screen::new();
    s.ws.dispatch(Command::Run(Action::GitLog));
    let chip = |name: &str, kind| GitRef {
        name: name.to_owned(),
        kind,
    };
    let commit =
        |hash: &str, parents: Vec<String>, refs: Vec<GitRef>, author: &str, subject: &str| {
            GitCommit {
                hash: hash.to_owned(),
                parents,
                author: author.to_owned(),
                time: 0,
                refs,
                subject: subject.to_owned(),
            }
        };
    s.ws.handle(Event::Git(GitEvent::LogRead {
        result: Ok(vec![
            commit(
                "aaaaaaaa1",
                vec!["b".into(), "c".into()],
                vec![chip("cur", GitRefKind::Head)],
                "Ann",
                "merged",
            ),
            commit(
                "bbbbbbbb2",
                vec!["c".into()],
                vec![
                    chip("loc", GitRefKind::Local),
                    chip("origin/r", GitRefKind::Remote),
                    chip("v1", GitRefKind::Tag),
                ],
                "Bo",
                "plain",
            ),
        ]),
        now: 0,
    }));
    let mut terminal = Terminal::new(TestBackend::new(150, 24)).unwrap();
    terminal
        .draw(|f| render(f, &s.ws, &mut s.ui, &Theme::DARCULA, &s.keymap))
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    let fg_of = |needle: &str| {
        for y in 0..24 {
            let row: Vec<String> = (0..150).map(|x| buf[(x, y)].symbol().to_owned()).collect();
            let text = row.concat();
            if let Some(at) = text.find(needle) {
                let x = text[..at].chars().count() as u16;
                return buf[(x, y)].fg;
            }
        }
        panic!("no {needle}");
    };
    let t = &Theme::DARCULA;
    assert_eq!(fg_of("cur]"), t.success);
    assert_eq!(fg_of("loc]"), t.success);
    assert_eq!(fg_of("origin/r"), t.syntax.function);
    assert_eq!(fg_of("v1]"), t.warning);
    assert_eq!(fg_of("merged"), t.fg_muted, "a merge is quieter");
    assert_eq!(fg_of("plain"), t.fg_primary);
    assert_eq!(fg_of("aaaaaaaa"), t.syntax.attribute);
    assert_ne!(fg_of("Ann"), t.fg_muted, "authors have color");
    assert_ne!(fg_of("Ann"), fg_of("Bo"), "and differ");
    assert_eq!(fg_of("commit "), t.fg_muted, "what a key does stays quiet");
    assert_eq!(fg_of("↑↓"), t.accent, "the key itself stands out");
}

impl Screen {
    fn buffer(&mut self, width: u16, height: u16) -> ratatui::buffer::Buffer {
        let mut terminal = Terminal::new(TestBackend::new(width, height)).unwrap();
        terminal
            .draw(|f| render(f, &self.ws, &mut self.ui, &Theme::DARCULA, &self.keymap))
            .unwrap();
        terminal.backend().buffer().clone()
    }
}

fn color_of(buf: &ratatui::buffer::Buffer, needle: &str) -> ratatui::style::Color {
    let area = buf.area;
    for y in 0..area.height {
        let cells: Vec<&str> = (0..area.width).map(|x| buf[(x, y)].symbol()).collect();
        let first = needle.chars().next().unwrap().to_string();
        let wanted: Vec<String> = needle.chars().map(|c| c.to_string()).collect();
        for x in 0..cells.len().saturating_sub(wanted.len() - 1) {
            if cells[x] == first && wanted.iter().enumerate().all(|(i, w)| cells[x + i] == w) {
                return buf[(x as u16, y)].fg;
            }
        }
    }
    panic!("no {needle}");
}

#[test]
fn the_left_icons_each_have_a_color_and_the_open_one_stands_out() {
    let mut s = Screen::new();
    let buf = s.buffer(100, 24);
    let t = Theme::DARCULA;
    assert_eq!(color_of(&buf, "◫"), t.syntax.function);
    assert_eq!(color_of(&buf, "⎇"), t.syntax.keyword);
    assert_eq!(color_of(&buf, "⌕"), t.success);
    let git = buf[(1, 3)].bg;
    s.ws.dispatch(Command::Run(Action::GitLog));
    let open = s.buffer(100, 24);
    assert_ne!(open[(1, 3)].bg, git, "the open tool sits on a patch");
}

#[test]
fn the_tree_colors_folders_and_file_kinds_without_moving_names() {
    let mut s = Screen::new();
    s.ws.handle(Event::DirListed {
        path: PathBuf::from("/proj"),
        result: Ok(vec![
            DirEntry {
                name: "src".into(),
                path: "/proj/src".into(),
                is_dir: true,
            },
            DirEntry {
                name: "main.rs".into(),
                path: "/proj/main.rs".into(),
                is_dir: false,
            },
            DirEntry {
                name: "x.toml".into(),
                path: "/proj/x.toml".into(),
                is_dir: false,
            },
        ]),
    });
    let rows = s.draw(100, 24);
    assert!(find_row(&rows, "main.rs").contains("▪ main.rs"));
    let buf = s.buffer(100, 24);
    let t = Theme::DARCULA;
    assert_eq!(color_of(&buf, "▸ src"), t.syntax.attribute);
    assert_eq!(color_of(&buf, "▪ main"), t.syntax.keyword);
    assert_eq!(color_of(&buf, "▪ x.toml"), t.syntax.attribute);
}

#[test]
fn tabs_and_the_status_bar_use_the_files_color_and_a_change_shows_in_warning() {
    let mut s = Screen::new();
    s.open("main.rs", "x\n");
    s.ws.dispatch(Command::Insert("y".into()));
    s.ws.dispatch(Command::Run(Action::SelectAll));
    let buf = s.buffer(100, 24);
    let t = Theme::DARCULA;
    assert_eq!(color_of(&buf, "main.rs ●"), t.syntax.keyword);
    assert_eq!(color_of(&buf, "● "), t.warning);
    assert_eq!(color_of(&buf, "Rust"), t.syntax.keyword);
    assert_eq!(color_of(&buf, "UTF-8"), t.fg_muted);
}

#[test]
fn the_palette_colors_categories_and_chords() {
    let mut s = Screen::new();
    s.ws.dispatch(Command::Run(Action::OpenPalette));
    s.ws.dispatch(Command::Insert("Save".into()));
    let rows = s.draw(120, 40);
    let buf = s.buffer(120, 40);
    let t = Theme::DARCULA;
    let (y, row) = rows
        .iter()
        .enumerate()
        .find(|(_, r)| r.contains("Save All"))
        .expect("listed");
    let chars: Vec<char> = row.chars().collect();
    let x = (0..chars.len())
        .find(|x| chars[*x..].starts_with(&['F', 'i', 'l', 'e']))
        .expect("its category");
    assert_eq!(buf[(x as u16, y as u16)].fg, t.category_color("File"));
    assert_eq!(color_of(&buf, "Ctrl+S"), t.accent);
}

#[test]
fn find_results_highlight_what_was_searched_for() {
    let mut s = Screen::new();
    s.ws.dispatch(Command::Run(Action::FindInProject));
    s.ws.dispatch(Command::Insert("needle".into()));
    let asked = s.ws.dispatch(Command::Newline);
    let id = asked
        .iter()
        .find_map(|e| match e {
            Effect::SearchProject { id, .. } => Some(*id),
            _ => None,
        })
        .unwrap();
    s.ws.handle(Event::SearchFinished {
        id,
        matches: vec![crate::core::SearchMatch {
            path: "/proj/a.rs".into(),
            line: 0,
            col: 8,
            len: 6,
            text: "    let needle = 1;".into(),
        }],
        truncated: false,
    });
    let buf = s.buffer(100, 24);
    assert_eq!(color_of(&buf, "needle ="), Theme::DARCULA.accent);
    assert_eq!(color_of(&buf, "let "), Theme::DARCULA.fg_primary);
}

#[test]
fn recent_files_popup_lists_files_under_its_title() {
    let mut s = Screen::new();
    s.open("a.rs", "a\n");
    s.open("b.rs", "b\n");
    s.run(Command::Run(Action::RecentFiles));
    let rows = s.draw(100, 30);
    find_row(&rows, "Recent Files");
    find_row(&rows, "│  a.rs");
    assert!(!rows.iter().any(|r| r.contains("│  b.rs")), "{rows:#?}");
}

#[test]
fn search_everywhere_labels_each_row_by_kind() {
    let mut s = Screen::new();
    s.ws.handle(Event::FilesIndexed {
        files: vec![PathBuf::from("/proj/save_game.rs")],
    });
    s.run(Command::Run(Action::SearchEverywhere));
    for c in "save".chars() {
        s.run(Command::Insert(c.to_string()));
    }
    let rows = s.draw(100, 30);
    find_row(&rows, "Search Everywhere");
    let file = find_row(&rows, "│  save_game.rs");
    assert!(file.contains("File"), "{file}");
    assert!(!file.contains("Action"), "{file}");
    let action = find_row(&rows, "│  Save  ");
    assert!(action.contains("Action"), "{action}");
    assert!(action.contains("Ctrl+S"), "{action}");
}

#[test]
fn tabs_are_drawn_and_reported_with_the_configured_width() {
    let mut s = Screen::new();
    s.ws.configure_editor(2, false);
    s.open("a.txt", "\tx\n");
    let rows = s.draw(100, 30);
    let text = find_row(&rows, "1 │");
    assert!(text.ends_with("1 │   x"), "{text}");
    let status = rows.last().expect("status bar");
    assert!(status.contains("2 spaces"), "{status}");
    s.ws.configure_editor(8, false);
    let rows = s.draw(100, 30);
    assert!(rows.last().expect("status bar").contains("8 spaces"));
    assert!(find_row(&rows, "1 │").ends_with("1 │         x"));
}

mod wrapped {
    use std::time::{Duration, Instant};

    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    use ropey::Rope;
    use unicode_segmentation::UnicodeSegmentation;

    use super::*;
    use crate::core::text;
    use crate::ui::mouse;

    const WIDTH: u16 = 60;
    const HEIGHT: u16 = 20;
    const LEFT: u16 = 4;

    impl Screen {
        fn wrapped(text: &str) -> Self {
            let mut s = Screen::new();
            s.ws.configure_editor(4, true);
            s.ws.dispatch(Command::Focus(Focus::Editor));
            s.open("a.txt", text);
            s.frame();
            s
        }

        fn frame(&mut self) -> Vec<Vec<String>> {
            self.draw(WIDTH, HEIGHT);
            let text_cols = self.ui.viewport;
            self.ws.dispatch(Command::Viewport { text_cols });
            let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
            terminal
                .draw(|f| render(f, &self.ws, &mut self.ui, &Theme::DARCULA, &self.keymap))
                .unwrap();
            let buf = terminal.backend().buffer().clone();
            (0..HEIGHT)
                .map(|y| {
                    (0..WIDTH)
                        .map(|x| buf[(x, y)].symbol().to_owned())
                        .collect()
                })
                .collect()
        }

        fn mouse(&mut self, kind: MouseEventKind, x: u16, y: u16) {
            let event = MouseEvent {
                kind,
                column: x,
                row: y,
                modifiers: KeyModifiers::NONE,
            };
            let now = Instant::now() + Duration::from_secs(self.clicks);
            self.clicks += 10;
            for command in mouse::handle(&event, &mut self.ui, &self.ws, now) {
                self.ws.dispatch(command);
            }
        }

        fn click(&mut self, x: u16, y: u16) {
            self.mouse(MouseEventKind::Down(MouseButton::Left), x, y);
            self.mouse(MouseEventKind::Up(MouseButton::Left), x, y);
        }

        fn cursor_idx(&self) -> usize {
            self.ws.active().unwrap().0.cursor.idx
        }

        fn rope(&self) -> Rope {
            self.ws.active().unwrap().1.text().clone()
        }

        fn text_x(&self) -> u16 {
            WIDTH - self.ui.viewport as u16
        }

        fn assert_cursor_shows_the_right_char(&mut self, context: &str) {
            let cells = self.frame();
            let at = self.ui.editor_cursor.unwrap_or_else(|| {
                panic!("{context}: no cursor on screen\n{}", dump(&cells));
            });
            assert!(
                at.x >= self.text_x() && at.x < WIDTH,
                "{context}: x {} outside the text\n{}",
                at.x,
                dump(&cells)
            );
            let rope = self.rope();
            let idx = self.cursor_idx();
            let line = rope.char_to_line(idx);
            let offset = idx - rope.line_to_char(line);
            let content = text::line_content(&rope, line);
            let mut chars = 0;
            let mut grapheme = None;
            for g in content.graphemes(true) {
                if chars == offset {
                    grapheme = Some(g.to_owned());
                    break;
                }
                chars += g.chars().count();
            }
            let cell = &cells[usize::from(at.y)][usize::from(at.x)];
            let expected = match grapheme.as_deref() {
                None | Some("\t") => " ",
                Some(g) => g,
            };
            assert_eq!(
                cell,
                expected,
                "{context}: idx {idx} at ({}, {})\n{}",
                at.x,
                at.y,
                dump(&cells)
            );
        }
    }

    fn dump(cells: &[Vec<String>]) -> String {
        cells
            .iter()
            .map(|r| r.concat())
            .collect::<Vec<_>>()
            .join("\n")
    }

    fn text_rows(cells: &[Vec<String>], text_x: u16) -> Vec<String> {
        cells
            .iter()
            .map(|r| r[usize::from(text_x)..].concat().trim_end().to_owned())
            .collect()
    }

    const LONG: &str = "The quick brown fox jumps over the lazy dog and then keeps \
                        running through the forest until the night falls down";

    #[test]
    fn a_long_line_continues_on_rows_without_a_number() {
        let mut s = Screen::wrapped(&format!("{LONG}\nshort\n"));
        let cells = s.frame();
        let rows: Vec<String> = cells.iter().map(|r| r.concat()).collect();
        let first = rows.iter().position(|r| r.contains("1 │")).expect("line 1");
        assert!(rows[first].contains("The quick"), "{}", rows[first]);
        let gutter = usize::from(s.text_x() - LEFT);
        for row in &cells[first + 1..first + 3] {
            let cell = row[usize::from(LEFT)..usize::from(s.text_x())].concat();
            assert!(!cell.chars().any(|c| c.is_ascii_digit()), "{cell}");
            assert_eq!(cell.chars().count(), gutter);
            assert!(cell.contains('│'), "{cell}");
        }
        let joined = text_rows(&cells, s.text_x())[first..first + 3].concat();
        assert!(joined.starts_with("The quick brown"), "{joined}");
        let second = rows.iter().position(|r| r.contains("2 │")).expect("line 2");
        assert!(second > first + 1);
        assert!(rows[second].contains("short"));
        let all: String = text_rows(&cells, s.text_x())[first..second]
            .iter()
            .map(String::as_str)
            .collect();
        assert_eq!(all.replace(' ', ""), LONG.replace(' ', ""));
    }

    #[test]
    fn without_wrap_the_line_is_cut_and_one_row_stays_one_row() {
        let mut s = Screen::new();
        s.ws.dispatch(Command::Focus(Focus::Editor));
        s.open("a.txt", &format!("{LONG}\nshort\n"));
        let cells = s.frame();
        let rows: Vec<String> = cells.iter().map(|r| r.concat()).collect();
        let first = rows.iter().position(|r| r.contains("1 │")).expect("line 1");
        assert!(rows[first + 1].contains("2 │"));
    }

    #[test]
    fn toggling_at_runtime_rewraps_and_keeps_the_cursor_visible() {
        let mut s = Screen::new();
        s.ws.dispatch(Command::Focus(Focus::Editor));
        s.open("a.txt", LONG);
        s.frame();
        s.ws.dispatch(Command::Run(Action::SelectAll));
        s.ws.dispatch(Command::Move(Motion::LineEnd));
        s.ws.dispatch(Command::Escape);
        s.assert_cursor_shows_the_right_char("scrolled right, not wrapped");
        s.ws.dispatch(Command::Run(Action::ToggleSoftWrap));
        s.assert_cursor_shows_the_right_char("wrapped");
        let cells = s.frame();
        assert!(
            dump(&cells).contains("The quick"),
            "scrolled back to the left"
        );
        s.ws.dispatch(Command::Run(Action::ToggleSoftWrap));
        s.assert_cursor_shows_the_right_char("not wrapped again");
    }

    const MIXED: &str = "plain words long enough to need wrapping more than one time here\n\
        \n\
        \tindented\twith\ttabs\tthat\tcross\tthe\tedge\tof\tthe\trow\tagain\tand\tagain\n\
        汉字汉字汉字汉字汉字汉字汉字汉字汉字汉字汉字汉字汉字汉字汉字汉字汉字汉字汉字汉字汉字汉字汉字\n\
        a汉b字c汉d字e汉f字g汉h字i汉j字k汉l字m汉n字o汉p字q汉r字s汉t字u汉v字w汉x字y汉z字end\n\
        e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}e\u{301}\n\
        exact";

    #[test]
    fn stepping_by_every_motion_always_shows_the_right_char_at_the_cursor() {
        let mut s = Screen::wrapped(MIXED);
        let len = s.rope().len_chars();
        let mut guard = 0;
        while s.cursor_idx() < len {
            s.ws.dispatch(Command::Move(Motion::Right));
            s.assert_cursor_shows_the_right_char("right");
            guard += 1;
            assert!(guard <= len);
        }
        for motion in [Motion::Up, Motion::Down] {
            let mut last = s.cursor_idx();
            let mut stuck = 0;
            for _ in 0..200 {
                s.ws.dispatch(Command::Move(motion));
                s.assert_cursor_shows_the_right_char("vertical");
                stuck = if s.cursor_idx() == last { stuck + 1 } else { 0 };
                last = s.cursor_idx();
                if stuck == 2 {
                    break;
                }
            }
            let rope = s.rope();
            let idx = s.cursor_idx();
            let line = rope.char_to_line(idx);
            if motion == Motion::Up {
                assert_eq!(line, 0);
                assert!(idx < s.ui.viewport, "first row, ended at {idx}");
            } else {
                assert_eq!(line, rope.len_lines() - 1);
            }
        }
    }

    fn next(seed: &mut u64) -> u64 {
        *seed ^= *seed << 13;
        *seed ^= *seed >> 7;
        *seed ^= *seed << 17;
        *seed
    }

    #[test]
    fn random_motions_at_random_tab_widths_keep_cursor_and_screen_in_step() {
        for start in 1..=4u64 {
            let mut seed = 0x9E37_79B9 * start;
            let mut s = Screen::wrapped(MIXED);
            s.ws.configure_editor(1 + (start as usize % 8), true);
            for step in 0..200 {
                let rows = 1 + (next(&mut seed) % 30) as usize;
                let motion = match next(&mut seed) % 10 {
                    0 | 1 => Motion::Up,
                    2 | 3 => Motion::Down,
                    4 => Motion::Left,
                    5 => Motion::Right,
                    6 => Motion::LineStart,
                    7 => Motion::LineEnd,
                    8 => Motion::PageUp { rows },
                    _ => Motion::PageDown { rows },
                };
                s.ws.dispatch(Command::Move(motion));
                s.assert_cursor_shows_the_right_char(&format!(
                    "seed {start} step {step} {motion:?}"
                ));
            }
        }
    }

    #[test]
    fn clicking_any_cell_puts_the_cursor_on_that_row_at_that_column_or_the_rows_end() {
        let mut s = Screen::wrapped(MIXED);
        let cells = s.frame();
        let body_top = 2 + cells[2..]
            .iter()
            .position(|r| r.concat().contains("1 │"))
            .expect("line 1") as u16;
        let text_x = s.text_x();
        let body_bottom = 1 + cells
            .iter()
            .position(|r| r.concat().contains("7 │"))
            .expect("line 7") as u16;
        let mut tried = 0;
        for y in body_top..body_bottom {
            for x in (text_x..WIDTH).step_by(3) {
                s.click(x, y);
                let cells = s.frame();
                let at = s.ui.editor_cursor.expect("a click leaves a cursor");
                assert_eq!(at.y, y, "clicked ({x}, {y})\n{}", dump(&cells));
                assert!(at.x <= x + 1, "clicked ({x}, {y}), cursor at {at:?}");
                s.assert_cursor_shows_the_right_char(&format!("click ({x}, {y})"));
                tried += 1;
            }
        }
        assert!(tried > 100, "only {tried} clicks");
        s.click(text_x + 2, body_bottom + 2);
        s.frame();
        assert_eq!(s.ui.editor_cursor.map(|c| c.y), Some(body_bottom - 1));
    }

    #[test]
    fn a_click_on_a_wide_chars_second_cell_lands_on_that_char() {
        let mut s = Screen::wrapped("汉字汉字");
        s.frame();
        let x = s.text_x();
        let y = s.ui.editor_cursor.expect("cursor").y;
        s.click(x + 3, y);
        assert_eq!(s.cursor_idx(), 1);
    }

    #[test]
    fn the_wheel_moves_by_rows_and_a_click_after_it_still_lands_right() {
        let mut s = Screen::wrapped(&format!("{LONG}{LONG}{LONG}\nend\n"));
        let cells = s.frame();
        let top_row = 2 + cells[2..]
            .iter()
            .position(|r| r.concat().contains("1 │"))
            .expect("line 1") as u16;
        let before = text_rows(&cells, s.text_x())[usize::from(top_row) + 3].clone();
        s.mouse(MouseEventKind::ScrollDown, s.text_x() + 1, top_row + 1);
        let cells = s.frame();
        assert_eq!(
            text_rows(&cells, s.text_x())[usize::from(top_row)],
            before,
            "three rows up\n{}",
            dump(&cells)
        );
        assert!(
            s.ui.editor_cursor.is_none(),
            "the cursor scrolled off and is not drawn over anything"
        );
        s.click(s.text_x(), top_row);
        let cells = s.frame();
        let at = s.ui.editor_cursor.expect("a click brings the cursor");
        assert_eq!(at.y, top_row, "{}", dump(&cells));
        s.assert_cursor_shows_the_right_char("after wheel and click");
        s.mouse(MouseEventKind::ScrollUp, s.text_x() + 1, top_row + 1);
        assert_eq!(s.ui.scroll.values().next(), Some(&(0, 0)));
    }

    #[test]
    fn the_wheel_stops_at_the_end_of_the_text() {
        let mut s = Screen::wrapped(&format!("{LONG}\n{LONG}"));
        s.frame();
        for _ in 0..50 {
            s.mouse(MouseEventKind::ScrollDown, s.text_x() + 1, 8);
        }
        let cells = s.frame();
        assert!(dump(&cells).contains("falls down"), "{}", dump(&cells));
        assert_eq!(s.ui.scroll.values().next().map(|v| v.0), Some(1));
    }

    #[test]
    fn dragging_across_rows_selects_exactly_what_was_covered() {
        let mut s = Screen::wrapped(&"abcdefgh".repeat(20));
        s.frame();
        let cols = s.ui.viewport;
        let (x, y) = (s.text_x(), s.ui.editor_cursor.expect("cursor").y);
        s.mouse(MouseEventKind::Down(MouseButton::Left), x + 2, y);
        s.mouse(MouseEventKind::Drag(MouseButton::Left), x + 5, y + 2);
        s.mouse(MouseEventKind::Up(MouseButton::Left), x + 5, y + 2);
        let (view, _) = s.ws.active().unwrap();
        assert_eq!(view.cursor.selection(), Some(2..2 * cols + 5));
    }

    #[test]
    fn narrow_splits_wrap_to_their_own_width() {
        let mut s = Screen::wrapped(&format!("{LONG}\n"));
        s.ws.dispatch(Command::Run(Action::SplitVertically));
        s.frame();
        let cols = s.ui.viewport;
        assert!(cols < usize::from(WIDTH) / 2, "{cols}");
        s.ws.dispatch(Command::Move(Motion::Down));
        assert_eq!(
            s.cursor_idx(),
            LONG[..cols].rfind(' ').map_or(cols, |i| i + 1)
        );
        s.assert_cursor_shows_the_right_char("in a split");
    }

    #[test]
    fn a_very_narrow_view_does_not_hang_or_lose_the_cursor() {
        let mut s = Screen::wrapped("汉字\tabc");
        s.ws.dispatch(Command::Run(Action::SplitVertically));
        s.ws.dispatch(Command::Run(Action::SplitVertically));
        s.ws.dispatch(Command::Run(Action::SplitVertically));
        for _ in 0..8 {
            s.frame();
            s.ws.dispatch(Command::Move(Motion::Right));
        }
        s.frame();
    }

    #[test]
    fn gutter_width_changes_do_not_break_the_layout_of_a_long_file() {
        let long = format!("{LONG}\n").repeat(1200);
        let mut s = Screen::wrapped(&long);
        s.ws.dispatch(Command::Move(Motion::DocEnd));
        s.assert_cursor_shows_the_right_char("doc end");
        let rows: Vec<String> = s.frame().iter().map(|r| r.concat()).collect();
        assert!(rows.iter().any(|r| r.contains("1201 │")), "{rows:#?}");
        s.ws.dispatch(Command::Move(Motion::DocStart));
        s.assert_cursor_shows_the_right_char("doc start");
    }

    #[test]
    fn the_line_end_exactly_at_the_edge_gets_its_own_row_for_the_cursor() {
        let mut s = Screen::wrapped("x");
        s.frame();
        let cols = s.ui.viewport;
        let mut s = Screen::wrapped(&"y".repeat(cols));
        s.ws.dispatch(Command::Move(Motion::LineEnd));
        s.assert_cursor_shows_the_right_char("end of an exactly full row");
        let at = s.ui.editor_cursor.expect("cursor");
        assert_eq!(at.x, s.text_x());
    }

    #[test]
    fn line_marks_and_highlights_still_paint_the_right_chars_on_later_rows() {
        let mut s = Screen::wrapped(&format!("{LONG} needle\n"));
        s.ws.dispatch(Command::Run(Action::Find));
        for c in "needle".chars() {
            s.ws.dispatch(Command::Insert(c.to_string()));
        }
        s.draw(WIDTH, HEIGHT);
        let mut terminal = Terminal::new(TestBackend::new(WIDTH, HEIGHT)).unwrap();
        terminal
            .draw(|f| render(f, &s.ws, &mut s.ui, &Theme::DARCULA, &s.keymap))
            .unwrap();
        let buf = terminal.backend().buffer().clone();
        let mut found = false;
        for y in 0..HEIGHT {
            let row: String = (0..WIDTH).map(|x| buf[(x, y)].symbol()).collect();
            if let Some(at) = row.find("needle").filter(|_| !row.contains(glyphs::PROMPT)) {
                let x = row[..at].chars().count() as u16;
                assert_eq!(buf[(x, y)].bg, Theme::DARCULA.selected.bg.unwrap());
                assert_ne!(buf[(x - 1, y)].bg, Theme::DARCULA.selected.bg.unwrap());
                found = true;
            }
        }
        assert!(found, "needle is on some row");
    }
}

#[test]
fn a_cursor_scrolled_off_by_the_wheel_is_not_drawn_at_all() {
    let mut s = Screen::new();
    s.ws.dispatch(Command::Focus(Focus::Editor));
    s.open("a.txt", &"line\n".repeat(300));
    s.draw(100, 30);
    assert!(s.ui.editor_cursor.is_some());
    let hit = s.ui.hits.at(40, 10).cloned();
    let Some(Hit::Text { view, .. }) = hit else {
        panic!("{hit:?}");
    };
    s.ui.scroll.insert(view, (100, 0));
    s.draw(100, 30);
    assert_eq!(s.ui.editor_cursor, None, "and no overflow working it out");
    s.ws.dispatch(Command::Move(Motion::Down));
    s.draw(100, 30);
    assert!(s.ui.editor_cursor.is_some(), "a move brings it back");
}

#[test]
fn the_find_bar_shows_options_and_a_regex_error() {
    let mut s = Screen::new();
    s.open("a.txt", "(a)\n");
    s.run(Command::Run(Action::Find));
    let rows = s.draw(100, 30);
    assert!(find_row(&rows, "Aa").ends_with("Aa   W   .*"));
    s.run(Command::Run(Action::ToggleFindRegex));
    s.run(Command::Run(Action::ToggleFindCase));
    s.run(Command::Insert("(".into()));
    let rows = s.draw(100, 30);
    let bar = find_row(&rows, "[Aa]");
    assert!(bar.contains("[.*]") && !bar.contains("[W]"), "{bar}");
    assert!(bar.contains("unclosed group"), "{bar}");
}

#[test]
fn find_in_files_shows_options_and_a_regex_error() {
    let mut s = Screen::new();
    s.run(Command::Run(Action::FindInProject));
    s.run(Command::Run(Action::ToggleFindRegex));
    s.run(Command::Insert("(".into()));
    s.run(Command::Newline);
    let rows = s.draw(100, 30);
    let title = find_row(&rows, "[.*]");
    assert!(title.contains("unclosed group"), "{title}");
}

#[test]
fn a_regex_hit_is_highlighted_for_its_own_length() {
    let mut s = Screen::new();
    s.run(Command::Run(Action::FindInProject));
    s.run(Command::Run(Action::ToggleFindRegex));
    s.run(Command::Insert("x+".into()));
    let fx = s.ws.dispatch(Command::Newline);
    let [Effect::SearchProject { id, .. }] = fx.as_slice() else {
        panic!("{fx:?}");
    };
    s.ws.handle(Event::SearchFinished {
        id: *id,
        matches: vec![crate::core::SearchMatch {
            path: "/proj/a.txt".into(),
            line: 0,
            col: 1,
            len: 4,
            text: "axxxxb".into(),
        }],
        truncated: false,
    });
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|f| render(f, &s.ws, &mut s.ui, &Theme::DARCULA, &s.keymap))
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    let y = (0..30)
        .find(|y| {
            (0..100)
                .map(|x| buf[(x, *y)].symbol())
                .collect::<String>()
                .contains("axxxxb")
        })
        .expect("the row");
    let x = (0..100)
        .find(|x| buf[(*x, y)].symbol() == "a")
        .expect("the a");
    let accent = Theme::DARCULA.accent;
    assert_ne!(buf[(x, y)].fg, accent);
    for i in 1..=4 {
        assert_eq!(buf[(x + i, y)].fg, accent, "x number {i}");
    }
    assert_ne!(buf[(x + 5, y)].fg, accent);
}

fn searched_for_foo(s: &mut Screen) {
    s.run(Command::Run(Action::FindInProject));
    s.run(Command::Insert("foo".into()));
    let fx = s.ws.dispatch(Command::Newline);
    let [Effect::SearchProject { id, .. }] = fx.as_slice() else {
        panic!("{fx:?}");
    };
    s.ws.handle(Event::SearchFinished {
        id: *id,
        matches: vec![crate::core::SearchMatch {
            path: "/proj/a.txt".into(),
            line: 0,
            col: 0,
            len: 3,
            text: "foo x".into(),
        }],
        truncated: false,
    });
}

#[test]
fn replace_in_files_shows_both_fields_then_each_change_and_then_a_report() {
    let mut s = Screen::new();
    s.open("a.txt", "foo x\n");
    searched_for_foo(&mut s);
    s.run(Command::Run(Action::ReplaceInFiles));
    let rows = s.draw(100, 30);
    let field = rows
        .iter()
        .position(|r| r.contains("❯ foo"))
        .expect("query");
    assert!(rows[field + 1].contains("preview"), "{}", rows[field + 1]);
    assert!(
        rows[field + 2].contains("a.txt"),
        "the list keeps its place"
    );
    s.run(Command::Insert("bar".into()));
    s.run(Command::Newline);
    let rows = s.draw(100, 30);
    let title = find_row(&rows, "Replace 1 occurrence in 1 file?");
    assert!(
        title.contains("y apply") && title.contains("Esc cancel"),
        "{title}"
    );
    assert!(find_row(&rows, "1 - foo x").contains("- foo x"));
    find_row(&rows, "+ bar x");
    s.ws.dispatch(Command::Insert("y".into()));
    let rows = s.draw(100, 30);
    find_row(&rows, "Replaced in 1 file, 0 skipped, 0 failed  Esc close");
    find_row(&rows, "1 │ bar x");
    s.run(Command::Escape);
    let rows = s.draw(100, 30);
    assert!(
        !rows.iter().any(|r| r.contains("failed  Esc close")),
        "{rows:#?}"
    );
}

#[test]
fn a_report_names_every_file_that_went_wrong_and_why() {
    let mut s = Screen::new();
    s.run(Command::Run(Action::FindInProject));
    s.run(Command::Insert("foo".into()));
    let fx = s.ws.dispatch(Command::Newline);
    let [Effect::SearchProject { id, .. }] = fx.as_slice() else {
        panic!("{fx:?}");
    };
    s.ws.handle(Event::SearchFinished {
        id: *id,
        matches: vec![crate::core::SearchMatch {
            path: "/proj/c.txt".into(),
            line: 0,
            col: 0,
            len: 3,
            text: "foo".into(),
        }],
        truncated: false,
    });
    s.ws.dispatch(Command::Run(Action::ReplaceInFiles));
    let fx = s.ws.dispatch(Command::Newline);
    let [Effect::PlanProjectReplace { id, .. }] = fx.as_slice() else {
        panic!("{fx:?}");
    };
    let path = PathBuf::from("/proj/c.txt");
    let query = crate::core::Query::new("foo", crate::core::FindOptions::default()).unwrap();
    let edits = query.plan("foo", &query.template("").unwrap());
    s.ws.handle(Event::ProjectReplacePlanned {
        id: *id,
        files: vec![crate::core::ClosedPlan {
            plan: crate::core::FilePlan::new(path.clone(), "foo", &edits),
            disk: None,
        }],
        failed: vec![],
    });
    let fx = s.ws.dispatch(Command::Insert("y".into()));
    let [Effect::ApplyProjectReplace { id, .. }] = fx.as_slice() else {
        panic!("{fx:?}");
    };
    s.ws.handle(Event::ProjectReplaceApplied {
        id: *id,
        changed: vec![],
        skipped: vec![(path.clone(), "changed on disk since the preview".to_owned())],
        failed: vec![("/proj/d.txt".into(), "Permission denied".to_owned())],
    });
    let rows = s.draw(100, 30);
    find_row(&rows, "c.txt: skipped, changed on disk since the preview");
    find_row(&rows, "d.txt: Permission denied");
    find_row(&rows, "0 files, 1 skipped, 1 failed");
}

#[test]
fn tool_room_keeps_both_panels_usable_at_every_height() {
    for screen in 0..=200 {
        for tab in [ToolTab::Find, ToolTab::Problems, ToolTab::Git] {
            for wanted in [None, Some(0), Some(1), Some(7), Some(40), Some(u16::MAX)] {
                let (rows, min, max) = tool_room(wanted, tab, screen);
                if screen < 16 {
                    assert_eq!((rows, min, max), (0, 0, 0));
                    continue;
                }
                assert!(min <= rows && rows <= max, "{screen} {wanted:?}");
                assert!(rows >= 5, "the panel at {screen}");
                assert!(screen - 1 - rows >= 5, "the editor at {screen}");
                if tab == ToolTab::Git {
                    assert!(rows >= 10, "a log at {screen}");
                }
            }
        }
    }
}

#[test]
fn without_a_choice_the_tool_window_keeps_its_old_share() {
    assert_eq!(tool_room(None, ToolTab::Find, 40).0, 12);
    assert_eq!(tool_room(None, ToolTab::Git, 40).0, 18);
    assert_eq!(tool_room(None, ToolTab::Find, 16).0, 6);
    assert_eq!(tool_room(None, ToolTab::Git, 16).0, 10);
}

#[test]
fn the_screen_follows_the_chosen_height_and_a_smaller_screen_clamps_it() {
    let mut s = Screen::new();
    s.open("a.txt", "x\n");
    s.run(Command::Run(Action::ShowProblems));
    let title_row = |rows: &[String]| {
        rows.iter()
            .position(|r| r.contains(" Problems"))
            .expect("panel")
    };
    let rows = s.draw(100, 40);
    let before = title_row(&rows);
    let (r, min, max) = s.ui.tool_room;
    s.run(Command::ToolRoom { rows: r, min, max });
    s.run(Command::Run(Action::ToolWindowTaller));
    let rows = s.draw(100, 40);
    assert_eq!(title_row(&rows), before - 2, "{rows:#?}");
    for _ in 0..40 {
        s.run(Command::Run(Action::ToolWindowTaller));
    }
    let rows = s.draw(100, 40);
    find_row(&rows, "1 │ x");
    let chosen = s.ws.tool_height();
    let small = s.draw(100, 20);
    find_row(&small, "1 │ x");
    title_row(&small);
    assert_eq!(s.ws.tool_height(), chosen, "the choice is not forgotten");
}

fn run_target(command: &str) -> crate::core::RunTarget {
    crate::core::ConfiguredRun {
        name: "build".to_owned(),
        command: command.to_owned(),
        cwd: None,
        from_project: false,
    }
    .target(std::path::Path::new("/proj"))
}

fn start_run(s: &mut Screen, command: &str) {
    s.ws.configure_run(vec![crate::core::ConfiguredRun {
        name: "build".to_owned(),
        command: command.to_owned(),
        cwd: None,
        from_project: false,
    }]);
    s.ws.dispatch(Command::Run(Action::RunChoose));
    s.ws.handle(Event::Run(RunEvent::FactsRead {
        facts: crate::core::run::RunFacts::default(),
    }));
    let effects = s.ws.dispatch(Command::Newline);
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Run(RunEffect::Start { .. }))),
        "{effects:?}"
    );
    assert_eq!(
        s.ws.run_panel().map(|p| p.target.clone()),
        Some(run_target(command))
    );
}

fn print(s: &mut Screen, lines: Vec<crate::core::RunLine>, partial: &str) {
    s.ws.handle(Event::Run(RunEvent::Output {
        id: 1,
        lines,
        partial: partial.to_owned(),
        dropped: 0,
    }));
}

fn text_line(text: &str) -> crate::core::RunLine {
    crate::core::RunLine {
        spans: Vec::new(),
        text: text.to_owned(),
        location: None,
    }
}

fn numbered(n: usize) -> Vec<crate::core::RunLine> {
    (0..n).map(|i| text_line(&format!("line {i}"))).collect()
}

#[test]
fn the_run_tab_shows_what_runs_and_how_it_ended() {
    let mut s = Screen::new();
    start_run(&mut s, "make -j4 all");
    print(&mut s, numbered(2), "");
    let rows = s.draw(100, 30);
    find_row(&rows, "Running");
    find_row(&rows, "$ make -j4 all");
    find_row(&rows, "line 1");
    find_row(&rows, "▶ build");
    s.ws.handle(Event::Run(RunEvent::Finished {
        id: 1,
        outcome: crate::core::RunOutcome::Exited(3),
        duration_ms: 1500,
    }));
    let rows = s.draw(100, 30);
    find_row(&rows, "Exit code 3 · 1.5 s");
    assert!(
        !rows.iter().any(|r| r.contains("▶ build")),
        "not running now"
    );
}

#[test]
fn a_run_that_could_not_start_says_why() {
    let mut s = Screen::new();
    start_run(&mut s, "x");
    s.ws.handle(Event::Run(RunEvent::Finished {
        id: 1,
        outcome: crate::core::RunOutcome::FailedToStart("pytest: not found".to_owned()),
        duration_ms: 1,
    }));
    find_row(&s.draw(100, 30), "Could not start: pytest: not found");
}

#[test]
fn the_view_follows_the_end_until_the_cursor_moves_up() {
    let mut s = Screen::new();
    start_run(&mut s, "x");
    print(&mut s, numbered(100), "");
    let rows = s.draw(100, 30);
    find_row(&rows, "line 99");
    assert!(!rows.iter().any(|r| r.contains("line 50")));

    s.ws.dispatch(Command::Run(Action::ShowRun));
    s.ws.dispatch(Command::Move(Motion::PageUp { rows: 30 }));
    let rows = s.draw(100, 30);
    find_row(&rows, "line 70");
    print(&mut s, numbered(5), "");
    let again = s.draw(100, 30);
    find_row(&again, "line 70");
    assert_eq!(s.ws.run_panel().unwrap().selected(), Some(70));

    s.ws.dispatch(Command::Move(Motion::DocEnd));
    find_row(&s.draw(100, 30), "line 4");
}

#[test]
fn a_partial_line_shows_at_the_end() {
    let mut s = Screen::new();
    start_run(&mut s, "x");
    print(&mut s, numbered(1), "Password: ");
    find_row(&s.draw(100, 30), "Password:");
}

#[test]
fn tabs_and_long_lines_stay_inside_the_tab() {
    let mut s = Screen::new();
    start_run(&mut s, "x");
    let long = format!("start\t{}end", "x".repeat(5000));
    let wide = "日本語".repeat(100);
    print(
        &mut s,
        vec![text_line(&long), text_line(&wide), text_line("a\tb")],
        "",
    );
    let rows = s.draw(60, 24);
    assert_eq!(rows.len(), 24);
    for row in &rows {
        assert!(row.chars().count() <= 60, "{row}");
    }
    let row = find_row(&rows, "start");
    assert!(row.contains("start   xxx"), "the tab is spaces: {row}");
    assert!(!row.contains("end"), "cut, not wrapped: {row}");
    find_row(&rows, "a   b");
    assert!(rows[23].contains("proj") || !rows[23].is_empty());

    s.ws.dispatch(Command::Run(Action::ShowRun));
    for _ in 0..620 {
        s.ws.dispatch(Command::Move(Motion::Right));
    }
    find_row(&s.draw(60, 24), "xxxend");
}

#[test]
fn a_tiny_window_does_not_panic_with_output() {
    let mut s = Screen::new();
    start_run(&mut s, &"x".repeat(1000));
    print(&mut s, numbered(1000), "partial");
    for (w, h) in [(40, 10), (41, 16), (40, 17), (120, 16), (200, 60)] {
        let rows = s.draw(w, h);
        assert_eq!(rows.len(), usize::from(h));
    }
}

#[test]
fn a_line_with_a_place_is_marked_and_clicking_it_opens_it() {
    use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
    let mut s = Screen::new();
    start_run(&mut s, "x");
    print(
        &mut s,
        vec![
            text_line("plain text"),
            crate::core::RunLine {
                spans: Vec::new(),
                text: "b.c:2:1: oops".to_owned(),
                location: Some(crate::core::RunLocation {
                    path: PathBuf::from("/proj/b.c"),
                    line: 1,
                    col: 0,
                    span: (0, 7),
                }),
            },
        ],
        "",
    );
    let mut terminal = Terminal::new(TestBackend::new(100, 30)).unwrap();
    terminal
        .draw(|f| render(f, &s.ws, &mut s.ui, &Theme::DARCULA, &s.keymap))
        .unwrap();
    let buf = terminal.backend().buffer().clone();
    let cell_of = |needle: &str| {
        (0..30u16).find_map(|y| {
            let row: String = (0..100u16).map(|x| buf[(x, y)].symbol()).collect();
            row.find(needle).map(|at| {
                let x = row[..at].chars().count() as u16;
                (x, y)
            })
        })
    };
    let (x, y) = cell_of("b.c:2:1").unwrap();
    assert!(
        buf[(x, y)]
            .modifier
            .contains(ratatui::style::Modifier::UNDERLINED),
        "the place is marked"
    );
    assert!(
        !buf[(x + 8, y)]
            .modifier
            .contains(ratatui::style::Modifier::UNDERLINED),
        "and only the place"
    );
    assert_eq!(buf[(x - 2, y)].symbol(), "›", "the line has a mark");
    let (px, py) = cell_of("plain text").unwrap();
    assert_eq!(buf[(px - 2, py)].symbol(), " ");

    let down = MouseEvent {
        kind: MouseEventKind::Down(MouseButton::Left),
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    };
    let commands = crate::ui::mouse::handle(&down, &mut s.ui, &s.ws, std::time::Instant::now());
    assert_eq!(commands, [Command::SelectRunLine { line: 1 }]);
    let wheel = MouseEvent {
        kind: MouseEventKind::ScrollUp,
        column: x,
        row: y,
        modifiers: KeyModifiers::NONE,
    };
    let commands = crate::ui::mouse::handle(&wheel, &mut s.ui, &s.ws, std::time::Instant::now());
    assert_eq!(commands, [Command::ScrollRun { delta: -3 }]);
}

fn ask(s: &mut Screen, command: &str) {
    s.ws.configure_run(vec![crate::core::ConfiguredRun {
        name: "setup".to_owned(),
        command: command.to_owned(),
        cwd: Some("tools".to_owned()),
        from_project: true,
    }]);
    s.ws.dispatch(Command::Run(Action::RunChoose));
    s.ws.handle(Event::Run(RunEvent::FactsRead {
        facts: crate::core::run::RunFacts::default(),
    }));
    s.ws.dispatch(Command::Newline);
    assert!(s.ws.run_confirm().is_some());
}

#[test]
fn the_question_shows_the_whole_command_and_where_it_runs() {
    let mut s = Screen::new();
    let tail = "rm -rf ~";
    ask(&mut s, &format!("echo hi;{}{tail}", " ".repeat(170)));
    let rows = s.draw(100, 30);
    find_row(&rows, "Run a command from this project's config?");
    find_row(&rows, "setup");
    find_row(&rows, "tools");
    find_row(&rows, tail);
    find_row(&rows, "y run");
}

#[test]
fn control_characters_in_a_command_are_spelled_out() {
    let mut s = Screen::new();
    ask(&mut s, "true\n\u{1b}[2J\u{202e}rm -rf ~");
    let rows = s.draw(100, 30);
    let row = find_row(&rows, "rm -rf ~");
    assert!(row.contains("true\\n"), "{row}");
    assert!(row.contains("\\u{1b}"), "{row}");
    assert!(row.contains("\\u{202e}"), "{row}");
    assert!(!rows.concat().contains('\u{1b}'));
}

#[test]
fn a_command_too_long_for_the_popup_says_there_is_more_and_scrolls() {
    let mut s = Screen::new();
    let command = format!("{}END", "a".repeat(5000));
    ask(&mut s, &command);
    let rows = s.draw(100, 24);
    find_row(&rows, "more rows");
    assert!(!rows.iter().any(|r| r.contains("END")));
    for _ in 0..200 {
        s.ws.dispatch(Command::Move(Motion::Down));
    }
    let rows = s.draw(100, 24);
    find_row(&rows, "END");
    assert!(!rows.iter().any(|r| r.contains("more rows")));
}

#[test]
fn the_question_fits_a_small_window_without_panicking() {
    let mut s = Screen::new();
    ask(&mut s, "true");
    for (w, h) in [(40, 10), (44, 11), (60, 12)] {
        let rows = s.draw(w, h);
        find_row(&rows, "y run");
    }
}

#[test]
fn the_wheel_moves_the_view_with_the_first_turn() {
    use ratatui::crossterm::event::{KeyModifiers, MouseEvent, MouseEventKind};
    let mut s = Screen::new();
    start_run(&mut s, "x");
    print(&mut s, numbered(100), "");
    let rows = s.draw(100, 30);
    find_row(&rows, "line 99");
    let y = rows.iter().position(|r| r.contains("line 98")).unwrap();
    let wheel = |kind| MouseEvent {
        kind,
        column: 40,
        row: y as u16,
        modifiers: KeyModifiers::NONE,
    };
    let now = std::time::Instant::now();
    for command in crate::ui::mouse::handle(&wheel(MouseEventKind::ScrollUp), &mut s.ui, &s.ws, now)
    {
        s.ws.dispatch(command);
    }
    let rows = s.draw(100, 30);
    assert!(!rows.iter().any(|r| r.contains("line 99")), "{rows:#?}");
    find_row(&rows, "line 96");
    for command in
        crate::ui::mouse::handle(&wheel(MouseEventKind::ScrollDown), &mut s.ui, &s.ws, now)
    {
        s.ws.dispatch(command);
    }
    find_row(&s.draw(100, 30), "line 99");
    assert_eq!(
        s.ws.run_panel().unwrap().selected(),
        None,
        "following again"
    );
}

fn with_server_state(name: &str, server: Language, state: LspState) -> Screen {
    let mut s = Screen::new();
    s.open(name, "x\n");
    s.ws.handle(Event::Lsp(LspEvent::StateChanged {
        server: server_for(server),
        state,
    }));
    s
}

fn status_bar(s: &mut Screen, width: u16) -> String {
    s.draw(width, 20).last().cloned().unwrap_or_default()
}

#[test]
fn the_status_bar_names_the_active_documents_server() {
    let mut s = with_server_state("a.py", Language::Python, LspState::Crashed);
    assert!(status_bar(&mut s, 120).contains("pyright-langserver crashed"));
    s.ws.handle(Event::Lsp(LspEvent::ServerNamed {
        server: server_for(Language::Python),
        name: "pylsp".to_owned(),
    }));
    assert!(status_bar(&mut s, 120).contains("pylsp crashed"));
    let mut s = with_server_state("a.go", Language::Go, LspState::Stopped);
    assert!(status_bar(&mut s, 120).contains("gopls stopped"));
    let mut s = with_server_state("a.rs", Language::Rust, LspState::NotFound);
    let bar = status_bar(&mut s, 120);
    assert!(bar.contains("rust-analyzer not found"), "{bar}");
}

#[test]
fn another_documents_server_is_not_this_ones_trouble() {
    let mut s = with_server_state("a.go", Language::Go, LspState::Crashed);
    s.open("b.py", "x\n");
    s.ws.handle(Event::Lsp(LspEvent::Ready {
        server: server_for(Language::Python),
        encoding: PositionEncoding::Utf8,
        completion_triggers: Vec::new(),
    }));
    let bar = status_bar(&mut s, 120);
    assert!(!bar.contains("crashed"), "{bar}");
    assert!(!bar.contains("gopls"), "{bar}");
}

#[test]
fn a_missing_server_shows_the_command_to_install_it_even_when_narrow() {
    let cases = [
        ("a.php", Language::Php, "npm install -g intelephense"),
        (
            "a.ts",
            Language::TypeScript,
            "npm install -g typescript-language-server typescript",
        ),
        ("A.java", Language::Java, "brew install jdtls"),
        (
            "a.go",
            Language::Go,
            "go install golang.org/x/tools/gopls@latest",
        ),
        ("a.py", Language::Python, "npm install -g pyright"),
        (
            "a.sh",
            Language::Bash,
            "npm install -g bash-language-server",
        ),
    ];
    for (name, language, command) in cases {
        let mut s = with_server_state(name, language, LspState::NotFound);
        let wide = status_bar(&mut s, 140);
        let server = language.server().unwrap().name();
        assert!(
            wide.contains(&format!("{server} not found: {command}")),
            "{wide}"
        );
        let narrow = status_bar(&mut s, 80);
        assert!(narrow.contains(command), "{name}: {narrow}");
        assert!(narrow.contains("not found"), "{name}: {narrow}");
    }
}

#[test]
fn a_disabled_server_shows_nothing() {
    let mut s = with_server_state("a.go", Language::Go, LspState::Disabled);
    let bar = status_bar(&mut s, 120);
    assert!(!bar.contains("gopls"), "{bar}");
    assert!(bar.contains("Go"), "{bar}");
}

#[test]
fn line_breaking_controls_inside_a_line_stay_off_the_screen() {
    let mut s = Screen::new();
    s.open(
        "a.py",
        "x = 1\u{c}\nform\u{c}feed\u{b}v\u{85}n\u{2028}l\u{2029}p\nnext\n",
    );
    let screen = s.draw(80, 20);
    let joined = screen.join("\n");
    for control in ['\u{c}', '\u{b}', '\u{85}', '\u{2028}', '\u{2029}'] {
        assert!(!joined.contains(control), "{control:?} in {joined}");
    }
    assert!(joined.contains("next"), "{joined}");
    let row = screen.iter().find(|l| l.contains("form")).unwrap();
    for text in ["form", "feed"] {
        assert!(row.contains(text), "{row}");
    }
}

#[test]
fn moving_past_a_form_feed_stays_in_step_with_the_screen() {
    let mut s = Screen::new();
    s.open("a.py", "a\u{c}b\nc\n");
    s.run(Command::Move(Motion::Down));
    let (view, d) = s.ws.active().unwrap();
    assert_eq!(d.text().char_to_line(view.cursor.idx), 1);
    assert_eq!(
        view.cursor.idx, 4,
        "the next real line, not the rest of this one"
    );
}

#[path = "git_render_tests.rs"]
mod git_render_tests;

mod debug {
    use serde_json::json;

    use super::*;
    use crate::core::debug::{AdapterCommand, AdapterId, DebugFacts, Transport};

    fn click(s: &mut Screen, x: u16, y: u16) {
        use ratatui::crossterm::event::{KeyModifiers, MouseButton, MouseEvent, MouseEventKind};
        let event = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: x,
            row: y,
            modifiers: KeyModifiers::NONE,
        };
        let now = std::time::Instant::now();
        for command in crate::ui::mouse::handle(&event, &mut s.ui, &s.ws, now) {
            s.ws.dispatch(command);
        }
    }

    fn pause_at(s: &mut Screen, line: i64) {
        s.ws.dispatch(Command::Run(Action::DebugChoose));
        let adapter = AdapterCommand {
            id: AdapterId::Debugpy,
            program: "python3".to_owned(),
            args: Vec::new(),
            transport: Transport::Stdio,
        };
        s.ws.handle(Event::Debug(DebugEvent::Detected {
            facts: DebugFacts {
                adapters: [(AdapterId::Debugpy, adapter)].into(),
                file: Some(PathBuf::from("/proj/a.py")),
                ..DebugFacts::default()
            },
        }));
        s.ws.dispatch(Command::Newline);
        let session = s.ws.debug_session().unwrap().id;
        let stopped = json!({"type": "event", "event": "stopped",
            "body": {"reason": "breakpoint", "threadId": 1}});
        let effects = s.ws.handle(Event::Debug(DebugEvent::Message {
            session,
            conn: 0,
            message: stopped,
        }));
        let seq = effects
            .iter()
            .find_map(|e| match e {
                Effect::Debug(DebugEffect::Send { message, .. })
                    if message["command"] == "stackTrace" =>
                {
                    Some(message["seq"].clone())
                }
                _ => None,
            })
            .unwrap();
        s.ws.handle(Event::Debug(DebugEvent::Message {
            session,
            conn: 0,
            message: json!({"type": "response", "request_seq": seq, "success": true, "body": {
                "stackFrames": [{"id": 1, "name": "work", "line": line, "column": 1,
                                 "source": {"path": "/proj/a.py"}}]}}),
        }));
        let generation = s.ws.debug_session().unwrap().frames.len() as u64;
        assert_eq!(generation, 1);
        s.ws.handle(Event::Debug(DebugEvent::FilesProbed {
            session,
            generation: 1,
            existing: vec![PathBuf::from("/proj/a.py")],
        }));
    }

    #[test]
    fn each_kind_of_breakpoint_has_its_own_shape_and_the_text_stays_put() {
        let mut s = Screen::new();
        s.open("a.py", "one\ntwo\nthree\n");
        let plain = s.draw(80, 24).join("\n");
        let view = s.ws.active_view().unwrap();
        s.ws.dispatch(Command::ToggleBreakpointAt { view, line: 1 });
        let marked = s.draw(80, 24).join("\n");
        let row = marked.lines().find(|l| l.contains("two")).unwrap();
        assert!(row.contains("●"), "{row}");
        assert_eq!(
            row.chars().position(|c| c == 't'),
            plain
                .lines()
                .find(|l| l.contains("two"))
                .unwrap()
                .chars()
                .position(|c| c == 't'),
            "a mark must not push the text"
        );
    }

    #[test]
    fn the_stop_is_marked_and_the_panel_says_why() {
        let mut s = Screen::new();
        s.open("a.py", "one\ntwo\n");
        pause_at(&mut s, 2);
        let screen = s.draw(120, 30).join("\n");
        let row = screen.lines().find(|l| l.contains("two")).unwrap();
        assert!(row.contains("▶"), "{row}");
        assert!(screen.contains("Paused: breakpoint"), "{screen}");
        assert!(screen.contains("work"), "{screen}");
        assert!(screen.contains("Frames"), "{screen}");
        assert!(screen.contains("Console"), "{screen}");
    }

    #[test]
    fn a_click_on_a_line_number_sets_a_breakpoint_and_on_the_text_does_not() {
        let mut s = Screen::new();
        s.open("a.py", "one\ntwo\n");
        let screen = s.draw(80, 24).join("\n");
        let (y, line) = screen
            .lines()
            .enumerate()
            .find(|(_, l)| l.contains("two"))
            .unwrap();
        let chars: Vec<char> = line.chars().collect();
        let text_x = chars.iter().position(|c| *c == 't').unwrap();
        click(&mut s, text_x as u16, y as u16);
        let path = PathBuf::from("/proj/a.py");
        assert!(s.ws.breakpoint_at(&path, 1).is_none());
        let number_x = chars.iter().position(|c| *c == '2').unwrap();
        click(&mut s, number_x as u16, y as u16);
        assert!(s.ws.breakpoint_at(&path, 1).is_some());
    }

    #[test]
    fn the_line_beside_the_number_is_part_of_what_can_be_clicked() {
        let mut s = Screen::new();
        s.open("a.py", "one\ntwo\n");
        let screen = s.draw(80, 24).join("\n");
        let (y, line) = screen
            .lines()
            .enumerate()
            .find(|(_, l)| l.contains("two"))
            .unwrap();
        let chars: Vec<char> = line.chars().collect();
        let word = chars.iter().position(|c| *c == 't').unwrap();
        assert_eq!(chars[word - 2], '│');
        let path = PathBuf::from("/proj/a.py");
        click(&mut s, word as u16 - 1, y as u16);
        assert!(s.ws.breakpoint_at(&path, 1).is_none(), "the space is text");
        click(&mut s, word as u16 - 2, y as u16);
        assert!(s.ws.breakpoint_at(&path, 1).is_some());
    }

    fn button_at(s: &Screen, screen: &str, text: &str) -> Option<Action> {
        let (y, line) = screen.lines().enumerate().find(|(_, l)| l.contains(text))?;
        let byte = line.find(text)?;
        let x = line[..byte].chars().count();
        match s.ui.hits.at(x as u16, y as u16) {
            Some(crate::ui::mouse::Hit::Button(action)) => Some(*action),
            _ => None,
        }
    }

    #[test]
    fn the_toolbar_has_run_debug_stop_and_the_chooser_in_the_title_row() {
        let mut s = Screen::new();
        s.open("a.py", "one\n");
        let rows = s.draw(120, 30);
        let title = rows[0].clone();
        let screen = rows.join("\n");
        assert_eq!(button_at(&s, &screen, "Run…"), Some(Action::RunChoose));
        assert_eq!(button_at(&s, &screen, "▶"), Some(Action::RerunLast));
        let at = |glyph: char| title.chars().position(|c| c == glyph).unwrap() as u16;
        let hit = |s: &Screen, x: u16| s.ui.hits.at(x, 0).cloned();
        assert_eq!(
            hit(&s, at('◉')),
            Some(crate::ui::mouse::Hit::Button(Action::DebugLast))
        );
        assert_eq!(
            hit(&s, at('■')),
            Some(crate::ui::mouse::Hit::Button(Action::StopRun))
        );
    }

    #[test]
    fn a_narrow_title_row_keeps_the_buttons_and_gives_up_the_chooser() {
        let mut s = Screen::new();
        let title = s.draw(80, 24)[0].clone();
        assert!(!title.contains("Run…"), "{title}");
        assert!(title.contains('■'), "{title}");
    }

    #[test]
    fn stop_reaches_a_debug_session_whatever_tab_is_showing() {
        let mut s = Screen::new();
        s.open("a.py", "one\ntwo\n");
        assert!(!s.ws.can_stop());
        pause_at(&mut s, 2);
        assert!(s.ws.can_stop());
        s.ws.dispatch(Command::Run(Action::ShowProblems));
        let effects = s.ws.dispatch(Command::Run(Action::StopRun));
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::Debug(DebugEffect::Stop { .. })))
        );
    }

    #[test]
    fn the_panel_buttons_step_the_program_and_change_with_its_state() {
        let mut s = Screen::new();
        s.open("a.py", "one\ntwo\n");
        pause_at(&mut s, 2);
        let screen = s.draw(140, 30).join("\n");
        assert_eq!(
            button_at(&s, &screen, "↷ n over"),
            Some(Action::DebugStepOver)
        );
        assert_eq!(button_at(&s, &screen, "■ s stop"), Some(Action::DebugStop));
        s.ws.dispatch(Command::Run(Action::DebugStepOver));
        let screen = s.draw(140, 30).join("\n");
        assert_eq!(
            button_at(&s, &screen, "‖ p pause"),
            Some(Action::DebugPause)
        );
        assert!(!screen.contains("n over"), "{screen}");
    }

    #[test]
    fn a_narrow_panel_shows_only_the_part_with_the_keys() {
        let mut s = Screen::new();
        s.open("a.py", "one\ntwo\n");
        pause_at(&mut s, 2);
        let screen = s.draw(70, 30).join("\n");
        assert!(screen.contains("Frames"), "{screen}");
        assert!(!screen.contains("Console"), "{screen}");
    }

    #[test]
    fn the_empty_panel_says_how_to_start() {
        let mut s = Screen::new();
        s.open("a.py", "one\n");
        s.ws.dispatch(Command::Run(Action::DebugShow));
        let screen = s.draw(120, 30).join("\n");
        assert!(screen.contains("Ctrl+F8"), "{screen}");
        assert!(screen.contains("Alt+Shift+F9"), "{screen}");
    }
}

#[test]
fn the_language_server_panel_shows_failure_and_install_command() {
    let mut s = Screen::new();
    s.open("a.rs", "fn main() {}\n");
    s.ws.handle(Event::Lsp(LspEvent::StateChanged {
        server: rust_server(),
        state: LspState::Crashed,
    }));
    s.ws.handle(Event::Lsp(LspEvent::Failed {
        server: rust_server(),
        reason: "exited with code 3: boom".to_owned(),
    }));
    s.ws.dispatch(Command::Run(Action::ShowLanguageServer));
    let screen = s.draw(100, 30).join("\n");
    assert!(screen.contains("Language Server"), "{screen}");
    assert!(screen.contains("Rust"), "{screen}");
    assert!(screen.contains("exited with code 3: boom"), "{screen}");
    assert!(screen.contains("Restart"), "{screen}");
    assert!(screen.contains("Choose"), "{screen}");
}

#[test]
fn the_language_server_panel_says_when_there_is_none() {
    let mut s = Screen::new();
    s.open("n.txt", "x\n");
    s.ws.dispatch(Command::Run(Action::ShowLanguageServer));
    let screen = s.draw(100, 30).join("\n");
    assert!(
        screen.contains("No language server for this file"),
        "{screen}"
    );
}

#[test]
fn a_plugin_command_has_a_row_of_its_own_in_the_palette() {
    let mut s = Screen::new();
    s.ws.handle(crate::core::Event::PluginsLoaded {
        plugins: vec![crate::core::PluginInfo {
            name: "hello".into(),
            commands: vec![crate::core::PluginCommand {
                id: "say".into(),
                title: "Say Hello".into(),
            }],
        }],
        problems: Vec::new(),
    });
    s.run(Command::Run(Action::OpenPalette));
    s.run(Command::Insert("Say Hello".into()));
    let rows = s.draw(120, 30);
    let line = find_row(&rows, "Plugin: Say Hello");
    assert!(line.contains("Plugins"), "{line}");
}

#[test]
fn the_http_tab_shows_the_answer_and_its_icon_opens_it() {
    let mut s = Screen::new();
    s.open("a.http", "GET http://a/{{path}}\n");
    let joined = |rows: Vec<String>| rows.join("\n");
    assert!(joined(s.draw(100, 24)).contains('⇄'));
    let fx = s.ws.dispatch(Command::Run(Action::HttpRun));
    let id = fx
        .iter()
        .find_map(|e| match e {
            Effect::Http(crate::core::http::HttpEffect::Send { id, .. }) => Some(*id),
            _ => None,
        })
        .expect("sent");
    let waiting = joined(s.draw(100, 24));
    assert!(
        waiting.contains("GET http://a/{{path}}") && waiting.contains("sending"),
        "{waiting}"
    );
    s.ws.handle(Event::Http(crate::core::http::HttpEvent::Finished {
        id,
        outcome: crate::core::http::HttpOutcome::Done(crate::core::http::HttpResponse {
            status: "HTTP/1.1 404 Not Found".to_owned(),
            code: 404,
            headers: vec!["Server: x".to_owned()],
            body: vec!["the body".to_owned()],
            body_bytes: 8,
            binary: false,
            truncated: false,
            elapsed_ms: 12,
        }),
    }));
    let done = joined(s.draw(100, 24));
    for wanted in ["HTTP/1.1 404 Not Found", "12 ms", "Server: x", "the body"] {
        assert!(done.contains(wanted), "{wanted}: {done}");
    }
    assert!(!done.contains("sending"));
}

#[test]
fn the_database_tab_shows_lists_a_grid_and_the_statement_before_it_writes() {
    use crate::core::db::{DbConnection, DbEndpoint, DbGrid, DbOutcome};
    let mut s = Screen::new();
    s.open("q.sql", "DROP TABLE users");
    s.ws.configure_databases(vec![DbConnection {
        name: "local".to_owned(),
        endpoint: DbEndpoint::Sqlite {
            path: "/secret/place.db".into(),
        },
    }]);
    let joined = |rows: Vec<String>| rows.join("\n");
    assert!(joined(s.draw(100, 24)).contains('⊟'));

    s.ws.dispatch(Command::Run(Action::DbRun));
    let asking = joined(s.draw(100, 24));
    for wanted in ["may change data in local", "DROP TABLE users", "y run"] {
        assert!(asking.contains(wanted), "{wanted}\n{asking}");
    }
    s.ws.dispatch(Command::Escape);

    s.ws.dispatch(Command::Run(Action::ShowDatabase));
    let fx = s.ws.dispatch(Command::Newline);
    let id = fx
        .iter()
        .find_map(|e| match e {
            Effect::Db(crate::core::db::DbEffect::Query { id, .. }) => Some(*id),
            _ => None,
        })
        .expect("sent");
    s.ws.handle(Event::Db(crate::core::db::DbEvent::Finished {
        id,
        outcome: DbOutcome::Done(DbGrid {
            columns: vec!["name".to_owned()],
            rows: vec![vec!["users".to_owned()]],
            ..DbGrid::default()
        }),
    }));
    let fx = s.ws.dispatch(Command::Newline);
    let id = fx
        .iter()
        .find_map(|e| match e {
            Effect::Db(crate::core::db::DbEffect::Query { id, .. }) => Some(*id),
            _ => None,
        })
        .expect("sent");
    s.ws.handle(Event::Db(crate::core::db::DbEvent::Finished {
        id,
        outcome: DbOutcome::Done(DbGrid {
            columns: vec!["id".to_owned(), "email".to_owned()],
            rows: vec![vec!["1".to_owned(), "a@b".to_owned()]],
            truncated: true,
            elapsed_ms: 3,
        }),
    }));
    let shown = joined(s.draw(100, 24));
    for wanted in ["● local", "users", "email", "a@b", "row 1/1", "not kept"] {
        assert!(shown.contains(wanted), "{wanted}\n{shown}");
    }
    assert!(!shown.contains("/secret/place.db"), "{shown}");

    s.ws.dispatch(Command::Move(crate::core::Motion::Right));
    let scrolled = joined(s.draw(100, 24));
    assert!(scrolled.contains("column 2/2"), "{scrolled}");
    assert!(!scrolled.contains("id │"), "{scrolled}");
}

#[test]
fn sizes_no_terminal_would_send_still_draw() {
    for command in [
        Command::ResizeTool { rows: u16::MAX },
        Command::ResizeTool { rows: 0 },
        Command::ResizeProject { columns: u16::MAX },
        Command::ResizeProject { columns: 0 },
        Command::ToolRoom {
            rows: 0,
            min: 100,
            max: 0,
        },
        Command::Viewport { text_cols: 0 },
        Command::Viewport {
            text_cols: usize::MAX,
        },
    ] {
        let mut s = Screen::new();
        s.run(Command::Run(Action::ShowProblems));
        s.run(command);
        drop(s.draw(120, 30));
        drop(s.draw(40, 10));
    }
}

#[test]
fn the_status_bar_says_how_far_indexing_is_and_stops_saying_it() {
    let mut s = Screen::new();
    s.ws.handle(Event::IndexProgress {
        done: 12,
        total: 340,
    });
    assert!(status_bar(&mut s, 120).contains("Indexing 12/340"));
    s.ws.handle(Event::IndexProgress {
        done: 340,
        total: 340,
    });
    assert!(!status_bar(&mut s, 120).contains("Indexing"));
}

mod tool_lists {
    use super::*;

    fn text(screen: &mut Screen) -> String {
        screen.draw(100, 30).join("\n")
    }

    #[test]
    fn the_title_says_when_a_project_is_not_trusted() {
        let mut screen = Screen::new();
        assert!(!text(&mut screen).contains("untrusted"));
        screen.ws.configure_trust(Some(false));
        assert!(
            text(&mut screen).contains("untrusted"),
            "{}",
            text(&mut screen)
        );
    }

    #[test]
    fn the_trust_question_names_its_reason_and_answers() {
        let mut screen = Screen::new();
        screen.ws.configure_trust(Some(false));
        let _ = screen.ws.dispatch(Command::Run(Action::AnalyzeProject));
        let text = text(&mut screen);
        assert!(text.contains("Trust This Project?"), "{text}");
        assert!(
            !text.contains("Trash"),
            "a yes or no here is not about deleting: {text}"
        );
        assert!(text.contains("Analyze Project runs"), "{text}");
    }

    #[test]
    fn the_empty_panels_say_what_they_are_for() {
        let mut screen = Screen::new();
        let _ = screen.ws.dispatch(Command::Run(Action::ShowBookmarks));
        assert!(
            text(&mut screen).contains("No bookmarks yet"),
            "{}",
            text(&mut screen)
        );
        let _ = screen.ws.dispatch(Command::Run(Action::ShowTodos));
        assert!(
            text(&mut screen).contains("searching"),
            "{}",
            text(&mut screen)
        );
    }

    #[test]
    fn the_stripe_shows_both_icons() {
        let screen_text = text(&mut Screen::new());
        assert!(screen_text.contains('☑'), "{screen_text}");
        assert!(screen_text.contains('⚑'), "{screen_text}");
    }
}

#[test]
fn coverage_marks_have_two_shapes_and_the_status_bar_says_how_much() {
    let mut s = Screen::new();
    s.open("a.txt", "ran\nmissed\nblank\n");
    let mut report = crate::core::coverage::CoverageReport::default();
    report.files.insert(
        PathBuf::from("/proj/a.txt"),
        [(1, 2), (2, 0)].into_iter().collect(),
    );
    s.ws.handle(Event::Coverage(
        crate::core::coverage::CoverageEvent::Read { result: Ok(report) },
    ));
    let rows = s.draw(100, 24);
    assert!(
        find_row(&rows, "ran").contains("✓ ran"),
        "{}",
        rows.join("\n")
    );
    assert!(find_row(&rows, "missed").contains("✗ missed"));
    assert!(find_row(&rows, "blank").contains("│ blank"));
    assert!(
        rows.iter().any(|r| r.contains("cov 50%")),
        "{}",
        rows.join("\n")
    );
}

#[test]
fn the_duplicates_panel_lists_groups_and_places_with_its_mode_in_words() {
    let mut s = Screen::new();
    s.ws.dispatch(Command::Run(Action::LocateDuplicates));
    let rows = s.draw(100, 24);
    assert!(
        rows.iter().any(|r| r.contains("scanning")),
        "{}",
        rows.join("\n")
    );
    assert!(rows.iter().any(|r| r.contains("a names may differ")));
    s.ws.dispatch(Command::Insert("a".into()));
    let rows = s.draw(100, 24);
    assert!(
        rows.iter().any(|r| r.contains("[names may differ]")),
        "{}",
        rows.join("\n")
    );
}

#[test]
fn the_compare_tab_shows_each_sides_commits_by_arrow_and_then_the_files() {
    use crate::core::{GitCompareCommit, GitCompared};
    let mut s = Screen::new();
    s.ws.dispatch(Command::Run(Action::GitCompareBranch));
    s.ws.handle(Event::Git(GitEvent::BranchesRead {
        result: Ok(vec![crate::core::GitBranch {
            name: "main".into(),
            remote: false,
        }]),
    }));
    s.ws.dispatch(Command::Newline);
    let commit = |subject: &str, ours| GitCompareCommit {
        commit: crate::core::GitCommit {
            hash: "abcdef1234".into(),
            parents: Vec::new(),
            author: "a".into(),
            time: 0,
            refs: Vec::new(),
            subject: subject.into(),
        },
        ours,
    };
    s.ws.handle(Event::Git(GitEvent::ComparedRead {
        base: "main".into(),
        path: None,
        result: Ok(GitCompared {
            commits: vec![commit("here", true), commit("there", false)],
            files: vec![crate::core::GitFile::new(
                PathBuf::from("b.txt"),
                crate::core::GitChange::Added,
            )],
        }),
    }));
    let rows = s.draw(120, 30);
    assert!(
        rows.iter().any(|r| r.contains("This branch against main")),
        "{}",
        rows.join("\n")
    );
    assert!(find_row(&rows, "here").contains('↑'));
    assert!(find_row(&rows, "there").contains('↓'));
    assert!(rows.iter().any(|r| r.contains("A b.txt")));
}
