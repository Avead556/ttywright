use super::*;

#[test]
fn docker_asks_on_open_and_only_the_compose_question_is_the_projects() {
    let mut ws = docker_ws();
    let fx = ws.dispatch(Command::Run(Action::ShowDocker));
    let runs = docker_runs(&fx);
    let in_project: Vec<_> = runs
        .iter()
        .filter(|r| r.2)
        .map(|r| r.1[0].clone())
        .collect();
    assert_eq!(in_project, vec!["compose"]);
    assert_eq!(ws.focus(), Focus::Docker);
    let again = ws.dispatch(Command::Insert("r".to_owned()));
    assert!(docker_runs(&again).is_empty(), "one round at a time");
    docker_answer(&mut ws, &runs);
    assert_eq!(ws.docker_panel().services[0].name, "web");
    assert_eq!(ws.docker_panel().containers[0].id, "abc");
    assert_eq!(ws.docker_panel().images[0].name, "nginx:1");
}

#[test]
fn an_untrusted_project_lists_containers_but_never_reads_its_compose_file() {
    let (mut ws, _) = Workspace::new(PathBuf::from(ROOT));
    ws.configure_trust(Some(false));
    let fx = ws.dispatch(Command::Run(Action::ShowDocker));
    let runs = docker_runs(&fx);
    assert_eq!(runs.len(), 2);
    assert!(runs.iter().all(|r| !r.2 && r.1[0] != "compose"));
    assert!(ws.docker_panel().compose_note.is_some());
    let up = insert(&mut ws, "U");
    assert!(
        !up.iter()
            .any(|e| matches!(e, Effect::Run(RunEffect::Start { .. })))
    );
    assert!(ws.file_prompt().is_some(), "offers trust");
}

#[test]
fn a_stopped_daemon_says_so_and_nothing_asks_again_by_itself() {
    use crate::core::cli::CliOutcome;
    let mut ws = docker_ws();
    let fx = ws.dispatch(Command::Run(Action::ShowDocker));
    for (id, _, _) in docker_runs(&fx) {
        let after = ws.handle(Event::Cli(crate::core::cli::CliEvent::Finished {
            tool: crate::core::cli::CliTool::Docker,
            id,
            outcome: CliOutcome::Failed(
                "Cannot connect to the Docker daemon at unix:///x. Is the docker daemon running?"
                    .to_owned(),
            ),
        }));
        assert!(docker_runs(&after).is_empty(), "no retry");
    }
    let note = ws.docker_panel().daemon_note.clone().unwrap();
    assert!(note.starts_with("Docker is not running"), "{note}");
}

#[test]
fn removing_needs_y_and_enter_is_not_y() {
    let mut ws = opened_docker();
    ws.dispatch(Command::Indent);
    insert(&mut ws, "d");
    assert_eq!(
        ws.docker_panel().confirm.as_ref().map(|c| c.args.clone()),
        Some(vec!["rm".to_owned(), "abc".to_owned()])
    );
    let fx = ws.dispatch(Command::Newline);
    assert!(docker_runs(&fx).is_empty());
    assert!(ws.docker_panel().confirm.is_none());
    insert(&mut ws, "d");
    let fx = insert(&mut ws, "y");
    assert_eq!(docker_runs(&fx)[0].1, vec!["rm", "abc"]);
}

#[test]
fn compose_down_needs_y_and_a_done_action_refreshes() {
    let mut ws = opened_docker();
    insert(&mut ws, "d");
    let fx = insert(&mut ws, "y");
    let runs = docker_runs(&fx);
    assert_eq!(runs[0].1, vec!["compose", "down"]);
    assert!(runs[0].2);
    let after = ws.handle(Event::Cli(crate::core::cli::CliEvent::Finished {
        tool: crate::core::cli::CliTool::Docker,
        id: runs[0].0,
        outcome: crate::core::cli::CliOutcome::Done(String::new()),
    }));
    assert_eq!(docker_runs(&after).len(), 3, "the lists are asked again");
}

#[test]
fn up_and_logs_stream_in_the_run_tab_without_a_shell() {
    let mut ws = opened_docker();
    let fx = insert(&mut ws, "u");
    let Some(Effect::Run(RunEffect::Start { target, .. })) = fx
        .iter()
        .find(|e| matches!(e, Effect::Run(RunEffect::Start { .. })))
    else {
        panic!("{fx:?}");
    };
    assert_eq!(
        target.command,
        crate::core::run::RunCommand::Argv(
            ["docker", "compose", "up", "-d", "web"]
                .map(str::to_owned)
                .to_vec()
        )
    );
    assert!(
        Effect::Run(RunEffect::Start {
            id: 0,
            target: target.clone(),
            root: PathBuf::new(),
            replaces: None
        })
        .needs_trust()
    );
    ws.dispatch(Command::Focus(Focus::Docker));
    ws.dispatch(Command::Indent);
    let fx = insert(&mut ws, "l");
    assert!(
        fx.iter()
            .any(|e| matches!(e, Effect::Run(RunEffect::Start { target, .. })
        if target.command == crate::core::run::RunCommand::Argv(
            ["docker", "logs", "-f", "--tail", "500", "abc"].map(str::to_owned).to_vec())))
    );
}

#[test]
fn a_shell_opens_in_a_terminal_tab_with_the_exact_arguments() {
    let mut ws = opened_docker();
    ws.dispatch(Command::Indent);
    ws.dispatch(Command::TerminalSize {
        rows: 10,
        columns: 40,
    });
    let fx = insert(&mut ws, "e");
    let started = fx.iter().find_map(|e| match e {
        Effect::Terminal(TerminalEffect::Start {
            command,
            project_command,
            ..
        }) => Some((command.clone(), *project_command)),
        _ => None,
    });
    assert_eq!(
        started,
        Some((
            ["docker", "exec", "-it", "abc", "sh"]
                .map(str::to_owned)
                .to_vec(),
            false
        ))
    );
    assert_eq!(ws.focus(), Focus::Terminal);
}
