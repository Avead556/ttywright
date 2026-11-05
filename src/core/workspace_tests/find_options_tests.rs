use super::*;

#[test]
fn smart_case_is_the_default_and_toggles_change_matches_at_once() {
    let mut h = Harness::new(&[("a.txt", "foo Foo foobar")]);
    h.open("a.txt");
    h.find("foo");
    assert_eq!(h.matches(), [0..3, 4..7, 8..11]);
    h.act(Action::ToggleFindCase);
    assert_eq!(h.matches(), [0..3, 8..11]);
    h.act(Action::ToggleFindWord);
    assert_eq!(h.matches(), vec![0..3]);
    h.act(Action::ToggleFindCase);
    h.act(Action::ToggleFindWord);
    assert_eq!(h.matches().len(), 3, "and back");
}

#[test]
fn a_regex_toggle_changes_what_the_query_means() {
    let mut h = Harness::new(&[("a.txt", "a1 a.")]);
    h.open("a.txt");
    h.find("a.");
    assert_eq!(h.matches(), vec![3..5], "literally a dot");
    h.act(Action::ToggleFindRegex);
    assert_eq!(h.matches(), [0..2, 3..5]);
}

#[test]
fn an_invalid_regex_shows_a_message_and_no_matches_and_changes_nothing() {
    let mut h = Harness::new(&[("a.txt", "(a) (a)")]);
    h.open("a.txt");
    h.act(Action::Replace);
    h.act(Action::ToggleFindRegex);
    h.typed("(a");
    assert!(h.matches().is_empty());
    assert!(h.ws.find_bar().unwrap().error.is_some());
    h.run(Command::Newline);
    h.act(Action::ReplaceAll);
    assert_eq!(h.ws.status().unwrap().level, Level::Error);
    assert_eq!(h.active_text(), "(a) (a)");
    h.typed(")");
    assert_eq!(h.matches().len(), 2);
    assert_eq!(h.ws.find_bar().unwrap().error, None);
}

#[test]
fn options_are_kept_when_the_bar_closes() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.act(Action::Find);
    h.act(Action::ToggleFindRegex);
    h.run(Command::Escape);
    h.act(Action::Find);
    assert!(h.ws.bar_options().regex);
}

#[test]
fn toggling_with_nothing_to_search_says_so() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.act(Action::ToggleFindRegex);
    assert!(!h.ws.bar_options().regex);
    assert!(h.ws.status().unwrap().text.contains("Open Find"));
}

#[test]
fn replace_all_expands_capture_groups_as_one_undo_step() {
    let mut h = Harness::new(&[("a.txt", "a=1\nb=2\n")]);
    h.open("a.txt");
    h.act(Action::Find);
    h.act(Action::ToggleFindRegex);
    h.run(Command::Escape);
    h.replace_in_bar(r"(\w)=(\d)", "$2=$1");
    h.act(Action::ReplaceAll);
    assert_eq!(h.active_text(), "1=a\n2=b\n");
    assert_eq!(h.ws.status().unwrap().text, "Replaced 2 occurrences");
    h.run(Command::Escape);
    h.act(Action::Undo);
    assert_eq!(h.active_text(), "a=1\nb=2\n");
}

#[test]
fn replace_one_expands_groups_of_the_match_at_the_cursor() {
    let mut h = Harness::new(&[("a.txt", "a=1 b=2")]);
    h.open("a.txt");
    h.act(Action::Find);
    h.act(Action::ToggleFindRegex);
    h.run(Command::Escape);
    h.replace_in_bar(r"(\w)=(\d)", "$2$1");
    h.run(Command::Indent);
    h.run(Command::Newline);
    h.run(Command::Indent);
    if h.cursor() != 0 {
        h.act(Action::FindPrevious);
    }
    assert_eq!(h.cursor(), 0);
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "1a b=2");
    h.run(Command::Newline);
    assert_eq!(h.active_text(), "1a 2b");
}

#[test]
fn whole_word_and_case_are_honoured_by_replace() {
    let mut h = Harness::new(&[("a.txt", "cat Cat catalog")]);
    h.open("a.txt");
    h.act(Action::Find);
    h.act(Action::ToggleFindWord);
    h.act(Action::ToggleFindCase);
    h.run(Command::Escape);
    h.replace_in_bar("cat", "dog");
    h.act(Action::ReplaceAll);
    assert_eq!(h.active_text(), "dog Cat catalog");
}

#[test]
fn a_group_that_does_not_exist_changes_nothing_and_says_why() {
    let mut h = Harness::new(&[("a.txt", "abc")]);
    h.open("a.txt");
    h.act(Action::Find);
    h.act(Action::ToggleFindRegex);
    h.run(Command::Escape);
    h.replace_in_bar("(b)", "$2");
    h.act(Action::ReplaceAll);
    assert_eq!(h.active_text(), "abc");
    let status = h.ws.status().unwrap();
    assert_eq!(status.level, Level::Error);
    assert!(status.text.contains("group 2"), "{}", status.text);
}

#[test]
fn plain_replace_takes_dollars_literally() {
    let mut h = Harness::new(&[("a.txt", "abc")]);
    h.open("a.txt");
    h.replace_in_bar("b", "$1");
    h.act(Action::ReplaceAll);
    assert_eq!(h.active_text(), "a$1c");
}

#[test]
fn find_in_files_sends_its_options_and_reruns_when_they_change() {
    let mut h = Harness::new(&[]);
    h.act(Action::FindInProject);
    h.typed("x+");
    h.act(Action::ToggleFindRegex);
    assert!(h.lsp.is_empty());
    let fx = h.ws.dispatch(Command::Newline);
    let [Effect::SearchProject { query, options, .. }] = fx.as_slice() else {
        panic!("{fx:?}");
    };
    assert_eq!(query, "x+");
    assert!(options.regex);
    let fx = h.ws.dispatch(Command::Run(Action::ToggleFindWord));
    assert!(
        matches!(fx.as_slice(), [Effect::SearchProject { options, .. }] if options.whole_word && options.regex),
        "{fx:?}"
    );
}

#[test]
fn an_invalid_regex_in_find_in_files_never_reaches_the_search() {
    let mut h = Harness::new(&[]);
    h.act(Action::FindInProject);
    h.act(Action::ToggleFindRegex);
    h.typed("(");
    let fx = h.ws.dispatch(Command::Newline);
    assert!(fx.is_empty(), "{fx:?}");
    assert!(h.ws.search().error.is_some());
    assert!(h.ws.search().results.is_empty());
}
