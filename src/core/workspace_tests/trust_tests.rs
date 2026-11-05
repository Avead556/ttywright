use super::*;
use crate::core::file_op::FilePromptKind;
use crate::core::git_command::GitEffect;
use crate::core::lsp_command::LspEffect;
use crate::core::run::{ConfiguredRun, RunCommand, RunSource, RunTarget};
use crate::core::run_state::{RunEffect, RunEvent};

fn untrusted(files: &[(&str, &str)]) -> Harness {
    let mut h = Harness::new(files);
    h.ws.configure_trust(Some(false));
    h
}

fn target(source: RunSource) -> RunTarget {
    RunTarget {
        name: "t".to_owned(),
        command: RunCommand::Shell("x".to_owned()),
        cwd: PathBuf::from(ROOT),
        source,
    }
}

fn asks_trust(h: &Harness) -> bool {
    matches!(
        h.ws.file_prompt().map(|p| &p.kind),
        Some(FilePromptKind::TrustProject { .. })
    )
}

#[test]
fn only_what_the_project_decides_needs_trust() {
    let run = |source| {
        Effect::Run(RunEffect::Start {
            id: 1,
            target: target(source),
            root: PathBuf::from(ROOT),
            replaces: None,
        })
    };
    for source in [
        RunSource::Detected,
        RunSource::CurrentFile,
        RunSource::ProjectConfig,
    ] {
        assert!(run(source).needs_trust());
    }
    assert!(!run(RunSource::UserConfig).needs_trust());
    assert!(!run(RunSource::Typed).needs_trust());
    let root = PathBuf::from(ROOT);
    assert!(Effect::Git(GitEffect::Status { root: root.clone() }).needs_trust());
    assert!(
        Effect::Analysis(crate::core::analysis::AnalysisEffect::Detect { root: root.clone() })
            .needs_trust()
    );
    assert!(!Effect::ListDir { path: root }.needs_trust());
}

#[test]
fn actions_that_would_run_the_project_are_refused_with_an_offer_to_trust() {
    for action in [
        Action::AnalyzeProject,
        Action::HttpRun,
        Action::ReformatCode,
        Action::DebugChoose,
        Action::DebugLast,
        Action::RunTest,
        Action::GitLog,
        Action::GitCommit,
        Action::GitPush,
    ] {
        let mut h = untrusted(&[
            ("a.http", "GET http://x\n"),
            ("a_test.py", "def test_a(): pass\n"),
        ]);
        h.open("a.http");
        let effects = h.ws.dispatch(Command::Run(action));
        assert!(
            effects.iter().all(|e| !e.needs_trust()),
            "{action:?}: {effects:?}"
        );
        assert!(asks_trust(&h), "{action:?} offers trust");
    }
}

#[test]
fn opening_a_file_starts_no_server_and_saving_asks_git_nothing() {
    let mut h = untrusted(&[("a.rs", "x")]);
    h.open("a.rs");
    assert!(
        !h.lsp.iter().any(|e| e.needs_trust()),
        "rust-analyzer would run build scripts: {:?}",
        h.lsp
    );
    h.run(Command::Insert("y".to_owned()));
    let effects = h.ws.dispatch(Command::Run(Action::Save));
    let written = effects
        .into_iter()
        .inspect(|e| assert!(!e.needs_trust(), "{e:?}"))
        .collect();
    h.perform(written);
    assert!(h.git.iter().all(|e| !e.needs_trust()), "{:?}", h.git);
    assert!(!h.written.is_empty(), "editing still works");
}

#[test]
fn asking_a_server_says_why_there_is_none() {
    let mut h = untrusted(&[("a.rs", "x")]);
    h.open("a.rs");
    h.act(Action::GotoDefinition);
    assert!(
        status_text(&h).contains("not trusted"),
        "{}",
        status_text(&h)
    );
}

#[test]
fn the_run_list_has_only_the_users_own_commands() {
    let mut h = untrusted(&[("a.py", "")]);
    h.open("a.py");
    let run = |name: &str, from_project| ConfiguredRun {
        name: name.to_owned(),
        command: "x".to_owned(),
        cwd: None,
        from_project,
    };
    h.ws.configure_run(vec![run("mine", false), run("theirs", true)]);
    h.run_facts.present.insert("Cargo.toml".to_owned());
    h.act(Action::RunChoose);
    let items = h.ws.picker_items(PickerKind::Run);
    assert_eq!(items.len(), 1, "{items:?}");
    assert!(items[0].contains("mine"));
}

