use super::*;

#[test]
fn local_history_opens_on_the_newest_version_and_shows_its_diff() {
    let mut h = Harness::new(&[("a.rs", "new\n")]);
    h.open("a.rs");
    h.open_history(&["v2", "v1"]);
    assert_eq!(h.history_reads, ["v2"]);
    h.version_read("v2", "old\n");
    let panel = h.ws.local_history().unwrap();
    assert_eq!(panel.selected, 0);
    assert_eq!(panel.diff[0].kind, DiffKind::Removed);
    assert_eq!(panel.diff[0].text, "old");
    assert_eq!(panel.diff[1].text, "new");
}

#[test]
fn local_history_without_versions_only_says_so() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.open_history(&[]);
    assert!(h.ws.local_history().is_none());
    assert!(h.ws.status().unwrap().text.contains("No Local History"));
}

#[test]
fn moving_through_versions_reads_each_and_ignores_late_answers() {
    let mut h = Harness::new(&[("a.rs", "x\n")]);
    h.open("a.rs");
    h.open_history(&["v2", "v1"]);
    h.run(Command::Move(Motion::Down));
    assert_eq!(h.history_reads, ["v2", "v1"]);
    h.version_read("v2", "late\n");
    assert!(h.ws.local_history().unwrap().version.is_none());
    h.version_read("v1", "old\n");
    assert_eq!(h.ws.local_history().unwrap().diff[0].text, "old");
    h.run(Command::Move(Motion::Down));
    assert_eq!(h.history_reads.len(), 2, "already at the last version");
    h.run(Command::SelectHistoryEntry { index: 0 });
    assert_eq!(h.ws.local_history().unwrap().selected, 0);
    assert!(h.ws.local_history().unwrap().diff.is_empty());
}

#[test]
fn revert_is_an_unsaved_undoable_step_that_keeps_the_replaced_text() {
    let mut h = Harness::new(&[("a.rs", "new")]);
    h.open("a.rs");
    h.run(Command::Move(Motion::DocEnd));
    h.open_history(&["v1"]);
    h.version_read("v1", "old");
    h.run(Command::Insert("r".into()));

    assert!(h.ws.local_history().is_none());
    assert_eq!(h.active_text(), "old");
    assert!(h.dirty());
    assert_eq!(
        h.snapshots,
        [(SnapshotLabel::BeforeRevert, b"new".to_vec())]
    );
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "new");
}

#[test]
fn revert_waits_for_the_version_to_arrive() {
    let mut h = Harness::new(&[("a.rs", "new")]);
    h.open("a.rs");
    h.open_history(&["v1"]);
    h.run(Command::Insert("r".into()));
    assert!(h.ws.local_history().is_some());
    assert_eq!(h.active_text(), "new");
    assert!(h.snapshots.is_empty());
}

#[test]
fn reverting_a_conflicted_document_keeps_autosave_stopped() {
    let mut h = Harness::new(&[("a.rs", "base")]);
    h.open("a.rs");
    h.typed("x");
    h.file_changed("theirs");
    h.open_history(&["v1"]);
    h.version_read("v1", "old");
    h.run(Command::Insert("r".into()));
    assert_eq!(h.conflict(), Some(Conflict::Modified));
    h.run(Command::Autosave);
    assert!(h.written.is_empty());
}

#[test]
fn the_panel_is_modal_and_escape_closes_it() {
    let mut h = Harness::new(&[("a.rs", "text")]);
    h.open("a.rs");
    h.open_history(&["v1"]);
    h.run(Command::Insert("z".into()));
    h.act(Action::CloseTab);
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "text");
    assert_eq!(h.tab_names(), ["a.rs"]);
    h.run(Command::Escape);
    assert!(h.ws.local_history().is_none());
    h.typed("z");
    assert_eq!(h.active_text(), "ztext");
}

#[test]
fn quitting_works_from_the_panel() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.open_history(&["v1"]);
    h.act(Action::Quit);
    assert!(h.ws.should_quit());
}

#[test]
fn reloading_over_unsaved_edits_keeps_them_in_history() {
    let mut h = Harness::new(&[("a.rs", "base")]);
    h.open("a.rs");
    h.typed("mine ");
    let fx = h.ws.dispatch(Command::Run(Action::ReloadFromDisk));
    assert!(matches!(
        &fx[0],
        Effect::Snapshot { bytes, label: SnapshotLabel::BeforeReload, .. } if bytes == b"mine base"
    ));
    assert!(matches!(fx[1], Effect::ReloadFile { .. }));
}
