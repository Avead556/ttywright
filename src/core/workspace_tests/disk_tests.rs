use super::*;

#[test]
fn autosave_writes_dirty_documents_without_a_message() {
    let mut h = Harness::new(&[("a.rs", "x"), ("b.rs", "y")]);
    h.open("a.rs");
    h.open("b.rs");
    h.typed("!");
    h.run(Command::Autosave);
    assert_eq!(h.written.len(), 1, "the clean document is left alone");
    assert_eq!(h.written[0].1, b"!y");
    assert!(!h.dirty());
    assert_eq!(h.ws.status(), None);
    assert!(!h.ws.has_autosavable());
}

#[test]
fn autosave_keeps_an_existing_message_and_a_pending_quit_choice() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.typed("x");
    h.fail_writes = true;
    h.act(Action::Quit);
    h.run(Command::Autosave);
    assert!(h.ws.status().unwrap().text.contains("quit again"));
    h.act(Action::Quit);
    assert!(h.ws.should_quit());
}

#[test]
fn switching_tabs_autosaves() {
    let mut h = Harness::new(&[("a.rs", ""), ("b.rs", "")]);
    h.open("a.rs");
    h.open("b.rs");
    h.typed("x");
    h.act(Action::NextTab);
    assert_eq!(h.written.len(), 1);
    assert_eq!(h.ws.status(), None);
}

#[test]
fn closing_a_dirty_tab_writes_once() {
    let mut h = Harness::new(&[("a.rs", ""), ("b.rs", "")]);
    h.open("a.rs");
    h.open("b.rs");
    h.typed("x");
    h.act(Action::CloseTab);
    assert_eq!(h.written.len(), 1);
}

#[test]
fn a_failing_autosave_reports_only_once_until_it_succeeds() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.typed("x");
    h.fail_writes = true;
    h.run(Command::Autosave);
    assert!(h.ws.status().is_some());
    h.typed("y");
    h.run(Command::Autosave);
    assert_eq!(h.ws.status(), None, "already told");
    assert!(h.dirty());
    h.fail_writes = false;
    h.run(Command::Autosave);
    h.typed("z");
    h.fail_writes = true;
    h.run(Command::Autosave);
    assert!(h.ws.status().is_some(), "a new failure after a success");
}

#[test]
fn external_change_reloads_a_clean_document_and_keeps_the_cursor() {
    let mut h = Harness::new(&[("a.rs", "one\ntwo\nthree")]);
    h.open("a.rs");
    h.run(Command::Move(Motion::DocEnd));
    h.file_changed("ONE\ntwo\nthree");
    assert_eq!(h.active_text(), "ONE\ntwo\nthree");
    assert!(!h.dirty());
    assert_eq!(h.ws.active().unwrap().0.cursor.idx, 13);
    assert_eq!(h.ws.active().unwrap().1.disk(), Some(OTHER));
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "one\ntwo\nthree", "reload can be undone");
}

#[test]
fn external_change_to_a_dirty_document_waits_for_the_user() {
    let mut h = Harness::new(&[("a.rs", "base")]);
    h.open("a.rs");
    h.typed("mine ");
    h.file_changed("theirs");
    assert_eq!(h.conflict(), Some(Conflict::Modified));
    assert_eq!(h.active_text(), "mine base");
    h.run(Command::Autosave);
    assert!(h.written.is_empty(), "autosave must not overwrite theirs");
    assert!(!h.ws.has_autosavable());
}

#[test]
fn keep_mine_overwrites_and_clears_the_conflict() {
    let mut h = Harness::new(&[("a.rs", "base")]);
    h.open("a.rs");
    h.typed("mine ");
    h.file_changed("theirs");
    h.act(Action::KeepMine);
    assert_eq!(h.written[0].1, b"mine base");
    assert_eq!(h.conflict(), None);
    assert!(!h.dirty());
}