#[test]
fn with_nothing_of_the_users_own_to_run_it_offers_trust() {
    let mut h = untrusted(&[("a.py", "")]);
    h.open("a.py");
    h.act(Action::RunChoose);
    assert!(asks_trust(&h));
}

#[test]
fn an_unknown_project_asks_once_and_enter_is_not_yes() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ws.configure_trust(None);
    assert!(asks_trust(&h));
    let effects = h.ws.dispatch(Command::Newline);
    assert!(
        effects
            .iter()
            .all(|e| !matches!(e, Effect::SaveTrust { .. }))
    );
    assert!(asks_trust(&h), "Enter neither grants nor closes");
    assert!(!h.ws.is_trusted());
}

#[test]
fn yes_is_remembered_and_starts_what_was_held_back() {
    let mut h = untrusted(&[("a.rs", "x")]);
    h.open("a.rs");
    h.act(Action::TrustProject);
    let effects = h.ws.dispatch(Command::Insert("y".to_owned()));
    assert!(h.ws.is_trusted());
    assert!(effects.iter().any(
        |e| matches!(e, Effect::SaveTrust { trusted: true, root } if root == Path::new(ROOT))
    ));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Status { .. })))
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Base { .. })))
    );
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Lsp(LspEffect::FindRoot { .. })))
    );
}

#[test]
fn no_is_remembered_and_escape_only_closes() {
    let mut h = Harness::new(&[]);
    h.ws.configure_trust(None);
    let effects = h.ws.dispatch(Command::Insert("n".to_owned()));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::SaveTrust { trusted: false, .. }))
    );
    assert!(h.ws.file_prompt().is_none());
    assert!(!h.ws.is_trusted());

    let mut h = Harness::new(&[]);
    h.ws.configure_trust(None);
    let effects = h.ws.dispatch(Command::Escape);
    assert!(
        effects
            .iter()
            .all(|e| !matches!(e, Effect::SaveTrust { .. }))
    );
    assert!(h.ws.file_prompt().is_none());
    assert!(!h.ws.is_trusted());
}

#[test]
fn a_separate_window_never_asks() {
    for (mut ws, _) in [
        Workspace::new_git_window(PathBuf::from(ROOT)),
        Workspace::new_run_window(PathBuf::from(ROOT)),
    ] {
        ws.configure_trust(None);
        assert!(ws.file_prompt().is_none());
        assert!(!ws.is_trusted());
        assert!(
            ws.without_untrusted(vec![Effect::Git(GitEffect::Log {
                root: PathBuf::from(ROOT)
            })])
            .is_empty()
        );
    }
}

#[test]
fn trusting_is_offered_only_when_it_means_something() {
    let mut h = Harness::new(&[]);
    h.act(Action::TrustProject);
    assert!(h.ws.file_prompt().is_none());
    assert!(status_text(&h).contains("already"), "{}", status_text(&h));
}

#[test]
fn a_project_made_here_is_trusted_before_it_opens() {
    let mut h = Harness::new(&[]);
    let dir = PathBuf::from("/new/project");
    let op = crate::core::file_op::FileOp::CreateProject {
        dir: dir.clone(),
        files: Vec::new(),
    };
    let effects = h.ws.handle(Event::FileOpDone { op, result: Ok(()) });
    let saved = effects
        .iter()
        .position(|e| matches!(e, Effect::SaveTrust { trusted: true, root } if *root == dir));
    let opened = effects
        .iter()
        .position(|e| matches!(e, Effect::OpenProject { .. }));
    assert!(saved.is_some() && saved < opened, "{effects:?}");
}

mod run_anything {
    use super::*;

    fn typed(h: &mut Harness, text: &str) -> Vec<Effect> {
        h.act(Action::RunAnything);
        h.run(Command::Insert(text.to_owned()));
        h.ws.dispatch(Command::Newline)
    }

    fn started(effects: &[Effect]) -> Option<&RunTarget> {
        effects.iter().find_map(|e| match e {
            Effect::Run(RunEffect::Start { target, .. }) => Some(target),
            _ => None,
        })
    }

    #[test]
    fn what_is_typed_runs_in_the_users_shell_in_the_project_folder() {
        let mut h = Harness::new(&[]);
        let effects = typed(&mut h, "ls | wc");
        let target = started(&effects).expect("it starts");
        assert_eq!(target.command, RunCommand::UserShell("ls | wc".to_owned()));
        assert_eq!(target.cwd, Path::new(ROOT));
        assert_eq!(target.source, RunSource::Typed);
        assert_eq!(target.command_line(), "ls | wc");
        assert!(h.ws.file_prompt().is_none());
    }

