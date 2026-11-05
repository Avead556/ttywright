use super::*;

#[test]
fn nothing_runs_until_y_when_the_commands_are_new() {
    use crate::core::analysis::AnalysisTool::Clippy;
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.asked_analysis();
    let effects = h.ws.handle(analysis_plan(false));
    assert!(!analysis_started(&effects));
    assert_eq!(
        h.ws.analysis_confirm()
            .map(|c| c.commands[0].command_line()),
        Some("/usr/bin/lint --json".to_owned())
    );
    let effects = h.ws.dispatch(Command::Newline);
    assert!(!analysis_started(&effects), "Enter is no answer");
    assert!(h.ws.analysis_confirm().is_none());
    assert!(h.ws.analysis_running().is_empty());

    h.asked_analysis();
    h.ws.handle(analysis_plan(false));
    let effects = h.ws.dispatch(Command::Insert("y".to_owned()));
    assert!(analysis_started(&effects));
    assert_eq!(h.ws.analysis_running(), vec![Clippy.name()]);
    assert_eq!(h.ws.tool_tab(), ToolTab::Problems);
}

#[test]
fn approved_commands_start_without_asking_but_only_after_the_action() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    let unasked = h.ws.handle(analysis_plan(true));
    assert!(
        !analysis_started(&unasked),
        "a plan nobody asked for starts nothing"
    );
    h.asked_analysis();
    let effects = h.ws.handle(analysis_plan(true));
    assert!(analysis_started(&effects));
    assert!(h.ws.analysis_confirm().is_none());
}

#[test]
fn findings_join_the_problems_marked_by_source_and_enter_jumps_there() {
    use crate::core::analysis::{AnalysisOutcome::Found, AnalysisTool::Clippy};
    let mut h = Harness::new(&[("a.rs", "one\ntwo\nthree")]);
    h.open("a.rs");
    h.publish("a.rs", (0, 0), (0, 1));
    h.ws.handle(Event::Analysis(
        crate::core::analysis::AnalysisEvent::Finished {
            tool: Clippy,
            outcome: Found(vec![analysis_finding(1, "len zero")]),
        },
    ));
    let problems = h.ws.problems();
    assert_eq!(problems.len(), 2);
    assert_eq!(problems[0].source, "lsp");
    assert_eq!(problems[1].source, "clippy");
    assert_eq!(problems[1].message, "len zero");
    assert_eq!(h.ws.problem_counts().1, 1);
    h.act(Action::ShowProblems);
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    h.run(Command::Newline);
    assert_eq!(h.active_name(), "a.rs");
    assert_eq!(h.cursor(), "one\n".len() + 2);
}

#[test]
fn a_rerun_replaces_only_that_tools_results() {
    use crate::core::analysis::{
        AnalysisOutcome::Found,
        AnalysisTool::{Clippy, Ruff},
    };
    let mut h = Harness::new(&[]);
    for (tool, message) in [(Clippy, "old"), (Ruff, "other")] {
        h.ws.handle(Event::Analysis(
            crate::core::analysis::AnalysisEvent::Finished {
                tool,
                outcome: Found(vec![analysis_finding(0, message)]),
            },
        ));
    }
    h.ws.handle(Event::Analysis(
        crate::core::analysis::AnalysisEvent::Finished {
            tool: Clippy,
            outcome: Found(vec![analysis_finding(0, "new")]),
        },
    ));
    let messages: Vec<String> = h.ws.problems().into_iter().map(|p| p.message).collect();
    assert_eq!(messages, ["new", "other"]);
}

#[test]
fn a_failed_or_stopped_run_keeps_what_was_found_and_says_so() {
    use crate::core::analysis::{AnalysisOutcome as O, AnalysisTool::Clippy};
    let mut h = Harness::new(&[]);
    h.ws.handle(Event::Analysis(
        crate::core::analysis::AnalysisEvent::Finished {
            tool: Clippy,
            outcome: O::Found(vec![analysis_finding(0, "kept")]),
        },
    ));
    h.ws.handle(Event::Analysis(
        crate::core::analysis::AnalysisEvent::Finished {
            tool: Clippy,
            outcome: O::Failed("no such command".to_owned()),
        },
    ));
    assert!(
        status_text(&h).contains("clippy: no such command"),
        "{}",
        status_text(&h)
    );
    h.ws.handle(Event::Analysis(
        crate::core::analysis::AnalysisEvent::Finished {
            tool: Clippy,
            outcome: O::Stopped,
        },
    ));
    assert!(status_text(&h).contains("stopped"));
    assert_eq!(h.ws.problems()[0].message, "kept");
}

#[test]
fn stop_asks_the_service_only_while_something_runs() {
    let mut h = Harness::new(&[]);
    let idle = h.ws.dispatch(Command::Run(Action::StopAnalysis));
    assert!(!idle.iter().any(|e| matches!(
        e,
        Effect::Analysis(crate::core::analysis::AnalysisEffect::Stop)
    )));
    h.asked_analysis();
    h.ws.handle(analysis_plan(true));
    let busy = h.ws.dispatch(Command::Run(Action::StopAnalysis));
    assert!(busy.iter().any(|e| matches!(
        e,
        Effect::Analysis(crate::core::analysis::AnalysisEffect::Stop)
    )));
    let again = h.ws.dispatch(Command::Run(Action::AnalyzeProject));
    assert!(again.is_empty(), "one analysis at a time");
}

#[test]
fn a_project_with_nothing_to_run_says_why() {
    let mut h = Harness::new(&[]);
    h.asked_analysis();
    h.ws.handle(Event::Analysis(
        crate::core::analysis::AnalysisEvent::Planned {
            plan: crate::core::analysis::AnalysisPlan {
                commands: Vec::new(),
                missing: vec!["ruff".to_owned()],
                approved: false,
            },
        },
    ));
    assert!(
        status_text(&h).contains("Not installed: ruff"),
        "{}",
        status_text(&h)
    );
    h.asked_analysis();
    h.ws.handle(Event::Analysis(
        crate::core::analysis::AnalysisEvent::Planned {
            plan: crate::core::analysis::AnalysisPlan::default(),
        },
    ));
    assert!(status_text(&h).contains("No linter"), "{}", status_text(&h));
}
