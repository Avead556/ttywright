use crate::core::git_command::{GitEffect, GitEvent};
use std::collections::BTreeSet;

use super::*;
use crate::core::git::{CommitChecks, GitBranch, GitCompareCommit, GitCompared, GitDiffSource};

fn file(name: &str) -> GitFile {
    GitFile::new(PathBuf::from(name), GitChange::Modified)
}

fn commit_row(hash: &str, ours: bool) -> GitCompareCommit {
    GitCompareCommit {
        commit: GitCommit {
            hash: hash.to_owned(),
            parents: Vec::new(),
            author: "a".to_owned(),
            time: 0,
            refs: Vec::new(),
            subject: hash.to_owned(),
        },
        ours,
    }
}

impl Harness {
    fn branches_arrive(&mut self) {
        self.lsp_event(Event::Git(GitEvent::BranchesRead {
            result: Ok(vec![GitBranch {
                name: "main".to_owned(),
                remote: false,
            }]),
        }));
    }

    fn compare_with_main(&mut self, action: Action) {
        self.act(action);
        self.branches_arrive();
        assert_eq!(
            self.ws.picker().map(|p| p.kind),
            Some(PickerKind::CompareBranches)
        );
        self.run(Command::Newline);
    }

    fn compare_sent(&self) -> Option<(String, Option<PathBuf>)> {
        self.git.iter().rev().find_map(|e| match e {
            Effect::Git(GitEffect::Compare { base, path, .. }) => {
                Some((base.clone(), path.clone()))
            }
            _ => None,
        })
    }

    fn compare_diff_sent(&self) -> Option<GitDiffSource> {
        self.git.iter().rev().find_map(|e| match e {
            Effect::Git(GitEffect::Diff { source, .. }) => Some(source.clone()),
            _ => None,
        })
    }

    fn committed(&self) -> bool {
        self.git.iter().any(|e| {
            matches!(
                e,
                Effect::Git(GitEffect::Commit { .. }) | Effect::Git(GitEffect::CommitStaged { .. })
            )
        })
    }

    fn ready_to_commit(&mut self, checks: CommitChecks) {
        self.ws.configure_commit_checks(checks);
        self.act(Action::GitCommit);
        self.git_status(vec![file("a.rs")]);
        while self.ws.git_panel().expect("open").field != GitField::Message {
            self.run(Command::Indent);
        }
        self.typed("msg");
    }
}

fn found() -> GitCompared {
    GitCompared {
        commits: vec![commit_row("aaa", true), commit_row("bbb", false)],
        files: vec![file("a.rs")],
    }
}

#[test]
fn comparing_branches_lists_commits_then_files_each_with_its_diff() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.compare_with_main(Action::GitCompareBranch);
    assert_eq!(h.compare_sent(), Some(("main".to_owned(), None)));
    h.lsp_event(Event::Git(GitEvent::ComparedRead {
        base: "main".to_owned(),
        path: None,
        result: Ok(found()),
    }));
    assert_eq!(
        h.compare_diff_sent(),
        Some(GitDiffSource::WholeCommit("aaa".to_owned()))
    );
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    assert_eq!(
        h.compare_diff_sent(),
        Some(GitDiffSource::Compare {
            base: "main".to_owned(),
            worktree: false,
            file: file("a.rs"),
        })
    );
}

#[test]
fn comparing_a_file_is_against_it_as_it_is_now() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.open("a.rs");
    h.compare_with_main(Action::GitComparePath);
    let path = Path::new(ROOT).join("a.rs");
    assert_eq!(
        h.compare_sent(),
        Some(("main".to_owned(), Some(path.clone())))
    );
    h.lsp_event(Event::Git(GitEvent::ComparedRead {
        base: "main".to_owned(),
        path: Some(path),
        result: Ok(GitCompared {
            commits: Vec::new(),
            files: vec![file("a.rs")],
        }),
    }));
    assert!(matches!(
        h.compare_diff_sent(),
        Some(GitDiffSource::Compare { worktree: true, .. })
    ));
}

#[test]
fn a_cancelled_compare_leaves_the_branch_list_to_its_own_askers() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.act(Action::GitCompareBranch);
    h.branches_arrive();
    h.run(Command::Escape);
    h.act(Action::GitBranches);
    h.branches_arrive();
    assert_eq!(h.ws.picker().map(|p| p.kind), Some(PickerKind::Branches));
}

#[test]
fn an_answer_for_another_comparison_is_dropped() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.compare_with_main(Action::GitCompareBranch);
    h.lsp_event(Event::Git(GitEvent::ComparedRead {
        base: "other".to_owned(),
        path: None,
        result: Ok(found()),
    }));
    let shown =
        h.ws.git_panel()
            .and_then(|p| p.compare.as_ref())
            .map(|c| c.row_count());
    assert_eq!(shown, Some(0));
}

