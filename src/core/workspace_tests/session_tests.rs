use super::*;

#[test]
fn session_restores_tabs_selection_and_cursors() {
    let mut h = Harness::new(&[("a.rs", "one\ntwo"), ("b.rs", "x"), ("c.rs", "y")]);
    h.open("a.rs");
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Right));
    h.open("b.rs");
    h.open("c.rs");
    h.act(Action::PreviousTab);
    h.act(Action::PreviousTab);

    let next = h.restart();
    assert_eq!(next.tab_names(), ["a.rs", "b.rs", "c.rs"]);
    assert_eq!(next.active_name(), "a.rs");
    assert_eq!(next.cursor(), 5);
    assert_eq!(next.ws.focus(), Focus::Editor);
}

#[test]
fn session_restores_splits_the_active_pane_and_tool_windows() {
    let mut h = Harness::new(&[("a.rs", "a"), ("b.rs", "b")]);
    h.open("a.rs");
    h.act(Action::SplitVertically);
    h.open("b.rs");
    h.act(Action::NextSplitter);
    h.act(Action::ToggleProject);
    h.act(Action::ToggleProject);
    h.act(Action::FindInProject);

    let next = h.restart();
    let Layout::Split { axis, children } = next.ws.layout() else {
        panic!("{:?}", next.ws.layout());
    };
    assert_eq!((*axis, children.len()), (Axis::Columns, 2));
    assert_eq!(next.active_name(), "a.rs");
    assert!(!next.ws.project_visible());
    assert!(next.ws.tool_window_visible());
    assert_eq!(next.ws.active_pane(), next.ws.layout().panes()[0]);
}

#[test]
fn session_drops_tabs_of_deleted_files_and_panes_left_empty() {
    let mut h = Harness::new(&[("a.rs", "a"), ("b.rs", "b"), ("c.rs", "c")]);
    h.open("a.rs");
    h.open("b.rs");
    h.act(Action::SplitVertically);
    h.open("c.rs");
    h.act(Action::PreviousTab);

    let mut gone_b = Harness::new(&[]);
    std::mem::swap(&mut gone_b.ws, &mut h.ws);
    gone_b.files.clone_from(&h.files);
    gone_b.files.retain(|(p, _)| !p.ends_with("b.rs"));
    let next = gone_b.restart();
    assert_eq!(next.ws.layout().panes().len(), 2);
    assert_eq!(next.tab_names(), ["c.rs"]);

    let mut h = Harness::new(&[("a.rs", "a"), ("b.rs", "b")]);
    h.open("a.rs");
    h.act(Action::SplitVertically);
    h.open("b.rs");
    h.act(Action::PreviousTab);
    h.act(Action::CloseTab);
    h.files.retain(|(p, _)| !p.ends_with("b.rs"));
    let next = h.restart();
    assert!(matches!(next.ws.layout(), Layout::Pane(_)));
    assert_eq!(next.tab_names(), ["a.rs"]);
}

#[test]
fn session_with_no_surviving_files_leaves_a_working_empty_editor() {
    let mut h = Harness::new(&[("a.rs", "a")]);
    h.open("a.rs");
    h.files.clear();
    let mut next = h.restart();
    assert!(next.ws.active().is_none());
    next.files.push((Path::new(ROOT).join("n.rs"), "n".into()));
    next.open("n.rs");
    assert_eq!(next.active_name(), "n.rs");
}

#[test]
fn session_cursor_is_clamped_when_the_file_got_shorter() {
    let mut h = Harness::new(&[("a.rs", "one\ntwo\nthree")]);
    h.open("a.rs");
    h.run(Command::Move(Motion::DocEnd));
    h.files[0].1 = "x".into();
    assert_eq!(h.restart().cursor(), 1);
}

#[test]
fn restored_documents_know_their_disk_state_and_are_clean() {
    let mut h = Harness::new(&[("a.rs", "a")]);
    h.open("a.rs");
    let session = match h.ws.dispatch(Command::SaveSession).pop() {
        Some(Effect::WriteSession(s)) => s,
        other => panic!("{other:?}"),
    };
    let mut next = Harness::new(&[]);
    next.ws.handle(Event::SessionLoaded {
        session,
        files: vec![SessionFile {
            path: Path::new(ROOT).join("a.rs"),
            disk: Some(OTHER),
            result: Ok(Some(b"a".to_vec())),
        }],
    });
    assert_eq!(next.ws.active().unwrap().1.disk(), Some(OTHER));
    assert!(!next.dirty());
}

#[test]
fn unsaved_text_lands_on_the_restored_tab_without_a_second_one() {
    let mut h = Harness::new(&[("a.rs", "disk"), ("b.rs", "b")]);
    h.open("a.rs");
    h.open("b.rs");
    let mut next = h.restart();
    next.ws.handle(Event::SwapsFound {
        swaps: vec![recovered(Some(4), Some(4))],
    });
    assert_eq!(next.tab_names(), ["a.rs", "b.rs"]);
    let pane = next.ws.pane(next.ws.active_pane()).unwrap();
    let a = next.ws.view(pane.views[0]).unwrap().doc;
    assert_eq!(next.ws.document(a).unwrap().text().to_string(), "unsaved");
    assert!(next.ws.document(a).unwrap().is_dirty());
    assert!(next.ws.status().unwrap().text.contains("Recovered"));
}

#[test]
fn session_is_written_only_when_it_changed() {
    let mut h = Harness::new(&[("a.rs", "ab")]);
    h.open("a.rs");
    assert_eq!(h.ws.dispatch(Command::SaveSession).len(), 1);
    assert!(h.ws.dispatch(Command::SaveSession).is_empty());
    h.run(Command::Move(Motion::Right));
    assert_eq!(h.ws.dispatch(Command::SaveSession).len(), 1);
}

#[test]
fn saving_the_session_keeps_messages_and_a_pending_quit_choice() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.typed("x");
    h.fail_writes = true;
    h.act(Action::Quit);
    let _ = h.ws.dispatch(Command::SaveSession);
    assert!(h.ws.status().is_some());
    h.act(Action::Quit);
    assert!(h.ws.should_quit());
}

#[test]
fn a_session_arriving_after_files_opened_is_ignored() {
    let mut h = Harness::new(&[("a.rs", "a")]);
    h.open("a.rs");
    let session = match h.ws.dispatch(Command::SaveSession).pop() {
        Some(Effect::WriteSession(s)) => s,
        other => panic!("{other:?}"),
    };
    h.ws.handle(Event::SessionLoaded {
        session,
        files: Vec::new(),
    });
    assert_eq!(h.tab_names(), ["a.rs"]);
}
