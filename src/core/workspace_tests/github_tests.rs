use super::*;

#[test]
fn github_needs_trust_and_asks_nothing_without_it() {
    let (mut ws, _) = Workspace::new(PathBuf::from(ROOT));
    ws.configure_trust(Some(false));
    let fx = ws.dispatch(Command::Run(Action::ShowGitHub));
    assert!(gh_runs(&fx).is_empty(), "{fx:?}");
    assert!(ws.file_prompt().is_some(), "offers trust");
    let any = Effect::Cli(crate::core::cli::CliEffect::Run {
        id: 1,
        tool: crate::core::cli::CliTool::Gh,
        root: PathBuf::new(),
        args: Vec::new(),
        input: None,
        in_project: true,
        write: true,
    });
    assert!(any.needs_trust());
}

#[test]
fn github_reads_the_account_the_repository_and_the_lists_and_only_reads() {
    let mut ws = docker_ws();
    let fx = ws.dispatch(Command::Run(Action::ShowGitHub));
    assert!(gh_writes(&fx).is_empty());
    gh_answer(&mut ws, gh_runs(&fx));
    let panel = ws.github_panel();
    assert_eq!(panel.account.as_deref(), Some("me"));
    assert_eq!(panel.repo.as_deref(), Some("o/r"));
    assert_eq!(panel.prs[0].number, 7);
    assert_eq!(panel.issues[0].number, 9);
    assert!(!format!("{panel:?}").contains("private@example.com"));
}

#[test]
fn a_comment_goes_out_only_after_y_and_its_text_only_by_stdin() {
    let mut ws = opened_pr();
    insert(&mut ws, "c");
    let fx = insert(&mut ws, "--repo evil/x");
    assert!(gh_writes(&fx).is_empty());
    let fx = ws.dispatch(Command::Newline);
    assert!(gh_writes(&fx).is_empty(), "the window comes first");
    assert!(ws.github_panel().confirm.is_some());
    let fx = insert(&mut ws, "y");
    let runs = gh_runs(&fx);
    assert_eq!(runs[0].1.join(" "), "pr comment 7 -R o/r --body-file -");
    assert_eq!(runs[0].2.as_deref(), Some("--repo evil/x"));
}

#[test]
fn enter_or_any_other_key_cancels_a_post() {
    for key in [
        Command::Newline,
        Command::Escape,
        Command::Insert("n".to_owned()),
    ] {
        let mut ws = opened_pr();
        insert(&mut ws, "c");
        insert(&mut ws, "hello");
        ws.dispatch(Command::Newline);
        let fx = ws.dispatch(key);
        assert!(gh_writes(&fx).is_empty());
        assert!(ws.github_panel().confirm.is_none());
    }
}

#[test]
fn typed_control_characters_never_reach_the_screen_or_github() {
    let mut ws = opened_pr();
    insert(&mut ws, "c");
    insert(&mut ws, "a\u{1b}]0;x\u{7}b");
    ws.dispatch(Command::Newline);
    let fx = insert(&mut ws, "y");
    let sent = gh_runs(&fx)[0].2.clone().unwrap();
    assert!(
        !sent.contains('\u{1b}') && !sent.contains('\u{7}'),
        "{sent:?}"
    );
}

#[test]
fn merging_takes_y_and_then_the_typed_number_and_binds_the_commit_it_read() {
    let mut ws = opened_pr();
    insert(&mut ws, "m");
    ws.dispatch(Command::Indent);
    let fx = insert(&mut ws, "y");
    assert!(gh_writes(&fx).is_empty(), "y alone does not merge");
    let fx = ws.dispatch(Command::Newline);
    assert!(gh_writes(&fx).is_empty(), "Enter with nothing typed");
    insert(&mut ws, "8");
    let fx = ws.dispatch(Command::Newline);
    assert!(gh_writes(&fx).is_empty(), "the wrong number");
    ws.dispatch(Command::DeleteBackward);
    insert(&mut ws, "7");
    let fx = ws.dispatch(Command::Newline);
    assert_eq!(
        gh_writes(&fx),
        vec!["pr merge 7 --squash -R o/r --match-head-commit abc123"]
    );
}

#[test]
fn closing_takes_the_second_answer_too_and_escape_there_sends_nothing() {
    let mut ws = opened_pr();
    insert(&mut ws, "d");
    insert(&mut ws, "y");
    assert!(ws.github_panel().confirm.as_ref().unwrap().typed.is_some());
    let fx = ws.dispatch(Command::Escape);
    assert!(gh_writes(&fx).is_empty());
    assert!(ws.github_panel().confirm.is_none());
}