    #[test]
    fn it_works_in_an_untrusted_project_as_the_user_typed_it() {
        let mut h = untrusted(&[]);
        let effects = typed(&mut h, "ls");
        assert!(started(&effects).is_some());
    }

    #[test]
    fn the_question_says_where_it_runs() {
        let mut h = Harness::new(&[]);
        h.act(Action::RunAnything);
        assert!(matches!(
            h.ws.file_prompt().map(|p| &p.kind),
            Some(FilePromptKind::RunAnything { .. })
        ));
    }

    #[test]
    fn empty_is_refused_in_the_prompt() {
        let mut h = Harness::new(&[]);
        let effects = typed(&mut h, "   ");
        assert!(started(&effects).is_none());
        assert!(h.ws.file_prompt().is_some_and(|p| p.error.is_some()));
    }

    #[test]
    fn history_goes_back_and_forward_and_is_saved_without_repeats() {
        let mut h = Harness::new(&[]);
        h.ws.configure_run_history(vec!["old".to_owned()]);
        let effects = typed(&mut h, "new");
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::SaveRunHistory { commands } if commands == &["old".to_owned(), "new".to_owned()]
        )));
        let effects = typed(&mut h, "old");
        assert!(effects.iter().any(|e| matches!(
            e,
            Effect::SaveRunHistory { commands } if commands == &["new".to_owned(), "old".to_owned()]
        )));
        h.act(Action::RunAnything);
        let shown = |h: &Harness| h.ws.file_prompt().map(|p| p.input.text().to_owned());
        h.run(Command::Move(Motion::Up));
        assert_eq!(shown(&h).as_deref(), Some("old"));
        h.run(Command::Move(Motion::Up));
        assert_eq!(shown(&h).as_deref(), Some("new"));
        h.run(Command::Move(Motion::Up));
        assert_eq!(shown(&h).as_deref(), Some("new"), "stops at the oldest");
        h.run(Command::Move(Motion::Down));
        assert_eq!(shown(&h).as_deref(), Some("old"));
        h.run(Command::Move(Motion::Down));
        assert_eq!(shown(&h).as_deref(), Some(""), "back to an empty line");
    }

    #[test]
    fn a_separate_window_does_not_run_anything() {
        let (mut ws, _) = Workspace::new_git_window(PathBuf::from(ROOT));
        let _ = ws.dispatch(Command::Run(Action::RunAnything));
        assert!(ws.file_prompt().is_none());
    }
}

mod scratch {
    use super::*;

    const DIR: &str = "/data/scratches";

    fn with_dir(files: &[(&str, &str)]) -> Harness {
        let mut h = Harness::new(files);
        h.ws.configure_scratch_dir(Some(PathBuf::from(DIR)));
        h
    }

    fn opened(h: &mut Harness, name: &str) -> PathBuf {
        let path = Path::new(DIR).join(name);
        h.files.push((path.clone(), "fn a() {}".to_owned()));
        let effects = h.ws.handle(Event::ScratchCreated {
            result: Ok(path.clone()),
        });
        h.perform(effects);
        path
    }

    #[test]
    fn choosing_a_kind_asks_for_a_file_with_its_extension() {
        let mut h = with_dir(&[]);
        h.act(Action::NewScratchFile);
        let rows = h.picker_labels();
        assert!(rows.iter().any(|r| r == "Rust"), "{rows:?}");
        h.run(Command::Insert("Python".to_owned()));
        let effects = h.ws.dispatch(Command::Newline);
        assert!(
            effects
                .iter()
                .any(|e| matches!(e, Effect::ScratchCreate { extension } if extension == "py")),
            "{effects:?}"
        );
    }

    #[test]
    fn the_new_file_opens_with_focus_and_autosaves_like_any_other() {
        let mut h = with_dir(&[]);
        let path = opened(&mut h, "scratch-1.rs");
        assert_eq!(h.ws.focus(), Focus::Editor);
        assert_eq!(h.active_name(), "scratch-1.rs");
        h.run(Command::Insert("x".to_owned()));
        h.run(Command::Autosave);
        assert!(h.written.iter().any(|(p, _)| *p == path), "{:?}", h.written);
    }

    #[test]
    fn no_language_server_is_started_for_a_scratch_file() {
        let mut h = with_dir(&[]);
        let _ = opened(&mut h, "scratch-1.rs");
        assert!(h.ws.is_trusted());
        let mut outside = Harness::new(&[]);
        let _ = opened(&mut outside, "scratch-1.rs");
        assert!(
            outside
                .lsp
                .iter()
                .any(|e| matches!(e, Effect::Lsp(LspEffect::Start { .. }))),
            "a server is wanted outside the scratch folder"
        );
        assert!(
            !h.lsp.iter().any(|e| matches!(
                e,
                Effect::Lsp(LspEffect::Start { .. }) | Effect::Lsp(LspEffect::FindRoot { .. })
            )),
            "{:?}",
            h.lsp
        );
    }

