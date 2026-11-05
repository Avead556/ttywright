use super::*;

#[test]
fn nothing_changes_until_the_preview_is_confirmed_and_enter_does_not_confirm() {
    let mut h = Harness::new(&[("a.txt", "foo foo\nx\n")]);
    h.open("a.txt");
    h.searched("foo", &["a.txt"]);
    let fx = h.ask_replace("bar");
    assert!(fx.is_empty(), "open files need no I/O: {fx:?}");
    assert_eq!(h.preview().occurrences(), 2);
    assert_eq!(h.preview().files[0].0.changes[0].new, "bar bar");
    h.run(Command::Newline);
    h.run(Command::Insert("x".to_owned()));
    assert_eq!(h.doc_text("a.txt"), "foo foo\nx\n");
    assert_eq!(h.preview().occurrences(), 2, "still waiting");
    h.run(Command::Insert("y".to_owned()));
    assert_eq!(h.doc_text("a.txt"), "bar bar\nx\n");
    assert_eq!(h.report().changed.len(), 1);
    assert_eq!(
        h.ws.status().unwrap().text,
        "Replaced in 1 file, 0 skipped, 0 failed"
    );
    assert!(
        h.ws.search().results.is_empty(),
        "they described the old text"
    );
}

#[test]
fn esc_at_the_preview_cancels_and_keeps_the_panel() {
    let mut h = Harness::new(&[("a.txt", "foo")]);
    h.open("a.txt");
    h.searched("foo", &["a.txt"]);
    assert!(h.ask_replace("bar").is_empty());
    h.run(Command::Escape);
    assert_eq!(h.doc_text("a.txt"), "foo");
    assert_eq!(h.ws.search().replace, None);
    assert_eq!(h.ws.focus(), Focus::FindInFiles);
    assert_eq!(h.ws.search().results.len(), 1, "results still there");
    h.run(Command::Insert("y".to_owned()));
    assert_eq!(h.doc_text("a.txt"), "foo", "y is only typed now");
}

#[test]
fn each_open_document_is_one_undo_step_of_its_own() {
    let mut h = Harness::new(&[("a.txt", "foo\nfoo\nfoo\n"), ("b.txt", "foo")]);
    h.open("a.txt");
    h.open("b.txt");
    h.searched("foo", &["a.txt", "b.txt"]);
    assert!(h.ask_replace("bar").is_empty());
    h.run(Command::Insert("y".to_owned()));
    assert_eq!(h.doc_text("a.txt"), "bar\nbar\nbar\n");
    assert_eq!(h.doc_text("b.txt"), "bar");
    assert_eq!(h.report().changed.len(), 2);
    h.run(Command::Escape);
    h.run(Command::Escape);
    h.run(Command::Escape);
    h.act(Action::Undo);
    assert_eq!(h.doc_text("b.txt"), "foo", "the active one goes back");
    assert_eq!(h.doc_text("a.txt"), "bar\nbar\nbar\n", "the other stays");
    h.act(Action::PreviousTab);
    h.act(Action::Undo);
    assert_eq!(h.doc_text("a.txt"), "foo\nfoo\nfoo\n", "all at once");
}

#[test]
fn an_open_document_edited_after_the_preview_is_skipped_and_reported() {
    let mut h = Harness::new(&[("a.txt", "foo"), ("b.txt", "foo")]);
    h.open("a.txt");
    h.open("b.txt");
    h.searched("foo", &["a.txt", "b.txt"]);
    assert!(h.ask_replace("bar").is_empty());
    h.run(Command::Focus(Focus::Editor));
    h.typed("zz");
    h.run(Command::Focus(Focus::FindInFiles));
    h.run(Command::Insert("y".to_owned()));
    assert_eq!(h.doc_text("b.txt"), "zzfoo", "kept as typed");
    assert_eq!(h.doc_text("a.txt"), "bar");
    let report = h.report();
    assert_eq!(report.changed.len(), 1);
    assert_eq!(report.skipped.len(), 1);
    assert!(report.skipped[0].0.ends_with("b.txt"));
    let status = h.ws.status().unwrap();
    assert_eq!(status.level, Level::Error);
    assert!(status.text.contains("1 skipped"), "{}", status.text);
}

