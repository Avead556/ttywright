use super::*;

#[test]
fn opening_a_sql_file_or_the_tab_sends_nothing_and_a_read_runs_when_asked() {
    let mut h = Harness::sql("SELECT 1; SELECT 2", &["only"]);
    assert_eq!(h.db_sent(Command::Run(Action::ShowDatabase)), None);
    h.ws.dispatch(Command::Focus(Focus::Editor));
    let sent = h.db_sent(Command::Run(Action::DbRun)).expect("sent");
    assert_eq!((sent.1.as_str(), sent.2), ("SELECT 1", true));
    assert_eq!(h.ws.tool_tab(), ToolTab::Database);
}

#[test]
fn a_write_waits_for_y_and_anything_else_cancels_it() {
    let mut h = Harness::sql("DROP TABLE t", &["only"]);
    assert_eq!(h.db_sent(Command::Run(Action::DbRun)), None);
    assert!(h.ws.db_panel().confirm.is_some());
    assert_eq!(h.db_sent(Command::Newline), None, "Enter is not a yes");
    assert!(h.ws.db_panel().confirm.is_none());
    assert_eq!(h.text_of("q.sql").as_deref(), Some("DROP TABLE t"));

    assert_eq!(h.db_sent(Command::Run(Action::DbRun)), None);
    let sent = h.db_sent(Command::Insert("y".to_owned())).expect("sent");
    assert_eq!((sent.1.as_str(), sent.2), ("DROP TABLE t", false));
}

#[test]
fn a_refused_statement_is_never_sent_or_offered() {
    let mut h = Harness::sql(".shell rm -rf /", &["only"]);
    assert_eq!(h.db_sent(Command::Run(Action::DbRun)), None);
    assert!(h.ws.db_panel().confirm.is_none());
    assert!(
        status_text(&h).contains("sqlite3 command"),
        "{}",
        status_text(&h)
    );
}

#[test]
fn a_selection_is_what_runs_and_its_several_statements_ask_first() {
    let mut h = Harness::sql("SELECT 1; SELECT 2", &["only"]);
    for _ in 0..18 {
        h.run(Command::Select(Motion::Right));
    }
    assert_eq!(h.db_sent(Command::Run(Action::DbRun)), None);
    let confirm = h.ws.db_panel().confirm.as_ref().expect("asks");
    assert_eq!(confirm.statement, "SELECT 1; SELECT 2");
}

#[test]
fn with_several_connections_one_must_be_chosen_and_then_is_remembered() {
    let mut h = Harness::sql("SELECT 1", &["a", "b"]);
    assert_eq!(h.db_sent(Command::Run(Action::DbRun)), None);
    assert_eq!(h.ws.focus(), Focus::Database);
    assert!(status_text(&h).contains("Choose a connection"));
    h.run(Command::Move(Motion::Down));
    let (id, listing, read_only) = h.db_sent(Command::Newline).expect("lists tables");
    assert!(listing.contains("sqlite_schema") && read_only);
    assert_eq!(h.ws.db_panel().active, Some(1));
    h.ws.handle(Event::Db(crate::core::db::DbEvent::Finished {
        id,
        outcome: crate::core::db::DbOutcome::Done(crate::core::db::DbGrid {
            columns: vec!["name".to_owned()],
            rows: vec![vec!["users".to_owned()]],
            ..Default::default()
        }),
    }));
    assert_eq!(h.ws.db_panel().tables[0].name, "users");
    let (_, browse, read_only) = h.db_sent(Command::Newline).expect("opens the table");
    assert!(
        browse.contains("FROM \"users\" LIMIT") && read_only,
        "{browse}"
    );
    h.ws.dispatch(Command::Focus(Focus::Editor));
    let sent = h
        .db_sent(Command::Run(Action::DbRun))
        .expect("no question now");
    assert_eq!(sent.1, "SELECT 1");
}

#[test]
fn an_old_answer_is_ignored_and_a_new_query_stops_the_old_one() {
    let mut h = Harness::sql("SELECT 1", &["only"]);
    let (old, ..) = h.db_sent(Command::Run(Action::DbRun)).unwrap();
    let fx = h.ws.dispatch(Command::Run(Action::DbRun));
    assert!(
        fx.iter().any(
            |e| matches!(e, Effect::Db(crate::core::db::DbEffect::Cancel { id }) if *id == old)
        )
    );
    h.ws.handle(Event::Db(crate::core::db::DbEvent::Finished {
        id: old,
        outcome: crate::core::db::DbOutcome::Failed("late".to_owned()),
    }));
    assert_eq!(h.ws.db_panel().outcome, None);
    assert!(h.ws.db_panel().running.is_some());
    let fx = h.ws.dispatch(Command::Run(Action::DbStop));
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::Db(crate::core::db::DbEffect::Cancel { .. })))
    );
}

#[test]
fn outside_a_sql_file_nothing_runs() {
    let mut h = Harness::new(&[("a.rs", "SELECT 1")]);
    h.open("a.rs");
    h.ws.configure_databases(vec![sqlite_connection("only")]);
    assert_eq!(h.db_sent(Command::Run(Action::DbRun)), None);
}
