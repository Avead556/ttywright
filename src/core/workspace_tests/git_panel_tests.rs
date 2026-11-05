use super::*;

#[test]
fn the_branch_is_asked_for_at_startup_and_when_the_terminal_regains_focus() {
    let (_, effects) = Workspace::new(PathBuf::from(ROOT));
    assert!(
        effects
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Status { .. })))
    );
    let mut h = Harness::new(&[]);
    assert_eq!(h.ws.git_branch(), None);
    h.git_status(Vec::new());
    assert_eq!(h.ws.git_branch(), Some("main"));
    h.git.clear();
    h.run(Command::CheckDisk);
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Status { .. })))
    );
}

#[test]
fn opening_the_panel_saves_first_so_git_sees_what_is_on_screen() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.typed("y");
    h.act(Action::GitCommit);
    assert_eq!(h.written.len(), 1);
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Status { .. })))
    );
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Log { .. })))
    );
    assert_eq!(h.panel().tab, GitTab::Commit);
}

#[test]
fn outside_a_repository_the_panel_says_why() {
    let mut h = Harness::new(&[]);
    h.act(Action::GitLog);
    h.lsp_event(Event::Git(GitEvent::StatusRead {
        branch: None,
        top: None,
        files: Err("Not a git repository".into()),
        state: crate::core::GitRepoState::default(),
    }));
    assert_eq!(
        h.panel().unavailable.as_deref(),
        Some("Not a git repository")
    );
    assert_eq!(h.ws.git_branch(), None);
}

#[test]
fn the_git_panel_takes_keys_only_while_focused() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.commit_view(vec![git_file("a.txt", GitChange::Modified)]);
    h.typed("z");
    h.run(Command::DeleteBackward);
    assert_eq!(h.active_text(), "x", "nothing reaches the editor");
    h.run(Command::Escape);
    assert_eq!(h.ws.focus(), Focus::Editor, "back in the editor");
    assert!(h.ws.tool_window_visible(), "with the panel still up");
    h.typed("t");
    assert_eq!(h.active_text(), "tx", "keys reach the editor again");
    h.run(Command::Escape);
    assert!(!h.ws.tool_window_visible(), "the next Esc hides it");
}

#[test]
fn quitting_works_from_the_git_panel() {
    let mut h = Harness::new(&[]);
    h.act(Action::GitLog);
    h.act(Action::Quit);
    assert!(h.ws.should_quit());
}

#[test]
fn moving_through_changed_files_shows_each_ones_diff_and_late_answers_are_dropped() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![
        git_file("a", GitChange::Modified),
        git_file("b", GitChange::Modified),
    ]);
    let (first, file) = h.last_hunks().expect("a diff request");
    assert_eq!(file.path, Path::new("a"));
    h.run(Command::Move(Motion::Down));
    let (second, file) = h.last_hunks().expect("a diff request");
    assert_eq!(file.path, Path::new("b"));
    let row = |text: &str| DiffRow {
        kind: DiffKind::Added,
        old_line: None,
        new_line: Some(1),
        text: text.to_owned(),
    };
    h.lsp_event(Event::Git(GitEvent::DiffRead {
        id: first,
        result: Ok(vec![row("late")]),
    }));
    assert!(h.panel().diff.is_empty());
    h.lsp_event(Event::Git(GitEvent::DiffRead {
        id: second,
        result: Ok(vec![row("b")]),
    }));
    assert_eq!(h.panel().diff[0].text, "b");
    h.run(Command::Move(Motion::Down));
    assert_eq!(h.panel().file_selected, 1, "stops at the end");
}

