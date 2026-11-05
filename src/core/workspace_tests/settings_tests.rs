use super::*;

#[test]
fn turned_off_autosave_leaves_tab_switches_alone_but_quit_still_saves() {
    let mut h = Harness::new(&[("a.rs", ""), ("b.rs", "")]);
    h.ws.configure(false, None);
    h.open("a.rs");
    h.typed("x");
    h.open("b.rs");
    assert!(h.written.is_empty(), "no write on switching");
    h.act(Action::Quit);
    assert_eq!(h.written.len(), 1, "quitting never loses edits");
}

#[test]
fn settings_open_as_a_file_and_say_when_they_apply() {
    let mut h = Harness::new(&[]);
    h.ws.configure(true, Some(PathBuf::from("/conf/config.toml")));
    h.act(Action::OpenSettings);
    let path = h.ws.active().map(|(_, d)| d.path().to_path_buf());
    assert_eq!(path, Some(PathBuf::from("/conf/config.toml")));
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("Settings apply after a restart")
    );
}

#[test]
fn settings_problems_are_shown_with_how_many_more() {
    let mut h = Harness::new(&[]);
    h.lsp_event(Event::ConfigProblems {
        messages: vec!["one".into(), "two".into(), "three".into()],
    });
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("one (and 2 more)")
    );
    let before = h.ws.status().cloned();
    h.lsp_event(Event::ConfigProblems {
        messages: Vec::new(),
    });
    assert_eq!(h.ws.status().cloned(), before);
}
