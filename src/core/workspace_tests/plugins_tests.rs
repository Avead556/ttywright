use super::*;

#[test]
fn a_plugin_command_is_in_the_palette_and_is_sent_with_the_document() {
    let mut h = Harness::new(&[("a.rs", "abc")]);
    h.open("a.rs");
    h.with_plugin();
    let items = h.ws.picker_items(PickerKind::Actions);
    assert_eq!(items.last().map(String::as_str), Some("Plugin: Say Hello"));
    let Some(Effect::RunPlugin {
        plugin,
        command,
        context,
        ..
    }) = h.run_plugin()
    else {
        panic!("not asked");
    };
    assert_eq!((plugin.as_str(), command.as_str()), ("hello", "say"));
    assert_eq!(context.text.as_deref(), Some("abc"));
}

#[test]
fn what_a_plugin_answers_is_applied_as_ordinary_input() {
    let mut h = Harness::new(&[("a.rs", "abc")]);
    h.open("a.rs");
    h.with_plugin();
    h.run_plugin();
    h.plugin_done(Ok(report(vec![Command::Insert("hi".to_owned())])));
    assert_eq!(h.text_of("a.rs").as_deref(), Some("hiabc"));
    assert!(
        status_text(&h).contains("Said hello"),
        "{}",
        status_text(&h)
    );
    h.act(Action::Undo);
    assert_eq!(h.text_of("a.rs").as_deref(), Some("abc"));
}

#[test]
fn a_plugin_answer_about_older_text_changes_nothing() {
    let mut h = Harness::new(&[("a.rs", "abc")]);
    h.open("a.rs");
    h.with_plugin();
    h.run_plugin();
    h.run(Command::Insert("x".to_owned()));
    h.plugin_done(Ok(report(vec![Command::Insert("hi".to_owned())])));
    assert_eq!(h.text_of("a.rs").as_deref(), Some("xabc"));
    assert!(status_text(&h).contains("changed"), "{}", status_text(&h));
}

#[test]
fn a_plugin_failure_is_shown_and_it_can_run_again() {
    let mut h = Harness::new(&[("a.rs", "abc")]);
    h.open("a.rs");
    h.with_plugin();
    h.run_plugin();
    h.lsp.clear();
    assert!(h.run_plugin().is_none(), "one run of a plugin at a time");
    h.plugin_done(Err("crashed: boom".to_owned()));
    assert!(
        status_text(&h).contains("hello: crashed: boom"),
        "{}",
        status_text(&h)
    );
    assert!(h.run_plugin().is_some());
}

#[test]
fn a_plugin_can_send_any_command_with_or_without_a_document() {
    let mut h = Harness::new(&[("a.rs", "abc")]);
    h.with_plugin();
    h.run_plugin();
    h.plugin_done(Ok(report(vec![Command::Run(Action::ShowProblems)])));
    assert_eq!(h.ws.focus(), Focus::Problems, "no document was needed");
    assert!(status_text(&h).contains("Said hello"));
}

#[test]
fn an_action_still_applies_after_the_text_moved_on_but_typing_does_not() {
    let mut h = Harness::new(&[("a.rs", "abc")]);
    h.open("a.rs");
    h.with_plugin();
    h.run_plugin();
    h.run(Command::Insert("x".to_owned()));
    h.plugin_done(Ok(report(vec![Command::Run(Action::ShowProblems)])));
    assert_eq!(h.ws.focus(), Focus::Problems);

    let mut h = Harness::new(&[("a.rs", "abc")]);
    h.open("a.rs");
    h.with_plugin();
    h.run_plugin();
    h.run(Command::Insert("x".to_owned()));
    h.plugin_done(Ok(report(vec![
        Command::Run(Action::ShowProblems),
        Command::Insert("late".to_owned()),
    ])));
    assert_eq!(h.text_of("a.rs").as_deref(), Some("xabc"));
    assert_ne!(h.ws.focus(), Focus::Problems, "all of it or none of it");
}

#[test]
fn a_plugin_quit_saves_first_and_nothing_after_it_runs() {
    let mut h = Harness::new(&[("a.rs", "abc")]);
    h.open("a.rs");
    h.with_plugin();
    h.run_plugin();
    h.plugin_done(Ok(report(vec![
        Command::Insert("hi".to_owned()),
        Command::Run(Action::Quit),
        Command::Insert("after".to_owned()),
    ])));
    assert_eq!(h.text_of("a.rs").as_deref(), Some("hiabc"));
    assert!(h.ws.should_quit(), "the save went through and it quit");
    assert_eq!(h.written.len(), 1, "saved on the way out");
}

#[test]
fn control_characters_from_a_plugin_never_reach_the_screen() {
    let mut h = Harness::new(&[("a.rs", "abc")]);
    h.open("a.rs");
    h.with_plugin();
    h.run_plugin();
    h.plugin_done(Err("bad\u{1b}[2Jthing".to_owned()));
    assert!(!status_text(&h).contains('\u{1b}'));
}

#[test]
fn problems_loading_plugins_are_shown() {
    let mut h = Harness::new(&[("a.rs", "abc")]);
    h.lsp_event(Event::PluginsLoaded {
        plugins: Vec::new(),
        problems: vec!["p: broken manifest".to_owned()],
    });
    assert!(status_text(&h).contains("broken"), "{}", status_text(&h));
}