#[test]
fn committing_takes_the_ticked_files_and_adds_the_untracked_among_them() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![
        git_file("mod.rs", GitChange::Modified),
        git_file("new.rs", GitChange::Untracked),
        git_file("skip.rs", GitChange::Modified),
    ]);
    h.run(Command::Move(Motion::Down));
    h.typed(" ");
    h.run(Command::Move(Motion::Down));
    h.typed(" ");
    h.run(Command::Indent);
    h.typed("fix: a thing");
    h.run(Command::Newline);
    let (message, mut paths, untracked) = h.commit_sent().expect("a commit");
    paths.sort();
    assert_eq!(message, "fix: a thing");
    assert_eq!(paths, [PathBuf::from("mod.rs"), PathBuf::from("new.rs")]);
    assert_eq!(untracked, [PathBuf::from("new.rs")]);
    assert!(h.panel().busy);
}

#[test]
fn a_renames_commit_takes_both_paths() {
    let mut h = Harness::new(&[]);
    let mut renamed = git_file("new", GitChange::Renamed);
    renamed.old_path = Some(PathBuf::from("old"));
    h.commit_view(vec![renamed]);
    h.run(Command::Indent);
    h.typed("move");
    h.run(Command::Newline);
    let (_, paths, _) = h.commit_sent().expect("a commit");
    assert_eq!(paths, [PathBuf::from("new"), PathBuf::from("old")]);
}

#[test]
fn a_commit_needs_files_and_a_message() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![git_file("a", GitChange::Modified)]);
    h.run(Command::Indent);
    h.typed("   ");
    h.run(Command::Newline);
    assert_eq!(h.commit_sent(), None);
    assert!(
        h.panel()
            .notice
            .as_ref()
            .is_some_and(|n| n.text.contains("message"))
    );
    h.run(Command::Indent);
    h.typed(" ");
    h.run(Command::Indent);
    h.typed("x");
    h.run(Command::Newline);
    assert_eq!(h.commit_sent(), None);
    assert!(
        h.panel()
            .notice
            .as_ref()
            .is_some_and(|n| n.text.contains("Tick"))
    );
}

#[test]
fn a_is_all_or_none() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![
        git_file("a", GitChange::Modified),
        git_file("b", GitChange::Untracked),
    ]);
    h.typed("a");
    assert_eq!(h.panel().checked.len(), 2);
    h.typed("a");
    assert!(h.panel().checked.is_empty());
}

#[test]
fn a_finished_commit_clears_the_message_and_refreshes() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![git_file("a", GitChange::Modified)]);
    h.run(Command::Indent);
    h.typed("msg");
    h.run(Command::Newline);
    h.git.clear();
    h.lsp_event(Event::Git(GitEvent::Committed {
        result: Ok("abc msg".into()),
    }));
    let panel = h.panel();
    assert_eq!(panel.message.text(), "");
    assert!(!panel.busy);
    assert_eq!(
        panel.notice.as_ref().map(|n| n.text.as_str()),
        Some("Committed abc msg")
    );
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Status { .. })))
    );
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Log { .. })))
    );
}

#[test]
fn a_failed_commit_keeps_the_message_and_shows_why() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![git_file("a", GitChange::Modified)]);
    h.run(Command::Indent);
    h.typed("msg");
    h.run(Command::Newline);
    h.lsp_event(Event::Git(GitEvent::Committed {
        result: Err("hook said no".into()),
    }));
    let panel = h.panel();
    assert_eq!(panel.message.text(), "msg");
    assert!(
        panel
            .notice
            .as_ref()
            .is_some_and(|n| n.error && n.text == "hook said no")
    );
    assert!(!panel.busy);
}

#[test]
fn a_second_enter_while_committing_sends_nothing() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![git_file("a", GitChange::Modified)]);
    h.run(Command::Indent);
    h.typed("msg");
    h.run(Command::Newline);
    h.run(Command::Newline);
    let sent = h
        .git
        .iter()
        .filter(|e| matches!(e, Effect::Git(GitEffect::Commit { .. })))
        .count();
    assert_eq!(sent, 1);
}