#[test]
fn a_changelist_gets_its_files_ticks_exactly_them_and_is_saved() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.act(Action::GitCommit);
    h.git_status(vec![file("a.rs"), file("b.rs"), file("c.rs")]);
    h.run(Command::Insert("e".to_owned()));
    h.typed("fix");
    h.run(Command::Newline);
    assert_eq!(h.ws.changelists().name_of(Path::new("a.rs")), Some("fix"));
    h.run(Command::Insert("j".to_owned()));
    let panel = h.ws.git_panel().expect("open");
    assert_eq!(panel.checked, BTreeSet::from([PathBuf::from("a.rs")]));
    assert_eq!(panel.commit_mode(), crate::core::git::CommitMode::Ticked);
    let saved = h.ws.dispatch(Command::SaveSession);
    assert!(
        saved.iter().any(|e| matches!(e, Effect::WriteSession(s)
            if s.changelists["fix"].contains(Path::new("a.rs")))),
        "{saved:?}"
    );
}

#[test]
fn a_file_without_changes_leaves_its_changelist_and_an_empty_one_goes() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.act(Action::GitCommit);
    h.git_status(vec![file("a.rs")]);
    h.run(Command::Insert("e".to_owned()));
    h.typed("fix");
    h.run(Command::Newline);
    h.git_status(Vec::new());
    assert_eq!(h.ws.changelists().names().count(), 0);
}

#[test]
fn ticking_a_list_moves_a_stage_commit_to_ticks_and_says_it_in_the_mode() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.act(Action::GitCommit);
    let mut staged = file("a.rs");
    staged.code = crate::core::GitCode::new('M', ' ');
    h.git_status(vec![staged]);
    assert_eq!(
        h.ws.git_panel().map(|p| p.commit_mode()),
        Some(crate::core::git::CommitMode::Staged)
    );
    h.run(Command::Insert("e".to_owned()));
    h.typed("fix");
    h.run(Command::Newline);
    h.run(Command::Insert("j".to_owned()));
    assert_eq!(
        h.ws.git_panel().map(|p| p.commit_mode()),
        Some(crate::core::git::CommitMode::Ticked)
    );
}

#[test]
fn with_no_checks_chosen_the_commit_goes_straight() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready_to_commit(CommitChecks::default());
    h.run(Command::Newline);
    assert!(h.committed());
    assert!(
        !h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::CommitChecks { .. })))
    );
}

#[test]
fn a_clean_check_commits_without_asking_and_only_once() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready_to_commit(CommitChecks {
        todo: true,
        ..CommitChecks::default()
    });
    h.run(Command::Newline);
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::CommitChecks { todo: true, .. })))
    );
    assert!(!h.committed(), "checks first");
    h.lsp_event(Event::Git(GitEvent::CommitChecked {
        warnings: Vec::new(),
    }));
    assert!(h.committed());
    assert!(
        !h.ws.git_panel().expect("open").checks_passed,
        "next commit checks again"
    );
}

#[test]
fn a_warning_needs_y_and_anything_else_goes_back_with_the_message_kept() {
    let mut h = Harness::new(&[("a.rs", "x")]);
    h.ready_to_commit(CommitChecks {
        todo: true,
        ..CommitChecks::default()
    });
    h.run(Command::Newline);
    h.lsp_event(Event::Git(GitEvent::CommitChecked {
        warnings: vec!["a.rs: TODO".to_owned()],
    }));
    let notice =
        h.ws.git_panel()
            .and_then(|p| p.notice.clone())
            .expect("told");
    assert!(notice.text.contains("a.rs: TODO"), "{}", notice.text);
    h.run(Command::Newline);
    assert!(!h.committed());
    assert_eq!(
        h.ws.git_panel()
            .map(|p| p.message.text().to_owned())
            .as_deref(),
        Some("msg")
    );

    h.run(Command::Newline);
    h.lsp_event(Event::Git(GitEvent::CommitChecked {
        warnings: vec!["a.rs: TODO".to_owned()],
    }));
    h.run(Command::Insert("y".to_owned()));
    assert!(h.committed());
    assert_eq!(
        h.ws.git_panel()
            .map(|p| p.message.text().to_owned())
            .as_deref(),
        Some("msg")
    );
}

#[test]
fn an_error_in_a_committed_file_is_a_warning_only_when_chosen() {
    for (chosen, warned) in [(true, true), (false, false)] {
        let mut h = Harness::new(&[("a.rs", "x")]);
        h.open("a.rs");
        h.ready_to_commit(CommitChecks {
            problems: chosen,
            command: Some("true".to_owned()),
            ..CommitChecks::default()
        });
        h.publish("a.rs", (0, 0), (0, 1));
        h.lsp_event(Event::Git(GitEvent::StatusRead {
            branch: None,
            top: Some(PathBuf::from(ROOT)),
            files: Ok(vec![file("a.rs")]),
            state: crate::core::GitRepoState::default(),
        }));
        h.run(Command::Newline);
        h.lsp_event(Event::Git(GitEvent::CommitChecked {
            warnings: Vec::new(),
        }));
        assert_eq!(!h.committed(), warned);
    }
}

#[test]
fn every_new_git_effect_waits_for_trust() {
    let compare = Effect::Git(GitEffect::Compare {
        root: PathBuf::new(),
        base: String::new(),
        path: None,
    });
    let checks = Effect::Git(GitEffect::CommitChecks {
        root: PathBuf::new(),
        staged: true,
        paths: Vec::new(),
        untracked: Vec::new(),
        todo: false,
        command: None,
    });
    assert!(compare.needs_trust());
    assert!(checks.needs_trust());
}