    #[test]
    fn the_chooser_lists_what_there_is_and_opens_it() {
        let mut h = with_dir(&[]);
        let path = Path::new(DIR).join("scratch-2.md");
        h.files.push((path.clone(), "note".to_owned()));
        let effects = h.ws.dispatch(Command::Run(Action::ShowScratches));
        assert!(effects.iter().any(|e| matches!(e, Effect::ScratchList)));
        let effects = h.ws.handle(Event::ScratchesListed {
            paths: vec![path.clone()],
        });
        h.perform(effects);
        assert_eq!(h.picker_labels(), ["scratch-2.md"]);
        h.run(Command::Newline);
        assert_eq!(h.active_name(), "scratch-2.md");
    }

    #[test]
    fn an_empty_chooser_and_a_failed_create_say_so() {
        let mut h = with_dir(&[]);
        let _ = h.ws.handle(Event::ScratchesListed { paths: Vec::new() });
        assert!(h.ws.picker().is_none());
        assert!(
            status_text(&h).contains("No scratch"),
            "{}",
            status_text(&h)
        );
        let _ = h.ws.handle(Event::ScratchCreated {
            result: Err("disk full".to_owned()),
        });
        assert!(status_text(&h).contains("disk full"), "{}", status_text(&h));
    }

    #[test]
    fn they_work_in_an_untrusted_project_and_not_in_a_separate_window() {
        let mut h = with_dir(&[]);
        h.ws.configure_trust(Some(false));
        h.act(Action::NewScratchFile);
        assert!(h.ws.picker().is_some());
        let (mut ws, _) = Workspace::new_git_window(PathBuf::from(ROOT));
        let _ = ws.dispatch(Command::Run(Action::NewScratchFile));
        assert!(ws.picker().is_none());
    }
}

mod list_panels {
    use super::*;

    fn found(name: &str, line: usize, col: usize, text: &str) -> SearchMatch {
        SearchMatch {
            path: Path::new(ROOT).join(name),
            line,
            col,
            len: 4,
            text: text.to_owned(),
        }
    }

    fn show_todos(h: &mut Harness, matches: Vec<SearchMatch>) {
        let fx = h.ws.dispatch(Command::Run(Action::ShowTodos));
        let id = fx
            .iter()
            .find_map(|e| match e {
                Effect::SearchProject { id, .. } => Some(*id),
                _ => None,
            })
            .expect("searched");
        h.perform(
            fx.into_iter()
                .filter(|e| !matches!(e, Effect::SearchProject { .. }))
                .collect(),
        );
        let fx = h.ws.handle(Event::SearchFinished {
            id,
            matches,
            truncated: false,
        });
        h.perform(fx);
    }

    #[test]
    fn todos_have_their_own_panel_and_leave_find_alone() {
        let mut h = Harness::new(&[("a.rs", "one\n// TODO x\n")]);
        h.ws.dispatch(Command::Run(Action::FindInProject));
        h.run(Command::Insert("kept".to_owned()));
        let fx = h.ws.dispatch(Command::Run(Action::ShowTodos));
        let (query, options) = fx
            .iter()
            .find_map(|e| match e {
                Effect::SearchProject { query, options, .. } => Some((query.clone(), *options)),
                _ => None,
            })
            .expect("searched");
        assert!(query.contains("TODO") && query.contains("FIXME"));
        assert!(options.regex && options.case_sensitive);
        assert_eq!(h.ws.tool_tab(), ToolTab::Todo);
        assert_eq!(h.ws.focus(), Focus::Todo);
        assert_eq!(h.ws.search().input.text(), "kept");
        assert!(!h.ws.search().options.regex, "no option stays switched on");
    }

    #[test]
    fn enter_goes_to_the_todo_and_arrows_stop_at_the_ends() {
        let mut h = Harness::new(&[("a.rs", "one\n// TODO x\n"), ("b.rs", "FIXME\n")]);
        show_todos(
            &mut h,
            vec![
                found("a.rs", 1, 3, "// TODO x"),
                found("b.rs", 0, 0, "FIXME"),
            ],
        );
        assert_eq!(h.ws.todo_panel().results.len(), 2);
        h.run(Command::Move(Motion::Up));
        assert_eq!(h.ws.todo_panel().selected, 0);
        h.run(Command::Move(Motion::Down));
        h.run(Command::Move(Motion::Down));
        assert_eq!(h.ws.todo_panel().selected, 1);
        h.run(Command::Move(Motion::Up));
        h.run(Command::Newline);
        assert_eq!(h.active_name(), "a.rs");
        assert_eq!(h.cursor(), 7);
        assert_eq!(h.ws.focus(), Focus::Editor);
    }

