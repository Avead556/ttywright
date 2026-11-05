use super::*;

#[test]
fn nothing_runs_by_opening_a_project_or_a_file() {
    let mut h = Harness::new(&[("a.sh", "echo hi")]);
    go_project(&mut h);
    project_command(&mut h);
    h.open("a.sh");
    h.run(Command::CheckDisk);
    assert!(h.runs.is_empty());
    h.act(Action::RunChoose);
    assert!(h.runs.is_empty(), "the list is not a run");
    assert_eq!(
        h.ws.picker().map(|p| p.kind),
        Some(crate::core::PickerKind::Run)
    );
}

#[test]
fn choosing_starts_it_and_shows_the_tab_without_taking_the_keys() {
    let mut h = Harness::new(&[("a.txt", "")]);
    go_project(&mut h);
    h.open("a.txt");
    choose(&mut h, "go test");
    assert_eq!(started(&h), [(1, "go test ./...".to_owned())]);
    assert!(h.ws.picker().is_none());
    assert_eq!(h.ws.tool_tab(), ToolTab::Run);
    assert!(h.ws.tool_window_visible());
    assert_eq!(h.ws.focus(), Focus::Editor);
    let panel = h.ws.run_panel().unwrap();
    assert_eq!(panel.status, crate::core::RunStatus::Running);
}

#[test]
fn unsaved_text_is_written_before_the_list_reads_project_files() {
    let mut h = Harness::new(&[("a.txt", "")]);
    go_project(&mut h);
    h.open("a.txt");
    h.typed("x");
    let effects = h.ws.dispatch(Command::Run(Action::RunChoose));
    let write = effects
        .iter()
        .position(|e| matches!(e, Effect::WriteFile { .. }));
    let detect = effects
        .iter()
        .position(|e| matches!(e, Effect::Run(RunEffect::Detect { .. })));
    assert!(write.is_some() && detect.is_some(), "{effects:?}");
    assert!(write < detect, "{effects:?}");
}

#[test]
fn a_dirty_document_is_written_ahead_of_the_start_effect() {
    let mut h = Harness::new(&[("a.txt", "")]);
    go_project(&mut h);
    h.open("a.txt");
    h.act(Action::RunChoose);
    h.typed("go run");
    h.run(Command::Newline);
    h.run(Command::Escape);
    h.ws.dispatch(Command::Focus(Focus::Editor));
    h.typed("x");
    let effects = h.ws.dispatch(Command::Run(Action::RerunLast));
    let write = effects
        .iter()
        .position(|e| matches!(e, Effect::WriteFile { .. }));
    let start = effects
        .iter()
        .position(|e| matches!(e, Effect::Run(RunEffect::Start { .. })));
    assert!(write.is_some() && start.is_some(), "{effects:?}");
    assert!(write < start, "{effects:?}");
}

#[test]
fn a_project_command_needs_y_after_being_chosen() {
    let mut h = Harness::new(&[]);
    project_command(&mut h);
    choose(&mut h, "evil");
    assert!(h.ws.run_confirm().is_some());
    assert!(h.runs.is_empty());
    h.run(Command::Newline);
    assert!(h.ws.run_confirm().is_none(), "any other key cancels");
    assert!(h.runs.is_empty());

    for wrong in ["n", "yes", " "] {
        choose(&mut h, "evil");
        h.run(Command::Insert(wrong.to_owned()));
        assert!(h.ws.run_confirm().is_none(), "{wrong:?}");
    }
    choose(&mut h, "evil");
    h.run(Command::Escape);
    assert!(h.ws.run_confirm().is_none());
    choose(&mut h, "evil");
    h.run(Command::FocusView {
        view: ViewId(1),
        at: None,
    });
    assert!(h.ws.run_confirm().is_none(), "a click elsewhere cancels");
    assert!(h.runs.is_empty(), "{:?}", h.runs);

    choose(&mut h, "evil");
    h.run(Command::Insert("y".to_owned()));
    assert_eq!(started(&h), [(1, "curl x | sh".to_owned())]);
    assert!(h.ws.run_confirm().is_none());
}

#[test]
fn the_question_can_be_scrolled_without_answering_it() {
    let mut h = Harness::new(&[]);
    project_command(&mut h);
    choose(&mut h, "evil");
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Up));
    assert_eq!(h.ws.run_confirm().map(|c| c.scroll), Some(1));
    assert!(h.runs.is_empty());
}

