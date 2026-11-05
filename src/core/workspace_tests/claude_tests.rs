use super::*;

#[test]
fn the_panel_starts_the_program_only_once_it_knows_its_size() {
    let mut h = Harness::new(&[]);
    h.act(Action::ClaudeOpen);
    assert_eq!(h.ws.focus(), Focus::Claude);
    assert_eq!(h.ws.tool_tab(), ToolTab::Claude);
    let starts = |h: &Harness| {
        h.git
            .iter()
            .filter(|e| matches!(e, Effect::ClaudeStart { .. }))
            .count()
    };
    assert_eq!(starts(&h), 0);
    h.run(Command::ClaudeSize {
        rows: 10,
        columns: 60,
    });
    assert_eq!(starts(&h), 1);
    h.run(Command::ClaudeSize {
        rows: 12,
        columns: 60,
    });
    assert_eq!(starts(&h), 1, "a resize is not a second program");
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::ClaudeResize { rows: 12, .. }))
    );
}

#[test]
fn f12_goes_to_the_editor_and_back_leaving_the_panel_up_and_running() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.open("a.rs");
    h.claude_up();
    h.act(Action::ClaudeOpen);
    assert_eq!(h.ws.focus(), Focus::Editor);
    assert!(h.ws.tool_window_visible(), "still watching");
    h.act(Action::ClaudeOpen);
    assert_eq!(h.ws.focus(), Focus::Claude);
    let starts = h
        .git
        .iter()
        .filter(|e| matches!(e, Effect::ClaudeStart { .. }))
        .count();
    assert_eq!(starts, 1);
}

#[test]
fn claude_code_hears_what_is_selected_only_when_it_changes() {
    let mut h = Harness::new(&[("a.rs", "one\ntwo\nthree\n")]);
    h.open("a.rs");
    assert!(
        h.notified("selection_changed").is_empty(),
        "nobody is listening"
    );
    h.lsp_event(Event::IdeConnected);
    let first = h.notified("selection_changed").len();
    assert_eq!(first, 1, "told at once where things stand");
    h.run(Command::Focus(Focus::Editor));
    assert_eq!(
        h.notified("selection_changed").len(),
        first,
        "nothing changed"
    );
    h.act(Action::SelectAll);
    let last = h.notified("selection_changed").pop().expect("told");
    assert_eq!(last["text"], "one\ntwo\nthree\n");
    assert_eq!(last["selection"]["isEmpty"], false);
    assert_eq!(last["selection"]["start"]["line"], 0);
    assert_eq!(last["selection"]["end"]["line"], 3);
    assert!(
        last["filePath"]
            .as_str()
            .is_some_and(|p| p.ends_with("a.rs"))
    );
}

#[test]
fn a_mention_waits_for_the_connection_and_counts_lines_from_zero() {
    let mut h = Harness::new(&[("a.rs", "one\ntwo\n")]);
    h.open("a.rs");
    h.act(Action::SelectAll);
    h.act(Action::ClaudeAsk);
    assert_eq!(h.ws.focus(), Focus::Claude);
    h.run(Command::ClaudeSize {
        rows: 10,
        columns: 60,
    });
    assert!(h.notified("at_mentioned").is_empty(), "not connected yet");
    h.lsp_event(Event::IdeConnected);
    let mention = h
        .notified("at_mentioned")
        .pop()
        .expect("sent on connecting");
    assert_eq!(mention["lineStart"], 0);
    assert_eq!(
        mention["lineEnd"], 1,
        "the end of the selection is on the next line"
    );
}