#[test]
fn a_document_in_conflict_with_the_disk_is_not_planned() {
    let mut h = Harness::new(&[("a.txt", "foo")]);
    h.open("a.txt");
    h.typed("x");
    h.file_changed("other");
    assert!(h.conflict().is_some());
    h.searched("foo", &["a.txt"]);
    assert!(h.ask_replace("bar").is_empty());
    assert_eq!(h.preview().files.len(), 0);
    assert_eq!(h.preview().failed.len(), 1);
    h.run(Command::Insert("y".to_owned()));
    assert_eq!(h.doc_text("a.txt"), "xfoo");
    assert_eq!(h.report().failed.len(), 1, "and said so at the end");
}

#[test]
fn closed_files_go_to_the_service_with_the_state_the_preview_saw() {
    let mut h = Harness::new(&[]);
    h.searched("foo", &["c.txt", "d.txt"]);
    let fx = h.ask_replace("bar");
    let id = planned_id(&fx);
    assert!(
        matches!(&fx[0], Effect::PlanProjectReplace { files, query, replacement, .. }
            if files.len() == 2 && query == "foo" && replacement == "bar"),
        "{fx:?}"
    );
    h.ws.handle(Event::ProjectReplacePlanned {
        id,
        files: vec![closed_plan("c.txt")],
        failed: vec![(Path::new(ROOT).join("d.txt"), "not UTF-8 text".to_owned())],
    });
    assert_eq!(h.preview().occurrences(), 1);
    let fx = h.ws.dispatch(Command::Insert("y".to_owned()));
    let [Effect::ApplyProjectReplace { id, files, .. }] = fx.as_slice() else {
        panic!("{fx:?}");
    };
    assert_eq!(*files, [(Path::new(ROOT).join("c.txt"), CLOSED_DISK)]);
    let id = *id;
    assert!(matches!(
        h.ws.search().replace,
        Some(ReplaceStage::Applying { .. })
    ));
    h.run(Command::Escape);
    assert!(
        matches!(h.ws.search().replace, Some(ReplaceStage::Applying { .. })),
        "writes in progress cannot be walked away from"
    );
    let fx = h.ws.handle(Event::ProjectReplaceApplied {
        id: id + 1,
        changed: vec![],
        skipped: vec![],
        failed: vec![],
    });
    assert!(fx.is_empty(), "somebody else's answer");
    h.ws.handle(Event::ProjectReplaceApplied {
        id,
        changed: vec![Path::new(ROOT).join("c.txt")],
        skipped: vec![(Path::new(ROOT).join("e.txt"), "changed".to_owned())],
        failed: vec![],
    });
    let report = h.report();
    assert_eq!(
        (
            report.changed.len(),
            report.skipped.len(),
            report.failed.len()
        ),
        (1, 1, 1),
        "the planning failure is in the report too"
    );
    assert_eq!(
        h.ws.status().unwrap().text,
        "Replaced in 1 file, 1 skipped, 1 failed"
    );
}

#[test]
fn a_closed_file_opened_after_the_preview_is_skipped() {
    let mut h = Harness::new(&[("c.txt", "foo")]);
    h.searched("foo", &["c.txt"]);
    let id = planned_id(&h.ask_replace("bar"));
    h.ws.handle(Event::ProjectReplacePlanned {
        id,
        files: vec![closed_plan("c.txt")],
        failed: vec![],
    });
    h.open("c.txt");
    h.run(Command::Focus(Focus::FindInFiles));
    let fx = h.ws.dispatch(Command::Insert("y".to_owned()));
    assert!(
        !fx.iter()
            .any(|e| matches!(e, Effect::ApplyProjectReplace { .. })),
        "{fx:?}"
    );
    assert_eq!(h.report().skipped.len(), 1);
    assert_eq!(h.doc_text("c.txt"), "foo");
}

#[test]
fn an_open_document_closed_after_the_preview_is_skipped() {
    let mut h = Harness::new(&[("a.txt", "foo")]);
    h.open("a.txt");
    h.searched("foo", &["a.txt"]);
    assert!(h.ask_replace("bar").is_empty());
    h.act(Action::CloseTab);
    h.run(Command::Focus(Focus::FindInFiles));
    let fx = h.ws.dispatch(Command::Insert("y".to_owned()));
    assert!(
        !fx.iter()
            .any(|e| matches!(e, Effect::ApplyProjectReplace { .. })),
        "{fx:?}"
    );
    assert_eq!(h.report().skipped.len(), 1);
}