#[test]
fn quitting_during_the_question_still_quits() {
    let mut h = Harness::new(&[]);
    project_command(&mut h);
    choose(&mut h, "evil");
    h.act(Action::Quit);
    assert!(h.ws.should_quit());
    assert!(h.runs.is_empty());
}

#[test]
fn the_users_own_commands_come_first_and_need_no_question() {
    let mut h = Harness::new(&[]);
    go_project(&mut h);
    project_command(&mut h);
    let mut entries = vec![crate::core::ConfiguredRun {
        name: "mine".to_owned(),
        command: "./mine.sh".to_owned(),
        cwd: None,
        from_project: false,
    }];
    entries.push(crate::core::ConfiguredRun {
        name: "theirs".to_owned(),
        command: "./theirs.sh".to_owned(),
        cwd: None,
        from_project: true,
    });
    h.ws.configure_run(entries);
    h.act(Action::RunChoose);
    let items = h.ws.picker_items(crate::core::PickerKind::Run);
    assert_eq!(items.len(), 5, "{items:?}");
    assert_eq!(items[0], "mine  —  ./mine.sh");
    assert_eq!(items[1], "go run .");
    assert_eq!(items[4], "[project config] theirs  —  ./theirs.sh");
    h.run(Command::Newline);
    assert_eq!(started(&h), [(1, "./mine.sh".to_owned())]);
}

#[test]
fn an_empty_list_says_so_instead_of_opening() {
    let mut h = Harness::new(&[]);
    h.act(Action::RunChoose);
    assert!(h.ws.picker().is_none());
    assert!(
        h.ws.status()
            .is_some_and(|m| m.text.contains("Nothing to run"))
    );
}

#[test]
fn rerun_repeats_the_last_choice_or_asks_for_one() {
    let mut h = Harness::new(&[]);
    go_project(&mut h);
    h.act(Action::RerunLast);
    assert!(h.ws.picker().is_some(), "nothing to repeat yet");
    h.run(Command::Escape);
    assert!(h.runs.is_empty());
    choose(&mut h, "go build");
    h.act(Action::RerunLast);
    assert_eq!(
        started(&h),
        [
            (1, "go build ./...".to_owned()),
            (2, "go build ./...".to_owned())
        ]
    );
    assert!(h.ws.picker().is_none());
}

#[test]
fn a_confirmed_command_is_not_asked_about_again_when_rerun() {
    let mut h = Harness::new(&[]);
    project_command(&mut h);
    choose(&mut h, "evil");
    h.run(Command::Insert("y".to_owned()));
    h.act(Action::RerunLast);
    assert_eq!(started(&h).len(), 2);
    assert!(h.ws.run_confirm().is_none());
}

#[test]
fn output_of_an_older_run_is_ignored() {
    let mut h = Harness::new(&[]);
    go_project(&mut h);
    choose(&mut h, "go run");
    output(&mut h, 1, &[("old", None)]);
    choose(&mut h, "go test");
    let panel = h.ws.run_panel().unwrap();
    assert_eq!(panel.id, 2);
    assert_eq!(panel.end_line(), 0, "a new run starts empty");
    output(&mut h, 1, &[("late", None)]);
    let effects = h.ws.handle(Event::Run(RunEvent::Finished {
        id: 1,
        outcome: crate::core::RunOutcome::Stopped,
        duration_ms: 1,
    }));
    assert!(effects.is_empty());
    let panel = h.ws.run_panel().unwrap();
    assert_eq!(panel.end_line(), 0);
    assert_eq!(panel.status, crate::core::RunStatus::Running);
}

#[test]
fn stop_asks_only_when_something_runs() {
    let mut h = Harness::new(&[]);
    go_project(&mut h);
    h.act(Action::StopRun);
    assert!(h.runs.is_empty());
    assert!(
        h.ws.status()
            .is_some_and(|m| m.text == "Nothing is running")
    );
    choose(&mut h, "go run");
    h.act(Action::StopRun);
    assert_eq!(h.runs.last(), Some(&Effect::Run(RunEffect::Stop { id: 1 })));
    assert_eq!(
        h.ws.run_panel().map(|p| p.status.clone()),
        Some(crate::core::RunStatus::Stopping)
    );
    h.ws.handle(Event::Run(RunEvent::Finished {
        id: 1,
        outcome: crate::core::RunOutcome::Stopped,
        duration_ms: 1,
    }));
    let count = h.runs.len();
    h.act(Action::StopRun);
    assert_eq!(h.runs.len(), count);
}