#[test]
fn a_file_with_a_conflict_blocks_the_commit() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.typed("mine ");
    h.file_changed("theirs");
    h.commit_view(vec![git_file("a.txt", GitChange::Modified)]);
    h.run(Command::Indent);
    h.typed("msg");
    h.run(Command::Newline);
    assert_eq!(h.commit_sent(), None);
    assert!(
        h.panel()
            .notice
            .as_ref()
            .is_some_and(|n| n.text.contains("resolve"))
    );
}

#[test]
fn enter_on_a_changed_file_opens_it_from_the_repository_folder() {
    let mut h = Harness::new(&[]);
    h.files.push((PathBuf::from("/repo/sub/a.txt"), "x".into()));
    h.commit_view(vec![git_file("sub/a.txt", GitChange::Modified)]);
    h.run(Command::Newline);
    assert_eq!(h.ws.focus(), Focus::Editor);
    assert_eq!(
        h.ws.active().map(|(_, d)| d.path().to_path_buf()),
        Some(PathBuf::from("/repo/sub/a.txt"))
    );
}

#[test]
fn choosing_a_commit_loads_its_files_then_the_first_files_diff() {
    let mut h = Harness::new(&[]);
    h.log_view(vec![git_commit("c2", &["c1"]), git_commit("c1", &[])]);
    assert!(h.git.contains(&Effect::Git(GitEffect::CommitFiles {
        root: PathBuf::from(ROOT),
        hash: "c2".into()
    })));
    h.run(Command::Move(Motion::Down));
    h.lsp_event(Event::Git(GitEvent::CommitFilesRead {
        hash: "c2".into(),
        result: Ok(vec![git_file("late", GitChange::Added)]),
    }));
    assert!(
        h.git.contains(&Effect::Git(GitEffect::CommitFiles {
            root: PathBuf::from(ROOT),
            hash: "c1".into()
        })),
        "the commit it moved to, once the first answer is in"
    );
    assert!(
        h.panel().commit_files.is_empty(),
        "an old commit's answer is dropped"
    );
    h.lsp_event(Event::Git(GitEvent::CommitFilesRead {
        hash: "c1".into(),
        result: Ok(vec![
            git_file("a", GitChange::Added),
            git_file("b", GitChange::Added),
        ]),
    }));
    let (_, source) = h.last_diff().expect("a diff");
    assert!(
        matches!(source, GitDiffSource::Commit { hash, file } if hash == "c1" && file.path == Path::new("a"))
    );
    h.run(Command::Indent);
    h.run(Command::Move(Motion::Down));
    let (_, source) = h.last_diff().expect("a diff");
    assert!(matches!(source, GitDiffSource::Commit { file, .. } if file.path == Path::new("b")));
    h.run(Command::Indent);
    assert_eq!(h.panel().field, GitField::List);
}

#[test]
fn a_filter_narrows_the_log_and_escape_brings_it_back() {
    let mut h = Harness::new(&[]);
    h.log_view(vec![git_commit("aa", &["bb"]), git_commit("bb", &[])]);
    h.typed("/");
    h.typed("subject bb");
    assert_eq!(h.panel().visible, [1]);
    assert_eq!(
        h.panel().selected_commit().map(|c| c.hash.as_str()),
        Some("bb")
    );
    h.run(Command::Escape);
    assert_eq!(h.panel().visible, [0, 1]);
    assert!(
        h.ws.git_panel().is_some(),
        "the first Esc only clears the filter"
    );
}

#[test]
fn y_copies_the_full_hash() {
    let mut h = Harness::new(&[]);
    h.log_view(vec![git_commit("abcdef", &[])]);
    h.typed("y");
    assert!(h.lsp.contains(&Effect::WriteClipboard {
        text: "abcdef".into()
    }));
}

#[test]
fn a_click_chooses_a_row() {
    let mut h = Harness::new(&[]);
    h.log_view(vec![git_commit("a", &["b"]), git_commit("b", &[])]);
    h.run(Command::SelectGitRow { index: 1 });
    assert_eq!(h.panel().commit_selected, 1);
    h.run(Command::SelectGitRow { index: 99 });
    assert_eq!(h.panel().commit_selected, 1, "stays in the list");
}

