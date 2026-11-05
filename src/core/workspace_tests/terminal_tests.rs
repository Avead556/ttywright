use super::*;

#[test]
fn a_shell_starts_once_the_panel_is_measured_and_each_tab_has_its_own() {
    let mut h = Harness::new(&[]);
    h.act(Action::TerminalToggle);
    assert_eq!(h.ws.focus(), Focus::Terminal);
    assert_eq!(h.ws.tool_tab(), ToolTab::Terminal);
    assert!(h.terminal_starts().is_empty());
    h.run(Command::TerminalSize {
        rows: 10,
        columns: 80,
    });
    assert_eq!(h.terminal_starts().len(), 1);
    h.act(Action::TerminalNew);
    assert_eq!(h.terminal_starts().len(), 2, "the size is known by now");
    assert_eq!(h.terminal_titles(), ["Local", "Local (2)"]);
    assert_eq!(h.ws.terminal_active(), 1);
    h.run(Command::TerminalSize {
        rows: 12,
        columns: 80,
    });
    let resized = h
        .git
        .iter()
        .filter(|e| matches!(e, Effect::Terminal(TerminalEffect::Resize { rows: 12, .. })))
        .count();
    assert_eq!(resized, 2, "every tab, also those out of sight");
}

#[test]
fn keys_and_output_go_to_their_own_tab() {
    let mut h = Harness::new(&[]);
    h.act(Action::TerminalToggle);
    h.run(Command::TerminalSize {
        rows: 5,
        columns: 40,
    });
    h.act(Action::TerminalNew);
    let ids = h.terminal_starts();
    h.lsp_event(Event::Terminal(TerminalEvent::Output {
        id: ids[0],
        bytes: b"first".to_vec(),
    }));
    h.lsp_event(Event::Terminal(TerminalEvent::Output {
        id: ids[1],
        bytes: b"second".to_vec(),
    }));
    let contents = |h: &Harness, i: usize| h.ws.terminal_tabs()[i].screen.screen().contents();
    assert!(contents(&h, 0).starts_with("first"));
    assert!(contents(&h, 1).starts_with("second"));
    h.run(Command::TerminalInput(b"x".to_vec()));
    let wrote = h.git.iter().rev().find_map(|e| match e {
        Effect::Terminal(TerminalEffect::Write { id, bytes }) => Some((*id, bytes.clone())),
        _ => None,
    });
    assert_eq!(wrote, Some((ids[1], b"x".to_vec())));
    h.act(Action::TerminalPrevious);
    assert_eq!(h.ws.terminal_active(), 0);
    h.act(Action::TerminalPrevious);
    assert_eq!(h.ws.terminal_active(), 1, "goes round");
}

#[test]
fn the_toggle_gives_the_keys_back_and_the_panel_stays() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.open("a.rs");
    h.act(Action::TerminalToggle);
    h.run(Command::TerminalSize {
        rows: 5,
        columns: 40,
    });
    h.act(Action::TerminalToggle);
    assert_eq!(h.ws.focus(), Focus::Editor);
    assert!(h.ws.tool_window_visible());
    h.act(Action::TerminalToggle);
    assert_eq!(h.ws.focus(), Focus::Terminal);
    assert_eq!(h.terminal_starts().len(), 1, "the same shell");
    assert_eq!(h.active_text(), "x");
}

#[test]
fn a_shell_exiting_closes_its_tab_and_the_last_one_the_panel() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.open("a.rs");
    h.act(Action::TerminalToggle);
    h.run(Command::TerminalSize {
        rows: 5,
        columns: 40,
    });
    h.act(Action::TerminalNew);
    h.act(Action::TerminalNew);
    let ids = h.terminal_starts();
    h.run(Command::SelectTerminalTab { index: 2 });
    h.lsp_event(Event::Terminal(TerminalEvent::Ended {
        id: ids[0],
        failure: None,
    }));
    assert_eq!(h.terminal_titles(), ["Local (2)", "Local (3)"]);
    assert_eq!(h.ws.terminal_active(), 1, "still on the same tab");
    h.run(Command::CloseTerminalTab { index: 1 });
    assert_eq!(h.ws.terminal_active(), 0);
    h.act(Action::TerminalCloseTab);
    assert!(h.ws.terminal_tabs().is_empty());
    assert!(!h.ws.tool_window_visible());
    assert_eq!(h.ws.focus(), Focus::Editor);
    let closed = h
        .git
        .iter()
        .filter(|e| matches!(e, Effect::Terminal(TerminalEffect::Close { .. })))
        .count();
    assert_eq!(closed, 3, "each process goes with its tab");
    h.act(Action::TerminalToggle);
    assert_eq!(h.terminal_titles(), ["Local"], "numbering starts over");
}

#[test]
fn a_shell_that_cannot_start_says_why() {
    let mut h = Harness::new(&[]);
    h.act(Action::TerminalToggle);
    h.run(Command::TerminalSize {
        rows: 5,
        columns: 40,
    });
    let id = h.terminal_starts()[0];
    h.lsp_event(Event::Terminal(TerminalEvent::Ended {
        id,
        failure: Some("Could not start /bin/x".into()),
    }));
    assert!(h.ws.terminal_tabs().is_empty());
    assert!(h.ws.status().is_some_and(|m| m.text.contains("/bin/x")));
}