    #[test]
    fn r_refreshes_and_a_find_answer_does_not_land_in_the_panel() {
        let mut h = Harness::new(&[]);
        show_todos(&mut h, vec![found("a.rs", 0, 0, "TODO")]);
        let fx = h.ws.dispatch(Command::Insert("r".to_owned()));
        assert!(fx.iter().any(|e| matches!(e, Effect::SearchProject { .. })));
        let _ = h.ws.handle(Event::SearchFinished {
            id: 999_999,
            matches: Vec::new(),
            truncated: false,
        });
        assert_eq!(h.ws.todo_panel().results.len(), 1);
    }

    #[test]
    fn a_click_selects_and_a_double_click_opens() {
        let mut h = Harness::new(&[("a.rs", "TODO\n"), ("b.rs", "TODO\n")]);
        show_todos(
            &mut h,
            vec![found("a.rs", 0, 0, "TODO"), found("b.rs", 0, 0, "TODO")],
        );
        h.run(Command::SelectTodoRow {
            index: 1,
            open: false,
        });
        assert_eq!(h.ws.todo_panel().selected, 1);
        assert_eq!(h.ws.focus(), Focus::Todo);
        h.run(Command::SelectTodoRow {
            index: 9,
            open: true,
        });
        assert_eq!(h.ws.focus(), Focus::Todo, "no such row");
        h.run(Command::SelectTodoRow {
            index: 1,
            open: true,
        });
        assert_eq!(h.active_name(), "b.rs");
    }

    #[test]
    fn the_bookmark_panel_lists_jumps_also_in_a_closed_file_and_removes() {
        let mut h = Harness::new(&[("a.rs", "one\ntwo\n"), ("b.rs", "x\ny\n")]);
        h.act(Action::ShowBookmarks);
        assert_eq!(h.ws.tool_tab(), ToolTab::Bookmarks);
        assert_eq!(h.ws.focus(), Focus::Bookmarks);
        h.run(Command::Newline);
        h.run(Command::Insert("d".to_owned()));
        h.run(Command::Escape);
        h.open("a.rs");
        h.run(Command::Move(Motion::Down));
        h.act(Action::ToggleBookmark);
        let row = |h: &Harness| h.ws.bookmark_row(&h.ws.bookmarks()[0]);
        assert!(row(&h).ends_with("a.rs:2  two"), "{}", row(&h));
        h.act(Action::CloseTab);
        h.open("b.rs");
        assert!(row(&h).ends_with("a.rs:2"), "closed: no text: {}", row(&h));
        h.act(Action::ShowBookmarks);
        h.run(Command::Newline);
        assert_eq!(h.active_text(), "one\ntwo\n");
        assert_eq!(h.ws.active().map(|(v, _)| v.cursor.idx), Some(4));
        assert_eq!(h.ws.focus(), Focus::Editor);
        h.act(Action::ShowBookmarks);
        h.run(Command::Insert("d".to_owned()));
        assert!(h.ws.bookmarks().is_empty());
        assert_eq!(h.ws.bookmark_selected(), 0);
    }

    #[test]
    fn delete_removes_the_selected_bookmark_only() {
        let mut h = Harness::new(&[("a.rs", "1\n2\n3\n")]);
        h.open("a.rs");
        h.act(Action::ToggleBookmark);
        h.run(Command::Move(Motion::Down));
        h.act(Action::ToggleBookmark);
        h.run(Command::Move(Motion::Down));
        h.act(Action::ToggleBookmark);
        h.act(Action::ShowBookmarks);
        h.run(Command::Move(Motion::Down));
        h.run(Command::DeleteBackward);
        let lines: Vec<usize> = h.ws.bookmarks().iter().map(|b| b.line).collect();
        assert_eq!(lines, [0, 2]);
        assert_eq!(h.active_text(), "1\n2\n3\n", "the text is not touched");
    }
}

#[test]
fn a_file_opened_while_the_question_is_up_opens_and_can_be_saved() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.ws.configure_trust(None);
    assert!(asks_trust(&h));
    h.open("a.txt");
    assert!(asks_trust(&h), "the question stays");
    assert_eq!(h.text_of("a.txt").as_deref(), Some("x"));
    h.run(Command::Insert("n".to_owned()));
    h.run(Command::Insert("Y".to_owned()));
    assert_eq!(h.text_of("a.txt").as_deref(), Some("Yx"));
}