#[test]
fn the_tabs_switch_and_refresh() {
    let mut h = Harness::new(&[]);
    h.commit_view(Vec::new());
    h.typed("l");
    assert_eq!(h.panel().tab, GitTab::Log);
    h.typed("c");
    assert_eq!(h.panel().tab, GitTab::Commit);
    h.act(Action::GitLog);
    assert_eq!(h.panel().tab, GitTab::Log);
}

#[test]
fn push_and_pull_report_outside_and_inside_the_panel() {
    let mut h = Harness::new(&[]);
    h.act(Action::GitPush);
    assert!(h.git.contains(&Effect::Git(GitEffect::Remote {
        root: PathBuf::from(ROOT),
        op: GitRemoteOp::Push
    })));
    h.lsp_event(Event::Git(GitEvent::RemoteDone {
        op: GitRemoteOp::Push,
        result: Err("no upstream".into()),
    }));
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("Push failed: no upstream")
    );
    h.act(Action::GitLog);
    h.act(Action::GitPull);
    assert!(h.panel().busy);
    h.lsp_event(Event::Git(GitEvent::RemoteDone {
        op: GitRemoteOp::Pull,
        result: Ok("Already up to date.".into()),
    }));
    assert!(!h.panel().busy);
    assert_eq!(
        h.panel().notice.as_ref().map(|n| n.text.as_str()),
        Some("Pull: Already up to date.")
    );
}

#[test]
fn alt_enter_starts_a_new_line_in_the_message_and_paste_keeps_lines() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![git_file("a", GitChange::Modified)]);
    h.run(Command::Indent);
    h.typed("subject");
    h.act(Action::ContextActions);
    h.act(Action::ContextActions);
    h.run(Command::Insert("body\r\nmore".into()));
    h.run(Command::Newline);
    let (message, _, _) = h.commit_sent().expect("a commit");
    assert_eq!(message, "subject\n\nbody\nmore");
}

#[test]
fn amend_loads_the_last_message_warns_when_pushed_and_can_be_turned_off() {
    let mut h = Harness::new(&[]);
    h.commit_view(Vec::new());
    h.typed("m");
    assert!(h.panel().amend);
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::ReadLastCommit { .. })))
    );
    h.lsp_event(Event::Git(GitEvent::LastCommit {
        result: Ok(("old message\n\n".into(), true)),
    }));
    assert_eq!(h.panel().message.text(), "old message");
    assert_eq!(h.panel().field, GitField::Message);
    assert!(
        h.panel()
            .notice
            .as_ref()
            .is_some_and(|n| n.text.contains("force push"))
    );
    h.run(Command::Escape);
    h.typed("m");
    assert!(!h.panel().amend);
    assert_eq!(h.panel().message.text(), "", "back to a fresh commit");
}

#[test]
fn amending_with_no_files_ticked_sends_the_commit_as_an_amend() {
    let mut h = Harness::new(&[]);
    h.commit_view(Vec::new());
    h.typed("m");
    h.lsp_event(Event::Git(GitEvent::LastCommit {
        result: Ok(("fixed".into(), false)),
    }));
    h.run(Command::Newline);
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Commit { amend: true, paths, .. }) if paths.is_empty()))
    );
    h.lsp_event(Event::Git(GitEvent::Committed {
        result: Ok("x".into()),
    }));
    assert!(!h.panel().amend, "only that one commit");
}

#[test]
fn rollback_takes_a_second_press_on_the_same_file() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![
        git_file("a", GitChange::Modified),
        git_file("b", GitChange::Modified),
    ]);
    let sent = |h: &Harness| {
        h.git
            .iter()
            .filter(|e| matches!(e, Effect::Git(GitEffect::Rollback { .. })))
            .count()
    };
    h.typed("u");
    assert_eq!(sent(&h), 0);
    assert!(
        h.panel()
            .notice
            .as_ref()
            .is_some_and(|n| n.text.contains("again"))
    );
    h.run(Command::Move(Motion::Down));
    h.typed("u");
    assert_eq!(sent(&h), 0, "moving away cancelled it");
    h.typed(" ");
    h.typed("u");
    assert_eq!(sent(&h), 0, "any other key cancels it");
    h.typed("u");
    assert_eq!(sent(&h), 1);
    assert!(h.git.iter().any(
        |e| matches!(e, Effect::Git(GitEffect::Rollback { absolute, .. }) if absolute == Path::new("/repo/b")
        )
    ));
}

