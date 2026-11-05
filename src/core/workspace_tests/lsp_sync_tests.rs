use super::*;

#[test]
fn the_server_starts_with_the_first_rust_file_and_gets_it_when_ready() {
    let mut h = Harness::new(&[("notes.txt", ""), ("a.rs", "fn")]);
    h.open("notes.txt");
    assert!(h.lsp.is_empty(), "plain text needs no server");
    h.open("a.rs");
    assert_eq!(h.lsp_names(), ["start"]);
    assert_eq!(h.ws.lsp_state(), LspState::Starting);
    h.lsp_event(Event::Lsp(LspEvent::Ready {
        server: rust_server(),
        encoding: PositionEncoding::Utf8,
        completion_triggers: vec![".".to_owned()],
    }));
    assert_eq!(h.lsp_names(), ["start", "open"]);
    assert!(matches!(
        &h.lsp[1],
        Effect::Lsp(LspEffect::DidOpen { text, .. }) if text == "fn"
    ));
}

#[test]
fn edits_saves_and_closes_reach_the_server_in_order() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.lsp_event(Event::Lsp(LspEvent::Ready {
        server: rust_server(),
        encoding: PositionEncoding::Utf8,
        completion_triggers: vec![".".to_owned()],
    }));
    h.typed("1");
    h.act(Action::Save);
    h.act(Action::CloseTab);
    assert_eq!(h.lsp_names(), ["start", "open", "change", "save", "close"]);
    assert!(matches!(
        &h.lsp[2],
        Effect::Lsp(LspEffect::DidChange { text, version, .. }) if text == "1" && *version == 1
    ));
}

#[test]
fn moving_the_cursor_tells_the_server_nothing() {
    let mut h = Harness::new(&[("a.rs", "abc")]);
    h.open("a.rs");
    h.lsp_event(Event::Lsp(LspEvent::Ready {
        server: rust_server(),
        encoding: PositionEncoding::Utf8,
        completion_triggers: vec![".".to_owned()],
    }));
    let before = h.lsp.len();
    h.run(Command::Move(Motion::Right));
    assert_eq!(h.lsp.len(), before);
}

#[test]
fn a_restarted_server_gets_every_document_again() {
    let mut h = Harness::new(&[("a.rs", "a"), ("b.rs", "b")]);
    h.open("a.rs");
    h.open("b.rs");
    h.lsp_event(Event::Lsp(LspEvent::Ready {
        server: rust_server(),
        encoding: PositionEncoding::Utf8,
        completion_triggers: vec![".".to_owned()],
    }));
    h.lsp.clear();
    h.lsp_event(Event::Lsp(LspEvent::StateChanged {
        server: rust_server(),
        state: LspState::Crashed,
    }));
    h.typed("x");
    assert!(h.lsp.is_empty(), "nobody is listening");
    h.lsp_event(Event::Lsp(LspEvent::Ready {
        server: rust_server(),
        encoding: PositionEncoding::Utf8,
        completion_triggers: vec![".".to_owned()],
    }));
    assert_eq!(h.lsp_names(), ["open", "open"]);
    assert!(
        h.lsp
            .iter()
            .any(|e| matches!(e, Effect::Lsp(LspEffect::DidOpen { text, .. }) if text == "xb")),
        "the current text, including edits made while it was down"
    );
}

#[test]
fn a_crash_does_not_touch_the_buffers_or_start_a_second_supervisor() {
    let mut h = Harness::new(&[("a.rs", "a")]);
    h.open("a.rs");
    h.lsp_event(Event::Lsp(LspEvent::StateChanged {
        server: rust_server(),
        state: LspState::Stopped,
    }));
    h.typed("x");
    assert_eq!(h.active_text(), "xa");
    assert_eq!(h.lsp_names(), ["start"]);
}

#[test]
fn restart_action_goes_to_the_supervisor_only_when_there_is_one() {
    let mut h = Harness::new(&[("a.txt", "")]);
    h.open("a.txt");
    h.act(Action::RestartLanguageServer);
    assert!(h.lsp.is_empty());
    assert!(h.ws.status().is_some());
    h.files.push((Path::new(ROOT).join("a.rs"), String::new()));
    h.open("a.rs");
    h.act(Action::RestartLanguageServer);
    assert_eq!(h.lsp_names(), ["start", "restart"]);
}