#[test]
fn reload_takes_their_version_and_can_be_undone() {
    let mut h = Harness::new(&[("a.rs", "base")]);
    h.open("a.rs");
    h.typed("mine ");
    h.file_changed("theirs");
    let doc = h.active_doc();
    h.ws.handle(Event::FileReloaded {
        doc,
        disk: Some(OTHER),
        result: Ok(Some(b"theirs".to_vec())),
    });
    assert_eq!(h.active_text(), "theirs");
    assert_eq!(h.conflict(), None);
    assert!(!h.dirty());
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "mine base", "my edits are not lost");
}

#[test]
fn same_content_on_disk_resolves_everything() {
    let mut h = Harness::new(&[("a.rs", "base")]);
    h.open("a.rs");
    h.typed("x");
    h.file_changed("other");
    h.file_changed("xbase");
    assert_eq!(h.conflict(), None);
    assert!(!h.dirty());
}

#[test]
fn deleted_file_stops_autosave_until_an_explicit_save() {
    let mut h = Harness::new(&[("a.rs", "base")]);
    h.open("a.rs");
    h.typed("x");
    h.file_deleted();
    assert_eq!(h.conflict(), Some(Conflict::Deleted));
    h.run(Command::Autosave);
    assert!(h.written.is_empty());
    h.act(Action::Save);
    assert_eq!(h.written[0].1, b"xbase");
    assert_eq!(h.conflict(), None);
}

#[test]
fn a_guarded_write_that_finds_a_change_becomes_a_conflict() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.typed("x");
    h.conflict_on_write = Some(Some(OTHER));
    h.act(Action::Save);
    assert_eq!(h.conflict(), Some(Conflict::Modified));
    assert!(h.ws.status().unwrap().text.contains("changed on disk"));
    assert!(h.written.is_empty());
}

#[test]
fn quitting_over_a_conflict_needs_a_second_quit() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.typed("x");
    h.conflict_on_write = Some(Some(OTHER));
    h.act(Action::Quit);
    assert!(!h.ws.should_quit());
    assert!(h.ws.status().unwrap().text.contains("quit again"));
    h.act(Action::Quit);
    assert!(h.ws.should_quit());
}

#[test]
fn closing_a_tab_that_cannot_be_written_brings_it_back() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.typed("x");
    h.conflict_on_write = Some(Some(OTHER));
    h.act(Action::CloseTab);
    assert_eq!(h.tab_names(), ["a.rs"]);
    assert_eq!(h.active_text(), "x");
}

#[test]
fn check_disk_asks_about_every_open_document() {
    let mut h = Harness::new(&[("a.rs", ""), ("b.rs", "")]);
    h.open("a.rs");
    h.open("b.rs");
    let fx = h.ws.dispatch(Command::CheckDisk);
    let checked = fx.iter().filter(|e| matches!(e, Effect::CheckFile { .. }));
    assert_eq!(checked.count(), 2);
}

#[test]
fn changed_paths_check_open_files_and_refresh_listed_directories() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    let root = PathBuf::from(ROOT);
    h.ws.handle(Event::DirListed {
        path: root.clone(),
        result: Ok(Vec::new()),
    });

    let fx = h.ws.handle(Event::PathsChanged {
        paths: vec![root.join("a.rs")],
    });
    assert!(matches!(fx[0], Effect::CheckFile { .. }));
    assert!(matches!(&fx[1], Effect::ListDir { path } if *path == root));
    assert!(
        !fx.iter().any(|e| matches!(e, Effect::IndexFiles { .. })),
        "saving an open file does not reindex"
    );

    let fx = h.ws.handle(Event::PathsChanged {
        paths: vec![root.join("new.rs"), root.join("unlisted/x.rs")],
    });
    assert!(matches!(&fx[0], Effect::ListDir { path } if *path == root));
    assert!(matches!(fx[1], Effect::IndexFiles { .. }));
    assert_eq!(fx.len(), 4, "and the status and the index");
}