#[test]
fn rollback_refuses_untracked_and_conflicted_files() {
    for change in [GitChange::Untracked, GitChange::Conflicted] {
        let mut h = Harness::new(&[]);
        h.commit_view(vec![git_file("a", change)]);
        h.typed("u");
        h.typed("u");
        assert!(
            !h.git
                .iter()
                .any(|e| matches!(e, Effect::Git(GitEffect::Rollback { .. })))
        );
    }
}

#[test]
fn a_finished_rollback_rereads_the_open_files_and_says_where_the_text_is() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.commit_view(Vec::new());
    h.git.clear();
    h.lsp_event(Event::Git(GitEvent::RolledBack {
        path: PathBuf::from("a.txt"),
        result: Ok(()),
    }));
    assert!(
        h.panel()
            .notice
            .as_ref()
            .is_some_and(|n| n.text.contains("Local History"))
    );
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Status { .. })))
    );
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Base { .. })))
    );
}

#[test]
fn the_branch_list_marks_the_current_one_and_enter_switches() {
    let mut h = Harness::new(&[]);
    h.git_status(Vec::new());
    h.act(Action::GitBranches);
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::ListBranches { .. })))
    );
    h.lsp_event(Event::Git(GitEvent::BranchesRead {
        result: Ok(vec![
            git_branch("main", false),
            git_branch("origin/x", true),
        ]),
    }));
    let kind = h.ws.picker().expect("a list").kind;
    assert_eq!(h.ws.picker_items(kind), ["● main", "  origin/x"]);
    h.run(Command::Move(Motion::Down));
    h.run(Command::Newline);
    assert!(h.git.contains(&Effect::Git(GitEffect::Switch {
        root: PathBuf::from(ROOT),
        branch: git_branch("origin/x", true),
        create: false,
    })));
}

#[test]
fn choosing_the_current_branch_does_nothing() {
    let mut h = Harness::new(&[]);
    h.git_status(Vec::new());
    h.act(Action::GitBranches);
    h.lsp_event(Event::Git(GitEvent::BranchesRead {
        result: Ok(vec![git_branch("main", false)]),
    }));
    h.run(Command::Newline);
    assert!(
        !h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Switch { .. })))
    );
}

#[test]
fn a_name_that_matches_nothing_creates_a_branch_and_saves_first() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.act(Action::GitBranches);
    h.lsp_event(Event::Git(GitEvent::BranchesRead {
        result: Ok(vec![git_branch("main", false)]),
    }));
    h.typed("xyz/new");
    h.typed("");
    h.run(Command::Newline);
    assert!(h.git.contains(&Effect::Git(GitEffect::Switch {
        root: PathBuf::from(ROOT),
        branch: git_branch("xyz/new", false),
        create: true,
    })));
}

#[test]
fn a_failed_switch_says_why_and_a_good_one_rereads_files() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.lsp_event(Event::Git(GitEvent::Switched {
        name: "main".into(),
        result: Err("would be overwritten".into()),
    }));
    assert!(
        h.ws.status()
            .is_some_and(|s| s.text.contains("would be overwritten"))
    );
    h.git.clear();
    h.lsp_event(Event::Git(GitEvent::Switched {
        name: "main".into(),
        result: Ok(()),
    }));
    assert_eq!(
        h.ws.status().map(|s| s.text.as_str()),
        Some("On branch main")
    );
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Base { .. })))
    );
}

