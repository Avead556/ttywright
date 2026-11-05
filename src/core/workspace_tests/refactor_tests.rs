use super::*;

#[test]
fn refactor_this_lists_rename_safe_delete_and_what_the_server_offers() {
    let mut h = Harness::new(&[("a.rs", "fn foo() {}\n")]);
    h.ready("a.rs");
    h.offer_refactorings(vec![offer(
        "Extract into function",
        &[("a.rs", (0, 0), "1")],
    )]);
    let rows = h.ws.picker_items(PickerKind::Refactorings);
    assert!(rows.iter().any(|r| r == "Rename Symbol"), "{rows:?}");
    assert!(rows.iter().any(|r| r == "Safe Delete"), "{rows:?}");
    assert!(
        rows.iter().any(|r| r == "Extract into function"),
        "{rows:?}"
    );
    assert!(
        rows.iter()
            .any(|r| r.contains("Introduce Variable") && r.contains("not offered")),
        "{rows:?}"
    );
}

#[test]
fn a_server_variable_action_replaces_the_not_offered_row() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.offer_refactorings(vec![offer(
        "Extract into variable",
        &[("a.rs", (0, 0), "1")],
    )]);
    let rows = h.ws.picker_items(PickerKind::Refactorings);
    assert!(!rows.iter().any(|r| r.contains("not offered")), "{rows:?}");
}

#[test]
fn the_not_offered_row_says_so_and_changes_nothing() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.offer_refactorings(Vec::new());
    h.pick_row("Introduce Variable");
    assert!(
        status_text(&h).contains("does not offer"),
        "{}",
        status_text(&h)
    );
    assert_eq!(h.text_of("a.rs").as_deref(), Some("x"));
}

#[test]
fn a_chosen_refactoring_is_previewed_applied_on_y_and_undone_as_one() {
    let mut h = Harness::new(&[("a.rs", "a"), ("b.rs", "b")]);
    h.open("b.rs");
    h.ready("a.rs");
    h.offer_refactorings(vec![offer(
        "Move it",
        &[("a.rs", (0, 0), "1"), ("b.rs", (0, 0), "2")],
    )]);
    h.pick_row("Move it");
    assert_eq!(h.text_of("a.rs").as_deref(), Some("a"), "only a preview");
    assert_eq!(h.ws.search().results.len(), 2);
    h.run(Command::Insert("y".to_owned()));
    assert_eq!(h.text_of("a.rs").as_deref(), Some("1a"));
    assert_eq!(h.text_of("b.rs").as_deref(), Some("2b"));
    h.act(Action::Undo);
    assert_eq!(h.text_of("a.rs").as_deref(), Some("a"));
    assert_eq!(h.text_of("b.rs").as_deref(), Some("b"));
}

#[test]
fn a_refactoring_offered_for_older_text_is_refused() {
    let mut h = Harness::new(&[("a.rs", "a")]);
    h.ready("a.rs");
    h.offer_refactorings(vec![offer("Move it", &[("a.rs", (0, 0), "1")])]);
    h.ws.code_actions_stamp += 1;
    h.pick_row("Move it");
    assert!(h.ws.file_prompt().is_none());
    assert!(
        status_text(&h).contains("text changed"),
        "{}",
        status_text(&h)
    );
}

#[test]
fn a_context_action_across_files_is_undone_as_one() {
    let mut h = Harness::new(&[("a.rs", "a"), ("b.rs", "b")]);
    h.open("b.rs");
    h.ready("a.rs");
    h.offer_actions(vec![code_action(
        "Fix",
        &[("a.rs", (0, 0), "1"), ("b.rs", (0, 0), "2")],
    )]);
    h.run(Command::Newline);
    assert_eq!(h.text_of("b.rs").as_deref(), Some("2b"));
    h.act(Action::Undo);
    assert_eq!(h.text_of("a.rs").as_deref(), Some("a"));
    assert_eq!(h.text_of("b.rs").as_deref(), Some("b"));
}

#[test]
fn safe_delete_without_usages_removes_the_declaration_as_one_undo() {
    let mut h = Harness::new(&[("a.rs", TWO_FNS)]);
    h.ready("a.rs");
    on_gone(&mut h);
    h.act(Action::SafeDelete);
    assert_eq!(
        h.text_of("a.rs").as_deref(),
        Some(TWO_FNS),
        "usages come first"
    );
    h.answer_safe_delete(vec![usage_at("a.rs", 4, 3), usage_at("a.rs", 5, 4)]);
    assert_eq!(
        h.text_of("a.rs").as_deref(),
        Some("fn keep() {}\n\n\nfn other() {}\n")
    );
    assert!(h.ws.file_prompt().is_none());
    h.act(Action::Undo);
    assert_eq!(h.text_of("a.rs").as_deref(), Some(TWO_FNS));
}

#[test]
fn safe_delete_with_usages_shows_them_and_needs_y() {
    for (answer, deleted) in [("y", true), ("n", false)] {
        let mut h = Harness::new(&[("a.rs", TWO_FNS), ("b.rs", "gone")]);
        h.ready("a.rs");
        on_gone(&mut h);
        h.act(Action::SafeDelete);
        h.answer_safe_delete(vec![usage_at("a.rs", 4, 3), usage_at("b.rs", 0, 0)]);
        assert_eq!(h.ws.search().results.len(), 1, "the other use is listed");
        assert_eq!(h.text_of("a.rs").as_deref(), Some(TWO_FNS));
        h.run(Command::Newline);
        assert_eq!(
            h.text_of("a.rs").as_deref(),
            Some(TWO_FNS),
            "Enter is no yes"
        );
        h.run(Command::Insert(answer.to_owned()));
        assert_eq!(h.text_of("a.rs").as_deref() != Some(TWO_FNS), deleted);
        assert_eq!(h.ws.focus(), Focus::Editor);
    }
}

#[test]
fn safe_delete_refuses_what_is_not_a_declaration_and_text_that_moved_on() {
    let mut h = Harness::new(&[("a.rs", TWO_FNS)]);
    h.ready("a.rs");
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    h.act(Action::SafeDelete);
    assert!(
        status_text(&h).contains("declaration"),
        "{}",
        status_text(&h)
    );
    assert!(
        !h.lsp
            .iter()
            .any(|e| matches!(e, Effect::Lsp(LspEffect::Query { .. })))
    );

    on_gone_from_top(&mut h);
    h.act(Action::SafeDelete);
    h.run(Command::Insert("z".to_owned()));
    h.answer_safe_delete(Vec::new());
    assert!(h.text_of("a.rs").is_some_and(|t| t.contains("gone()")));
}
