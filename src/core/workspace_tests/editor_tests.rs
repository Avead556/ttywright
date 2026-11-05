use super::*;

#[test]
fn opening_files_adds_tabs_and_reopening_activates() {
    let mut h = Harness::new(&[("a.rs", "a"), ("b.rs", "b")]);
    h.open("a.rs");
    h.open("b.rs");
    assert_eq!(h.tab_names(), ["a.rs", "b.rs"]);
    h.open("a.rs");
    assert_eq!(h.active_name(), "a.rs");
    assert_eq!(h.tab_names().len(), 2);
    assert_eq!(h.ws.focus(), Focus::Editor);
}

#[test]
fn opening_at_a_line_places_the_cursor() {
    let mut h = Harness::new(&[("a.rs", "x\ny\nz")]);
    h.run(Command::OpenFile {
        path: PathBuf::from("a.rs"),
        at: Some((2, 0)),
    });
    assert_eq!(h.ws.active().unwrap().0.cursor.idx, 4);
}

#[test]
fn tabs_cycle_in_both_directions() {
    let mut h = Harness::new(&[("a.rs", ""), ("b.rs", ""), ("c.rs", "")]);
    for f in ["a.rs", "b.rs", "c.rs"] {
        h.open(f);
    }
    h.act(Action::NextTab);
    assert_eq!(h.active_name(), "a.rs");
    h.act(Action::PreviousTab);
    assert_eq!(h.active_name(), "c.rs");
}

#[test]
fn typing_then_undo_and_redo() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.typed("abc");
    h.run(Command::Move(Motion::Left));
    h.typed("X");
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "abc");
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "");
    h.act(Action::Redo);
    assert_eq!(h.active_text(), "abc");
    assert_eq!(h.ws.active().unwrap().0.cursor.idx, 3);
}

#[test]
fn word_motions_stop_at_word_edges_and_cross_lines() {
    let mut h = Harness::new(&[("a.rs", "foo_bar (baz)\nqux")]);
    h.open("a.rs");
    let mut stops = Vec::new();
    for _ in 0..4 {
        h.run(Command::Move(Motion::WordRight));
        stops.push(h.ws.active().unwrap().0.cursor.idx);
    }
    assert_eq!(stops, [7, 12, 17, 17]);
    stops.clear();
    for _ in 0..4 {
        h.run(Command::Move(Motion::WordLeft));
        stops.push(h.ws.active().unwrap().0.cursor.idx);
    }
    assert_eq!(stops, [14, 9, 0, 0]);
}

#[test]
fn word_right_never_lands_inside_a_grapheme() {
    let mut h = Harness::new(&[("a.rs", "e\u{301} x")]);
    h.open("a.rs");
    h.run(Command::Move(Motion::WordRight));
    assert_eq!(h.ws.active().unwrap().0.cursor.idx, 2);
}

#[test]
fn word_motions_work_in_search_fields() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.open("a.rs");
    h.act(Action::Find);
    h.typed("foo bar");
    h.run(Command::Move(Motion::WordLeft));
    h.typed("_");
    assert_eq!(h.ws.find_bar().unwrap().input.text(), "foo _bar");
}

#[test]
fn split_shows_the_same_document_and_edits_appear_in_both() {
    let mut h = Harness::new(&[("a.rs", "hello")]);
    h.open("a.rs");
    h.run(Command::Move(Motion::LineEnd));
    h.act(Action::SplitVertically);
    assert_eq!(h.ws.layout().panes().len(), 2);
    h.run(Command::Move(Motion::DocStart));
    h.typed(">");
    h.act(Action::NextSplitter);
    let (view, doc) = h.ws.active().unwrap();
    assert_eq!(doc.text().to_string(), ">hello");
    assert_eq!(view.cursor.idx, 6, "cursor mapped through the edit");
}

#[test]
fn closing_the_last_tab_of_a_split_removes_the_split() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.act(Action::SplitHorizontally);
    h.act(Action::CloseTab);
    assert_eq!(h.ws.layout().panes().len(), 1);
    assert_eq!(h.active_name(), "a.rs", "the original split remains");
}

#[test]
fn closing_a_modified_tab_saves_it_first() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.typed("x");
    h.act(Action::CloseTab);
    assert_eq!(h.written, [(PathBuf::from("/proj/a.rs"), b"x".to_vec())]);
    assert!(h.ws.active().is_none());
}