#[test]
fn a_late_plan_after_cancelling_is_ignored() {
    let mut h = Harness::new(&[]);
    h.searched("foo", &["c.txt"]);
    let id = planned_id(&h.ask_replace("bar"));
    h.run(Command::Escape);
    h.ws.handle(Event::ProjectReplacePlanned {
        id,
        files: vec![closed_plan("c.txt")],
        failed: vec![],
    });
    assert_eq!(h.ws.search().replace, None);
}

#[test]
fn replacing_needs_a_finished_text_search_for_this_query() {
    let mut h = Harness::new(&[("a.txt", "foo")]);
    h.open("a.txt");
    h.act(Action::ReplaceInFiles);
    h.typed("foo");
    h.run(Command::Indent);
    let fx = h.ws.dispatch(Command::Newline);
    assert!(fx.is_empty());
    assert_eq!(h.ws.search().replace, None);
    assert!(h.ws.status().unwrap().text.contains("search first"));

    h.searched("foo", &["a.txt"]);
    h.act(Action::ReplaceInFiles);
    h.run(Command::Indent);
    h.typed("x");
    h.run(Command::Indent);
    let fx = h.ws.dispatch(Command::Newline);
    assert!(fx.is_empty());
    assert_eq!(
        h.ws.search().replace,
        None,
        "the results are of another query"
    );
}

#[test]
fn usages_are_not_a_search_to_replace() {
    let mut h = Harness::new(&[("a.rs", "foo")]);
    h.ready("a.rs");
    let usage = LspUsage {
        location: LspLocation {
            path: Path::new(ROOT).join("a.rs"),
            start: (0, 0),
        },
        disk_line: "foo".to_owned(),
    };
    h.act(Action::FindUsages);
    let Some(Effect::Lsp(LspEffect::Query { id, .. })) = h.lsp.last().cloned() else {
        panic!("{:?}", h.lsp);
    };
    h.lsp_event(Event::Lsp(LspEvent::Answered {
        server: rust_server(),
        id,
        result: Ok(LspAnswer::References(vec![usage])),
    }));
    h.act(Action::ReplaceInFiles);
    assert!(h.ws.status().unwrap().text.contains("usages"));
    h.run(Command::Indent);
    let fx = h.ws.dispatch(Command::Newline);
    assert!(fx.is_empty(), "{fx:?}");
    assert_eq!(h.ws.search().replace, None);
    assert!(h.ws.status().unwrap().text.contains("usages"));
}

#[test]
fn a_bad_group_in_the_replacement_stops_before_any_plan() {
    let mut h = Harness::new(&[("a.txt", "foo")]);
    h.open("a.txt");
    h.act(Action::FindInProject);
    h.act(Action::ToggleFindRegex);
    h.searched("foo", &["a.txt"]);
    let fx = h.ask_replace("$3");
    assert!(fx.is_empty());
    assert_eq!(h.ws.search().replace, None);
    assert_eq!(h.ws.status().unwrap().level, Level::Error);
}

#[test]
fn esc_steps_out_of_the_replacement_before_leaving_the_panel() {
    let mut h = Harness::new(&[]);
    h.act(Action::ReplaceInFiles);
    assert!(h.ws.search().replacement.is_some());
    h.run(Command::Escape);
    assert!(h.ws.search().replacement.is_none());
    assert_eq!(h.ws.focus(), Focus::FindInFiles);
    h.run(Command::Escape);
    assert_eq!(h.ws.focus(), Focus::Editor);
}

#[test]
fn the_preview_scrolls_within_its_rows() {
    let text = "foo\n".repeat(30);
    let mut h = Harness::new(&[("a.txt", &text)]);
    h.open("a.txt");
    h.searched("foo", &["a.txt"]);
    assert!(h.ask_replace("bar").is_empty());
    h.run(Command::Move(Motion::PageDown { rows: 1000 }));
    assert_eq!(h.preview().scroll, h.preview().rows() - 1);
    h.run(Command::Move(Motion::Up));
    assert_eq!(h.preview().scroll, h.preview().rows() - 2);
    h.run(Command::Move(Motion::PageUp { rows: 1000 }));
    assert_eq!(h.preview().scroll, 0);
}

#[test]
fn replace_in_files_is_in_the_palette() {
    let mut h = Harness::new(&[]);
    h.act(Action::OpenPalette);
    h.typed("Replace in Files");
    assert_eq!(h.picker_labels()[0], "Replace in Files");
    h.run(Command::Newline);
    assert!(h.ws.search().replacement.is_some());
}
