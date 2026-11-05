use super::*;

#[test]
fn shift_motions_grow_and_shrink_the_selection() {
    let mut h = Harness::new(&[("a.txt", "hello\nworld")]);
    h.open("a.txt");
    h.run(Command::Select(Motion::Right));
    h.run(Command::Select(Motion::Right));
    assert_eq!(h.selected().as_deref(), Some("he"));
    h.run(Command::Select(Motion::Down));
    assert_eq!(h.selected().as_deref(), Some("hello\nwo"));
    h.run(Command::Select(Motion::Up));
    h.run(Command::Select(Motion::Left));
    h.run(Command::Select(Motion::Left));
    assert_eq!(h.selected(), None, "back at the anchor nothing is selected");
    h.run(Command::Select(Motion::LineEnd));
    assert_eq!(h.selected().as_deref(), Some("hello"));
}

#[test]
fn selecting_backwards_works_and_the_anchor_stays() {
    let mut h = Harness::new(&[("a.txt", "abcd")]);
    h.open("a.txt");
    h.run(Command::Move(Motion::LineEnd));
    h.run(Command::Select(Motion::Left));
    h.run(Command::Select(Motion::Left));
    assert_eq!(h.selected().as_deref(), Some("cd"));
    h.run(Command::Select(Motion::Right));
    assert_eq!(h.selected().as_deref(), Some("d"));
}

#[test]
fn a_plain_motion_ends_the_selection_at_its_edge() {
    let mut h = Harness::new(&[("a.txt", "abcdef")]);
    h.open("a.txt");
    h.run(Command::Move(Motion::Right));
    for _ in 0..3 {
        h.run(Command::Select(Motion::Right));
    }
    h.run(Command::Move(Motion::Left));
    assert_eq!((h.selected(), h.cursor()), (None, 1));
    for _ in 0..3 {
        h.run(Command::Select(Motion::Left));
    }
    h.run(Command::Select(Motion::Right));
    h.run(Command::Move(Motion::Right));
    assert_eq!(h.selected(), None);
}

#[test]
fn typing_deleting_and_enter_replace_the_selection_as_one_undo_step() {
    for (input, expected) in [
        (Command::Insert("Z".into()), "aZd"),
        (Command::Insert("ы\nы".into()), "aы\nыd"),
        (Command::DeleteBackward, "ad"),
        (Command::DeleteForward, "ad"),
        (Command::Newline, "a\nd"),
    ] {
        let mut h = Harness::new(&[("a.txt", "abcd")]);
        h.open("a.txt");
        h.run(Command::Move(Motion::Right));
        h.run(Command::Select(Motion::Right));
        h.run(Command::Select(Motion::Right));
        h.run(input);
        assert_eq!(h.active_text(), expected);
        assert_eq!(h.selected(), None);
        h.act(Action::Undo);
        assert_eq!(h.active_text(), "abcd");
    }
}

#[test]
fn escape_clears_the_selection_before_anything_else() {
    let mut h = Harness::new(&[("a.txt", "abc")]);
    h.open("a.txt");
    h.act(Action::FindInProject);
    h.run(Command::Focus(Focus::Editor));
    h.act(Action::SelectAll);
    assert_eq!(h.selected().as_deref(), Some("abc"));
    h.run(Command::Escape);
    assert_eq!(h.selected(), None);
    assert!(
        h.ws.tool_window_visible(),
        "the panel waits for the next Esc"
    );
    h.run(Command::Escape);
    assert!(!h.ws.tool_window_visible());
}

#[test]
fn a_drag_selects_by_display_column_from_where_it_started() {
    let mut h = Harness::new(&[("a.txt", "\tab\ncd")]);
    h.open("a.txt");
    h.run(Command::SelectTo { line: 0, col: 5 });
    assert_eq!(h.selected().as_deref(), Some("\ta"));
    h.run(Command::SelectTo { line: 99, col: 1 });
    assert_eq!(h.selected().as_deref(), Some("\tab\nc"));
}

#[test]
fn a_click_ends_the_selection() {
    let mut h = Harness::new(&[("a.txt", "abc")]);
    h.open("a.txt");
    h.act(Action::SelectAll);
    let view = h.ws.active_view().expect("a view");
    h.run(Command::FocusView {
        view,
        at: Some((0, 1)),
    });
    assert_eq!((h.selected(), h.cursor()), (None, 1));
}

#[test]
fn a_double_click_selects_the_word() {
    let mut h = Harness::new(&[("a.txt", "foo bar_1 ы!")]);
    h.open("a.txt");
    let view = h.ws.active_view().expect("a view");
    h.run(Command::FocusView {
        view,
        at: Some((0, 5)),
    });
    h.run(Command::SelectWord);
    assert_eq!(h.selected().as_deref(), Some("bar_1"));
    h.run(Command::FocusView {
        view,
        at: Some((0, 3)),
    });
    h.run(Command::SelectWord);
    assert_eq!(h.selected().as_deref(), Some("foo"), "at a word's end");
}

