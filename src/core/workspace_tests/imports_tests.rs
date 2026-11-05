use super::*;

#[test]
fn accepting_an_item_with_an_import_adds_both_as_one_undo_step() {
    let mut h = Harness::new(&[("a.rs", "fn f() {\n    Hash\n}\n")]);
    h.ready("a.rs");
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::LineEnd));
    h.act(Action::Completion);
    h.answer_completion(vec![importing("HashMap")], false);
    h.run(Command::Newline);
    assert!(matches!(
        h.last_lsp_query(),
        Some(LspQuery::ResolveCompletion { .. })
    ));
    assert_eq!(
        h.active_text(),
        "fn f() {\n    Hash\n}\n",
        "the text waits for the answer"
    );
    h.answer_lsp(LspAnswer::Edits(vec![import_edit()]));
    assert_eq!(
        h.active_text(),
        "use std::collections::HashMap;\nfn f() {\n    HashMap\n}\n"
    );
    let cursor = h.ws.active().map(|(v, d)| {
        let line = d.text().char_to_line(v.cursor.idx);
        (line, v.cursor.idx - d.text().line_to_char(line))
    });
    assert_eq!(cursor, Some((2, 11)), "right after the word");
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "fn f() {\n    Hash\n}\n");
}

#[test]
fn a_key_pressed_before_the_answer_loses_only_the_import() {
    let mut h = Harness::new(&[("a.rs", "Hash")]);
    h.ready("a.rs");
    h.run(Command::Move(Motion::DocEnd));
    h.act(Action::Completion);
    h.answer_completion(vec![importing("HashMap")], false);
    h.run(Command::Newline);
    h.typed("!");
    assert_eq!(h.active_text(), "HashMap!");
    h.answer_lsp(LspAnswer::Edits(vec![import_edit()]));
    assert_eq!(h.active_text(), "HashMap!", "too late to touch anything");
}

#[test]
fn a_server_that_will_not_say_more_still_gets_the_word_in() {
    let mut h = Harness::new(&[("a.rs", "Hash")]);
    h.ready("a.rs");
    h.run(Command::Move(Motion::DocEnd));
    h.act(Action::Completion);
    h.answer_completion(vec![importing("HashMap")], false);
    h.run(Command::Newline);
    let id = h.last_query();
    h.lsp_event(Event::Lsp(LspEvent::Answered {
        server: rust_server(),
        id,
        result: Err("method not found".into()),
    }));
    assert_eq!(h.active_text(), "HashMap");
    assert!(
        h.ws.status()
            .is_none_or(|m| !m.text.contains("method not found"))
    );
}

#[test]
fn an_import_that_came_with_the_item_needs_no_asking() {
    let mut h = Harness::new(&[("a.rs", "Hash")]);
    h.ready("a.rs");
    h.run(Command::Move(Motion::DocEnd));
    h.act(Action::Completion);
    let mut offered = item("HashMap");
    offered.additional = vec![import_edit()];
    h.answer_completion(vec![offered], false);
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "use std::collections::HashMap;\nHashMap");
}

#[test]
fn an_edit_on_the_word_itself_is_left_out() {
    let mut h = Harness::new(&[("a.rs", "Hash")]);
    h.ready("a.rs");
    h.run(Command::Move(Motion::DocEnd));
    h.act(Action::Completion);
    let mut offered = item("HashMap");
    offered.additional = vec![LspTextEdit {
        start: (0, 1),
        end: (0, 2),
        text: "Z".into(),
    }];
    h.answer_completion(vec![offered], false);
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "HashMap");
}