#[test]
fn nothing_is_offered_before_it_is_known_where_and_as_whom() {
    use crate::core::cli::CliOutcome;
    let mut ws = docker_ws();
    let fx = ws.dispatch(Command::Run(Action::ShowGitHub));
    let runs = gh_runs(&fx);
    let account = runs.iter().find(|r| r.1[0] == "api").unwrap().0;
    ws.handle(Event::Cli(crate::core::cli::CliEvent::Finished {
        tool: crate::core::cli::CliTool::Gh,
        id: account,
        outcome: CliOutcome::Failed(
            "To get started with GitHub CLI, please run:  gh auth login".to_owned(),
        ),
    }));
    assert!(
        ws.github_panel()
            .note
            .as_deref()
            .unwrap()
            .starts_with("Not logged in")
    );
    let repo = runs.iter().find(|r| r.1[0] == "repo").unwrap().clone();
    gh_answer(&mut ws, vec![repo]);
    let fx = ws.dispatch(Command::Newline);
    gh_answer(&mut ws, gh_runs(&fx));
    insert(&mut ws, "c");
    insert(&mut ws, "x");
    ws.dispatch(Command::Newline);
    assert!(ws.github_panel().confirm.is_none(), "no account, no window");
}

#[test]
fn an_option_like_branch_cannot_be_checked_out() {
    use crate::core::cli::CliOutcome;
    let mut ws = opened_github();
    let fx = ws.dispatch(Command::Newline);
    for (id, args, _) in gh_runs(&fx) {
        let text = if args[1] == "view" {
            GH_PR_VIEW.replace("\"fix\"", "\"--upload-pack=evil\"")
        } else {
            "[]".to_owned()
        };
        ws.handle(Event::Cli(crate::core::cli::CliEvent::Finished {
            tool: crate::core::cli::CliTool::Gh,
            id,
            outcome: CliOutcome::Done(text),
        }));
    }
    insert(&mut ws, "o");
    assert!(ws.github_panel().confirm.is_none());
    assert!(
        status_of(&ws).contains("cannot be used safely"),
        "{}",
        status_of(&ws)
    );
}

#[test]
fn a_write_that_was_stopped_says_it_may_have_gone_through() {
    use crate::core::cli::CliOutcome;
    let mut ws = opened_pr();
    insert(&mut ws, "c");
    insert(&mut ws, "hi");
    ws.dispatch(Command::Newline);
    let fx = insert(&mut ws, "y");
    let id = gh_runs(&fx)[0].0;
    let after = ws.handle(Event::Cli(crate::core::cli::CliEvent::Finished {
        tool: crate::core::cli::CliTool::Gh,
        id,
        outcome: CliOutcome::TimedOut,
    }));
    assert!(gh_writes(&after).is_empty(), "never sent twice");
    assert!(status_of(&ws).contains("whether it went through"));
}

#[test]
fn escape_goes_up_a_level_before_it_leaves_the_panel() {
    let mut ws = opened_pr();
    assert_eq!(ws.github_panel().view, crate::core::github::GhView::Detail);
    ws.dispatch(Command::Escape);
    assert_eq!(ws.github_panel().view, crate::core::github::GhView::List);
    assert_eq!(ws.focus(), Focus::GitHub);
    ws.dispatch(Command::Escape);
    assert_eq!(ws.focus(), Focus::Editor);
}

#[test]
fn a_new_pull_request_is_refused_from_the_default_branch_and_shows_title_and_text() {
    let mut ws = opened_github();
    on_branch(&mut ws, "main");
    insert(&mut ws, "n");
    assert!(ws.github_panel().compose.is_none());
    on_branch(&mut ws, "--evil");
    insert(&mut ws, "n");
    assert!(ws.github_panel().compose.is_none());
    on_branch(&mut ws, "feature");
    insert(&mut ws, "n");
    insert(&mut ws, "My title");
    ws.dispatch(Command::Newline);
    insert(&mut ws, "My text");
    ws.dispatch(Command::Newline);
    let confirm = ws.github_panel().confirm.as_ref().unwrap();
    assert!(confirm.write.text().iter().any(|l| l == "My text"));
    let fx = insert(&mut ws, "y");
    let runs = gh_runs(&fx);
    assert_eq!(
        runs[0].1.join(" "),
        "pr create -R o/r --head feature --base main --title=My title --body-file -"
    );
    assert_eq!(runs[0].2.as_deref(), Some("My text"));
}

