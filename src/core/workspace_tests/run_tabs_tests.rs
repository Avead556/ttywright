use super::*;

#[test]
fn another_target_gets_its_own_tab_and_the_same_one_reuses_its_tab() {
    let mut h = Harness::new(&[]);
    h.run_named("server");
    h.run_named("tests");
    assert_eq!(h.run_tab_names(), ["server", "tests"]);
    assert_eq!(h.ws.run_active(), 1);
    h.run_named("server");
    assert_eq!(h.run_tab_names(), ["server", "tests"]);
    assert_eq!(h.ws.run_active(), 0);
    let starts = h.run_starts();
    assert_eq!(starts[0].1, None);
    assert_eq!(starts[1].1, None, "the server goes on running");
    assert_eq!(starts[2].1, Some(starts[0].0), "only the old server stops");
}

#[test]
fn output_goes_to_its_tab_also_when_another_is_showing() {
    let mut h = Harness::new(&[]);
    h.run_named("a");
    h.run_named("b");
    let starts = h.run_starts();
    h.lsp_event(Event::Run(RunEvent::Output {
        id: starts[0].0,
        lines: Vec::new(),
        partial: "from a".into(),
        dropped: 0,
    }));
    assert_eq!(
        h.ws.run_panel().map(|p| p.partial.clone()),
        Some(String::new())
    );
    h.run(Command::SelectRunTab { index: 0 });
    assert_eq!(
        h.ws.run_panel().map(|p| p.partial.clone()),
        Some("from a".to_owned())
    );
    assert_eq!(h.ws.focus(), Focus::Run);
    h.run(Command::Indent);
    assert_eq!(h.ws.run_active(), 1, "Tab goes to the next");
}

#[test]
fn stop_acts_on_the_tab_showing_and_closing_a_running_tab_stops_it() {
    let mut h = Harness::new(&[]);
    h.run_named("a");
    h.run_named("b");
    let starts = h.run_starts();
    h.act(Action::StopRun);
    let stopped = |h: &Harness| {
        h.runs
            .iter()
            .filter_map(|e| match e {
                Effect::Run(RunEffect::Stop { id }) => Some(*id),
                _ => None,
            })
            .collect::<Vec<_>>()
    };
    assert_eq!(stopped(&h), [starts[1].0]);
    h.run(Command::SelectRunTab { index: 0 });
    h.run(Command::Insert("x".into()));
    assert_eq!(h.run_tab_names(), ["b"]);
    assert_eq!(stopped(&h), [starts[1].0, starts[0].0]);
    h.lsp_event(Event::Run(RunEvent::Finished {
        id: starts[1].0,
        outcome: crate::core::RunOutcome::Stopped,
        duration_ms: 1,
    }));
    let before = stopped(&h).len();
    h.run(Command::Insert("x".into()));
    assert!(h.run_tab_names().is_empty());
    assert_eq!(stopped(&h).len(), before, "already over");
}

#[test]
fn the_test_at_the_cursor_runs_after_saving_with_nothing_more_to_choose() {
    let mut h = Harness::new(&[("t.rs", "#[test]\nfn works() {\n    x();\n}\n")]);
    h.open("t.rs");
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    h.typed("y");
    h.act(Action::RunTest);
    assert!(h.ws.picker().is_none());
    let started = h.run_starts().pop().expect("started");
    assert_eq!(started.2, "Test: works");
    assert!(
        !h.written.is_empty(),
        "the text on screen is what gets tested"
    );
    let target = h.ws.run_panel().map(|p| p.target.command_line());
    assert_eq!(target.as_deref(), Some("cargo test works"));
}

#[test]
fn no_test_there_says_so_and_the_next_run_choice_is_unaffected() {
    let mut h = Harness::new(&[("t.rs", "fn plain() {}\n")]);
    h.open("t.rs");
    h.act(Action::RunTest);
    assert!(h.run_starts().is_empty());
    assert!(h.ws.status().is_some_and(|m| m.text.contains("No test")));
}
