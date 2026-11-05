use super::*;

#[test]
fn file_structure_lists_nested_symbols_starting_at_the_cursor_and_jumps() {
    let mut h = Harness::new(&[("a.rs", "one\ntwo\nthree\nfour\n")]);
    h.ready("a.rs");
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    h.act(Action::FileStructure);
    assert_eq!(h.last_lsp_query(), Some(LspQuery::Symbols));
    h.answer_lsp(LspAnswer::Symbols(vec![
        symbol("a", 0, 0),
        symbol("b", 1, 1),
        symbol("c", 0, 3),
    ]));
    let picker = h.ws.picker().expect("up");
    assert_eq!(picker.kind, PickerKind::Symbols);
    assert_eq!(picker.selected, 1, "the one the cursor is in");
    assert!(h.ws.picker_items(PickerKind::Symbols)[1].starts_with("  b"));
    h.run(Command::Move(Motion::Down));
    h.run(Command::Newline);
    assert_eq!(h.ws.active().map(|(v, _)| v.cursor.idx), Some(14));
    h.act(Action::NavigateBack);
    assert_eq!(
        h.ws.active().map(|(v, _)| v.cursor.idx),
        Some(8),
        "back works"
    );
}

#[test]
fn callers_take_two_steps_and_land_in_the_find_panel_under_their_own_name() {
    let mut h = Harness::new(&[("a.rs", "fn f() {}\nf();\n")]);
    h.ready("a.rs");
    h.act(Action::CallersOf);
    assert!(matches!(
        h.last_lsp_query(),
        Some(LspQuery::PrepareCalls {
            outgoing: false,
            ..
        })
    ));
    h.answer_lsp(LspAnswer::CallRoot {
        root: Some(("f".into(), "{\"raw\":1}".into())),
        outgoing: false,
    });
    assert_eq!(
        h.last_lsp_query(),
        Some(LspQuery::Calls {
            item: "{\"raw\":1}".into(),
            outgoing: false
        })
    );
    h.answer_lsp(LspAnswer::Calls(vec![usage("a.rs", 1, 0, "f();")]));
    let search = h.ws.search();
    assert_eq!(search.usages_of.as_deref(), Some("f"));
    assert_eq!(search.usages_word, "caller");
    assert_eq!(h.ws.focus(), Focus::FindInFiles);
    h.run(Command::Escape);
    h.act(Action::FindUsages);
    h.answer_usages(vec![usage("a.rs", 1, 0, "f();")]);
    assert_eq!(
        h.ws.search().usages_word,
        "",
        "the next usages are usages again"
    );
}

#[test]
fn nothing_to_follow_or_no_callers_is_said_plainly() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready("a.rs");
    h.act(Action::CalleesOf);
    h.answer_lsp(LspAnswer::CallRoot {
        root: None,
        outgoing: true,
    });
    assert!(
        h.ws.status()
            .is_some_and(|m| m.text.contains("No function"))
    );
    h.act(Action::CalleesOf);
    h.answer_lsp(LspAnswer::CallRoot {
        root: Some(("f".into(), "{}".into())),
        outgoing: true,
    });
    h.answer_lsp(LspAnswer::Calls(Vec::new()));
    assert!(
        h.ws.status()
            .is_some_and(|m| m.text == "No callees of f found")
    );
}

#[test]
fn a_bookmark_follows_its_line_through_edits_and_toggles_off() {
    let mut h = Harness::new(&[("a.rs", "one\ntwo\nthree\n")]);
    h.open("a.rs");
    h.run(Command::Move(Motion::Down));
    h.act(Action::ToggleBookmark);
    let path = Path::new(ROOT).join("a.rs");
    assert!(h.ws.is_bookmarked(&path, 1));
    h.run(Command::Move(Motion::DocStart));
    h.run(Command::Newline);
    h.run(Command::Newline);
    assert!(
        h.ws.is_bookmarked(&path, 3),
        "two lines were added above it"
    );
    h.act(Action::Undo);
    assert!(h.ws.is_bookmarked(&path, 2) || h.ws.is_bookmarked(&path, 1));
    h.run(Command::Move(Motion::DocEnd));
    h.typed("end");
    assert!(
        h.ws.bookmarks().iter().all(|b| b.line < 3),
        "edits below change nothing"
    );
    let line = h.ws.bookmarks()[0].line;
    h.run(Command::Move(Motion::DocStart));
    for _ in 0..line {
        h.run(Command::Move(Motion::Down));
    }
    h.act(Action::ToggleBookmark);
    assert!(h.ws.bookmarks().is_empty());
    assert!(h.ws.status().is_some_and(|m| m.text == "Bookmark removed"));
}
