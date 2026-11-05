use super::*;

#[test]
fn a_chosen_action_edits_the_file_as_one_undo_step() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.offer_actions(vec![
        code_action("First", &[("a.rs", (0, 0), "1")]),
        code_action("Second", &[("a.rs", (0, 0), "2")]),
    ]);
    let kind = h.ws.picker().expect("a list").kind;
    assert_eq!(h.ws.picker_items(kind), ["First", "Second"]);
    h.run(Command::Move(Motion::Down));
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "2x");
    assert!(h.ws.picker().is_none());
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "x");
}

#[test]
fn filtering_the_list_still_runs_the_chosen_action() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.offer_actions(vec![
        code_action("First", &[("a.rs", (0, 0), "1")]),
        code_action("Second", &[("a.rs", (0, 0), "2")]),
    ]);
    h.typed("sec");
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "2x");
}

#[test]
fn escape_applies_nothing() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.offer_actions(vec![code_action("Fix", &[("a.rs", (0, 0), "1")])]);
    h.run(Command::Escape);
    assert_eq!(h.active_text(), "x");
    assert!(!h.dirty());
}

#[test]
fn no_actions_are_said() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.offer_actions(Vec::new());
    assert!(h.ws.picker().is_none());
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("No actions available here")
    );
}

#[test]
fn an_action_across_files_edits_open_ones_opens_the_rest_and_comes_back() {
    let mut h = Harness::new(&[("a.rs", "a"), ("b.rs", "b"), ("c.rs", "c")]);
    h.open("b.rs");
    h.ready("a.rs");
    h.offer_actions(vec![code_action(
        "Rename",
        &[
            ("a.rs", (0, 0), "1"),
            ("b.rs", (0, 0), "2"),
            ("c.rs", (0, 0), "3"),
        ],
    )]);
    h.run(Command::Newline);
    assert_eq!(h.text_of("a.rs").as_deref(), Some("1a"));
    assert_eq!(h.text_of("b.rs").as_deref(), Some("2b"));
    assert_eq!(
        h.text_of("c.rs").as_deref(),
        Some("3c"),
        "opened to be edited"
    );
    assert_eq!(h.active_name(), "a.rs", "the user's tab is back");
}

#[test]
fn an_edit_in_another_open_file_leaves_the_users_cursor_alone() {
    let mut h = Harness::new(&[("a.rs", "abc"), ("b.rs", "b")]);
    h.open("b.rs");
    h.ready("a.rs");
    h.run(Command::Move(Motion::Right));
    h.offer_actions(vec![code_action("Other", &[("b.rs", (0, 0), "22")])]);
    h.run(Command::Newline);
    assert_eq!(h.cursor(), 1);
}

#[test]
fn an_unreadable_file_is_reported_and_the_rest_still_applies() {
    let mut h = Harness::new(&[("a.rs", "a")]);
    h.ready("a.rs");
    h.offer_actions(vec![code_action(
        "Both",
        &[("a.rs", (0, 0), "1"), ("gone.rs", (0, 0), "2")],
    )]);
    let effects = h.ws.dispatch(Command::Newline);
    assert!(effects.iter().any(|e| matches!(e, Effect::ReadFile { .. })));
    h.lsp_event(Event::FileRead {
        path: Path::new(ROOT).join("gone.rs"),
        at: None,
        disk: None,
        result: Err("denied".into()),
    });
    assert_eq!(h.active_text(), "1a");
    assert!(h.ws.status().is_some_and(|s| s.text.contains("gone.rs")));
    assert_eq!(h.active_name(), "a.rs");
}

#[test]
fn an_outside_reload_under_the_list_refuses_the_stale_positions() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.offer_actions(vec![code_action("Fix", &[("a.rs", (0, 0), "1")])]);
    h.file_changed("different");
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "different");
    assert!(h.ws.status().is_some_and(|s| s.text.contains("changed")));
}
