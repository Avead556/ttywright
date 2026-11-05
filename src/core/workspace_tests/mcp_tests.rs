use super::*;

#[test]
fn reads_see_unsaved_text() {
    let mut h = Harness::new(&[("a.rs", "one\ntwo\nthree\n")]);
    h.open("a.rs");
    h.typed("X");
    h.mcp(
        1,
        "read_buffer",
        serde_json::json!({ "path": "a.rs", "from_line": 1, "to_line": 2 }),
    );
    let answer = h.mcp_json(1);
    assert_eq!(answer["text"], "Xone\ntwo\n");
    assert_eq!(answer["unsaved_changes"], true);
    assert_eq!(answer["total_lines"], 3);
    h.mcp(2, "get_open_files", serde_json::json!({}));
    let answer = h.mcp_json(2);
    assert_eq!(answer["files"][0]["active"], true);
    assert_eq!(answer["files"][0]["unsaved_changes"], true);
}

#[test]
fn the_selection_is_as_the_user_sees_it() {
    let mut h = Harness::new(&[("a.rs", "one\ntwo\n")]);
    h.open("a.rs");
    h.mcp(1, "get_selection", serde_json::json!({}));
    assert_eq!(h.mcp_json(1)["text"], serde_json::Value::Null);
    h.act(Action::SelectAll);
    h.mcp(2, "get_selection", serde_json::json!({}));
    let answer = h.mcp_json(2);
    assert_eq!(answer["text"], "one\ntwo\n");
    assert_eq!(answer["start"]["line"], 1);
    assert_eq!(answer["end"]["line"], 3);
}

#[test]
fn a_file_not_open_comes_from_disk_and_a_path_outside_is_refused() {
    let mut h = Harness::new(&[]);
    h.mcp(1, "read_buffer", serde_json::json!({ "path": "b.rs" }));
    assert!(h.mcp_answer(1).is_none(), "read first");
    let call = h.git.iter().rev().find_map(|e| match e {
        Effect::McpReadFiles { call, paths, .. } => Some((call.clone(), paths.clone())),
        _ => None,
    });
    let (call, paths) = call.expect("asked to read");
    assert_eq!(paths, [Path::new(ROOT).join("b.rs")]);
    let disk = paths
        .into_iter()
        .map(|p| (p, Ok("on disk\n".to_owned())))
        .collect();
    h.lsp_event(Event::McpFilesRead { id: 1, call, disk });
    assert_eq!(h.mcp_json(1)["text"], "on disk\n");
    h.mcp(
        2,
        "read_buffer",
        serde_json::json!({ "path": "/etc/passwd" }),
    );
    assert!(h.mcp_answer(2).expect("answered").is_err());
}

#[test]
fn a_proposal_changes_nothing_and_steals_no_keys_until_the_review_is_opened() {
    let mut h = Harness::new(&[("a.rs", "one\n")]);
    h.open("a.rs");
    h.propose(1, "a.rs", "one", "uno");
    assert_eq!(h.active_text(), "one\n");
    assert!(h.mcp_answer(1).is_none(), "the call waits for the decision");
    assert_eq!(h.ws.mcp_pending(), 1);
    h.typed("y");
    assert_eq!(h.active_text(), "yone\n", "typing is typing, not a yes");
    assert!(h.ws.mcp_proposal().is_none());
}

#[test]
fn accepting_edits_the_document_as_one_undo_step_and_answers() {
    let mut h = Harness::new(&[("a.rs", "one\ntwo\n")]);
    h.open("a.rs");
    h.run(Command::Move(Motion::DocEnd));
    h.propose(1, "a.rs", "one", "uno!");
    h.act(Action::ClaudeReview);
    assert!(h.ws.mcp_proposal().is_some());
    h.run(Command::Insert("a".into()));
    assert_eq!(h.active_text(), "uno!\ntwo\n");
    assert!(h.ws.mcp_proposal().is_none(), "nothing left to review");
    assert_eq!(
        h.ws.active().map(|(v, _)| v.cursor.idx),
        Some(9),
        "the cursor stays with its text"
    );
    assert_eq!(h.mcp_json(1)["accepted"][0], format!("{ROOT}/a.rs"));
    assert!(
        h.git
            .iter()
            .all(|e| !matches!(e, Effect::McpReadFiles { .. }))
    );
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "one\ntwo\n");
}

