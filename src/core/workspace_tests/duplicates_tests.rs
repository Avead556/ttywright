use super::*;

#[test]
fn locate_duplicates_lists_groups_and_enter_goes_to_the_place() {
    let mut h = Harness::new(&[("a.rs", "x\n"), ("b.rs", "y\nz\n")]);
    h.act(Action::LocateDuplicates);
    assert_eq!(h.ws.tool_tab(), ToolTab::Duplicates);
    let (id, anonymize) = h.duplicates_sent().expect("asked");
    assert!(!anonymize);
    h.lsp_event(Event::Duplicates(crate::core::DuplicatesEvent::Finished {
        id,
        report: two_places(),
    }));
    assert_eq!(h.ws.duplicates_panel().row_count(), 3);
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    h.run(Command::Newline);
    assert_eq!(h.active_name(), "b.rs");
    assert_eq!(h.ws.focus(), Focus::Editor);
}

#[test]
fn a_late_answer_of_an_older_scan_is_dropped_and_stop_stops() {
    let mut h = Harness::new(&[("a.rs", "x\n")]);
    h.act(Action::LocateDuplicates);
    let (first, _) = h.duplicates_sent().expect("asked");
    h.run(Command::Insert("a".to_owned()));
    let (second, anonymize) = h.duplicates_sent().expect("asked again");
    assert!(anonymize, "names may differ now");
    h.lsp_event(Event::Duplicates(crate::core::DuplicatesEvent::Finished {
        id: first,
        report: two_places(),
    }));
    assert_eq!(h.ws.duplicates_panel().row_count(), 0);
    h.run(Command::Insert("s".to_owned()));
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Duplicates(crate::core::DuplicatesEffect::Stop)))
    );
    h.lsp_event(Event::Duplicates(crate::core::DuplicatesEvent::Finished {
        id: second,
        report: two_places(),
    }));
    assert_eq!(
        h.ws.duplicates_panel().row_count(),
        0,
        "stopped stays stopped"
    );
}
