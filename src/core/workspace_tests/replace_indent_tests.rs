use super::*;

#[test]
fn enter_in_the_replacement_replaces_one_and_moves_to_the_next() {
    let mut h = Harness::replacing("foo foo foo", "foo", "bar");
    assert_eq!(h.cursor(), 0, "typing the query went to the first match");
    assert_eq!(
        h.active_text(),
        "foo foo foo",
        "nothing changes before Enter"
    );
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "bar foo foo");
    assert_eq!(h.cursor(), 4);
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "bar bar foo");
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "bar bar bar");
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("No more matches")
    );
}

#[test]
fn each_replace_is_its_own_undo_step() {
    let mut h = Harness::replacing("foo foo", "foo", "bar");
    h.run(Command::Newline);
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "bar bar");
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "bar foo");
}

#[test]
fn a_replacement_containing_the_query_cannot_loop_over_itself() {
    let mut h = Harness::replacing("a a", "a", "aa");
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "aa a");
    assert_eq!(
        h.cursor(),
        3,
        "past the new text, on the next original match"
    );
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "aa aa");
}

#[test]
fn enter_away_from_a_match_only_moves_there() {
    let mut h = Harness::new(&[("a.txt", "x foo")]);
    h.open("a.txt");
    h.act(Action::Replace);
    h.typed("foo");
    h.run(Command::Indent);
    h.typed("bar");
    h.run(Command::Focus(Focus::Editor));
    h.run(Command::Move(Motion::DocStart));
    h.run(Command::Focus(Focus::FindBar));
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "x foo");
    assert_eq!(h.cursor(), 2);
}

#[test]
fn replace_all_is_one_undo_step_and_says_how_many() {
    let mut h = Harness::replacing("foo\nx foo\nFoo", "foo", "ы");
    h.act(Action::ReplaceAll);
    assert_eq!(h.active_text(), "ы\nx ы\nы", "smart case, as in Find");
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("Replaced 3 occurrences")
    );
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "foo\nx foo\nFoo");
}

#[test]
fn replace_all_keeps_crlf_between_matches() {
    let mut h = Harness::replacing("foo\r\nfoo\r\n", "foo", "b");
    h.act(Action::ReplaceAll);
    assert_eq!(h.active_text(), "b\r\nb\r\n");
}

#[test]
fn an_empty_replacement_deletes() {
    let mut h = Harness::replacing("a-b-c", "-", "");
    h.act(Action::ReplaceAll);
    assert_eq!(h.active_text(), "abc");
}

#[test]
fn replace_all_without_matches_or_outside_replacing_changes_nothing() {
    let mut h = Harness::replacing("abc", "zzz", "y");
    h.act(Action::ReplaceAll);
    assert_eq!(h.active_text(), "abc");
    assert_eq!(h.ws.status().map(|s| s.text.as_str()), Some("No matches"));

    let mut h = Harness::new(&[("a.txt", "abc")]);
    h.open("a.txt");
    h.act(Action::Find);
    h.typed("a");
    h.act(Action::ReplaceAll);
    assert_eq!(h.active_text(), "abc", "plain Find has no replacement");
}

#[test]
fn tab_switches_fields_and_typing_goes_to_the_chosen_one() {
    let mut h = Harness::replacing("abc", "a", "X");
    let bar = |h: &Harness| {
        let b = h.ws.find_bar().expect("a bar");
        (
            b.input.text().to_owned(),
            b.replacement.as_ref().map(|r| r.text().to_owned()),
        )
    };
    assert_eq!(bar(&h), ("a".to_owned(), Some("X".to_owned())));
    h.run(Command::Outdent);
    h.typed("b");
    assert_eq!(bar(&h), ("ab".to_owned(), Some("X".to_owned())));
}