#[test]
fn rejecting_leaves_the_text_and_says_so() {
    let mut h = Harness::new(&[("a.rs", "one\n")]);
    h.open("a.rs");
    h.propose(1, "a.rs", "one", "uno");
    h.act(Action::ClaudeReview);
    h.run(Command::Insert("r".into()));
    assert_eq!(h.active_text(), "one\n");
    assert_eq!(h.mcp_json(1)["rejected"][0], format!("{ROOT}/a.rs"));
}

#[test]
fn text_that_changed_since_the_proposal_is_not_overwritten() {
    let mut h = Harness::new(&[("a.rs", "one\n")]);
    h.open("a.rs");
    h.propose(1, "a.rs", "one", "uno");
    h.typed("Z");
    h.act(Action::ClaudeReview);
    h.run(Command::Insert("y".into()));
    assert_eq!(h.active_text(), "Zone\n");
    assert_eq!(
        h.mcp_json(1)["changed_before_it_could_be_applied"][0],
        format!("{ROOT}/a.rs")
    );
}

#[test]
fn an_edit_that_cannot_apply_is_refused_at_once_with_the_reason() {
    let mut h = Harness::new(&[("a.rs", "x x\n")]);
    h.open("a.rs");
    h.propose(1, "a.rs", "x", "y");
    assert!(
        h.mcp_answer(1)
            .expect("answered")
            .unwrap_err()
            .contains("2 times")
    );
    assert_eq!(h.ws.mcp_pending(), 0);
}

#[test]
fn escape_leaves_it_for_later_and_a_closed_file_is_applied_when_read() {
    let mut h = Harness::new(&[("a.rs", "one\n"), ("b.rs", "bee\n")]);
    h.open("a.rs");
    h.mcp(
        1,
        "propose_edit",
        serde_json::json!({ "edits": [{ "path": "b.rs", "old_text": "bee", "new_text": "wasp" }] }),
    );
    let call = h.git.iter().rev().find_map(|e| match e {
        Effect::McpReadFiles { call, paths, .. } => Some((call.clone(), paths.clone())),
        _ => None,
    });
    let (call, paths) = call.expect("asked to read");
    let disk = paths
        .into_iter()
        .map(|p| (p, Ok("bee\n".to_owned())))
        .collect();
    h.lsp_event(Event::McpFilesRead { id: 1, call, disk });
    h.act(Action::ClaudeReview);
    h.run(Command::Escape);
    assert!(h.ws.mcp_proposal().is_none());
    assert_eq!(h.ws.mcp_pending(), 1);
    h.act(Action::ClaudeReview);
    h.run(Command::Insert("a".into()));
    assert_eq!(h.mcp_json(1)["accepted"][0], format!("{ROOT}/b.rs"));
    assert!(
        h.ws.active()
            .is_some_and(|(_, d)| d.path().ends_with("a.rs")),
        "back where they were"
    );
    h.open("b.rs");
    assert_eq!(h.active_text(), "wasp\n");
}

#[test]
fn each_file_of_a_proposal_is_decided_on_its_own() {
    let mut h = Harness::new(&[("a.rs", "one\n"), ("b.rs", "bee\n")]);
    h.open("b.rs");
    h.open("a.rs");
    h.mcp(
        1,
        "propose_edit",
        serde_json::json!({ "edits": [
            { "path": "a.rs", "old_text": "one", "new_text": "1" },
            { "path": "b.rs", "old_text": "bee", "new_text": "2" },
        ] }),
    );
    h.act(Action::ClaudeReview);
    h.run(Command::Insert("r".into()));
    assert!(h.mcp_answer(1).is_none(), "one file still to decide");
    assert_eq!(h.ws.mcp_proposal().map(|(_, r)| r.file), Some(1));
    h.run(Command::Insert("a".into()));
    let answer = h.mcp_json(1);
    assert_eq!(answer["rejected"][0], format!("{ROOT}/a.rs"));
    assert_eq!(answer["accepted"][0], format!("{ROOT}/b.rs"));
    assert_eq!(h.active_text(), "one\n");
}
