use super::*;

#[test]
fn the_menu_opens_walks_and_runs_what_is_chosen() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.open("a.rs");
    h.act(Action::MainMenu);
    assert_eq!(h.ws.menu().map(|m| (m.menu, m.entry)), Some((0, 0)));
    h.run(Command::Move(Motion::Right));
    assert_eq!(h.ws.menu().map(|m| m.menu), Some(1));
    h.run(Command::Insert("q".into()));
    assert_eq!(h.active_text(), "x", "nothing reaches the text under it");
    h.run(Command::Move(Motion::Left));
    h.run(Command::Newline);
    assert!(h.ws.menu().is_none());
    assert!(h.ws.file_prompt().is_some(), "File's first entry: New File");
}

#[test]
fn clicking_a_title_again_closes_and_escape_closes() {
    let mut h = Harness::new(&[]);
    h.run(Command::OpenMenu { menu: 2 });
    assert_eq!(h.ws.menu().map(|m| m.menu), Some(2));
    h.run(Command::OpenMenu { menu: 3 });
    assert_eq!(h.ws.menu().map(|m| m.menu), Some(3));
    h.run(Command::OpenMenu { menu: 3 });
    assert!(h.ws.menu().is_none());
    h.run(Command::OpenMenu { menu: 0 });
    h.run(Command::Escape);
    assert!(h.ws.menu().is_none());
}

#[test]
fn a_click_on_an_entry_runs_it_and_a_separator_does_nothing() {
    let mut h = Harness::new(&[]);
    h.run(Command::OpenMenu { menu: 0 });
    h.run(Command::PickMenuItem { entry: 2 });
    assert!(h.ws.menu().is_none());
    assert!(h.ws.file_prompt().is_none());
    h.run(Command::OpenMenu { menu: 0 });
    h.run(Command::PickMenuItem { entry: 1 });
    assert!(matches!(
        h.ws.file_prompt().map(|p| &p.kind),
        Some(crate::core::file_op::FilePromptKind::NewFolder { .. })
    ));
}

#[test]
fn a_new_file_goes_beside_the_open_one_and_opens() {
    let mut h = Harness::new(&[("src/a.rs", "x")]);
    h.open("src/a.rs");
    h.act(Action::NewFile);
    h.typed_prompt("b.rs");
    assert_eq!(h.active_text(), "x", "typing stays in the prompt");
    h.run(Command::Newline);
    assert_eq!(
        h.last_file_op(),
        Some(crate::core::file_op::FileOp::NewFile(
            Path::new(FO_ROOT).join("src/b.rs")
        ))
    );
    assert!(h.ws.file_prompt().is_some_and(|p| p.busy));
    h.run(Command::Newline);
    assert_eq!(
        h.git
            .iter()
            .filter(|e| matches!(e, Effect::FileOp(_)))
            .count(),
        1,
        "a second Enter does not ask twice"
    );
    h.file_op_ok();
    assert!(h.ws.file_prompt().is_none());
    assert!(
        h.ws.active()
            .is_some_and(|(_, d)| d.path().ends_with("src/b.rs"))
    );
}

#[test]
fn a_name_that_leaves_the_folder_stays_in_the_prompt_with_the_reason() {
    let mut h = Harness::new(&[]);
    h.act(Action::NewFile);
    h.typed_prompt("../evil");
    h.run(Command::Newline);
    assert_eq!(h.last_file_op(), None);
    assert!(h.ws.file_prompt().and_then(|p| p.error.as_ref()).is_some());
    h.run(Command::DeleteBackward);
    assert!(h.ws.file_prompt().and_then(|p| p.error.as_ref()).is_none());
}

#[test]
fn a_failure_keeps_the_prompt_open_to_fix_the_name() {
    let mut h = Harness::new(&[]);
    h.act(Action::NewFile);
    h.typed_prompt("a");
    h.run(Command::Newline);
    let op = h.last_file_op().expect("asked");
    h.lsp_event(Event::FileOpDone {
        op,
        result: Err("File exists".into()),
    });
    let prompt = h.ws.file_prompt().expect("still up");
    assert!(!prompt.busy);
    assert_eq!(prompt.error.as_deref(), Some("File exists"));
}

#[test]
fn a_rename_closes_the_tab_and_brings_it_back_under_the_new_name() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.open("a.rs");
    h.act(Action::RenameFile);
    assert_eq!(
        h.ws.file_prompt()
            .map(|p| p.input.text().to_owned())
            .as_deref(),
        Some("a.rs")
    );
    for _ in 0..4 {
        h.run(Command::DeleteBackward);
    }
    h.typed_prompt("b.rs");
    h.run(Command::Newline);
    assert!(
        h.ws.active().is_none(),
        "no tab can write the old name back meanwhile"
    );
    h.file_op_ok();
    assert!(
        h.ws.active()
            .is_some_and(|(_, d)| d.path().ends_with("b.rs"))
    );
}

#[test]
fn a_failed_rename_or_delete_brings_the_tab_back_as_it_was() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.open("a.rs");
    h.act(Action::DeleteFile);
    h.run(Command::Insert("y".into()));
    assert!(h.ws.active().is_none());
    let op = h.last_file_op().expect("asked");
    assert_eq!(
        op,
        crate::core::file_op::FileOp::Trash(Path::new(FO_ROOT).join("a.rs"))
    );
    h.lsp_event(Event::FileOpDone {
        op,
        result: Err("no trash here".into()),
    });
    assert_eq!(h.active_text(), "x");
}

#[test]
fn delete_asks_first_and_no_keeps_the_file() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.open("a.rs");
    h.act(Action::DeleteFile);
    h.run(Command::Insert("z".into()));
    assert_eq!(h.last_file_op(), None);
    h.run(Command::Insert("n".into()));
    assert!(h.ws.file_prompt().is_none());
    assert_eq!(h.last_file_op(), None);
    assert_eq!(h.active_text(), "x");
}

#[test]
fn unsaved_text_is_never_moved_or_trashed() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.open("a.rs");
    h.typed("y");
    for action in [Action::RenameFile, Action::DeleteFile] {
        h.act(action);
        assert!(h.ws.file_prompt().is_none(), "{action:?}");
    }
    assert_eq!(h.last_file_op(), None);
    assert!(h.dirty());
}

#[test]
fn the_project_folder_itself_is_not_offered() {
    let mut h = Harness::new(&[]);
    for action in [Action::RenameFile, Action::DeleteFile] {
        h.act(action);
        assert!(h.ws.file_prompt().is_none(), "{action:?}");
    }
}