#[test]
fn claude_s_own_edit_is_reviewed_and_it_writes_the_file_itself() {
    let mut h = Harness::new(&[("a.rs", "one\n")]);
    h.open("a.rs");
    h.claude_up();
    h.lsp_event(Event::IdeConnected);
    open_diff(&mut h, 1, "uno\n");
    assert!(
        h.ws.mcp_proposal().is_some(),
        "opens at once for someone waiting in the panel"
    );
    assert!(h.mcp_answer(1).is_none());
    h.run(Command::Insert("a".into()));
    let answer = h.mcp_answer(1).expect("answered").expect("no error");
    assert_eq!(
        serde_json::from_str::<Vec<String>>(&answer).unwrap(),
        ["FILE_SAVED", "uno\n"]
    );
    assert_eq!(
        h.active_text(),
        "one\n",
        "the file changes on disk, and the editor follows"
    );
}

#[test]
fn an_edit_arriving_while_typing_elsewhere_steals_no_keys() {
    let mut h = Harness::new(&[("a.rs", "one\n")]);
    h.open("a.rs");
    h.lsp_event(Event::IdeConnected);
    open_diff(&mut h, 1, "uno\n");
    assert!(h.ws.mcp_proposal().is_none());
    assert_eq!(h.ws.mcp_pending(), 1);
}

#[test]
fn rejecting_or_having_unsaved_text_tells_claude_no() {
    let mut h = Harness::new(&[("a.rs", "one\n")]);
    h.open("a.rs");
    h.claude_up();
    h.lsp_event(Event::IdeConnected);
    open_diff(&mut h, 1, "uno\n");
    h.run(Command::Insert("r".into()));
    let answer = h.mcp_answer(1).expect("answered").expect("no error");
    assert!(answer.contains("DIFF_REJECTED"), "{answer}");
    h.act(Action::ClaudeOpen);
    h.typed("Z");
    open_diff(&mut h, 2, "uno\n");
    h.act(Action::ClaudeReview);
    h.run(Command::Insert("a".into()));
    let answer = h.mcp_answer(2).expect("answered").expect("no error");
    assert!(
        answer.contains("DIFF_REJECTED"),
        "unsaved text must not be overwritten: {answer}"
    );
}

#[test]
fn an_answer_given_in_its_own_terminal_closes_the_review() {
    let mut h = Harness::new(&[("a.rs", "one\n")]);
    h.open("a.rs");
    h.claude_up();
    h.lsp_event(Event::IdeConnected);
    open_diff(&mut h, 1, "uno\n");
    h.mcp(
        2,
        "close_tab",
        serde_json::json!({ "tab_name": "edit a.rs" }),
    );
    assert!(h.ws.mcp_proposal().is_none());
    assert_eq!(h.ws.mcp_pending(), 0);
    assert!(
        h.mcp_answer(1)
            .expect("answered")
            .expect("no error")
            .contains("TAB_CLOSED")
    );
}

#[test]
fn claude_exiting_ends_its_diffs_and_enter_starts_it_again() {
    let mut h = Harness::new(&[("a.rs", "one\n")]);
    h.open("a.rs");
    h.claude_up();
    h.lsp_event(Event::IdeConnected);
    open_diff(&mut h, 1, "uno\n");
    h.lsp_event(Event::ClaudeEnded {
        state: crate::core::ClaudeState::Exited(Some(0)),
    });
    assert_eq!(h.ws.mcp_pending(), 0);
    assert!(!h.ws.claude().is_some_and(|p| p.connected));
    h.run(Command::ClaudeInput(b"x".to_vec()));
    assert!(h.written_to_claude().is_empty(), "nothing there to type at");
    h.run(Command::ClaudeInput(b"\r".to_vec()));
    let starts = h
        .git
        .iter()
        .filter(|e| matches!(e, Effect::ClaudeStart { .. }))
        .count();
    assert_eq!(starts, 2);
}

#[test]
fn fix_types_the_problem_and_leaves_enter_to_the_user() {
    let mut h = Harness::new(&[("a.rs", "one\n")]);
    h.open("a.rs");
    h.act(Action::ClaudeFix);
    assert!(h.ws.claude().is_none(), "no problems, nothing to start");
}
