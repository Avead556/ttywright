use super::*;

#[test]
fn every_edit_reaches_the_journal_in_its_own_dispatch() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    assert!(h.journal.is_empty(), "opening is not an edit");
    h.typed("a");
    assert_eq!(
        h.journal_log(),
        [(true, 1)],
        "the first one brings the text"
    );
    h.run(Command::Move(Motion::Left));
    assert_eq!(h.journal_log().len(), 1, "a motion is not an edit");
    h.typed("b");
    h.run(Command::DeleteBackward);
    h.act(Action::Undo);
    assert_eq!(
        h.journal_log(),
        [(true, 1), (false, 1), (false, 1), (false, 1)]
    );
}

#[test]
fn the_journal_starts_with_the_text_before_the_first_edit() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.typed("a");
    assert!(matches!(
        &h.journal[0],
        Effect::JournalAppend { start: Some(text), .. } if text == b"x"
    ));
}

#[test]
fn saving_ends_the_journal_and_the_next_edit_starts_afresh() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.typed("a");
    h.act(Action::Save);
    assert_eq!(h.journal_removed(), 1);
    h.typed("b");
    assert_eq!(h.journal_log(), [(true, 1), (true, 1)]);
    assert!(
        matches!(
            h.journal.last(),
            Some(Effect::JournalAppend { start: Some(text), .. }) if text == b"ax"
        ),
        "from the saved text"
    );
}

#[test]
fn the_journal_goes_ahead_of_a_write_of_the_same_edit() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    let effects = h.ws.dispatch(Command::Insert("a".into()));
    assert!(matches!(effects[0], Effect::JournalAppend { .. }));
    let closing = h.ws.dispatch(Command::Run(Action::CloseTab));
    assert!(
        closing
            .iter()
            .any(|e| matches!(e, Effect::WriteFile { .. }))
    );
    assert!(
        !closing
            .iter()
            .any(|e| matches!(e, Effect::JournalAppend { .. }))
    );
}

#[test]
fn a_failed_quit_keeps_the_journal_for_the_next_run() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.typed("a");
    h.fail_writes = true;
    h.act(Action::Quit);
    h.act(Action::Quit);
    assert_eq!(h.journal_removed(), 0);
}
