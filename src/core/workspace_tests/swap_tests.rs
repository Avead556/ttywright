use super::*;

#[test]
fn swaps_cover_each_new_version_once_and_only_unsaved_documents() {
    let mut h = Harness::new(&[("a.rs", "x"), ("b.rs", "y")]);
    h.open("a.rs");
    h.open("b.rs");
    assert!(!h.ws.has_unswapped());
    h.typed("1");
    assert!(h.ws.has_unswapped());
    h.run(Command::WriteSwaps);
    h.run(Command::WriteSwaps);
    h.typed("2");
    h.run(Command::WriteSwaps);
    let written: Vec<_> = h.swaps.iter().map(|(_, t)| t.clone().unwrap()).collect();
    assert_eq!(written, [b"1y".to_vec(), b"12y".to_vec()]);
    assert!(!h.ws.has_unswapped());
}

#[test]
fn saving_removes_the_swap() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.typed("x");
    h.run(Command::WriteSwaps);
    h.run(Command::Autosave);
    assert_eq!(h.swaps.last().unwrap().1, None);
    let count = h.swaps.len();
    h.run(Command::Autosave);
    assert_eq!(h.swaps.len(), count, "removed once");
}

#[test]
fn closing_a_dirty_tab_removes_the_swap_with_the_document() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.typed("x");
    h.run(Command::WriteSwaps);
    h.act(Action::CloseTab);
    assert_eq!(h.swaps.last().unwrap().1, None);
    assert!(h.tab_names().is_empty());
}

#[test]
fn a_failed_save_keeps_the_swap() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.typed("x");
    h.run(Command::WriteSwaps);
    h.fail_writes = true;
    h.run(Command::Autosave);
    assert!(h.swaps.iter().all(|(_, t)| t.is_some()));
}

#[test]
fn recovery_continues_the_edits_when_the_file_is_unchanged() {
    let mut h = Harness::new(&[]);
    h.ws.handle(Event::SwapsFound {
        swaps: vec![recovered(Some(4), Some(4))],
    });
    assert_eq!(h.active_text(), "unsaved");
    assert!(h.dirty());
    assert_eq!(h.conflict(), None);
    assert!(h.ws.status().unwrap().text.contains("Recovered"));
    assert!(!h.ws.has_unswapped(), "already in the swap file");
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "disk", "undo leads back to the file");
}

#[test]
fn recovery_writes_nothing_over_a_file_changed_since() {
    let mut h = Harness::new(&[]);
    h.ws.handle(Event::SwapsFound {
        swaps: vec![recovered(Some(4), Some(9))],
    });
    assert_eq!(h.active_text(), "unsaved");
    assert_eq!(h.conflict(), Some(Conflict::Modified));
    h.run(Command::Autosave);
    assert!(h.written.is_empty());
}

#[test]
fn recovery_of_a_deleted_file_needs_an_explicit_save() {
    let mut h = Harness::new(&[]);
    h.ws.handle(Event::SwapsFound {
        swaps: vec![recovered(Some(4), None)],
    });
    assert_eq!(h.active_text(), "unsaved");
    assert_eq!(h.conflict(), Some(Conflict::Deleted));
    h.run(Command::Autosave);
    assert!(h.written.is_empty());
    h.act(Action::Save);
    assert_eq!(h.written[0].1, b"unsaved");
    assert_eq!(h.swaps.last().unwrap().1, None);
}

#[test]
fn saving_a_recovered_document_clears_its_swap() {
    let mut h = Harness::new(&[]);
    h.ws.handle(Event::SwapsFound {
        swaps: vec![recovered(Some(4), Some(4))],
    });
    h.run(Command::Autosave);
    assert_eq!(h.written[0].1, b"unsaved");
    assert_eq!(h.swaps, [(Path::new(ROOT).join("a.rs"), None)]);
}
