use super::*;

#[test]
fn hover_opens_scrolls_and_esc_closes_only_it() {
    let mut h = Harness::new(&[("a.rs", "one\ntwo")]);
    let id = h.ask_hover("a.rs");
    h.answer_hover(id, Some("a\nb\nc"));
    assert_eq!(h.hover_texts(), ["a", "b", "c"]);
    let before = h.cursor();
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    assert_eq!(
        h.ws.hover().map(|p| p.scroll),
        Some(2),
        "stops at the last line"
    );
    assert_eq!(
        h.cursor(),
        before,
        "the arrows scroll the popup, not the text"
    );
    h.run(Command::Move(Motion::PageUp { rows: 40 }));
    assert_eq!(h.ws.hover().map(|p| p.scroll), Some(0));
    h.run(Command::Escape);
    assert!(h.ws.hover().is_none());
    assert_eq!(h.active_name(), "a.rs");
}

#[test]
fn typing_closes_hover_and_still_types() {
    let mut h = Harness::new(&[("a.rs", "")]);
    let id = h.ask_hover("a.rs");
    h.answer_hover(id, Some("docs"));
    h.typed("z");
    assert!(h.ws.hover().is_none());
    assert_eq!(h.active_text(), "z");
}

#[test]
fn background_work_leaves_hover_open() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    let id = h.ask_hover("a.rs");
    h.answer_hover(id, Some("docs"));
    for command in [
        Command::Autosave,
        Command::CheckDisk,
        Command::WriteSwaps,
        Command::SaveSession,
    ] {
        let _ = h.ws.dispatch(command);
    }
    assert!(h.ws.hover().is_some());
}

#[test]
fn hover_with_nothing_to_show_says_so() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    let id = h.ask_hover("a.rs");
    h.answer_hover(id, None);
    assert!(h.ws.hover().is_none());
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("No documentation found")
    );
}

#[test]
fn hover_puts_problems_under_the_cursor_above_the_docs() {
    let mut h = Harness::new(&[("a.rs", "bad good")]);
    h.open("a.rs");
    h.lsp_event(Event::Lsp(LspEvent::Ready {
        server: rust_server(),
        encoding: PositionEncoding::Utf8,
        completion_triggers: vec![".".to_owned()],
    }));
    h.publish("a.rs", (0, 0), (0, 3));
    h.act(Action::QuickDocumentation);
    let id = h.last_query();
    h.answer_hover(id, Some("docs"));
    assert_eq!(h.hover_texts(), ["bad", "", "docs"]);

    h.run(Command::Escape);
    h.run(Command::Move(Motion::LineEnd));
    h.act(Action::QuickDocumentation);
    let id = h.last_query();
    h.answer_hover(id, Some("docs"));
    assert_eq!(h.hover_texts(), ["docs"], "the problem is elsewhere");

    h.run(Command::Escape);
    h.run(Command::Move(Motion::LineStart));
    h.act(Action::QuickDocumentation);
    let id = h.last_query();
    h.answer_hover(id, None);
    assert_eq!(
        h.hover_texts(),
        ["bad"],
        "a server with no docs still shows it"
    );
}

#[test]
fn an_answer_is_dropped_when_a_list_or_another_panel_took_over() {
    let mut h = Harness::new(&[("a.rs", "x"), ("b.rs", "b")]);
    let id = h.ask_definition("a.rs");
    h.act(Action::OpenPalette);
    h.answer(id, vec![location("b.rs", 0, 0)]);
    assert_eq!(h.active_name(), "a.rs");
    assert!(h.ws.picker().is_some());
}

#[test]
fn asking_outside_the_editor_does_nothing() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    let _ = h.ask_hover("a.rs");
    let sent = h.lsp.len();
    h.run(Command::Focus(Focus::Project));
    h.act(Action::QuickDocumentation);
    assert_eq!(h.lsp.len(), sent);
}