#[test]
fn changed_files_and_their_folders_are_known_by_absolute_path() {
    let mut h = Harness::new(&[]);
    h.git_status(vec![git_file("src/core/a.rs", GitChange::Modified)]);
    assert_eq!(
        h.ws.git_change(Path::new("/repo/src/core/a.rs")),
        Some(GitChange::Modified)
    );
    assert!(h.ws.git_dir_changed(Path::new("/repo/src/core")));
    assert!(h.ws.git_dir_changed(Path::new("/repo/src")));
    assert!(!h.ws.git_dir_changed(Path::new("/")));
    h.git_status(Vec::new());
    assert_eq!(h.ws.git_change(Path::new("/repo/src/core/a.rs")), None);
    assert!(!h.ws.git_dir_changed(Path::new("/repo/src")));
}

#[test]
fn saving_and_outside_changes_refresh_the_status_and_opening_asks_for_the_base() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Base { .. })))
    );
    h.typed("y");
    h.git.clear();
    h.act(Action::Save);
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Status { .. })))
    );
    h.git.clear();
    h.lsp_event(Event::PathsChanged {
        paths: vec![PathBuf::from("/proj/z")],
    });
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Status { .. })))
    );
}

#[test]
fn the_git_panel_lives_in_the_tool_window_and_toggles() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.open("a.txt");
    h.act(Action::GitLog);
    assert_eq!(h.ws.tool_tab(), ToolTab::Git);
    assert!(h.ws.tool_window_visible());
    assert_eq!(h.ws.focus(), Focus::Git);
    h.act(Action::GitLog);
    assert!(!h.ws.tool_window_visible(), "the same chord hides it");
    assert_eq!(h.ws.focus(), Focus::Editor);
    h.act(Action::GitLog);
    h.act(Action::GitCommit);
    assert!(h.ws.tool_window_visible(), "the other tab just switches");
    assert_eq!(h.panel().tab, GitTab::Commit);
    h.act(Action::ShowProblems);
    assert_eq!(h.ws.tool_tab(), ToolTab::Problems);
    h.act(Action::GitCommit);
    assert_eq!(
        h.ws.tool_tab(),
        ToolTab::Git,
        "from another tool it comes back"
    );
}

#[test]
fn global_actions_work_from_the_git_panel() {
    let mut h = Harness::new(&[("a.txt", "x"), ("b.txt", "y")]);
    h.open("a.txt");
    h.act(Action::GitLog);
    h.act(Action::OpenPalette);
    assert!(h.ws.picker().is_some());
    h.run(Command::Escape);
    assert_eq!(h.ws.focus(), Focus::Git, "Esc closed only the list");
    h.run(Command::OpenFile {
        path: PathBuf::from("b.txt"),
        at: None,
    });
    assert_eq!(h.active_name(), "b.txt");
    assert_eq!(h.ws.focus(), Focus::Editor);
    assert!(h.ws.tool_window_visible());
}

#[test]
fn a_click_in_the_panel_lists_focuses_it() {
    let mut h = Harness::new(&[]);
    h.log_view(vec![git_commit("a", &["b"]), git_commit("b", &[])]);
    h.run(Command::Focus(Focus::Editor));
    h.run(Command::SelectGitRow { index: 1 });
    assert_eq!(h.ws.focus(), Focus::Git);
    assert_eq!(h.panel().commit_selected, 1);
}

#[test]
fn branches_open_over_the_panel() {
    let mut h = Harness::new(&[]);
    h.act(Action::GitCommit);
    h.typed("b");
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::ListBranches { .. })))
    );
    h.lsp_event(Event::Git(GitEvent::BranchesRead {
        result: Ok(vec![git_branch("main", false)]),
    }));
    assert_eq!(h.ws.picker().map(|p| p.kind), Some(PickerKind::Branches));
    h.run(Command::Escape);
    assert!(h.ws.picker().is_none());
    assert_eq!(h.ws.focus(), Focus::Git);
}