#[test]
fn typing_with_no_file_open_says_so_instead_of_vanishing() {
    let mut h = Harness::new(&[]);
    h.run(Command::Insert("x".to_owned()));
    assert!(
        status_text(&h).contains("No file is open"),
        "{}",
        status_text(&h)
    );
}

#[test]
fn unsaved_text_found_in_an_untrusted_project_waits_and_is_not_lost() {
    let mut h = Harness::new(&[]);
    h.ws.configure_trust(None);
    h.ws.handle(Event::SwapsFound {
        swaps: vec![recovered(Some(4), Some(4))],
    });
    assert!(h.ws.active().is_none(), "not believed yet");
    h.run(Command::Insert("n".to_owned()));
    assert!(h.ws.active().is_none(), "still not");
    h.act(Action::TrustProject);
    h.run(Command::Insert("y".to_owned()));
    assert_eq!(h.active_text(), "unsaved", "and now it comes back");
    assert!(h.dirty());
}

fn run_line(text: &str) -> crate::core::run_output::RunLine {
    crate::core::run_output::RunLine {
        text: text.to_owned(),
        location: None,
        spans: Vec::new(),
    }
}

#[test]
fn test_results_fill_from_any_run_and_lead_to_the_output() {
    let mut h = Harness::new(&[]);
    h.ws.dispatch(Command::Focus(Focus::Editor));
    let target = RunTarget {
        command: RunCommand::Argv(vec!["cargo".to_owned(), "test".to_owned()]),
        source: RunSource::UserConfig,
        ..target(RunSource::UserConfig)
    };
    let effects = h.ws.run_target_for_test(target);
    let id = effects
        .iter()
        .find_map(|e| match e {
            Effect::Run(RunEffect::Start { id, .. }) => Some(id),
            _ => None,
        })
        .expect("started");
    h.ws.handle(Event::Run(RunEvent::Output {
        id: *id,
        lines: vec![
            run_line("test good ... ok"),
            run_line("test bad ... FAILED"),
        ],
        partial: String::new(),
        dropped: 0,
    }));
    h.act(Action::ShowTests);
    assert_eq!(h.ws.focus(), Focus::Tests);
    let panel = h.ws.test_panel();
    assert_eq!(panel.rows[0].name, "bad", "failures first");
    h.run(Command::Newline);
    assert_eq!(h.ws.focus(), Focus::Run, "no place named: its output");
    h.act(Action::ShowTests);
    let again = h.ws.dispatch(Command::Insert("f".to_owned()));
    let narrowed = again.iter().find_map(|e| match e {
        Effect::Run(RunEffect::Start { target, .. }) => Some(target.clone()),
        _ => None,
    });
    assert_eq!(
        narrowed.expect("run again").command,
        RunCommand::Argv(
            ["cargo", "test", "--", "--exact", "bad"]
                .map(str::to_owned)
                .to_vec()
        )
    );
}

fn asked_completion(h: &Harness) -> bool {
    h.lsp.iter().any(|e| {
        matches!(
            e,
            Effect::Lsp(LspEffect::Query {
                query: LspQuery::Completion { .. },
                ..
            })
        )
    })
}

#[test]
fn typing_in_a_prompt_asks_nothing_about_the_file_behind_it() {
    let mut h = Harness::new(&[("a.rs", "fn main() {}\n")]);
    h.ready("a.rs");
    h.act(Action::RunAnything);
    h.run(Command::Insert("c".to_owned()));
    h.run(Command::Insert("a".to_owned()));
    assert!(!asked_completion(&h));
    h.run(Command::Escape);
    h.run(Command::Insert("b".to_owned()));
    assert!(asked_completion(&h), "in the file itself it still does");
}

#[test]
fn an_answer_that_comes_while_a_prompt_is_open_changes_nothing() {
    let mut h = Harness::new(&[("a.rs", "x\n")]);
    h.ready("a.rs");
    h.run(Command::Insert("b".to_owned()));
    h.act(Action::RunAnything);
    h.answer_completion(vec![item("bbb")], false);
    assert!(h.ws.completion().is_none(), "no popup over the prompt");
    h.run(Command::Insert("l".to_owned()));
    h.run(Command::Newline);
    assert_eq!(
        h.text_of("a.rs").as_deref(),
        Some("bx\n"),
        "the file is as typed"
    );
}

