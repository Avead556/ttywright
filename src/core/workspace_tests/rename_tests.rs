use super::*;

#[test]
fn rename_starts_from_the_word_under_the_cursor() {
    let mut h = Harness::new(&[("a.rs", "foo bar")]);
    h.ready("a.rs");
    h.act(Action::RenameSymbol);
    assert_eq!(h.ws.file_prompt().map(|p| p.input.text()), Some("foo"));
}

#[test]
fn rename_changes_nothing_until_the_places_are_confirmed() {
    let mut h = Harness::new(&[("a.rs", "foo"), ("b.rs", "foo")]);
    h.open("b.rs");
    h.ready("a.rs");
    h.ask_rename("baz");
    let asked = h.lsp.iter().any(|e| {
        matches!(e, Effect::Lsp(LspEffect::Query { query: LspQuery::Rename { new_name, .. }, .. }) if new_name == "baz")
    });
    assert!(asked, "the server is asked with the typed name");
    h.answer_rename(Some(vec![
        renamed_at("a.rs", (0, 0), (0, 3), "baz"),
        renamed_at("b.rs", (0, 0), (0, 3), "baz"),
    ]));
    assert_eq!(h.text_of("a.rs").as_deref(), Some("foo"), "only a preview");
    assert_eq!(h.ws.search().results.len(), 2, "every place is listed");
    h.run(Command::Insert("y".to_owned()));
    assert_eq!(h.text_of("a.rs").as_deref(), Some("baz"));
    assert_eq!(h.text_of("b.rs").as_deref(), Some("baz"));
    assert_eq!(h.active_name(), "a.rs");
    assert_eq!(h.ws.focus(), Focus::Editor);
}

#[test]
fn declining_a_rename_changes_nothing_and_returns_to_the_editor() {
    for decline in [Command::Escape, Command::Insert("n".to_owned())] {
        let mut h = Harness::new(&[("a.rs", "foo")]);
        h.ready("a.rs");
        h.ask_rename("baz");
        h.answer_rename(Some(vec![renamed_at("a.rs", (0, 0), (0, 3), "baz")]));
        h.run(decline);
        assert!(h.ws.file_prompt().is_none());
        assert_eq!(h.text_of("a.rs").as_deref(), Some("foo"));
        assert_eq!(h.ws.focus(), Focus::Editor);
    }
}

#[test]
fn a_rename_the_server_cannot_do_says_so() {
    let mut h = Harness::new(&[("a.rs", "foo")]);
    h.ready("a.rs");
    h.ask_rename("baz");
    h.answer_rename(None);
    assert!(h.ws.file_prompt().is_none());
    assert_eq!(h.text_of("a.rs").as_deref(), Some("foo"));
    assert!(
        status_text(&h).contains("Cannot rename"),
        "{}",
        status_text(&h)
    );
}

#[test]
fn an_empty_or_unchanged_name_is_refused_in_the_prompt() {
    let mut h = Harness::new(&[("a.rs", "foo")]);
    h.ready("a.rs");
    for name in ["  ", "foo"] {
        h.ask_rename(name);
        let prompt = h.ws.file_prompt().expect("still asking");
        assert!(prompt.error.is_some(), "{name:?}");
        h.run(Command::Escape);
    }
    assert!(
        !h.lsp
            .iter()
            .any(|e| matches!(e, Effect::Lsp(LspEffect::Query { .. })))
    );
}

#[test]
fn context_actions_ask_about_the_whole_selection() {
    let mut h = Harness::new(&[("a.rs", "abcd")]);
    h.ready("a.rs");
    h.run(Command::Move(Motion::Right));
    h.run(Command::Select(Motion::Right));
    h.run(Command::Select(Motion::Right));
    h.act(Action::ContextActions);
    let range = h.lsp.iter().rev().find_map(|e| match e {
        Effect::Lsp(LspEffect::Query {
            query: LspQuery::CodeActions { at, to },
            ..
        }) => Some((*at, *to)),
        _ => None,
    });
    assert_eq!(range, Some(((0, 1), (0, 3))));
}

#[test]
fn the_language_server_panel_says_what_serves_the_file() {
    let mut h = Harness::new(&[("a.rs", "x"), ("n.txt", "x")]);
    h.open("n.txt");
    h.act(Action::ShowLanguageServer);
    assert_eq!(h.ws.tool_tab(), ToolTab::Language);
    assert_eq!(h.ws.focus(), Focus::Language);
    assert!(h.ws.lsp_panel().is_none(), "plain text has no server");
    h.ready("a.rs");
    let panel = h.ws.lsp_panel().expect("a server");
    assert_eq!(panel.language, "Rust");
    assert_eq!(panel.status.state, LspState::Ready);
    assert_eq!(panel.open_files, 1);
    assert!(panel.can_choose);
}

#[test]
fn the_panel_keeps_why_a_server_failed_until_it_works_again() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.open("a.rs");
    h.lsp_event(Event::Lsp(LspEvent::Failed {
        server: rust_server(),
        reason: "exited\u{1b}[31m with code 3".to_owned(),
    }));
    let failure = h.ws.lsp_panel().and_then(|p| p.failure);
    assert_eq!(
        failure.as_deref(),
        Some("exited[31m with code 3"),
        "no escapes reach the screen"
    );
    ready(&mut h, rust_server());
    assert_eq!(h.ws.lsp_panel().and_then(|p| p.failure), None);
}

#[test]
fn panel_letters_restart_and_choose_the_server() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.act(Action::ShowLanguageServer);
    h.run(Command::Insert("r".to_owned()));
    assert!(
        h.lsp
            .iter()
            .any(|e| matches!(e, Effect::Lsp(LspEffect::Restart { .. })))
    );
    h.run(Command::Insert("s".to_owned()));
    assert!(
        h.lsp
            .iter()
            .any(|e| matches!(e, Effect::Lsp(LspEffect::ListServers)))
    );
    h.run(Command::Escape);
    assert_eq!(h.ws.focus(), Focus::Editor);
}
