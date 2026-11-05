use super::*;

#[test]
fn nothing_typed_looks_where_tools_write_and_a_name_reads_that_file() {
    for (typed, wanted) in [("", None), ("my.lcov", Some(PathBuf::from("my.lcov")))] {
        let mut h = Harness::new(&[("a.rs", "x")]);
        h.act(Action::LoadCoverage);
        h.typed(typed);
        h.run(Command::Newline);
        let asked = h.git.iter().rev().find_map(|e| match e {
            Effect::Coverage(crate::core::coverage::CoverageEffect::Read { file, .. }) => {
                Some(file.clone())
            }
            _ => None,
        });
        assert_eq!(asked, Some(wanted));
    }
}

#[test]
fn a_loaded_report_marks_lines_says_the_total_and_toggles() {
    let mut h = Harness::new(&[("a.rs", "one\ntwo\nthree\n")]);
    h.open("a.rs");
    assert_eq!(h.active_doc_coverage(0), None);
    h.coverage_loaded(coverage_of("a.rs", &[(1, 3), (2, 0)]));
    assert!(status_text(&h).contains("50%"), "{}", status_text(&h));
    assert_eq!(h.active_doc_coverage(0), Some(true));
    assert_eq!(h.active_doc_coverage(1), Some(false));
    assert_eq!(h.active_doc_coverage(2), None, "not code");
    h.act(Action::ToggleCoverage);
    assert_eq!(h.active_doc_coverage(0), None);
    h.act(Action::ToggleCoverage);
    assert_eq!(h.active_doc_coverage(0), Some(true));
}

#[test]
fn toggling_with_no_report_asks_for_one() {
    let mut h = Harness::new(&[]);
    h.act(Action::ToggleCoverage);
    assert!(matches!(
        h.ws.file_prompt().map(|p| &p.kind),
        Some(crate::core::file_op::FilePromptKind::CoverageReport)
    ));
}

#[test]
fn an_edit_makes_the_marks_stale_instead_of_pointing_at_the_wrong_lines() {
    let mut h = Harness::new(&[("a.rs", "one\ntwo\n")]);
    h.open("a.rs");
    h.coverage_loaded(coverage_of("a.rs", &[(1, 3)]));
    h.run(Command::Newline);
    assert_eq!(h.active_doc_coverage(0), None);
    let (_, d) = h.ws.active().expect("open");
    assert_eq!(
        h.ws.coverage_state(d),
        Some(crate::core::CoverageState::Stale)
    );
}

#[test]
fn a_file_changed_after_the_report_is_stale_even_when_saved() {
    let mut h = Harness::new(&[("a.rs", "one\n")]);
    h.open("a.rs");
    let mut report = coverage_of("a.rs", &[(1, 3)]);
    let disk =
        h.ws.active()
            .and_then(|(_, d)| d.disk())
            .and_then(|s| s.modified);
    let Some(file_time) = disk else {
        report.made = Some(std::time::SystemTime::UNIX_EPOCH);
        h.coverage_loaded(report);
        assert_eq!(h.active_doc_coverage(0), Some(true));
        return;
    };
    report.made = file_time.checked_sub(std::time::Duration::from_secs(1));
    h.coverage_loaded(report);
    assert_eq!(h.active_doc_coverage(0), None);
}

#[test]
fn a_report_with_nothing_for_this_project_is_refused_with_a_reason() {
    let mut h = Harness::new(&[]);
    let report = crate::core::coverage::CoverageReport {
        dropped: 2,
        ..Default::default()
    };
    h.coverage_loaded(report);
    assert!(
        status_text(&h).contains("2 files outside"),
        "{}",
        status_text(&h)
    );
}

#[test]
fn structural_search_is_a_mode_of_find_in_files_and_its_bad_pattern_is_shown() {
    let mut h = Harness::new(&[("a.rs", "fn a() { foo(1); }")]);
    h.act(Action::StructuralSearch);
    assert!(h.ws.search().options.structural);
    assert_eq!(h.ws.focus(), Focus::FindInFiles);
    h.typed("$A$");
    h.run(Command::Newline);
    assert!(h.ws.search().error.is_some(), "one hole matches everything");
    h.act(Action::ToggleStructural);
    assert!(!h.ws.search().options.structural);
}

#[test]
fn structural_replace_in_an_open_file_goes_through_the_preview_as_one_undo() {
    let mut h = Harness::new(&[("a.rs", "fn a() { foo(1, 2); }")]);
    h.open("a.rs");
    h.act(Action::StructuralSearch);
    h.searched("foo($A$, $B$)", &["a.rs"]);
    assert!(h.ask_replace("bar($B$, $A$)").is_empty());
    assert_eq!(h.preview().occurrences(), 1);
    assert_eq!(h.doc_text("a.rs"), "fn a() { foo(1, 2); }");
    h.run(Command::Insert("y".to_owned()));
    assert_eq!(h.doc_text("a.rs"), "fn a() { bar(2, 1); }");
    h.run(Command::Escape);
    h.run(Command::Focus(Focus::Editor));
    h.act(Action::Undo);
    assert_eq!(h.doc_text("a.rs"), "fn a() { foo(1, 2); }");
}