#[test]
fn a_macro_repeats_what_was_typed_and_stops_when_told() {
    let mut h = Harness::new(&[("a.txt", "one\ntwo\nthree\n")]);
    h.open("a.txt");
    h.ws.dispatch(Command::Focus(Focus::Editor));
    h.act(Action::MacroRecord);
    assert!(status_text(&h).contains("Recording"), "{}", status_text(&h));
    h.run(Command::Insert("> ".to_owned()));
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::LineStart));
    h.act(Action::MacroRecord);
    assert!(status_text(&h).contains("3 steps"), "{}", status_text(&h));
    h.act(Action::MacroPlay);
    assert_eq!(h.text_of("a.txt").as_deref(), Some("> one\n> two\nthree\n"));
    h.act(Action::MacroPlay);
    assert_eq!(
        h.text_of("a.txt").as_deref(),
        Some("> one\n> two\n> three\n")
    );
}

#[test]
fn playing_during_a_recording_does_not_feed_itself() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.ws.dispatch(Command::Focus(Focus::Editor));
    h.act(Action::MacroRecord);
    h.run(Command::Insert("a".to_owned()));
    h.act(Action::MacroPlay);
    h.act(Action::MacroRecord);
    h.act(Action::MacroPlay);
    assert_eq!(
        h.text_of("a.txt").as_deref(),
        Some("aax"),
        "one step, played once"
    );
}

#[test]
fn a_macro_records_neither_a_quit_nor_what_only_the_screen_reports() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.act(Action::MacroRecord);
    h.ws.dispatch(Command::Viewport { text_cols: 50 });
    h.ws.dispatch(Command::Autosave);
    h.act(Action::Save);
    h.act(Action::Quit);
    h.ws.dispatch(Command::Run(Action::MacroRecord));
    assert!(
        status_text(&h).contains("Nothing was recorded"),
        "{}",
        status_text(&h)
    );
}

#[test]
fn with_no_macro_there_is_nothing_to_play_and_it_says_so() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.act(Action::MacroPlay);
    assert!(status_text(&h).contains("No macro"), "{}", status_text(&h));
}

fn dep(name: &str, file: &str, line: usize) -> crate::core::deps::Dependency {
    crate::core::deps::Dependency {
        name: name.to_owned(),
        version: "1".to_owned(),
        section: "dependencies".to_owned(),
        file: file.to_owned(),
        line,
    }
}

#[test]
fn dependencies_are_read_on_opening_and_enter_goes_to_the_line() {
    let mut h = Harness::new(&[("Cargo.toml", "[dependencies]\nserde = \"1\"\n")]);
    let fx = h.ws.dispatch(Command::Run(Action::ShowDependencies));
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::ReadDependencies { .. }))
    );
    assert_eq!(h.ws.focus(), Focus::Dependencies);
    h.ws.handle(Event::DependenciesRead {
        dependencies: vec![dep("serde", "Cargo.toml", 1)],
        problems: Vec::new(),
    });
    h.run(Command::Newline);
    assert_eq!(h.active_name(), "Cargo.toml");
    assert_eq!(h.ws.focus(), Focus::Editor);
}

#[test]
fn asking_what_is_old_runs_the_tool_only_in_a_trusted_project() {
    let mut h = untrusted(&[]);
    h.ws.dispatch(Command::Run(Action::ShowDependencies));
    h.ws.handle(Event::DependenciesRead {
        dependencies: vec![dep("serde", "Cargo.toml", 1)],
        problems: Vec::new(),
    });
    let fx = h.ws.dispatch(Command::Insert("o".to_owned()));
    assert!(
        !fx.iter()
            .any(|e| matches!(e, Effect::Run(RunEffect::Start { .. })))
    );
    assert!(asks_trust(&h));
    h.run(Command::Insert("y".to_owned()));
    let fx = h.ws.dispatch(Command::Run(Action::ShowDependencies));
    drop(fx);
    let fx = h.ws.dispatch(Command::Insert("o".to_owned()));
    let target = fx.iter().find_map(|e| match e {
        Effect::Run(RunEffect::Start { target, .. }) => Some(target.clone()),
        _ => None,
    });
    assert_eq!(
        target.expect("runs").command,
        RunCommand::Argv(vec!["cargo".to_owned(), "outdated".to_owned()])
    );
}

