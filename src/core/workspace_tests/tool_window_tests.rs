use super::*;

#[test]
fn the_tool_window_grows_and_shrinks_within_its_limits_by_steps() {
    let mut h = Harness::new(&[]);
    h.act(Action::ShowProblems);
    h.run(Command::ToolRoom {
        rows: 8,
        min: 5,
        max: 13,
    });
    assert_eq!(
        h.ws.tool_height(),
        None,
        "default until the user says otherwise"
    );
    h.act(Action::ToolWindowTaller);
    assert_eq!(h.ws.tool_height(), Some(10));
    h.act(Action::ToolWindowTaller);
    h.act(Action::ToolWindowTaller);
    assert_eq!(
        h.ws.tool_height(),
        Some(13),
        "two presses before a frame, then the top"
    );
    h.act(Action::ToolWindowTaller);
    assert_eq!(h.ws.tool_height(), Some(13));
    assert!(h.ws.status().unwrap().text.contains("largest"));
    for _ in 0..10 {
        h.act(Action::ToolWindowShorter);
    }
    assert_eq!(h.ws.tool_height(), Some(5));
    assert!(h.ws.status().unwrap().text.contains("smallest"));
}

#[test]
fn resizing_a_hidden_tool_window_shows_it_first() {
    let mut h = Harness::new(&[]);
    h.run(Command::ToolRoom {
        rows: 8,
        min: 5,
        max: 13,
    });
    h.act(Action::ToolWindowTaller);
    assert!(h.ws.tool_window_visible());
    assert_eq!(h.ws.tool_height(), None);
}

#[test]
fn with_no_room_for_a_tool_window_resizing_only_says_so() {
    let mut h = Harness::new(&[]);
    h.act(Action::ShowProblems);
    h.run(Command::ToolRoom {
        rows: 0,
        min: 0,
        max: 0,
    });
    h.act(Action::ToolWindowShorter);
    assert_eq!(h.ws.tool_height(), None);
    assert!(h.ws.status().unwrap().text.contains("too small"));
}

#[test]
fn the_chosen_height_survives_a_restart() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.act(Action::ShowProblems);
    h.run(Command::ToolRoom {
        rows: 8,
        min: 5,
        max: 13,
    });
    h.act(Action::ToolWindowShorter);
    let h = h.restart();
    assert_eq!(h.ws.tool_height(), Some(6));
}

#[test]
fn the_confirm_key_is_the_same_key_in_a_russian_layout() {
    let mut h = Harness::new(&[("a.txt", "foo")]);
    h.open("a.txt");
    h.searched("foo", &["a.txt"]);
    assert!(h.ask_replace("bar").is_empty());
    h.run(Command::Insert("н".to_owned()));
    assert_eq!(h.doc_text("a.txt"), "bar");
}