#[test]
fn a_one_line_selection_becomes_the_query_and_focus_goes_to_the_replacement() {
    let mut h = Harness::new(&[("a.txt", "foo bar\nfoo")]);
    h.open("a.txt");
    for _ in 0..3 {
        h.run(Command::Select(Motion::Right));
    }
    h.act(Action::Replace);
    let bar = h.ws.find_bar().expect("a bar");
    assert_eq!(bar.input.text(), "foo");
    assert!(bar.in_replacement);
    assert_eq!(bar.matches.len(), 2);

    let mut h = Harness::new(&[("a.txt", "a\nb")]);
    h.open("a.txt");
    h.act(Action::SelectAll);
    h.act(Action::Find);
    assert_eq!(
        h.ws.find_bar().map(|b| b.input.text().to_owned()),
        Some(String::new())
    );
}

#[test]
fn find_drops_back_from_replacing_and_escape_closes_all_of_it() {
    let mut h = Harness::replacing("abc", "a", "X");
    h.act(Action::Find);
    assert!(h.ws.find_bar().is_some_and(|b| b.replacement.is_none()));
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "abc", "Enter only searches now");
    h.run(Command::Escape);
    assert!(h.ws.find_bar().is_none());
}

#[test]
fn tab_without_a_selection_inserts_spaces_at_the_cursor() {
    let mut h = Harness::new(&[("a.txt", "ab")]);
    h.open("a.txt");
    h.run(Command::Move(Motion::Right));
    h.run(Command::Indent);
    assert_eq!(h.active_text(), "a    b");
    assert_eq!(h.cursor(), 5);
}

#[test]
fn tab_with_a_selection_indents_its_lines_and_keeps_it() {
    let mut h = Harness::new(&[("a.txt", "one\n\ntwo\nkeep\n")]);
    h.open("a.txt");
    h.run(Command::Move(Motion::Right));
    h.run(Command::Select(Motion::Down));
    h.run(Command::Select(Motion::Down));
    h.run(Command::Indent);
    assert_eq!(
        h.active_text(),
        "    one\n\n    two\nkeep\n",
        "empty lines stay empty"
    );
    assert_eq!(h.selected().as_deref(), Some("ne\n\n    t"));
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "one\n\ntwo\nkeep\n", "one undo step");
}

#[test]
fn a_selection_ending_at_a_line_start_leaves_that_line_alone() {
    let mut h = Harness::new(&[("a.txt", "one\ntwo\n")]);
    h.open("a.txt");
    h.run(Command::Select(Motion::Down));
    h.run(Command::Indent);
    assert_eq!(h.active_text(), "    one\ntwo\n");
    assert_eq!(
        h.selected().as_deref(),
        Some("    one\n"),
        "includes the new indent"
    );
}

#[test]
fn shift_tab_outdents_spaces_or_a_tab_with_or_without_a_selection() {
    let mut h = Harness::new(&[("a.txt", "      six\n\tt\n  two\nnone")]);
    h.open("a.txt");
    h.run(Command::Move(Motion::LineEnd));
    h.run(Command::Outdent);
    assert_eq!(h.active_text(), "  six\n\tt\n  two\nnone");
    assert_eq!(h.cursor(), 5, "the cursor stays on its text");
    h.act(Action::SelectAll);
    h.run(Command::Outdent);
    assert_eq!(h.active_text(), "six\nt\ntwo\nnone");
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "  six\n\tt\n  two\nnone");
}

#[test]
fn shift_tab_with_a_cursor_inside_the_indent_lands_at_the_line_start() {
    let mut h = Harness::new(&[("a.txt", "    x")]);
    h.open("a.txt");
    h.run(Command::Move(Motion::Right));
    h.run(Command::Outdent);
    assert_eq!(h.active_text(), "x");
    assert_eq!(h.cursor(), 0);
}

#[test]
fn shift_tab_on_a_line_with_no_indent_is_not_an_edit() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.run(Command::Outdent);
    assert!(!h.dirty());
}

#[test]
fn indenting_keeps_crlf() {
    let mut h = Harness::new(&[("a.txt", "a\r\nb\r\n")]);
    h.open("a.txt");
    h.act(Action::SelectAll);
    h.run(Command::Indent);
    assert_eq!(h.active_text(), "    a\r\n    b\r\n");
}
