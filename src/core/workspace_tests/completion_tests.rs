use super::*;

#[test]
fn completion_lists_items_in_the_servers_order_and_filters_what_is_already_typed() {
    let mut h = Harness::new(&[]);
    h.files
        .push((Path::new(ROOT).join("a.rs"), "al".to_owned()));
    h.ready("a.rs");
    h.run(Command::Move(Motion::LineEnd));
    h.act(Action::Completion);
    let mut last = item("alpaca");
    last.sort = "9".into();
    let mut first = item("alpha");
    first.sort = "1".into();
    h.answer_completion(vec![last, first, item("beta")], false);
    assert_eq!(h.popup_labels(), ["alpha", "alpaca"]);
}

#[test]
fn accepting_replaces_what_was_typed_and_is_one_undo_step() {
    for accept in [Command::Newline, Command::Indent] {
        let mut h = Harness::new(&[]);
        h.popup("let al", &["alpha", "alpaca"]);
        h.run(accept);
        assert_eq!(h.active_text(), "let alpha");
        assert_eq!(h.cursor(), 9);
        assert!(h.ws.completion().is_none());
        h.act(Action::Undo);
        assert_eq!(h.active_text(), "let al");
    }
}

#[test]
fn the_arrows_choose_and_stay_inside_the_list() {
    let mut h = Harness::new(&[]);
    h.popup("", &["a", "b"]);
    h.run(Command::Move(Motion::Up));
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "b");
}

#[test]
fn a_servers_start_position_is_used_in_its_units() {
    let mut h = Harness::new(&[]);
    h.files
        .push((Path::new(ROOT).join("a.rs"), "ы.fo".to_owned()));
    h.ready("a.rs");
    h.run(Command::Move(Motion::LineEnd));
    h.act(Action::Completion);
    let mut foo = item("foo");
    foo.insert = "foo()".into();
    foo.start = Some((0, 3));
    h.answer_completion(vec![foo], false);
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "ы.foo()");
}

#[test]
fn typing_narrows_the_list_and_backspace_widens_it() {
    let mut h = Harness::new(&[]);
    h.popup("", &["alpha", "alpaca", "beta"]);
    assert_eq!(h.popup_labels().len(), 3);
    h.typed("al");
    assert_eq!(h.popup_labels(), ["alpha", "alpaca"]);
    h.typed("ph");
    assert_eq!(h.popup_labels(), ["alpha"]);
    h.run(Command::DeleteBackward);
    h.run(Command::DeleteBackward);
    assert_eq!(h.popup_labels(), ["alpha", "alpaca"]);
    h.typed("zz");
    assert!(h.ws.completion().is_none(), "nothing matches");
    assert_eq!(h.active_text(), "alzz");
}

#[test]
fn deleting_past_the_start_closes_the_popup() {
    let mut h = Harness::new(&[]);
    h.popup("a x", &["xa", "xb"]);
    h.run(Command::DeleteBackward);
    assert!(h.ws.completion().is_some(), "still at the word start");
    h.run(Command::DeleteBackward);
    assert!(h.ws.completion().is_none());
}

#[test]
fn other_keys_close_the_popup_and_do_their_job() {
    let mut h = Harness::new(&[]);
    h.popup("xy", &["xyz"]);
    h.run(Command::Move(Motion::Left));
    assert!(h.ws.completion().is_none());
    assert_eq!(h.cursor(), 1);

    let mut h = Harness::new(&[]);
    h.popup("xy", &["xyz"]);
    h.run(Command::Escape);
    assert!(h.ws.completion().is_none());
    assert_eq!(h.active_text(), "xy");
    assert_eq!(h.active_name(), "c.rs");
}

#[test]
fn a_trigger_character_asks_after_the_server_has_the_text() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.lsp.clear();
    h.typed(".");
    assert_eq!(h.lsp_names()[0], "change");
    assert!(matches!(h.lsp[1], Effect::Lsp(LspEffect::Query { .. })));
}

#[test]
fn only_a_single_typed_trigger_asks() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.run(Command::Insert("a.b.".to_owned()));
    assert_eq!(h.query_count(), 0, "a paste is not typing");
    assert_eq!(h.active_text(), "a.b.x");
}

#[test]
fn asking_on_its_own_stays_silent_when_it_gets_nothing() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.typed(".");
    h.answer_completion(Vec::new(), false);
    assert!(h.ws.status().is_none());
    h.typed(".");
    let id = h.last_query();
    h.lsp_event(Event::Lsp(LspEvent::Answered {
        server: rust_server(),
        id,
        result: Err("busy".into()),
    }));
    assert!(h.ws.status().is_none());

    h.act(Action::Completion);
    h.answer_completion(Vec::new(), false);
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("No completions")
    );
}