#[test]
fn spelling_results_lead_to_the_word_and_a_word_can_be_added() {
    let mut h = Harness::new(&[("a.txt", "wrod here\n")]);
    h.open("a.txt");
    h.ws.dispatch(Command::Focus(Focus::Editor));
    let fx = h.ws.dispatch(Command::Run(Action::CheckSpelling));
    assert!(fx.iter().any(|e| matches!(e, Effect::SpellCheck { .. })));
    assert!(!fx.iter().any(|e| e.needs_trust()), "reads word lists only");
    let path = Path::new(ROOT).join("a.txt");
    h.ws.handle(Event::SpellChecked {
        result: Ok(vec![SearchMatch {
            path,
            line: 0,
            col: 0,
            len: 4,
            text: "wrod here".to_owned(),
        }]),
    });
    assert_eq!(h.ws.focus(), Focus::FindInFiles);
    assert_eq!(h.ws.search().results.len(), 1);
    h.ws.dispatch(Command::Focus(Focus::Editor));
    let fx = h.ws.dispatch(Command::Run(Action::AddToDictionary));
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::SpellAddWord { word } if word == "wrod"))
    );
    h.ws.handle(Event::SpellChecked {
        result: Ok(Vec::new()),
    });
    assert!(
        status_text(&h).contains("No spelling"),
        "{}",
        status_text(&h)
    );
    h.ws.handle(Event::SpellChecked {
        result: Err("no word list".to_owned()),
    });
    assert!(
        status_text(&h).contains("no word list"),
        "{}",
        status_text(&h)
    );
}

fn finished(h: &mut Harness, code: i32) {
    let target = target(RunSource::UserConfig);
    let id =
        h.ws.run_target_for_test(target)
            .iter()
            .find_map(|e| match e {
                Effect::Run(RunEffect::Start { id, .. }) => Some(*id),
                _ => None,
            })
            .expect("started");
    h.ws.dispatch(Command::Focus(Focus::Editor));
    h.act(Action::HideToolWindow);
    h.ws.handle(Event::Run(RunEvent::Finished {
        id,
        outcome: crate::core::run_output::RunOutcome::Exited(code),
        duration_ms: 1200,
    }));
}

#[test]
fn a_run_that_ends_out_of_sight_says_so_in_the_corner_and_fades() {
    let mut h = Harness::new(&[]);
    finished(&mut h, 0);
    let shown = h.ws.notifications().shown().to_vec();
    assert_eq!(shown.len(), 1);
    assert!(shown[0].text.contains("Exit code 0"), "{shown:?}");
    assert_eq!(shown[0].action, Some(Action::ShowRun));
    for _ in 0..20 {
        h.ws.dispatch(Command::NotificationTick);
    }
    assert!(h.ws.notifications().shown().is_empty(), "gone by itself");
    assert_eq!(h.ws.notifications().history().len(), 1, "still in the list");
}

#[test]
fn a_failure_stays_until_closed_and_its_action_can_be_clicked() {
    let mut h = Harness::new(&[]);
    finished(&mut h, 3);
    for _ in 0..20 {
        h.ws.dispatch(Command::NotificationTick);
    }
    let shown = h.ws.notifications().shown().to_vec();
    assert_eq!(shown.len(), 1, "an error does not fade");
    h.ws.dispatch(Command::NotificationClick { id: shown[0].id });
    assert!(h.ws.notifications().shown().is_empty());
    assert_eq!(h.ws.focus(), Focus::Run, "it did what it offered");
}

#[test]
fn the_system_is_told_only_while_the_terminal_is_not_in_front() {
    let mut h = Harness::new(&[]);
    let told = |fx: &[Effect]| fx.iter().any(|e| matches!(e, Effect::SystemNotify { .. }));
    let target = target(RunSource::UserConfig);
    let fx = h.ws.run_target_for_test(target);
    let id = fx
        .iter()
        .find_map(|e| match e {
            Effect::Run(RunEffect::Start { id, .. }) => Some(*id),
            _ => None,
        })
        .expect("started");
    let end = Event::Run(RunEvent::Finished {
        id,
        outcome: crate::core::run_output::RunOutcome::Exited(0),
        duration_ms: 1,
    });
    assert!(
        !told(&h.ws.handle(end.clone())),
        "in front: the corner is enough"
    );
    h.ws.dispatch(Command::TerminalFocus { focused: false });
    assert!(told(&h.ws.handle(end)));
}

#[test]
fn a_run_in_plain_sight_needs_no_notification() {
    let mut h = Harness::new(&[]);
    let target = target(RunSource::UserConfig);
    let id =
        h.ws.run_target_for_test(target)
            .iter()
            .find_map(|e| match e {
                Effect::Run(RunEffect::Start { id, .. }) => Some(*id),
                _ => None,
            })
            .expect("started");
    h.ws.handle(Event::Run(RunEvent::Finished {
        id,
        outcome: crate::core::run_output::RunOutcome::Exited(0),
        duration_ms: 1,
    }));
    assert!(h.ws.notifications().shown().is_empty());
}