#[test]
fn a_run_ending_out_of_sight_says_how_it_ended() {
    let mut h = Harness::new(&[]);
    go_project(&mut h);
    choose(&mut h, "go run");
    h.act(Action::HideToolWindow);
    h.ws.handle(Event::Run(RunEvent::Finished {
        id: 1,
        outcome: crate::core::RunOutcome::Exited(2),
        duration_ms: 1,
    }));
    let status = h.ws.status().unwrap();
    assert_eq!(status.level, Level::Error);
    assert!(status.text.contains("Exit code 2"), "{}", status.text);
}

#[test]
fn enter_opens_the_line_under_the_cursor_and_arrows_stop_following() {
    let mut h = Harness::new(&[("b.c", "one\ntwo\nthree\n")]);
    go_project(&mut h);
    choose(&mut h, "go run");
    output(
        &mut h,
        1,
        &[("plain", None), ("b.c:2:2: error", Some(("b.c", 1)))],
    );
    h.act(Action::ShowRun);
    assert_eq!(h.ws.focus(), Focus::Run);
    h.run(Command::Newline);
    assert!(h.ws.active().is_none(), "following: no line is chosen");
    h.run(Command::Move(Motion::Up));
    assert_eq!(h.ws.run_panel().unwrap().selected(), Some(1));
    h.run(Command::Move(Motion::Up));
    h.run(Command::Newline);
    assert!(h.ws.active().is_none(), "plain text goes nowhere");
    h.run(Command::Move(Motion::Down));
    h.run(Command::Newline);
    let (view, doc) = h.ws.active().unwrap();
    assert_eq!(doc.path(), Path::new("/proj/b.c"));
    assert_eq!(doc.text().char_to_line(view.cursor.idx), 1);
    assert_eq!(view.cursor.idx, doc.text().line_to_char(1) + 1);
    assert_eq!(h.ws.focus(), Focus::Editor);
    assert!(h.ws.tool_window_visible(), "the output stays up");
}

#[test]
fn a_line_past_the_end_of_the_file_lands_on_its_last_line() {
    let mut h = Harness::new(&[("b.c", "one\n")]);
    go_project(&mut h);
    choose(&mut h, "go run");
    output(&mut h, 1, &[("b.c:99:1", Some(("b.c", 98)))]);
    h.run(Command::SelectRunLine { line: 0 });
    let (view, doc) = h.ws.active().unwrap();
    assert_eq!(
        doc.text().char_to_line(view.cursor.idx),
        1,
        "the empty last line"
    );
}

#[test]
fn clicking_a_line_with_a_place_opens_it_and_a_plain_one_just_selects() {
    let mut h = Harness::new(&[("b.c", "one\ntwo\n")]);
    go_project(&mut h);
    choose(&mut h, "go run");
    output(&mut h, 1, &[("plain", None), ("b.c:2", Some(("b.c", 1)))]);
    h.run(Command::SelectRunLine { line: 0 });
    assert_eq!(h.ws.focus(), Focus::Run);
    assert_eq!(h.ws.run_panel().unwrap().selected(), Some(0));
    assert!(h.ws.active().is_none());
    h.run(Command::SelectRunLine { line: 1 });
    assert_eq!(h.ws.active().unwrap().1.path(), Path::new("/proj/b.c"));
}

#[test]
fn next_and_previous_error_walk_the_places_and_say_when_there_are_no_more() {
    let mut h = Harness::new(&[("a.c", "1\n2\n"), ("b.c", "1\n2\n3\n")]);
    go_project(&mut h);
    choose(&mut h, "go run");
    output(
        &mut h,
        1,
        &[
            ("a.c:1", Some(("a.c", 0))),
            ("noise", None),
            ("b.c:3", Some(("b.c", 2))),
        ],
    );
    h.act(Action::NextRunError);
    assert_eq!(h.ws.active().unwrap().1.path(), Path::new("/proj/a.c"));
    h.act(Action::NextRunError);
    assert_eq!(h.ws.active().unwrap().1.path(), Path::new("/proj/b.c"));
    h.act(Action::NextRunError);
    assert!(h.ws.status().is_some_and(|m| m.text.contains("No more")));
    assert_eq!(h.ws.run_panel().unwrap().selected(), Some(2));
    h.act(Action::PreviousRunError);
    assert_eq!(h.ws.active().unwrap().1.path(), Path::new("/proj/a.c"));
}