#[test]
fn a_git_window_starts_on_the_log_and_owns_nothing() {
    let (ws, effects) = Workspace::new_git_window(PathBuf::from(ROOT));
    assert!(ws.is_window());
    assert_eq!(ws.focus(), Focus::Git);
    assert_eq!(ws.git_panel().map(|p| p.tab), Some(GitTab::Log));
    assert!(
        effects.iter().all(|e| matches!(
            e,
            Effect::Git(GitEffect::Status { .. }) | Effect::Git(GitEffect::Log { .. })
        )),
        "no session, no swaps, no watcher: {effects:?}"
    );
}

#[test]
fn in_a_git_window_esc_stays_and_enter_on_a_file_asks_the_main_window() {
    let (ws, _) = Workspace::new_git_window(PathBuf::from(ROOT));
    let mut h = Harness::new(&[]);
    h.ws = ws;
    h.git_log(vec![git_commit("c", &[])]);
    h.lsp_event(Event::Git(GitEvent::CommitFilesRead {
        hash: "c".into(),
        result: Ok(vec![git_file("src/a.rs", GitChange::Modified)]),
    }));
    h.git.clear();
    h.run(Command::Indent);
    h.run(Command::Newline);
    assert!(
        h.git.contains(&Effect::OpenInMainWindow {
            root: PathBuf::from(ROOT),
            path: PathBuf::from("/repo").join("src/a.rs").clone(),
            at: None,
        }) || h.git.iter().any(
            |e| matches!(e, Effect::OpenInMainWindow { path, .. } if path.ends_with("src/a.rs"))
        )
    );
    assert!(h.ws.active().is_none(), "it opens no file itself");
    h.run(Command::Escape);
    h.run(Command::Escape);
    assert_eq!(h.ws.focus(), Focus::Git, "nowhere to go back to");
    h.act(Action::GitLog);
    assert!(h.ws.tool_window_visible(), "and nothing to hide");
    h.lsp_event(Event::OpenedInMainWindow {
        result: Err("The main window is not running".into()),
    });
    assert!(h.panel().notice.as_ref().is_some_and(|n| n.error));
}

#[test]
fn a_request_from_another_window_opens_the_file_here() {
    let mut h = Harness::new(&[("a.txt", "x")]);
    h.lsp_event(Event::ExternalOpen {
        path: Path::new(ROOT).join("a.txt"),
        at: None,
    });
    assert_eq!(h.active_name(), "a.txt");
    assert_eq!(h.ws.focus(), Focus::Editor);
}

#[test]
fn w_opens_a_window_and_a_failure_tells_what_to_run() {
    let mut h = Harness::new(&[]);
    h.log_view(Vec::new());
    h.typed("w");
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::OpenGitWindow { .. }))
    );
    h.lsp_event(Event::GitWindowOpened {
        result: Err("Run in another terminal: ttyw --window git-log".into()),
    });
    assert!(
        h.panel()
            .notice
            .as_ref()
            .is_some_and(|n| n.text.contains("--window"))
    );
}

#[test]
fn holding_an_arrow_asks_for_the_commit_it_stops_on_not_every_one_it_passes() {
    let mut h = Harness::new(&[]);
    let log: Vec<GitCommit> = ["a", "b", "c", "d"]
        .iter()
        .map(|c| git_commit(c, &[]))
        .collect();
    h.log_view(log);
    assert_eq!(commits_loaded(&h), 1, "a");
    for _ in 0..3 {
        h.run(Command::Move(Motion::Down));
    }
    assert_eq!(commits_loaded(&h), 1, "still waiting for the first answer");
    assert!(h.panel().commit_files.is_empty());
    h.lsp_event(Event::Git(GitEvent::CommitFilesRead {
        hash: "a".into(),
        result: Ok(vec![git_file("old", GitChange::Added)]),
    }));
    assert_eq!(commits_loaded(&h), 2, "now the one it stopped on");
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::CommitFiles { hash, .. }) if hash == "d"))
    );
    assert!(
        h.panel().commit_files.is_empty(),
        "the old answer is not shown"
    );
    h.lsp_event(Event::Git(GitEvent::CommitFilesRead {
        hash: "d".into(),
        result: Ok(vec![git_file("new", GitChange::Added)]),
    }));
    assert_eq!(h.panel().commit_files[0].path, Path::new("new"));
    assert_eq!(commits_loaded(&h), 2);
}