#[test]
fn a_failed_save_on_close_reopens_the_tab() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.typed("x");
    h.fail_writes = true;
    h.act(Action::CloseTab);
    assert_eq!(h.active_text(), "x", "edits are not lost");
    assert_eq!(h.ws.status().map(|s| s.level), Some(Level::Error));
}

#[test]
fn quit_saves_every_modified_document() {
    let mut h = Harness::new(&[("a.rs", ""), ("b.rs", "")]);
    h.open("a.rs");
    h.typed("1");
    h.open("b.rs");
    h.typed("2");
    h.act(Action::Quit);
    assert_eq!(h.written.len(), 2);
    assert!(h.ws.should_quit());
}

#[test]
fn failed_quit_save_needs_a_second_quit() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.typed("1");
    h.fail_writes = true;
    h.act(Action::Quit);
    assert!(!h.ws.should_quit());
    h.act(Action::Quit);
    assert!(h.ws.should_quit());
}

#[test]
fn palette_runs_the_selected_action() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.act(Action::OpenPalette);
    h.typed("split vert");
    h.run(Command::Newline);
    assert!(h.ws.picker().is_none());
    assert_eq!(h.ws.layout().panes().len(), 2);
}

#[test]
fn go_to_file_opens_the_fuzzy_match() {
    let mut h = Harness::new(&[("src/main.rs", "fn main() {}")]);
    h.ws.handle(Event::FilesIndexed {
        files: vec![
            PathBuf::from("/proj/Cargo.toml"),
            PathBuf::from("/proj/src/main.rs"),
        ],
    });
    h.act(Action::GotoFile);
    h.typed("main");
    h.run(Command::Newline);
    assert_eq!(h.active_name(), "main.rs");
}

#[test]
fn escape_closes_the_picker_without_running_anything() {
    let mut h = Harness::new(&[]);
    h.act(Action::OpenPalette);
    h.run(Command::Escape);
    assert!(h.ws.picker().is_none());
    assert!(!h.ws.should_quit());
}

#[test]
fn tree_enter_opens_the_selected_file() {
    let mut h = Harness::new(&[("a.rs", "a")]);
    h.ws.handle(Event::DirListed {
        path: PathBuf::from(ROOT),
        result: Ok(vec![DirEntry {
            name: "a.rs".into(),
            path: PathBuf::from("/proj/a.rs"),
            is_dir: false,
        }]),
    });
    assert_eq!(h.ws.focus(), Focus::Project);
    h.run(Command::Move(Motion::Down));
    h.run(Command::Newline);
    assert_eq!(h.active_name(), "a.rs");
    assert_eq!(h.ws.focus(), Focus::Editor);
}

#[test]
fn find_jumps_between_matches_and_escape_returns_to_the_editor() {
    let mut h = Harness::new(&[("a.rs", "foo bar foo")]);
    h.open("a.rs");
    h.act(Action::Find);
    h.typed("foo");
    assert_eq!(h.ws.find_bar().unwrap().matches.len(), 2);
    assert_eq!(h.ws.active().unwrap().0.cursor.idx, 0);
    h.run(Command::Newline);
    assert_eq!(h.ws.active().unwrap().0.cursor.idx, 8);
    h.run(Command::Newline);
    assert_eq!(h.ws.active().unwrap().0.cursor.idx, 0, "wraps around");
    h.run(Command::Escape);
    assert_eq!(h.ws.focus(), Focus::Editor);
    assert!(h.ws.find_bar().is_none());
}

#[test]
fn find_in_files_searches_and_opens_a_result() {
    let mut h = Harness::new(&[("src/lib.rs", "a\nlet needle = 1;")]);
    h.act(Action::FindInProject);
    h.typed("needle");
    let effects = h.ws.dispatch(Command::Newline);
    let [Effect::SearchProject { id, query, .. }] = effects.as_slice() else {
        panic!("expected a search, got {effects:?}");
    };
    assert_eq!(query, "needle");
    h.ws.handle(Event::SearchFinished {
        id: *id,
        matches: vec![SearchMatch {
            path: PathBuf::from("/proj/src/lib.rs"),
            line: 1,
            col: 4,
            len: 3,
            text: "let needle = 1;".into(),
        }],
        truncated: false,
    });
    h.run(Command::Move(Motion::Down));
    h.run(Command::Newline);
    assert_eq!(h.active_name(), "lib.rs");
    assert_eq!(h.ws.active().unwrap().0.cursor.idx, 6);
}

