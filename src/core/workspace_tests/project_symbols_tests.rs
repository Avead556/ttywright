use super::*;

#[test]
fn go_to_symbol_asks_as_it_is_typed_and_a_late_answer_changes_nothing() {
    let mut h = Harness::new(&[("a.rs", "x\n  fn alpha\n")]);
    h.open("a.rs");
    h.act(Action::GotoSymbol);
    let fx = h.ws.dispatch(Command::Insert("al".to_owned()));
    let (old, query) = h.symbols_query(&fx).expect("asked");
    assert_eq!(query, "al");
    let fx = h.ws.dispatch(Command::Insert("p".to_owned()));
    let (id, query) = h.symbols_query(&fx).expect("asked");
    assert_eq!(query, "alp");
    assert_ne!(old, id);

    h.ws.handle(Event::SymbolsFound {
        id: old,
        symbols: vec![project_symbol("alpOLD", "a.rs", (0, 0))],
    });
    assert!(h.ws.picker().unwrap().matches.is_empty(), "too late");
    h.ws.handle(Event::SymbolsFound {
        id,
        symbols: vec![project_symbol("alpha", "a.rs", (1, 5))],
    });
    let items = h.ws.picker_items(PickerKind::ProjectSymbols);
    assert!(
        items[0].contains("alpha") && items[0].contains("a.rs:2"),
        "{items:?}"
    );
    assert_eq!(h.ws.picker().unwrap().matches.len(), 1);
    h.run(Command::Newline);
    assert_eq!(h.active_name(), "a.rs");
    assert_eq!(h.cursor(), 2 + 5, "the name, by byte column");
}

#[test]
fn search_everywhere_lists_symbols_and_still_tells_files_and_actions_apart() {
    let mut h = Harness::new(&[("a.rs", "x\nfn alpha\n"), ("b.rs", "y")]);
    h.act(Action::SearchEverywhere);
    let fx = h.ws.dispatch(Command::Insert("alpha".to_owned()));
    let (id, _) = h.symbols_query(&fx).expect("asked");
    h.ws.handle(Event::SymbolsFound {
        id,
        symbols: vec![project_symbol("alpha", "a.rs", (1, 3))],
    });
    let items = h.ws.picker_items(PickerKind::Everywhere);
    let row = items
        .iter()
        .position(|i| i.contains("a.rs:2"))
        .expect("listed");
    assert_eq!(Workspace::everywhere_action(row, items.len()), None);
    assert_eq!(
        h.ws.everywhere_symbol(row, items.len())
            .map(|s| s.name.as_str()),
        Some("alpha")
    );
    let last = items.len() - 1;
    assert!(Workspace::everywhere_action(last, items.len()).is_some());
    h.run(Command::Newline);
    assert_eq!(h.active_name(), "a.rs");
    assert_eq!(h.cursor(), 2 + 3);
}

#[test]
fn an_empty_query_asks_nothing_and_forgets_the_names() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.act(Action::GotoSymbol);
    let fx = h.ws.dispatch(Command::Insert("a".to_owned()));
    let (id, _) = h.symbols_query(&fx).expect("asked");
    h.ws.handle(Event::SymbolsFound {
        id,
        symbols: vec![project_symbol("a", "a.rs", (0, 0))],
    });
    let fx = h.ws.dispatch(Command::DeleteBackward);
    assert!(h.symbols_query(&fx).is_none());
    assert!(h.ws.picker_items(PickerKind::ProjectSymbols).is_empty());
}

#[test]
fn indexing_progress_shows_until_it_is_done() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    assert_eq!(h.ws.index_progress(), None);
    h.ws.handle(Event::IndexProgress { done: 3, total: 10 });
    assert_eq!(h.ws.index_progress(), Some((3, 10)));
    h.ws.handle(Event::IndexProgress {
        done: 10,
        total: 10,
    });
    assert_eq!(h.ws.index_progress(), None);
}

#[test]
fn the_index_hears_about_changes_and_about_coming_back_to_the_terminal() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    let fx = h.ws.dispatch(Command::CheckDisk);
    assert!(fx.contains(&Effect::RescanIndex));
    let fx = h.ws.handle(Event::PathsChanged {
        paths: vec![Path::new(ROOT).join("other.rs")],
    });
    assert!(fx.contains(&Effect::RescanIndex));
}

#[test]
fn the_main_window_starts_the_index_and_other_windows_do_not() {
    let root = PathBuf::from(ROOT);
    let (_, fx) = Workspace::new(root.clone());
    assert!(fx.iter().any(|e| matches!(e, Effect::IndexProject { .. })));
    let (_, fx) = Workspace::new_git_window(root.clone());
    assert!(!fx.iter().any(|e| matches!(e, Effect::IndexProject { .. })));
    let (_, fx) = Workspace::new_run_window(root);
    assert!(!fx.iter().any(|e| matches!(e, Effect::IndexProject { .. })));
}

#[test]
fn file_structure_without_a_server_reads_the_files_own_syntax() {
    let mut h = Harness::new(&[("a.rs", "fn one() {}\nfn two() {}\n")]);
    h.open("a.rs");
    let fx = h.ws.dispatch(Command::Run(Action::FileStructure));
    let asked = fx.iter().find_map(|e| match e {
        Effect::FileSymbols {
            path,
            version,
            text,
            ..
        } => Some((path.clone(), *version, text.clone())),
        _ => None,
    });
    let (path, version, text) = asked.expect("asked for the syntax");
    assert!(text.contains("fn two"), "the document's text, saved or not");
    let symbols = vec![crate::core::LspSymbol {
        name: "two".to_owned(),
        kind: "function".to_owned(),
        depth: 0,
        start: (1, 3),
    }];
    h.ws.handle(Event::FileSymbolsRead {
        path: path.clone(),
        version: version + 1,
        symbols: symbols.clone(),
    });
    assert!(h.ws.picker().is_none(), "about older text");
    h.ws.handle(Event::FileSymbolsRead {
        path,
        version,
        symbols,
    });
    assert_eq!(h.ws.picker().map(|p| p.kind), Some(PickerKind::Symbols));
    h.run(Command::Newline);
    assert_eq!(h.cursor(), 12 + 3);
}

#[test]
fn file_structure_prefers_a_ready_server() {
    let mut h = Harness::new(&[("a.rs", "fn one() {}\n")]);
    h.ready("a.rs");
    let fx = h.ws.dispatch(Command::Run(Action::FileStructure));
    assert!(!fx.iter().any(|e| matches!(e, Effect::FileSymbols { .. })));
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::Lsp(LspEffect::Query { .. })))
    );
}