#[test]
fn a_failed_answer_does_not_leave_the_list_waiting_forever() {
    let mut h = Harness::new(&[]);
    h.log_view(vec![git_commit("a", &[]), git_commit("b", &[])]);
    h.lsp_event(Event::Git(GitEvent::CommitFilesRead {
        hash: "a".into(),
        result: Err("boom".into()),
    }));
    h.run(Command::Move(Motion::Down));
    assert_eq!(commits_loaded(&h), 2);
}

#[test]
fn reopening_the_log_with_the_same_commit_keeps_what_is_shown() {
    let mut h = Harness::new(&[]);
    h.log_view(vec![git_commit("a", &[])]);
    h.lsp_event(Event::Git(GitEvent::CommitFilesRead {
        hash: "a".into(),
        result: Ok(vec![git_file("f", GitChange::Added)]),
    }));
    let (id, _) = h.last_diff().expect("a diff");
    h.lsp_event(Event::Git(GitEvent::DiffRead {
        id,
        result: Ok(vec![added_row("kept")]),
    }));
    h.act(Action::GitLog);
    h.act(Action::GitLog);
    assert_eq!(
        h.panel().diff[0].text,
        "kept",
        "nothing blanks while it refreshes"
    );
    let loaded = commits_loaded(&h);
    h.git_log(vec![git_commit("a", &[])]);
    assert_eq!(
        commits_loaded(&h),
        loaded,
        "the same commit needs no new requests"
    );
    assert_eq!(h.panel().diff[0].text, "kept");
    h.git_log(vec![git_commit("new", &["a"]), git_commit("a", &[])]);
    assert_eq!(commits_loaded(&h), loaded + 1, "a new commit on top does");
}

#[test]
fn a_status_refresh_keeps_the_commit_views_diff_and_its_scroll() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![git_file("a", GitChange::Modified)]);
    let (id, _) = h.last_hunks().expect("a diff");
    let rows = vec![crate::core::GitHunk {
        side: crate::core::HunkSide::Unstaged,
        whole: false,
        patch: Vec::new(),
        title: "@@".into(),
        rows: (0..48).map(|n| added_row(&n.to_string())).collect(),
    }];
    h.lsp_event(Event::Git(GitEvent::HunksRead {
        id,
        result: Ok(rows.clone()),
    }));
    h.run(Command::Move(Motion::PageDown { rows: 20 }));
    assert_eq!(h.panel().diff_scroll, 20);
    h.git_status(vec![git_file("a", GitChange::Modified)]);
    assert_eq!(
        h.panel().diff.len(),
        50,
        "still there while the new one is on its way"
    );
    let (id, _) = h.last_hunks().expect("a diff");
    h.lsp_event(Event::Git(GitEvent::HunksRead {
        id,
        result: Ok(rows),
    }));
    assert_eq!(h.panel().diff_scroll, 20, "saving does not lose the place");
}

#[test]
fn moving_to_another_file_clears_the_old_diff_at_once() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![
        git_file("a", GitChange::Modified),
        git_file("b", GitChange::Modified),
    ]);
    let (id, _) = h.last_hunks().expect("a diff");
    h.lsp_event(Event::Git(GitEvent::HunksRead {
        id,
        result: Ok(vec![crate::core::GitHunk {
            side: crate::core::HunkSide::Unstaged,
            whole: false,
            patch: Vec::new(),
            title: "@@".into(),
            rows: vec![added_row("of a")],
        }]),
    }));
    h.run(Command::Move(Motion::Down));
    assert!(h.panel().diff.is_empty(), "never shows a's diff under b");
}
