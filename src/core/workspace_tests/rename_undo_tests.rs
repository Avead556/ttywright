use super::*;

#[test]
fn one_undo_takes_a_rename_back_in_every_file() {
    let mut h = Harness::renamed_in_two_files();
    h.act(Action::Undo);
    assert_eq!(h.text_of("a.rs").as_deref(), Some("foo"));
    assert_eq!(h.text_of("b.rs").as_deref(), Some("foo"));
}

#[test]
fn a_rename_can_be_undone_from_any_file_it_touched() {
    let mut h = Harness::renamed_in_two_files();
    h.open("b.rs");
    h.act(Action::Undo);
    assert_eq!(h.text_of("a.rs").as_deref(), Some("foo"));
    assert_eq!(h.text_of("b.rs").as_deref(), Some("foo"));
}

#[test]
fn a_file_edited_after_the_rename_stops_its_undo_until_that_edit_is_undone() {
    let mut h = Harness::renamed_in_two_files();
    h.open("b.rs");
    h.run(Command::Insert("!".to_owned()));
    let edited = h.text_of("b.rs");
    h.open("a.rs");
    h.act(Action::Undo);
    assert_eq!(h.text_of("a.rs").as_deref(), Some("baz"), "nothing changes");
    assert_eq!(h.text_of("b.rs"), edited, "nothing changes");
    assert!(status_text(&h).contains("b.rs"), "{}", status_text(&h));

    h.open("b.rs");
    h.act(Action::Undo);
    assert_eq!(h.text_of("a.rs").as_deref(), Some("baz"), "only the typing");
    assert_eq!(h.text_of("b.rs").as_deref(), Some("baz"));
    h.act(Action::Undo);
    assert_eq!(h.text_of("a.rs").as_deref(), Some("foo"));
    assert_eq!(h.text_of("b.rs").as_deref(), Some("foo"));
}

#[test]
fn typing_after_a_rename_is_undone_first_as_usual() {
    let mut h = Harness::renamed_in_two_files();
    h.run(Command::Insert("!".to_owned()));
    h.act(Action::Undo);
    assert!(!h.text_of("a.rs").unwrap_or_default().contains('!'));
    assert_eq!(h.text_of("b.rs").as_deref(), Some("baz"));
}

#[test]
fn a_context_action_after_a_rename_keeps_its_own_undo() {
    let mut h = Harness::renamed_in_two_files();
    h.offer_actions(vec![code_action("Fix", &[("a.rs", (0, 0), "1")])]);
    h.run(Command::Newline);
    assert_eq!(h.text_of("a.rs").as_deref(), Some("1baz"));
    h.act(Action::Undo);
    assert_eq!(h.text_of("a.rs").as_deref(), Some("baz"));
    assert_eq!(h.text_of("b.rs").as_deref(), Some("baz"));
}

#[test]
fn a_file_of_the_rename_that_was_closed_is_named_and_the_next_undo_is_for_this_file() {
    let mut h = Harness::renamed_in_two_files();
    h.open("b.rs");
    h.act(Action::CloseTab);
    h.open("a.rs");
    h.act(Action::Undo);
    assert_eq!(h.text_of("a.rs").as_deref(), Some("baz"));
    assert!(status_text(&h).contains("closed"), "{}", status_text(&h));
    h.act(Action::Undo);
    assert_eq!(h.text_of("a.rs").as_deref(), Some("foo"));
}

#[test]
fn a_file_opened_for_the_rename_comes_back_with_the_rest() {
    let mut h = Harness::new(&[("a.rs", "foo"), ("b.rs", "foo")]);
    h.ready("a.rs");
    h.ask_rename("baz");
    h.answer_rename(Some(vec![
        renamed_at("a.rs", (0, 0), (0, 3), "baz"),
        renamed_at("b.rs", (0, 0), (0, 3), "baz"),
    ]));
    h.run(Command::Insert("y".to_owned()));
    assert_eq!(
        h.text_of("b.rs").as_deref(),
        Some("baz"),
        "opened to be edited"
    );
    h.act(Action::Undo);
    assert_eq!(h.text_of("a.rs").as_deref(), Some("foo"));
    assert_eq!(h.text_of("b.rs").as_deref(), Some("foo"));
}