#[test]
fn diagnostics_follow_edits_and_vanish_when_cleared() {
    let mut h = Harness::new(&[("a.rs", "let x = 1;")]);
    h.open("a.rs");
    h.lsp_event(Event::Lsp(LspEvent::Ready {
        server: rust_server(),
        encoding: PositionEncoding::Utf8,
        completion_triggers: vec![".".to_owned()],
    }));
    h.publish("a.rs", (0, 4), (0, 5));
    let path = Path::new(ROOT).join("a.rs");
    assert_eq!(h.ws.diagnostics(&path)[0].chars, Some(4..5));
    h.typed("ab");
    assert_eq!(
        h.ws.diagnostics(&path)[0].chars,
        Some(6..7),
        "moved with the text"
    );
    assert_eq!(h.ws.problem_counts(), (1, 0));
    h.lsp_event(Event::Lsp(LspEvent::Diagnostics {
        server: rust_server(),
        path: path.clone(),
        items: Vec::new(),
    }));
    assert!(h.ws.diagnostics(&path).is_empty());
    assert_eq!(h.ws.problem_counts(), (0, 0));
}

#[test]
fn diagnostics_of_a_closed_file_get_positions_when_it_opens() {
    let mut h = Harness::new(&[("a.rs", "a"), ("b.rs", "ы bad")]);
    h.open("a.rs");
    h.lsp_event(Event::Lsp(LspEvent::Ready {
        server: rust_server(),
        encoding: PositionEncoding::Utf8,
        completion_triggers: vec![".".to_owned()],
    }));
    h.publish("b.rs", (0, 3), (0, 6));
    let path = Path::new(ROOT).join("b.rs");
    assert_eq!(h.ws.diagnostics(&path)[0].chars, None);
    h.open("b.rs");
    assert_eq!(
        h.ws.diagnostics(&path)[0].chars,
        Some(2..5),
        "UTF-8 columns"
    );
}

#[test]
fn a_new_session_starts_without_old_diagnostics() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.open("a.rs");
    h.publish("a.rs", (0, 0), (0, 1));
    h.lsp_event(Event::Lsp(LspEvent::Ready {
        server: rust_server(),
        encoding: PositionEncoding::Utf8,
        completion_triggers: vec![".".to_owned()],
    }));
    assert_eq!(h.ws.problem_counts(), (0, 0));
}

#[test]
fn problems_panel_navigates_and_opens_the_place() {
    let mut h = Harness::new(&[("a.rs", "one\ntwo"), ("b.rs", "x\ny")]);
    h.open("a.rs");
    h.publish("a.rs", (1, 1), (1, 2));
    h.publish("b.rs", (1, 0), (1, 1));
    h.act(Action::ShowProblems);
    assert_eq!(h.ws.focus(), Focus::Problems);
    assert_eq!(h.ws.tool_tab(), ToolTab::Problems);
    assert!(h.ws.tool_window_visible());
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    assert_eq!(h.ws.problem_selected(), Some(1));
    h.run(Command::Move(Motion::Down));
    assert_eq!(h.ws.problem_selected(), Some(1), "stops at the last");
    h.run(Command::Newline);
    assert_eq!(h.active_name(), "b.rs");
    assert_eq!(h.ws.active().unwrap().0.cursor.idx, 2);
    assert_eq!(h.ws.focus(), Focus::Editor);
}

#[test]
fn selection_stays_valid_when_problems_shrink_and_empty_panel_is_inert() {
    let mut h = Harness::new(&[("a.rs", "x"), ("b.rs", "y")]);
    h.open("a.rs");
    h.publish("a.rs", (0, 0), (0, 1));
    h.publish("b.rs", (0, 0), (0, 1));
    h.act(Action::ShowProblems);
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    h.lsp_event(Event::Lsp(LspEvent::Diagnostics {
        server: rust_server(),
        path: Path::new(ROOT).join("b.rs"),
        items: Vec::new(),
    }));
    assert_eq!(h.ws.problem_selected(), Some(0));
    h.lsp_event(Event::Lsp(LspEvent::Diagnostics {
        server: rust_server(),
        path: Path::new(ROOT).join("a.rs"),
        items: Vec::new(),
    }));
    assert_eq!(h.ws.problem_selected(), None);
    h.run(Command::Newline);
    h.run(Command::Move(Motion::Down));
    assert_eq!(h.ws.problem_selected(), None);
}

#[test]
fn find_in_files_and_problems_share_the_tool_window() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.act(Action::ShowProblems);
    h.act(Action::FindInProject);
    assert_eq!(h.ws.tool_tab(), ToolTab::Find);
    assert_eq!(h.ws.focus(), Focus::FindInFiles);
    h.act(Action::ShowProblems);
    h.run(Command::Escape);
    assert_eq!(h.ws.focus(), Focus::Editor);
}
