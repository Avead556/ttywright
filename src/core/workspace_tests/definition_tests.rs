use super::*;

#[test]
fn the_query_carries_the_cursor_in_server_units() {
    let mut h = Harness::new(&[("a.rs", "ыы x")]);
    h.open("a.rs");
    h.lsp_event(Event::Lsp(LspEvent::Ready {
        server: rust_server(),
        encoding: PositionEncoding::Utf8,
        completion_triggers: vec![".".to_owned()],
    }));
    h.run(Command::Move(Motion::LineEnd));
    h.act(Action::GotoDefinition);
    assert!(h.lsp.iter().any(|e| matches!(
        e,
        Effect::Lsp(LspEffect::Query {
            query: LspQuery::Definition { at: (0, 6) },
            ..
        })
    )));
}

#[test]
fn a_single_definition_jumps_there_counting_columns_in_server_units() {
    let mut h = Harness::new(&[("a.rs", "x"), ("b.rs", "\nыы target")]);
    let id = h.ask_definition("a.rs");
    h.answer(id, vec![location("b.rs", 1, 5)]);
    assert_eq!(h.active_name(), "b.rs");
    assert_eq!(h.cursor(), 1 + 3, "ыы and a space are 5 bytes and 3 chars");
    assert_eq!(h.ws.focus(), Focus::Editor);
}

#[test]
fn a_definition_in_an_open_file_moves_within_it() {
    let mut h = Harness::new(&[("a.rs", "fn a\nfn b")]);
    let id = h.ask_definition("a.rs");
    h.answer(id, vec![location("a.rs", 1, 3)]);
    assert_eq!(h.tab_names().len(), 1);
    assert_eq!(h.cursor(), 8);
}

#[test]
fn several_definitions_become_a_list() {
    let mut h = Harness::new(&[("a.rs", "x"), ("b.rs", "1\n2\n3")]);
    let id = h.ask_definition("a.rs");
    h.answer(id, vec![location("b.rs", 0, 0), location("b.rs", 2, 0)]);
    assert_eq!(h.active_name(), "a.rs", "nothing happens until a choice");
    let picker = h.ws.picker().expect("a list").kind;
    assert_eq!(h.ws.picker_items(picker), ["b.rs:1", "b.rs:3"]);
    h.run(Command::Move(Motion::Down));
    h.run(Command::Newline);
    assert_eq!(h.active_name(), "b.rs");
    assert_eq!(h.cursor(), 4);
    assert!(h.ws.picker().is_none());
}

#[test]
fn no_definition_is_said_and_changes_nothing() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    let id = h.ask_definition("a.rs");
    h.answer(id, Vec::new());
    assert_eq!(h.active_name(), "a.rs");
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("No definition found")
    );
}

#[test]
fn a_server_error_is_shown_to_the_user() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    let id = h.ask_definition("a.rs");
    h.lsp_event(Event::Lsp(LspEvent::Answered {
        server: rust_server(),
        id,
        result: Err("still indexing".into()),
    }));
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("Language server: still indexing")
    );
}

#[test]
fn an_answer_is_dropped_when_the_user_moved_on() {
    for leave in [Command::Insert("y".into()), Command::Move(Motion::Right)] {
        let mut h = Harness::new(&[("a.rs", "xx"), ("b.rs", "b")]);
        let id = h.ask_definition("a.rs");
        h.run(leave);
        h.answer(id, vec![location("b.rs", 0, 0)]);
        assert_eq!(h.active_name(), "a.rs");
    }
    let mut h = Harness::new(&[("a.rs", "x"), ("b.rs", "b"), ("c.rs", "c")]);
    let id = h.ask_definition("a.rs");
    h.open("c.rs");
    h.answer(id, vec![location("b.rs", 0, 0)]);
    assert_eq!(h.active_name(), "c.rs");
}

#[test]
fn a_new_query_cancels_the_old_one() {
    let mut h = Harness::new(&[("a.rs", "x"), ("b.rs", "b")]);
    let first = h.ask_definition("a.rs");
    h.act(Action::GotoDefinition);
    let second = h.last_query();
    assert_ne!(first, second);
    assert!(h.lsp.contains(&Effect::Lsp(LspEffect::Cancel {
        server: rust_server(),
        id: first,
    })));
    h.answer(first, vec![location("b.rs", 0, 0)]);
    assert_eq!(h.active_name(), "a.rs", "the stale answer is ignored");
    h.answer(second, vec![location("b.rs", 0, 0)]);
    assert_eq!(h.active_name(), "b.rs");
}

#[test]
fn a_server_restart_forgets_the_query() {
    let mut h = Harness::new(&[("a.rs", "x"), ("b.rs", "b")]);
    let id = h.ask_definition("a.rs");
    h.lsp_event(Event::Lsp(LspEvent::StateChanged {
        server: rust_server(),
        state: LspState::Crashed,
    }));
    h.answer(id, vec![location("b.rs", 0, 0)]);
    assert_eq!(h.active_name(), "a.rs");
}

#[test]
fn asking_without_a_server_says_so_and_sends_nothing() {
    let mut h = Harness::new(&[("a.txt", "x"), ("b.rs", "x")]);
    h.open("a.txt");
    h.act(Action::GotoDefinition);
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("No language server for this file")
    );
    h.open("b.rs");
    h.act(Action::GotoDefinition);
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("Language server is not ready")
    );
    assert!(
        !h.lsp
            .iter()
            .any(|e| matches!(e, Effect::Lsp(LspEffect::Query { .. })))
    );
}

#[test]
fn back_returns_to_where_the_jump_started() {
    let mut h = Harness::new(&[("a.rs", "one\ntwo"), ("b.rs", "b")]);
    h.open("a.rs");
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Right));
    h.lsp_event(Event::Lsp(LspEvent::Ready {
        server: rust_server(),
        encoding: PositionEncoding::Utf8,
        completion_triggers: vec![".".to_owned()],
    }));
    h.act(Action::GotoDefinition);
    let id = h.last_query();
    h.answer(id, vec![location("b.rs", 0, 0)]);
    assert_eq!(h.active_name(), "b.rs");
    h.act(Action::NavigateBack);
    assert_eq!(h.active_name(), "a.rs");
    assert_eq!(h.cursor(), 5);
    h.act(Action::NavigateBack);
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("Nowhere to go back to")
    );
}

#[test]
fn a_jump_to_an_unreadable_file_is_forgotten() {
    let mut h = Harness::new(&[("a.rs", "x"), ("b.rs", "1\n2")]);
    let id = h.ask_definition("a.rs");
    let answered = h.ws.handle(Event::Lsp(LspEvent::Answered {
        server: rust_server(),
        id,
        result: Ok(LspAnswer::Locations(vec![location("b.rs", 1, 0)])),
    }));
    assert!(matches!(answered[0], Effect::ReadFile { .. }));
    h.lsp_event(Event::FileRead {
        path: Path::new(ROOT).join("b.rs"),
        at: None,
        disk: None,
        result: Err("denied".into()),
    });
    h.open("b.rs");
    assert_eq!(h.cursor(), 0);
}