#[test]
fn line_remarks_show_whichever_answer_comes_first_and_only_once() {
    use crate::core::cli::CliOutcome;
    let remarks = r#"[{"user":{"login":"r"},"path":"x.rs","line":3,"body":"nit"}]"#;
    for remarks_first in [true, false] {
        let mut ws = opened_github();
        let fx = ws.dispatch(Command::Newline);
        let mut runs = gh_runs(&fx);
        if remarks_first {
            runs.reverse();
        }
        for (id, args, _) in runs {
            let text = if args[1] == "view" {
                GH_PR_VIEW
            } else {
                remarks
            };
            ws.handle(Event::Cli(crate::core::cli::CliEvent::Finished {
                tool: crate::core::cli::CliTool::Gh,
                id,
                outcome: CliOutcome::Done(text.to_owned()),
            }));
        }
        let lines = &ws.github_panel().detail.as_ref().unwrap().lines;
        let shown = lines.iter().filter(|l| l.as_str() == "nit").count();
        assert_eq!(shown, 1, "remarks first: {remarks_first}: {lines:?}");
    }
}

#[test]
fn alt_enter_starts_a_new_line_in_a_comment() {
    let mut ws = opened_pr();
    insert(&mut ws, "c");
    insert(&mut ws, "one");
    ws.dispatch(Command::Run(Action::ContextActions));
    insert(&mut ws, "two");
    ws.dispatch(Command::Newline);
    let fx = insert(&mut ws, "y");
    assert_eq!(gh_runs(&fx)[0].2.as_deref(), Some("one\ntwo"));
}

#[test]
fn a_grammar_in_a_file_is_asked_for_and_the_text_gets_its_colours_when_it_arrives() {
    crate::lang::syntax::pretend_pending(Some(Language::Go));
    let mut h = Harness::new(&[("a.go", "package a\n")]);
    h.open("a.go");
    let doc = h.ws.active().unwrap().1;
    assert!(doc.syntax().is_none(), "shown plain meanwhile");
    assert_eq!(doc.text().to_string(), "package a\n");
    let rope = ropey::Rope::from_str("x");
    assert!(crate::lang::syntax::Syntax::new(Language::Go, &rope).is_none());
    let fx = h.ws.dispatch(Command::Move(Motion::Right));
    let asked = fx
        .iter()
        .filter(|e| {
            matches!(
                e,
                Effect::LoadGrammar {
                    language: Language::Go
                }
            )
        })
        .count();
    assert_eq!(asked, 1);
    let fx = h.ws.dispatch(Command::Move(Motion::Right));
    assert!(
        !fx.iter().any(|e| matches!(e, Effect::LoadGrammar { .. })),
        "once"
    );
    crate::lang::syntax::pretend_pending(None);
    h.ws.handle(Event::GrammarLoaded {
        language: Language::Go,
        error: None,
    });
    assert!(h.ws.active().unwrap().1.syntax().is_some(), "coloured now");
}

#[test]
fn a_grammar_that_cannot_load_says_why_and_the_file_stays_editable() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.ws.handle(Event::GrammarLoaded {
        language: Language::Php,
        error: Some("lib/grammars/php: no such file".to_owned()),
    });
    let shown = status_text(&h);
    assert!(shown.contains("No colours for PHP"), "{shown}");
    assert!(shown.contains("lib/grammars/php"), "{shown}");
    h.run(Command::Insert("y".to_owned()));
    assert_eq!(h.ws.active().unwrap().1.text().to_string(), "yx");
}

#[test]
fn a_message_outlives_what_the_screen_reports_about_itself_but_not_a_key() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    for command in [
        Command::Viewport { text_cols: 50 },
        Command::ToolRoom {
            rows: 20,
            min: 3,
            max: 15,
        },
        Command::TerminalSize {
            rows: 9,
            columns: 40,
        },
        Command::ClaudeSize {
            rows: 9,
            columns: 40,
        },
        Command::CheckDisk,
    ] {
        h.ws.handle(Event::GrammarLoaded {
            language: Language::Php,
            error: Some("gone".to_owned()),
        });
        h.run(command);
        assert!(status_text(&h).contains("gone"));
    }
    h.run(Command::Insert("y".to_owned()));
    assert!(!status_text(&h).contains("gone"));
}
