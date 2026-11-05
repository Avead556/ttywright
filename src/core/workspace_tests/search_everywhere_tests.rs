use super::*;

#[test]
fn recent_files_list_the_latest_first_without_the_active_one() {
    let mut h = Harness::new(&[("a.rs", "a"), ("b.rs", "b"), ("c.rs", "c")]);
    h.open("a.rs");
    h.open("b.rs");
    h.open("c.rs");
    h.act(Action::RecentFiles);
    assert_eq!(h.picker_labels(), ["b.rs", "a.rs"]);
    h.run(Command::Newline);
    assert_eq!(h.active_name(), "b.rs");
    h.act(Action::RecentFiles);
    assert_eq!(
        h.picker_labels(),
        ["c.rs", "a.rs"],
        "reactivating a file moves it up"
    );
}

#[test]
fn recent_files_follow_tab_switches_and_keep_closed_files() {
    let mut h = Harness::new(&[("a.rs", "a"), ("b.rs", "b")]);
    h.open("a.rs");
    h.open("b.rs");
    h.act(Action::PreviousTab);
    assert_eq!(h.active_name(), "a.rs");
    h.act(Action::RecentFiles);
    assert_eq!(h.picker_labels(), ["b.rs"]);
    h.run(Command::Escape);
    h.act(Action::NextTab);
    h.act(Action::CloseTab);
    h.act(Action::RecentFiles);
    assert_eq!(
        h.picker_labels(),
        ["b.rs"],
        "closing does not forget a file"
    );
    h.run(Command::Newline);
    assert_eq!(h.active_name(), "b.rs", "and it opens again");
}

#[test]
fn recent_files_survive_a_restart_even_without_a_tab() {
    let mut h = Harness::new(&[("a.rs", "a"), ("b.rs", "b")]);
    h.open("a.rs");
    h.open("b.rs");
    h.act(Action::CloseTab);
    h.open("b.rs");
    let mut h = h.restart();
    h.act(Action::RecentFiles);
    assert_eq!(h.picker_labels(), ["a.rs"]);
}

#[test]
fn recent_files_are_filtered_like_file_names() {
    let mut h = Harness::new(&[("src/alpha.rs", "a"), ("src/beta.rs", "b"), ("c.rs", "c")]);
    h.open("src/alpha.rs");
    h.open("src/beta.rs");
    h.open("c.rs");
    h.act(Action::RecentFiles);
    h.typed("alp");
    assert_eq!(h.picker_labels(), ["src/alpha.rs"]);
}

#[test]
fn search_everywhere_finds_files_and_actions_in_one_list() {
    let mut h = Harness::new(&[("save_game.rs", "x")]);
    indexed(&mut h, &["save_game.rs", "other.rs"]);
    h.act(Action::SearchEverywhere);
    h.typed("save");
    let labels = h.picker_labels();
    assert!(labels.contains(&"save_game.rs".to_owned()), "{labels:?}");
    assert!(labels.contains(&"Save".to_owned()), "{labels:?}");
    assert!(!labels.contains(&"other.rs".to_owned()), "{labels:?}");
}

#[test]
fn search_everywhere_opens_a_file_or_runs_an_action() {
    let mut h = Harness::new(&[("alpha.rs", "text")]);
    indexed(&mut h, &["alpha.rs"]);
    h.act(Action::SearchEverywhere);
    h.typed("alpha");
    h.run(Command::Newline);
    assert_eq!(h.active_name(), "alpha.rs");
    assert!(h.ws.picker().is_none());

    h.act(Action::SearchEverywhere);
    h.typed("Split Vertically");
    let first = h.picker_labels()[0].clone();
    assert_eq!(first, "Split Vertically");
    h.run(Command::Newline);
    assert_eq!(h.ws.layout().panes().len(), 2);
}

#[test]
fn search_everywhere_starts_with_recent_files_without_repeating_them() {
    let mut h = Harness::new(&[("a.rs", "a"), ("b.rs", "b")]);
    indexed(&mut h, &["a.rs", "b.rs", "z.rs"]);
    h.open("a.rs");
    h.open("b.rs");
    h.act(Action::SearchEverywhere);
    let labels = h.picker_labels();
    assert_eq!(labels[..3], ["a.rs", "b.rs", "z.rs"]);
    assert_eq!(labels.iter().filter(|l| *l == "a.rs").count(), 1);
    assert_eq!(labels.len(), 3 + Action::ALL.len());
    assert_eq!(labels[3], Action::ALL[0].spec().title);
}

#[test]
fn search_everywhere_is_in_the_palette() {
    let mut h = Harness::new(&[]);
    h.act(Action::OpenPalette);
    h.typed("Search Everywhere");
    assert_eq!(h.picker_labels()[0], "Search Everywhere");
    h.run(Command::Newline);
    assert_eq!(h.ws.picker().map(|p| p.kind), Some(PickerKind::Everywhere));
}

#[test]
fn tab_key_and_shifting_lines_use_the_configured_width() {
    let mut h = Harness::new(&[("a.txt", "one\ntwo\n")]);
    h.ws.configure_editor(2, false);
    h.open("a.txt");
    h.run(Command::Indent);
    assert_eq!(h.active_text(), "  one\ntwo\n");
    h.run(Command::Outdent);
    assert_eq!(h.active_text(), "one\ntwo\n");
    h.act(Action::SelectAll);
    h.run(Command::Indent);
    assert_eq!(h.active_text(), "  one\n  two\n");
}

#[test]
fn vertical_moves_and_clicks_count_columns_with_the_configured_width() {
    let mut h = Harness::new(&[("a.txt", "\tx\nabcdefgh\n")]);
    h.ws.configure_editor(2, false);
    h.open("a.txt");
    h.run(Command::Move(Motion::Right));
    h.run(Command::Move(Motion::Down));
    assert_eq!(h.cursor(), 3 + 2, "the tab is 2 columns wide");
    h.run(Command::SelectTo { line: 0, col: 2 });
    assert_eq!(h.cursor(), 1);
}

#[test]
fn formatting_asks_the_server_for_the_configured_width() {
    let mut h = Harness::new(&[("a.rs", "fn a() {}\n")]);
    h.ws.configure_editor(3, false);
    h.ready("a.rs");
    h.act(Action::ReformatCode);
    assert!(
        h.lsp.iter().any(|e| matches!(
            e,
            Effect::Lsp(LspEffect::Query {
                query: LspQuery::Formatting { tab_size: 3 },
                ..
            })
        )),
        "{:?}",
        h.lsp
    );
}