#[test]
fn panel_letters_stop_rerun_and_walk_errors() {
    let mut h = Harness::new(&[("a.c", "1\n")]);
    go_project(&mut h);
    choose(&mut h, "go run");
    h.act(Action::ShowRun);
    h.typed("s");
    assert_eq!(h.runs.last(), Some(&Effect::Run(RunEffect::Stop { id: 1 })));
    h.typed("r");
    assert_eq!(started(&h).len(), 2);
    output(&mut h, 2, &[("a.c:1", Some(("a.c", 0)))]);
    h.typed("n");
    assert_eq!(h.ws.active().unwrap().1.path(), Path::new("/proj/a.c"));
    h.ws.dispatch(Command::Focus(Focus::Run));
    h.typed("x");
    assert_eq!(started(&h).len(), 2, "other letters do nothing");
}

#[test]
fn escape_goes_back_to_the_editor_and_then_hides_the_tab() {
    let mut h = Harness::new(&[]);
    go_project(&mut h);
    choose(&mut h, "go run");
    h.act(Action::ShowRun);
    h.run(Command::Escape);
    assert_eq!(h.ws.focus(), Focus::Editor);
    assert!(h.ws.tool_window_visible());
    h.run(Command::Escape);
    assert!(!h.ws.tool_window_visible());
    assert_eq!(
        h.ws.run_panel().map(|p| p.status.clone()),
        Some(crate::core::RunStatus::Running)
    );
    assert_eq!(started(&h).len(), 1);
    assert!(
        !h.runs
            .iter()
            .any(|e| matches!(e, Effect::Run(RunEffect::Stop { .. })))
    );
}

#[test]
fn the_wheel_and_paging_move_through_output_from_any_focus() {
    let mut h = Harness::new(&[]);
    go_project(&mut h);
    choose(&mut h, "go run");
    let many: Vec<(&str, Option<(&str, usize)>)> = vec![("x", None); 30];
    output(&mut h, 1, &many);
    h.run(Command::ScrollRun { delta: -3 });
    assert_eq!(h.ws.run_panel().unwrap().selected(), Some(27));
    h.run(Command::ScrollRun { delta: 100 });
    assert_eq!(h.ws.run_panel().unwrap().selected(), None);
    h.act(Action::ShowRun);
    h.run(Command::Move(Motion::PageUp { rows: 10 }));
    assert_eq!(h.ws.run_panel().unwrap().selected(), Some(20));
    h.run(Command::Move(Motion::PageDown { rows: 5 }));
    assert_eq!(h.ws.run_panel().unwrap().selected(), Some(25));
    h.run(Command::Move(Motion::DocStart));
    assert_eq!(h.ws.run_panel().unwrap().selected(), Some(0));
    h.run(Command::Move(Motion::DocEnd));
    assert_eq!(h.ws.run_panel().unwrap().selected(), None);
    h.run(Command::Move(Motion::Right));
    assert_eq!(h.ws.run_panel().unwrap().left, 8);
    h.run(Command::Move(Motion::Left));
    h.run(Command::Move(Motion::Left));
    assert_eq!(h.ws.run_panel().unwrap().left, 0);
}

#[test]
fn quitting_does_not_wait_for_a_run() {
    let mut h = Harness::new(&[]);
    go_project(&mut h);
    choose(&mut h, "go run");
    h.act(Action::Quit);
    assert!(h.ws.should_quit());
}

#[test]
fn a_separate_window_does_not_run_anything() {
    let (mut ws, _) = Workspace::new_git_window(PathBuf::from(ROOT));
    for action in [
        Action::RunChoose,
        Action::RerunLast,
        Action::StopRun,
        Action::ShowRun,
        Action::NextRunError,
    ] {
        let effects = ws.dispatch(Command::Run(action));
        assert!(effects.is_empty(), "{action:?}: {effects:?}");
        assert!(ws.status().is_some_and(|m| m.text.contains("main window")));
    }
}