#[test]
fn copy_takes_the_selection_or_the_whole_line() {
    let mut h = Harness::new(&[("a.txt", "one\ntwo")]);
    h.open("a.txt");
    h.run(Command::Select(Motion::Right));
    h.act(Action::Copy);
    assert_eq!(h.copied(), Some("o"));
    assert_eq!(h.active_text(), "one\ntwo");
    h.run(Command::Move(Motion::Down));
    h.act(Action::Copy);
    assert_eq!(h.copied(), Some("two"), "the last line has no line ending");
    h.run(Command::Move(Motion::DocStart));
    h.act(Action::Copy);
    assert_eq!(h.copied(), Some("one\n"));
}

#[test]
fn cut_removes_the_selection_or_the_whole_line_as_one_undo_step() {
    let mut h = Harness::new(&[("a.txt", "one\ntwo\n")]);
    h.open("a.txt");
    h.act(Action::Cut);
    assert_eq!(h.copied(), Some("one\n"));
    assert_eq!(h.active_text(), "two\n");
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "one\ntwo\n");
    h.act(Action::SelectAll);
    h.act(Action::Cut);
    assert_eq!(h.active_text(), "");
}

#[test]
fn paste_inserts_over_the_selection_and_is_one_undo_step() {
    let mut h = Harness::new(&[("a.txt", "abc")]);
    h.open("a.txt");
    h.run(Command::Select(Motion::Right));
    h.act(Action::Paste);
    assert!(h.lsp.contains(&Effect::ReadClipboard));
    h.pasted(Some("XY\nZ"));
    assert_eq!(h.active_text(), "XY\nZbc");
    assert_eq!(h.cursor(), 4);
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "abc");
}

#[test]
fn paste_falls_back_on_the_last_copy_when_there_is_no_system_clipboard() {
    let mut h = Harness::new(&[("a.txt", "abc")]);
    h.open("a.txt");
    h.act(Action::Paste);
    h.pasted(None);
    assert_eq!(h.active_text(), "abc", "nothing was copied yet");
    h.run(Command::Select(Motion::Right));
    h.act(Action::Copy);
    h.run(Command::Move(Motion::DocEnd));
    h.act(Action::Paste);
    h.pasted(None);
    assert_eq!(h.active_text(), "abca");
}

#[test]
fn paste_goes_to_whatever_has_focus() {
    let mut h = Harness::new(&[("a.txt", "abc")]);
    h.open("a.txt");
    h.act(Action::Find);
    h.act(Action::Paste);
    h.pasted(Some("b"));
    assert_eq!(
        h.ws.find_bar().map(|b| b.input.text().to_owned()),
        Some("b".to_owned())
    );
    assert_eq!(h.active_text(), "abc");
}

#[test]
fn copy_outside_the_editor_does_nothing() {
    let mut h = Harness::new(&[("a.txt", "abc")]);
    h.open("a.txt");
    h.act(Action::Find);
    h.act(Action::Copy);
    h.act(Action::Cut);
    assert_eq!(h.copied(), None);
    assert_eq!(h.active_text(), "abc");
}

#[test]
fn selection_commands_outside_the_editor_change_nothing() {
    let mut h = Harness::new(&[("a.txt", "abc")]);
    h.open("a.txt");
    h.act(Action::Find);
    h.run(Command::Select(Motion::Right));
    h.run(Command::SelectTo { line: 0, col: 2 });
    h.run(Command::SelectWord);
    h.act(Action::SelectAll);
    h.run(Command::Focus(Focus::Editor));
    assert_eq!(h.selected(), None);
}

#[test]
fn a_late_clipboard_answer_never_lands_where_the_user_has_moved_on_to() {
    let leave: [fn(&mut Harness); 4] = [
        |h| h.run(Command::Move(Motion::Right)),
        |h| h.typed("x"),
        |h| h.act(Action::SelectAll),
        |h| h.act(Action::Find),
    ];
    for moved_on in leave {
        let mut h = Harness::new(&[("a.txt", "abc")]);
        h.open("a.txt");
        h.act(Action::Paste);
        moved_on(&mut h);
        let before = h.active_text();
        h.pasted(Some("LATE"));
        assert_eq!(h.active_text(), before);
        assert!(
            h.ws.status()
                .is_some_and(|s| s.text.contains("paste again"))
        );
    }
}

#[test]
fn a_clipboard_answer_nobody_asked_for_is_ignored() {
    let mut h = Harness::new(&[("a.txt", "abc")]);
    h.open("a.txt");
    h.pasted(Some("X"));
    assert_eq!(h.active_text(), "abc");
}
