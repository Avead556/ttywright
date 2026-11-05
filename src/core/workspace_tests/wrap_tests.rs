use super::*;

#[test]
fn up_and_down_go_by_screen_rows_on_a_wrapped_line() {
    let mut h = Harness::wrapped(&[("a.txt", "abcdefghijkl\nxy")], 4);
    h.run(Command::Move(Motion::Right));
    assert_eq!(h.at_each_press(Motion::Down, 2), [5, 9]);
    assert_eq!(h.at_each_press(Motion::Down, 2), [12, 14]);
    assert_eq!(h.at_each_press(Motion::Up, 4), [12, 9, 5, 1]);
}

#[test]
fn without_wrap_the_same_keys_go_by_lines() {
    let mut h = Harness::new(&[("a.txt", "abcdefghijkl\nxy")]);
    h.run(Command::Viewport { text_cols: 4 });
    h.open("a.txt");
    assert_eq!(h.at_each_press(Motion::Down, 1), [13]);
}

#[test]
fn the_column_is_kept_across_shorter_rows() {
    let mut h = Harness::wrapped(&[("a.txt", "abcdefgh\n\nabcdefgh")], 8);
    for _ in 0..5 {
        h.run(Command::Move(Motion::Right));
    }
    let stops = h.at_each_press(Motion::Down, 4);
    assert_eq!(stops, [8, 9, 15, 18]);
}

#[test]
fn a_shorter_row_stops_on_its_last_char_and_not_on_the_next_row() {
    let mut h = Harness::wrapped(&[("a.txt", "abcdefg\none two three\nx")], 8);
    for _ in 0..6 {
        h.run(Command::Move(Motion::Right));
    }
    h.act(Action::Find);
    h.run(Command::Escape);
    let mut stops = Vec::new();
    for _ in 0..2 {
        h.run(Command::Move(Motion::Down));
        stops.push(h.cursor());
    }
    assert_eq!(
        stops,
        [8 + 6, 8 + 13],
        "same column, then the end of the last row"
    );
}

#[test]
fn page_keys_count_screen_rows() {
    let mut h = Harness::wrapped(&[("a.txt", &"abcd".repeat(10))], 5);
    h.run(Command::Move(Motion::PageDown { rows: 3 }));
    assert_eq!(h.cursor(), 15);
    h.run(Command::Move(Motion::PageUp { rows: 2 }));
    assert_eq!(h.cursor(), 5);
    h.run(Command::Move(Motion::PageUp { rows: 100 }));
    assert_eq!(h.cursor(), 0);
    h.run(Command::Move(Motion::PageDown { rows: 100 }));
    assert_eq!(h.cursor(), 40);
}

#[test]
fn home_goes_to_the_rows_start_first_and_end_to_the_lines_end() {
    let mut h = Harness::wrapped(&[("a.txt", "  abcdefghijkl")], 6);
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Right));
    assert_eq!(h.cursor(), 7);
    let stops = h.at_each_press(Motion::LineStart, 3);
    assert_eq!(stops, [6, 2, 0], "row start, first non-blank, column 0");
    h.run(Command::Move(Motion::LineEnd));
    assert_eq!(h.cursor(), 14);
}

#[test]
fn shift_selects_by_screen_rows_too() {
    let mut h = Harness::wrapped(&[("a.txt", "abcdefgh")], 4);
    h.run(Command::Select(Motion::Down));
    assert_eq!(h.selected().as_deref(), Some("abcd"));
}

#[test]
fn wide_chars_are_never_entered_by_moving_between_rows() {
    let mut h = Harness::wrapped(&[("a.txt", "汉字汉字汉字")], 5);
    h.run(Command::Move(Motion::Right));
    h.run(Command::Move(Motion::Right));
    assert_eq!(h.cursor(), 2);
    assert_eq!(h.at_each_press(Motion::Down, 2), [4, 4], "the last row");
    assert_eq!(h.at_each_press(Motion::Up, 2), [2, 0]);
}

#[test]
fn toggling_wrap_is_an_action_that_says_what_it_did() {
    let mut h = Harness::new(&[("a.txt", "abcdefgh\nx")]);
    h.run(Command::Viewport { text_cols: 4 });
    h.open("a.txt");
    h.act(Action::OpenPalette);
    h.typed("Toggle Soft Wrap");
    assert_eq!(h.picker_labels()[0], "Toggle Soft Wrap");
    h.run(Command::Newline);
    assert!(h.ws.soft_wrap());
    assert_eq!(h.ws.status().unwrap().text, "Soft wrap on");
    assert_eq!(h.at_each_press(Motion::Down, 1), [4], "by rows now");
    h.act(Action::ToggleSoftWrap);
    assert!(!h.ws.soft_wrap());
    assert_eq!(h.at_each_press(Motion::Down, 1), [10], "by lines again");
}

#[test]
fn before_the_first_frame_wrap_falls_back_to_lines_rather_than_guessing() {
    let mut h = Harness::new(&[("a.txt", "abcdefgh\nx")]);
    h.ws.configure_editor(4, true);
    h.open("a.txt");
    assert_eq!(h.at_each_press(Motion::Down, 1), [9]);
}

#[test]
fn the_viewport_report_keeps_messages_and_undo_groups_alone() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.ws.configure_editor(4, true);
    h.act(Action::ToggleSoftWrap);
    let status = h.ws.status().cloned();
    h.run(Command::Viewport { text_cols: 10 });
    assert_eq!(h.ws.status().cloned(), status);
}