#[test]
fn stale_search_results_are_ignored() {
    let mut h = Harness::new(&[]);
    h.ws.handle(Event::SearchFinished {
        id: 99,
        matches: Vec::new(),
        truncated: true,
    });
    assert!(!h.ws.search().truncated);
}

#[test]
fn toggle_project_shows_focuses_and_hides() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.act(Action::ToggleProject);
    assert_eq!(h.ws.focus(), Focus::Project);
    h.act(Action::ToggleProject);
    assert!(!h.ws.project_visible());
    assert_eq!(h.ws.focus(), Focus::Editor);
    h.act(Action::ToggleProject);
    assert!(h.ws.project_visible());
}

#[test]
fn clicking_text_focuses_that_split_and_places_the_cursor_by_display_column() {
    let mut h = Harness::new(&[("a.rs", "x\n\tab")]);
    h.open("a.rs");
    let first = h.ws.active_view().unwrap();
    h.act(Action::SplitVertically);
    h.act(Action::ToggleProject);
    h.run(Command::FocusView {
        view: first,
        at: Some((1, 5)),
    });
    assert_eq!(h.ws.active_view(), Some(first));
    assert_eq!(h.ws.focus(), Focus::Editor);
    assert_eq!(h.ws.active().unwrap().0.cursor.idx, 4);
}

#[test]
fn clicking_a_tab_activates_it_without_moving_its_cursor() {
    let mut h = Harness::new(&[("a.rs", "abc"), ("b.rs", "")]);
    h.open("a.rs");
    h.run(Command::Move(Motion::LineEnd));
    let a = h.ws.active_view().unwrap();
    h.open("b.rs");
    h.run(Command::FocusView { view: a, at: None });
    assert_eq!(h.active_name(), "a.rs");
    assert_eq!(h.ws.active().unwrap().0.cursor.idx, 3);
}

#[test]
fn double_clicking_a_tree_file_opens_it() {
    let mut h = Harness::new(&[("a.rs", "a")]);
    h.ws.handle(Event::DirListed {
        path: PathBuf::from(ROOT),
        result: Ok(vec![DirEntry {
            name: "a.rs".into(),
            path: PathBuf::from("/proj/a.rs"),
            is_dir: false,
        }]),
    });
    let path = PathBuf::from("/proj/a.rs");
    h.run(Command::SelectTreeRow {
        path: path.clone(),
        activate: false,
    });
    assert_eq!(h.ws.tree().selected(), path.as_path());
    assert!(h.ws.active().is_none(), "a single click only selects");
    h.run(Command::SelectTreeRow {
        path,
        activate: true,
    });
    assert_eq!(h.active_name(), "a.rs");
}

#[test]
fn clicking_a_picker_item_runs_it() {
    let mut h = Harness::new(&[("a.rs", "")]);
    h.open("a.rs");
    h.act(Action::OpenPalette);
    h.typed("split vert");
    h.run(Command::SelectPickerItem {
        index: 0,
        accept: true,
    });
    assert_eq!(h.ws.layout().panes().len(), 2);
}

#[test]
fn clicking_a_search_result_opens_it() {
    let mut h = Harness::new(&[("a.rs", "one\ntwo")]);
    h.act(Action::FindInProject);
    h.typed("two");
    let fx = h.ws.dispatch(Command::Newline);
    let [Effect::SearchProject { id, .. }] = fx.as_slice() else {
        panic!("{fx:?}");
    };
    h.ws.handle(Event::SearchFinished {
        id: *id,
        matches: vec![SearchMatch {
            path: PathBuf::from("/proj/a.rs"),
            line: 1,
            col: 0,
            len: 3,
            text: "two".into(),
        }],
        truncated: false,
    });
    h.run(Command::SelectSearchResult {
        index: 0,
        open: true,
    });
    assert_eq!(h.active_name(), "a.rs");
    assert_eq!(h.ws.active().unwrap().0.cursor.idx, 4);
}
