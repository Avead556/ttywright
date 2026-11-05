use super::*;

#[test]
fn with_no_server_the_tool_formats_as_one_undo_step_keeping_the_cursor() {
    let mut h = Harness::new(&[("a.rs", "fn  a() {}\nlast\n")]);
    h.open("a.rs");
    h.run(Command::Move(Motion::DocEnd));
    h.act(Action::ReformatCode);
    let (doc, version, text) = format_request(&h).expect("the tool asked");
    assert_eq!(text, "fn  a() {}\nlast\n");
    h.lsp_event(Event::Formatted {
        doc,
        version,
        result: Ok("fn a() {}\nlast\n".into()),
    });
    assert_eq!(h.active_text(), "fn a() {}\nlast\n");
    assert_eq!(h.ws.active().map(|(v, _)| v.cursor.idx), Some(15));
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "fn  a() {}\nlast\n");
}

#[test]
fn a_tool_s_answer_about_older_text_changes_nothing_and_errors_are_shown() {
    let mut h = Harness::new(&[("a.rs", "x\n")]);
    h.open("a.rs");
    h.act(Action::ReformatCode);
    let (doc, version, _) = format_request(&h).expect("asked");
    h.typed("y");
    h.lsp_event(Event::Formatted {
        doc,
        version,
        result: Ok("z\n".into()),
    });
    assert_eq!(h.active_text(), "yx\n");
    h.lsp_event(Event::Formatted {
        doc,
        version,
        result: Err("no formatter".into()),
    });
    assert!(h.ws.status().is_some_and(|m| m.text == "no formatter"));
}