#[test]
fn a_trigger_without_a_server_says_nothing() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.open("a.rs");
    h.typed(".");
    assert!(h.ws.status().is_none());
    assert_eq!(h.query_count(), 0);
}

#[test]
fn typing_while_waiting_asks_again_for_the_new_text() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.typed(".");
    let first = h.last_query();
    h.typed("f");
    let second = h.last_query();
    assert_ne!(first, second);
    assert!(h.lsp.contains(&Effect::Lsp(LspEffect::Cancel {
        server: rust_server(),
        id: first,
    })));
    h.answer_completion(vec![item("foo"), item("bar")], false);
    assert_eq!(h.popup_labels(), ["foo"], "filtered by the f already typed");
}

#[test]
fn an_incomplete_list_asks_again_as_you_type() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.typed(".");
    h.answer_completion(vec![item("foo")], true);
    let before = h.query_count();
    h.typed("f");
    assert_eq!(h.query_count(), before + 1);
    assert_eq!(
        h.popup_labels(),
        ["foo"],
        "the old list stays until the answer"
    );
    h.answer_completion(vec![item("fob")], false);
    assert_eq!(h.popup_labels(), ["fob"]);
    h.typed("o");
    assert_eq!(
        h.query_count(),
        before + 1,
        "a complete list needs no server"
    );
}

#[test]
fn an_outside_reload_closes_the_popup() {
    let mut h = Harness::new(&[]);
    h.popup("xy", &["xyz"]);
    h.file_changed("completely different");
    assert!(h.ws.completion().is_none());
}

#[test]
fn the_first_letter_of_a_word_opens_the_popup() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.ready("a.rs");
    h.lsp.clear();
    h.typed("u");
    assert_eq!(
        h.lsp_names()[0],
        "change",
        "the server sees the letter first"
    );
    assert_eq!(h.query_count(), 1);
    h.answer_completion(vec![item("union"), item("use"), item("break")], false);
    assert_eq!(h.popup_labels(), ["union", "use"]);
    h.typed("n");
    assert_eq!(h.query_count(), 1, "a complete list filters on its own");
    assert_eq!(h.popup_labels(), ["union"]);
}

#[test]
fn a_word_that_matched_nothing_keeps_asking() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.ready("a.rs");
    h.typed("x");
    h.answer_completion(Vec::new(), false);
    h.typed("y");
    assert_eq!(h.query_count(), 2);
    assert!(h.ws.status().is_none());
}

#[test]
fn numbers_do_not_open_the_popup() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.ready("a.rs");
    h.typed("12");
    h.typed("u");
    assert_eq!(h.query_count(), 0);
}

#[test]
fn escape_keeps_the_popup_closed_until_the_next_word() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.ready("a.rs");
    h.typed("u");
    h.answer_completion(vec![item("use")], false);
    h.run(Command::Escape);
    let asked = h.query_count();
    h.typed("s");
    assert_eq!(h.query_count(), asked, "same word");
    h.typed(" ");
    h.typed("v");
    assert_eq!(h.query_count(), asked + 1, "a new word");
}

#[test]
fn escape_while_waiting_stops_the_popup_from_appearing() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.ready("a.rs");
    h.typed("u");
    let id = h.last_query();
    h.run(Command::Escape);
    assert!(h.lsp.contains(&Effect::Lsp(LspEffect::Cancel {
        server: rust_server(),
        id,
    })));
    h.answer_completion(vec![item("use")], false);
    assert!(h.ws.completion().is_none());
    h.typed("s");
    assert_eq!(h.query_count(), 1);
}

#[test]
fn an_explicit_request_overrides_an_earlier_escape() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.ready("a.rs");
    h.typed("u");
    h.answer_completion(vec![item("use")], false);
    h.run(Command::Escape);
    h.act(Action::Completion);
    h.answer_completion(vec![item("use")], false);
    assert_eq!(h.popup_labels(), ["use"]);
}

#[test]
fn a_word_retyped_after_deleting_the_closed_one_opens_the_popup() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.ready("a.rs");
    h.typed("u");
    h.answer_completion(vec![item("use")], false);
    h.run(Command::Escape);
    h.run(Command::DeleteBackward);
    let asked = h.query_count();
    h.typed("v");
    assert_eq!(h.query_count(), asked + 1);
}
