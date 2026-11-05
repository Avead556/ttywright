use super::*;

#[test]
fn usages_fill_the_find_panel_sorted_and_open_on_enter() {
    let mut h = Harness::new(&[("a.rs", "helper"), ("b.rs", "\n\nыы helper")]);
    h.ready("a.rs");
    h.act(Action::FindUsages);
    h.answer_usages(vec![
        usage("b.rs", 2, 5, "ыы helper"),
        usage("a.rs", 0, 0, "ignored"),
    ]);
    assert_eq!(h.ws.focus(), Focus::FindInFiles);
    assert!(h.ws.tool_window_visible());
    let s = h.ws.search();
    assert_eq!(s.usages_of.as_deref(), Some("helper"));
    assert_eq!(s.results.len(), 2);
    assert_eq!(s.results[0].path, Path::new(ROOT).join("a.rs"));
    assert_eq!(
        s.results[0].text, "helper",
        "open documents speak for themselves"
    );
    assert_eq!(s.results[1].text, "ыы helper");
    assert_eq!(s.results[1].col, 3, "5 bytes are 3 chars");
    h.run(Command::Move(Motion::Down));
    h.run(Command::Newline);
    assert_eq!(h.active_name(), "b.rs");
    assert_eq!(h.cursor(), 2 + 3);
}

#[test]
fn no_usages_are_said_and_the_panel_stays_as_it_was() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.act(Action::FindUsages);
    h.answer_usages(Vec::new());
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("No usages found")
    );
    assert!(!h.ws.tool_window_visible());
    assert_eq!(h.ws.focus(), Focus::Editor);
}

#[test]
fn formatting_is_one_undo_step_and_keeps_the_cursor_on_its_text() {
    let mut h = Harness::new(&[("a.rs", "fn a(){}\nfn b(){}\n")]);
    h.ready("a.rs");
    h.run(Command::Move(Motion::Down));
    h.act(Action::ReformatCode);
    h.answer_edits(vec![inserted((0, 6), " "), inserted((1, 6), " ")]);
    assert_eq!(h.active_text(), "fn a() {}\nfn b() {}\n");
    assert_eq!(h.cursor(), 10, "still at the start of the second line");
    assert!(h.dirty());
    assert_eq!(h.ws.status().map(|s| s.text.as_str()), Some("Formatted"));
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "fn a(){}\nfn b(){}\n");
}

#[test]
fn formatting_with_nothing_to_do_changes_nothing() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.act(Action::ReformatCode);
    h.answer_edits(Vec::new());
    assert!(!h.dirty());
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("Already formatted")
    );
}

#[test]
fn formatting_is_dropped_when_the_text_changed_meanwhile() {
    let mut h = Harness::new(&[("a.rs", "ab")]);
    h.ready("a.rs");
    h.act(Action::ReformatCode);
    let id = h.last_query();
    h.typed("1");
    h.lsp_event(Event::Lsp(LspEvent::Answered {
        server: rust_server(),
        id,
        result: Ok(LspAnswer::Edits(vec![inserted((0, 0), "zz")])),
    }));
    assert_eq!(h.active_text(), "1ab");
}

#[test]
fn overlapping_edits_are_refused_whole() {
    let mut h = Harness::new(&[("a.rs", "abcd")]);
    h.ready("a.rs");
    h.act(Action::ReformatCode);
    let overlap = |start, end| LspTextEdit {
        start: (0, start),
        end: (0, end),
        text: "x".to_owned(),
    };
    h.answer_edits(vec![overlap(0, 2), overlap(1, 3)]);
    assert_eq!(h.active_text(), "abcd");
    assert!(
        h.ws.status()
            .is_some_and(|s| s.text.contains("overlapping"))
    );
}

#[test]
fn a_text_search_after_usages_is_a_text_search_again() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.act(Action::FindUsages);
    h.answer_usages(vec![usage("a.rs", 0, 0, "x")]);
    h.typed("q");
    let _ = h.ws.dispatch(Command::Newline);
    assert_eq!(h.ws.search().usages_of, None);
    assert_eq!(h.ws.search().searched.as_deref(), Some("q"));
}
