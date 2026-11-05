use super::*;
use crate::core::git_command::{GitEffect, GitEvent};
use crate::core::merge::MergeView;
use crate::core::run_state::RunEvent;

fn opened(name: &str, text: &str) -> Harness {
    let mut h = Harness::new(&[(name, text)]);
    h.open(&format!("{ROOT}/{name}"));
    h
}

fn doc_path(name: &str) -> PathBuf {
    Path::new(ROOT).join(name)
}

fn file_commit(hash: &str, name: &str) -> crate::core::GitFileCommit {
    crate::core::GitFileCommit {
        commit: git_commit(hash, &[]),
        file: git_file(name, GitChange::Modified),
    }
}

impl Harness {
    fn history_sent(&self) -> Option<PathBuf> {
        self.git.iter().rev().find_map(|e| match e {
            Effect::Git(GitEffect::FileHistory { path, .. }) => Some(path.clone()),
            _ => None,
        })
    }

    fn blame_sent(&self) -> Vec<u64> {
        self.git
            .iter()
            .filter_map(|e| match e {
                Effect::Git(GitEffect::Blame { version, .. }) => Some(*version),
                _ => None,
            })
            .collect()
    }

    fn current_doc(&self) -> DocumentId {
        self.ws.active().unwrap().0.doc
    }

    fn blame_answer(&mut self, version: u64) {
        let doc = self.current_doc();
        let blame = crate::core::GitBlame {
            commits: vec![crate::core::GitBlameCommit {
                hash: "abcdef1234".into(),
                author: "Ann".into(),
                time: 1,
            }],
            lines: vec![0, 0],
        };
        self.lsp_event(Event::Git(GitEvent::BlameRead {
            doc,
            version,
            result: Ok(blame),
            now: 100,
        }));
    }

    fn doc_version(&self) -> u64 {
        self.ws.active().unwrap().1.version()
    }
}

#[test]
fn file_history_asks_about_the_active_file_saving_it_first() {
    let mut h = opened("a.txt", "one\n");
    h.run(Command::Insert("x".into()));
    h.act(Action::GitFileHistory);
    assert_eq!(h.history_sent(), Some(doc_path("a.txt")));
    assert_eq!(h.written.len(), 1, "git reads the disk");
    assert_eq!(h.panel().tab, GitTab::History);
    assert!(h.panel().history_loading);
}

#[test]
fn with_no_file_open_history_says_so_and_asks_nothing() {
    let mut h = Harness::new(&[]);
    h.act(Action::GitFileHistory);
    assert_eq!(h.history_sent(), None);
    assert!(h.ws.status().unwrap().text.contains("Open a file"));
}

#[test]
fn history_shows_the_chosen_commits_diff_under_the_name_it_had_then() {
    let mut h = opened("new.txt", "one\n");
    h.act(Action::GitFileHistory);
    h.lsp_event(Event::Git(GitEvent::FileHistoryRead {
        path: doc_path("new.txt"),
        result: Ok(vec![
            file_commit("c2", "new.txt"),
            file_commit("c1", "old.txt"),
        ]),
        now: 500,
    }));
    assert!(!h.panel().history_loading);
    assert_eq!(h.panel().now, 500);
    let hash_and_file = |h: &Harness| {
        let GitDiffSource::Commit { hash, file } = h.last_diff().unwrap().1 else {
            panic!("not a commit's diff");
        };
        (hash, file.path)
    };
    assert_eq!(
        hash_and_file(&h),
        ("c2".to_owned(), PathBuf::from("new.txt"))
    );
    h.run(Command::Move(Motion::Down));
    assert_eq!(
        hash_and_file(&h),
        ("c1".to_owned(), PathBuf::from("old.txt"))
    );
    h.run(Command::Move(Motion::Down));
    assert_eq!(h.panel().history_selected, 1, "stops at the oldest");
    h.run(Command::SelectGitRow { index: 0 });
    assert_eq!(
        hash_and_file(&h),
        ("c2".to_owned(), PathBuf::from("new.txt"))
    );
}

#[test]
fn enter_in_history_opens_the_current_file_even_from_an_old_commit() {
    let mut h = Harness::new(&[("a.txt", "a\n"), ("b.txt", "b\n")]);
    h.run(Command::OpenFile {
        path: doc_path("a.txt"),
        at: None,
    });
    h.act(Action::GitFileHistory);
    h.lsp_event(Event::Git(GitEvent::FileHistoryRead {
        path: doc_path("a.txt"),
        result: Ok(vec![
            file_commit("c2", "a.txt"),
            file_commit("c1", "older.txt"),
        ]),
        now: 5,
    }));
    h.run(Command::Move(Motion::Down));
    h.run(Command::OpenFile {
        path: doc_path("b.txt"),
        at: None,
    });
    h.run(Command::Focus(Focus::Git));
    h.run(Command::Newline);
    assert_eq!(h.ws.active().unwrap().1.path(), doc_path("a.txt"));
}

#[test]
fn an_answer_about_a_file_no_longer_asked_about_is_dropped() {
    let mut h = opened("a.txt", "a\n");
    h.act(Action::GitFileHistory);
    h.lsp_event(Event::Git(GitEvent::FileHistoryRead {
        path: doc_path("elsewhere.txt"),
        result: Ok(vec![file_commit("c9", "elsewhere.txt")]),
        now: 1,
    }));
    assert!(h.panel().history.is_empty());
    assert!(h.panel().history_loading);
    h.lsp_event(Event::Git(GitEvent::FileHistoryRead {
        path: doc_path("a.txt"),
        result: Err("fatal: bad".into()),
        now: 1,
    }));
    assert_eq!(h.panel().notice.as_ref().unwrap().text, "fatal: bad");
}

#[test]
fn blame_saves_first_asks_once_and_is_stale_the_moment_the_text_changes() {
    let mut h = opened("a.txt", "one\ntwo\n");
    h.act(Action::GitBlame);
    let version = h.doc_version();
    assert_eq!(h.blame_sent(), [version], "asked without waiting");
    assert!(
        h.ws.blame(h.current_doc()).unwrap().blame.is_none(),
        "nothing known yet"
    );
    h.blame_answer(version);
    let doc = h.current_doc();
    assert_eq!(
        h.ws.blame(doc)
            .unwrap()
            .blame
            .unwrap()
            .line(1)
            .unwrap()
            .author,
        "Ann"
    );
    h.run(Command::Insert("x".into()));
    assert!(h.ws.blame(doc).unwrap().blame.is_none(), "lines have moved");
    assert_eq!(
        h.blame_sent().len(),
        1,
        "unsaved text is not on disk to ask about"
    );
    h.blame_answer(version);
    assert!(h.ws.blame(doc).unwrap().blame.is_none());
    h.act(Action::Save);
    let saved = h.doc_version();
    assert_eq!(h.blame_sent(), [version, saved], "saved: ask again");
    h.blame_answer(saved);
    assert!(h.ws.blame(doc).unwrap().blame.is_some());
}

#[test]
fn blame_toggles_off_and_a_refusal_turns_it_off_with_git_words() {
    let mut h = opened("a.txt", "one\n");
    h.act(Action::GitBlame);
    let doc = h.current_doc();
    h.act(Action::GitBlame);
    assert!(h.ws.blame(doc).is_none());
    h.act(Action::GitBlame);
    h.lsp_event(Event::Git(GitEvent::BlameRead {
        doc,
        version: 0,
        result: Err("fatal: no such path 'a.txt' in HEAD".into()),
        now: 1,
    }));
    assert!(h.ws.blame(doc).is_none());
    assert!(h.ws.status().unwrap().text.contains("no such path"));
}

#[test]
fn a_commit_asks_for_blame_again_as_the_same_text_has_new_answers() {
    let mut h = opened("a.txt", "one\n");
    h.act(Action::GitBlame);
    let before = h.blame_sent().len();
    h.commit_view(vec![git_file("a.txt", GitChange::Modified)]);
    assert_eq!(h.blame_sent().len(), before, "nothing changed yet");
    h.lsp_event(Event::Git(GitEvent::Committed {
        result: Ok("abc done".into()),
    }));
    assert_eq!(h.blame_sent().len(), before + 1);
}

#[test]
fn a_document_closed_while_blamed_is_forgotten() {
    let mut h = opened("a.txt", "one\n");
    h.act(Action::GitBlame);
    let doc = h.current_doc();
    h.act(Action::CloseTab);
    h.lsp_event(Event::Git(GitEvent::BlameRead {
        doc,
        version: 1,
        result: Ok(crate::core::GitBlame::default()),
        now: 1,
    }));
    assert!(h.ws.blame(doc).is_none());
}

fn coded(path: &str, change: GitChange, index: char, worktree: char) -> GitFile {
    let mut file = git_file(path, change);
    file.code = crate::core::GitCode::new(index, worktree);
    file
}

fn hunk(side: crate::core::HunkSide, title: &str) -> crate::core::GitHunk {
    crate::core::GitHunk {
        side,
        whole: false,
        patch: title.as_bytes().to_vec(),
        title: title.to_owned(),
        rows: vec![DiffRow {
            kind: DiffKind::Added,
            old_line: None,
            new_line: Some(1),
            text: format!("text of {title}"),
        }],
    }
}

impl Harness {
    fn state_status(&mut self, files: Vec<GitFile>, state: crate::core::GitRepoState) {
        self.lsp_event(Event::Git(GitEvent::StatusRead {
            branch: Some("main".into()),
            top: Some(PathBuf::from("/repo")),
            files: Ok(files),
            state,
        }));
    }

    fn hunks_arrive(&mut self, hunks: Vec<crate::core::GitHunk>) {
        let (id, _) = self.last_hunks().expect("asked");
        self.lsp_event(Event::Git(GitEvent::HunksRead {
            id,
            result: Ok(hunks),
        }));
    }

    fn write_message(&mut self, text: &str) {
        self.run(Command::Indent);
        while self.panel().field != GitField::Message {
            self.run(Command::Indent);
        }
        self.typed(text);
    }
}

#[test]
fn the_commit_takes_ticks_until_something_is_staged_then_the_index() {
    use crate::core::CommitMode::{Staged, Ticked};
    let mut h = Harness::new(&[]);
    h.commit_view(vec![git_file("a", GitChange::Modified)]);
    assert_eq!(h.panel().commit_mode(), Ticked);
    h.git_status(vec![coded("a", GitChange::Modified, 'M', 'M')]);
    assert_eq!(
        h.panel().commit_mode(),
        Staged,
        "plain git would take the index"
    );
    h.run(Command::Insert("s".into()));
    assert_eq!(h.panel().commit_mode(), Ticked, "the user's word");
    h.git_status(vec![coded("a", GitChange::Modified, 'M', ' ')]);
    assert_eq!(h.panel().commit_mode(), Ticked, "it stays until the commit");
    h.lsp_event(Event::Git(GitEvent::Committed {
        result: Ok("abc x".into()),
    }));
    assert_eq!(h.panel().commit_mode(), Staged);
}

#[test]
fn a_staged_commit_goes_with_the_id_of_the_index_it_showed() {
    let mut h = Harness::new(&[]);
    h.commit_view(Vec::new());
    h.state_status(
        vec![coded("a", GitChange::Modified, 'M', 'M')],
        crate::core::GitRepoState {
            operation: None,
            staged_id: "shown".into(),
        },
    );
    h.write_message("part of a");
    h.run(Command::Newline);
    let sent = h.git.iter().rev().find_map(|e| match e {
        Effect::Git(GitEffect::CommitStaged {
            message, staged_id, ..
        }) => Some((message.clone(), staged_id.clone())),
        _ => None,
    });
    assert_eq!(sent, Some(("part of a".to_owned(), "shown".to_owned())));
    assert_eq!(h.commit_sent(), None, "not the ticked way as well");
}

#[test]
fn with_ticks_chosen_a_commit_ignores_the_index_as_before() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![coded("a", GitChange::Modified, 'M', 'M')]);
    h.run(Command::Insert("s".into()));
    h.write_message("whole");
    h.run(Command::Newline);
    assert_eq!(
        h.commit_sent(),
        Some(("whole".to_owned(), vec![PathBuf::from("a")], Vec::new()))
    );
}

#[test]
fn nothing_staged_refuses_with_a_way_out() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![git_file("a", GitChange::Modified)]);
    h.run(Command::Insert("s".into()));
    h.write_message("msg");
    h.run(Command::Newline);
    assert!(
        h.panel()
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("Nothing staged")
    );
    assert!(
        !h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::CommitStaged { .. })))
    );
}

#[test]
fn space_moves_the_chosen_hunk_and_the_file_is_reread_after() {
    use crate::core::HunkSide::{Staged, Unstaged};
    let mut h = Harness::new(&[]);
    h.commit_view(vec![coded("a", GitChange::Modified, 'M', 'M')]);
    h.hunks_arrive(vec![
        hunk(Staged, "staged one"),
        hunk(Unstaged, "first"),
        hunk(Unstaged, "second"),
    ]);
    assert_eq!(
        h.panel().hunks[0].title,
        "first",
        "as drawn: unstaged on top"
    );
    assert_eq!(h.panel().diff[0].text, "Not staged (2)");
    h.run(Command::Indent);
    assert_eq!(h.panel().field, GitField::Hunks);
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    assert_eq!(h.panel().hunk_selected, 2);
    assert_eq!(h.panel().diff[h.panel().diff_scroll].text, "staged one");
    h.run(Command::Move(Motion::Down));
    assert_eq!(h.panel().hunk_selected, 2, "stops at the last");
    h.run(Command::Insert(" ".into()));
    let moved = h.git.iter().rev().find_map(|e| match e {
        Effect::Git(GitEffect::ApplyHunk { hunk, .. }) => Some(hunk.title.clone()),
        _ => None,
    });
    assert_eq!(moved.as_deref(), Some("staged one"));
    assert!(h.panel().busy);
    let asks = h
        .git
        .iter()
        .filter(|e| matches!(e, Effect::Git(GitEffect::ApplyHunk { .. })))
        .count();
    h.run(Command::Insert(" ".into()));
    assert_eq!(
        h.git
            .iter()
            .filter(|e| matches!(e, Effect::Git(GitEffect::ApplyHunk { .. })))
            .count(),
        asks,
        "one write to the index at a time"
    );
    h.git.clear();
    h.lsp_event(Event::Git(GitEvent::StageDone { result: Ok(()) }));
    assert!(!h.panel().busy);
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Status { .. })))
    );
    h.git.clear();
    h.git_status(vec![coded("a", GitChange::Modified, ' ', 'M')]);
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Hunks { .. })))
    );
}

#[test]
fn git_words_about_a_refused_patch_reach_the_panel() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![git_file("a", GitChange::Modified)]);
    h.lsp_event(Event::Git(GitEvent::StageDone {
        result: Err("error: patch failed: a:1".into()),
    }));
    assert_eq!(
        h.panel().notice.as_ref().unwrap().text,
        "error: patch failed: a:1"
    );
    assert!(h.panel().notice.as_ref().unwrap().error);
}

#[test]
fn staged_unsaved_text_goes_to_disk_before_git_and_a_document_at_odds_stops_it() {
    let mut h = opened("a.txt", "one\n");
    h.commit_view(vec![coded("a.txt", GitChange::Modified, 'M', ' ')]);
    h.hunks_arrive(vec![hunk(crate::core::HunkSide::Staged, "x")]);
    let doc = h.current_doc();
    h.run(Command::Focus(Focus::Editor));
    h.run(Command::Insert("typed".into()));
    h.run(Command::Focus(Focus::Git));
    h.run(Command::Indent);
    let before = h.written.len();
    h.run(Command::Insert(" ".into()));
    assert_eq!(h.written.len(), before + 1, "saved first");
    h.lsp_event(Event::Git(GitEvent::StageDone { result: Ok(()) }));

    h.run(Command::Focus(Focus::Editor));
    h.run(Command::Insert("more".into()));
    h.lsp_event(Event::FileChanged {
        doc,
        disk: None,
        result: Ok(Some(b"changed outside\n".to_vec())),
    });
    h.run(Command::Focus(Focus::Git));
    let moves = h
        .git
        .iter()
        .filter(|e| matches!(e, Effect::Git(GitEffect::ApplyHunk { .. })))
        .count();
    h.run(Command::Insert(" ".into()));
    assert_eq!(
        h.git
            .iter()
            .filter(|e| matches!(e, Effect::Git(GitEffect::ApplyHunk { .. })))
            .count(),
        moves
    );
    assert!(
        h.panel()
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("unsaved edits")
    );
}

#[test]
fn space_and_a_stage_whole_files_when_the_commit_takes_the_index() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![
        coded("a", GitChange::Modified, 'M', 'M'),
        coded("b", GitChange::Untracked, '?', '?'),
        coded("c", GitChange::Modified, 'M', ' '),
        coded("!", GitChange::Conflicted, 'U', 'U'),
    ]);
    let moved = |h: &Harness| {
        h.git.iter().rev().find_map(|e| match e {
            Effect::Git(GitEffect::StageFiles { files, stage, .. }) => Some((
                files.iter().map(|f| f.path.clone()).collect::<Vec<_>>(),
                *stage,
            )),
            _ => None,
        })
    };
    h.run(Command::Insert("a".into()));
    assert_eq!(
        moved(&h),
        Some((vec![PathBuf::from("a"), PathBuf::from("b")], true)),
        "not the conflict"
    );
    h.lsp_event(Event::Git(GitEvent::StageDone { result: Ok(()) }));
    h.git_status(vec![coded("a", GitChange::Modified, 'M', ' ')]);
    h.run(Command::Insert(" ".into()));
    assert_eq!(
        moved(&h),
        Some((vec![PathBuf::from("a")], false)),
        "out again"
    );
}

const CONFLICTED: &str = "top\n<<<<<<< HEAD\nmine\n=======\nyours\n>>>>>>> other\nmid\n<<<<<<< HEAD\nA\n=======\nB\n>>>>>>> other\n";

fn merging() -> crate::core::GitRepoState {
    crate::core::GitRepoState {
        operation: Some(crate::core::GitOperation::Merging),
        staged_id: String::new(),
    }
}

fn in_conflict() -> Harness {
    let mut h = Harness::new(&[("c.txt", CONFLICTED)]);
    h.act(Action::GitCommit);
    h.lsp_event(Event::Git(GitEvent::StatusRead {
        branch: Some("main".into()),
        top: Some(PathBuf::from(ROOT)),
        files: Ok(vec![coded("c.txt", GitChange::Conflicted, 'U', 'U')]),
        state: merging(),
    }));
    h
}

fn text_now(h: &Harness) -> String {
    h.ws.active().unwrap().1.text().to_string()
}

fn cursor_line(h: &Harness) -> usize {
    let (view, doc) = h.ws.active().unwrap();
    doc.text().char_to_line(view.cursor.idx)
}

#[test]
fn stepping_opens_the_file_at_its_first_conflict_and_keeps_the_keys_in_the_panel() {
    let mut h = in_conflict();
    h.run(Command::Insert("n".into()));
    assert_eq!(cursor_line(&h), 1, "at the marker");
    assert_eq!(h.ws.focus(), Focus::Git);
    h.run(Command::Insert("n".into()));
    assert_eq!(cursor_line(&h), 7);
    h.run(Command::Insert("n".into()));
    assert_eq!(cursor_line(&h), 1, "round again");
    h.run(Command::Insert("p".into()));
    assert_eq!(cursor_line(&h), 7);
}

#[test]
fn a_choice_needs_the_text_on_screen_first_and_is_one_step_of_undo() {
    let mut h = in_conflict();
    h.run(Command::Insert("o".into()));
    assert_eq!(text_now(&h), CONFLICTED, "opened, nothing chosen unseen");
    assert!(
        h.panel()
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("press again")
    );
    h.run(Command::Insert("o".into()));
    assert_eq!(
        text_now(&h),
        "top\nmine\nmid\n<<<<<<< HEAD\nA\n=======\nB\n>>>>>>> other\n"
    );
    assert!(
        h.panel()
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("1 conflicts left")
    );
    h.run(Command::Insert("n".into()));
    h.run(Command::Insert("b".into()));
    assert_eq!(text_now(&h), "top\nmine\nmid\nA\nB\n");
    h.act(Action::Undo);
    assert!(text_now(&h).contains("<<<<<<<"), "undone in one step");
    assert!(text_now(&h).contains("mid\n<<<<<<< HEAD\nA"));
    h.act(Action::Undo);
    assert_eq!(text_now(&h), CONFLICTED);
    h.act(Action::Redo);
    assert!(text_now(&h).starts_with("top\nmine\nmid"));
}

#[test]
fn accepting_from_the_palette_works_on_the_conflict_under_the_cursor() {
    let mut h = Harness::new(&[("c.txt", CONFLICTED)]);
    h.open(&format!("{ROOT}/c.txt"));
    h.act(Action::GitNextConflict);
    h.act(Action::GitNextConflict);
    h.act(Action::GitAcceptTheirs);
    assert_eq!(
        text_now(&h),
        "top\n<<<<<<< HEAD\nmine\n=======\nyours\n>>>>>>> other\nmid\nB\n"
    );
    h.act(Action::GitAcceptTheirs);
    assert!(
        h.ws.status().unwrap().text.contains("No conflict"),
        "below the cursor none"
    );
}

#[test]
fn marking_resolved_needs_a_second_press_while_markers_remain() {
    let mut h = in_conflict();
    let added = |h: &Harness| {
        h.git
            .iter()
            .filter(|e| matches!(e, Effect::Git(GitEffect::StageFiles { stage: true, .. })))
            .count()
    };
    h.run(Command::Insert(" ".into()));
    assert_eq!(added(&h), 0);
    assert!(
        h.panel()
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("not been looked at")
    );
    h.run(Command::Move(Motion::Down));
    h.run(Command::Insert(" ".into()));
    assert_eq!(added(&h), 0, "another key took the question back");
    assert!(
        h.panel()
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("2 conflicts")
    );
    h.run(Command::Insert(" ".into()));
    assert_eq!(added(&h), 1, "the second press in a row");
}

#[test]
fn a_file_with_no_markers_left_is_added_at_once_saved_first() {
    let mut h = in_conflict();
    h.run(Command::Insert("n".into()));
    h.run(Command::Insert("o".into()));
    h.run(Command::Insert("n".into()));
    h.run(Command::Insert("t".into()));
    assert!(
        h.panel()
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("No conflicts left")
    );
    let before = h.written.len();
    h.run(Command::Insert(" ".into()));
    assert_eq!(h.written.len(), before + 1);
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::StageFiles { stage: true, .. })))
    );
}

#[test]
fn a_side_that_deleted_the_file_has_nothing_to_choose_between() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![coded("gone", GitChange::Conflicted, 'U', 'D')]);
    h.run(Command::Insert("o".into()));
    assert!(h.panel().notice.as_ref().unwrap().text.contains("deleted"));
    h.run(Command::Insert(" ".into()));
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::StageFiles { .. }))),
        "no markers to wait for"
    );
}

fn ops(h: &Harness) -> Vec<crate::core::GitOp> {
    h.git
        .iter()
        .filter_map(|e| match e {
            Effect::Git(GitEffect::Op { op, .. }) => Some(op.clone()),
            _ => None,
        })
        .collect()
}

#[test]
fn abort_says_what_goes_needs_a_second_press_and_keeps_the_text() {
    use crate::core::{GitOp, GitOperation};
    let mut h = in_conflict();
    h.run(Command::Insert("x".into()));
    assert!(ops(&h).is_empty());
    let text = &h.panel().notice.as_ref().unwrap().text;
    assert!(text.contains("Press X again") && text.contains("merge") && text.contains("discarded"));
    h.run(Command::Move(Motion::Up));
    h.run(Command::Insert("x".into()));
    assert!(ops(&h).is_empty(), "the question was taken back");
    h.run(Command::Insert("x".into()));
    assert_eq!(ops(&h), [GitOp::Abort(GitOperation::Merging)]);
    let keep = h.git.iter().find_map(|e| match e {
        Effect::Git(GitEffect::Op { keep, .. }) => Some(keep.clone()),
        _ => None,
    });
    assert_eq!(keep, Some(vec![doc_path("c.txt")]), "Local History first");
}

#[test]
fn continuing_goes_by_what_git_is_in_the_middle_of_and_nothing_means_nothing() {
    use crate::core::{GitOp, GitOperation};
    let mut h = in_conflict();
    h.run(Command::Insert("c".into()));
    assert_eq!(ops(&h), [GitOp::Continue(GitOperation::Merging)]);
    h.lsp_event(Event::Git(GitEvent::OpDone {
        op: GitOp::Continue(GitOperation::Merging),
        result: Err("error: Committing is not possible".into()),
    }));
    assert!(
        h.panel()
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("not possible")
    );
    let mut quiet = Harness::new(&[]);
    quiet.commit_view(Vec::new());
    quiet.run(Command::Insert("c".into()));
    quiet.run(Command::Insert("x".into()));
    assert!(ops(&quiet).is_empty());
}

#[test]
fn during_a_merge_the_commit_takes_the_index_and_waits_for_the_conflicts() {
    use crate::core::CommitMode;
    let mut h = in_conflict();
    assert_eq!(h.panel().commit_mode(), CommitMode::Staged);
    h.run(Command::Insert("s".into()));
    assert_eq!(
        h.panel().commit_mode(),
        CommitMode::Staged,
        "no way around it"
    );
    h.write_message("the merge");
    h.run(Command::Newline);
    assert!(
        h.panel()
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("Resolve the conflicts")
    );
    assert!(
        !h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::CommitStaged { .. })))
    );
}

#[test]
fn stash_conflicts_are_conflicts_with_nothing_to_abort() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![coded("f", GitChange::Conflicted, 'U', 'U')]);
    h.run(Command::Insert("x".into()));
    assert!(
        h.panel()
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("came from a stash")
    );
    assert!(ops(&h).is_empty());
}

fn stash(index: usize, hash: &str) -> crate::core::GitStash {
    crate::core::GitStash {
        index,
        hash: hash.into(),
        time: 1,
        subject: format!("On main: {hash}"),
    }
}

fn in_stash_tab() -> Harness {
    let mut h = Harness::new(&[]);
    h.act(Action::GitStash);
    h.lsp_event(Event::Git(GitEvent::StashesRead {
        result: Ok(vec![stash(0, "aaa"), stash(1, "bbb")]),
        now: 9,
    }));
    h
}

#[test]
fn stash_keys_act_on_the_chosen_entry_and_show_its_diff() {
    use crate::core::GitOp;
    let mut h = in_stash_tab();
    let shown = |h: &Harness| h.last_diff().unwrap().1;
    assert_eq!(shown(&h), GitDiffSource::Stash("aaa".into()));
    h.run(Command::Move(Motion::Down));
    assert_eq!(shown(&h), GitDiffSource::Stash("bbb".into()));
    h.run(Command::Insert("a".into()));
    assert_eq!(ops(&h), [GitOp::StashApply(stash(1, "bbb"))]);
    h.lsp_event(Event::Git(GitEvent::OpDone {
        op: GitOp::StashApply(stash(1, "bbb")),
        result: Ok("ok".into()),
    }));
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::ListStashes { .. })))
    );
    h.run(Command::Insert("o".into()));
    assert_eq!(ops(&h).last(), Some(&GitOp::StashPop(stash(1, "bbb"))));
}

#[test]
fn dropping_a_stash_says_which_and_needs_a_second_press_on_the_same_one() {
    use crate::core::GitOp;
    let mut h = in_stash_tab();
    h.run(Command::Insert("d".into()));
    assert!(ops(&h).is_empty());
    let text = &h.panel().notice.as_ref().unwrap().text;
    assert!(
        text.contains("stash@{0}") && text.contains("lost"),
        "{text}"
    );
    h.run(Command::Move(Motion::Down));
    h.run(Command::Insert("d".into()));
    assert!(ops(&h).is_empty(), "moving away took the question back");
    h.run(Command::Insert("d".into()));
    assert_eq!(ops(&h), [GitOp::StashDrop(stash(1, "bbb"))]);
}

#[test]
fn a_new_stash_takes_a_name_and_may_take_untracked_files_saving_first() {
    use crate::core::GitOp;
    let mut h = opened("a.txt", "x\n");
    h.run(Command::Insert("typed".into()));
    h.act(Action::GitStash);
    h.lsp_event(Event::Git(GitEvent::StashesRead {
        result: Ok(Vec::new()),
        now: 1,
    }));
    h.run(Command::Insert("n".into()));
    assert_eq!(h.panel().field, GitField::StashMessage);
    h.typed("wip d");
    h.run(Command::Indent);
    assert!(h.panel().stash_untracked);
    let before = h.written.len();
    h.run(Command::Newline);
    assert_eq!(
        ops(&h),
        [GitOp::StashPush {
            message: "wip d".into(),
            untracked: true
        }]
    );
    assert!(h.written.len() >= before, "and text on disk");
    assert_eq!(h.panel().field, GitField::List);
}

#[test]
fn a_stash_is_refused_while_a_document_is_at_odds_with_the_disk() {
    let mut h = opened("a.txt", "x\n");
    let doc = h.current_doc();
    h.run(Command::Insert("mine".into()));
    h.lsp_event(Event::FileChanged {
        doc,
        disk: None,
        result: Ok(Some(b"theirs\n".to_vec())),
    });
    h.act(Action::GitStash);
    h.lsp_event(Event::Git(GitEvent::StashesRead {
        result: Ok(Vec::new()),
        now: 1,
    }));
    h.run(Command::Insert("n".into()));
    h.run(Command::Newline);
    assert!(ops(&h).is_empty());
    assert!(
        h.panel()
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("unsaved edits")
    );
}

fn branch(name: &str, remote: bool) -> GitBranch {
    GitBranch {
        name: name.into(),
        remote,
    }
}

fn in_branches() -> Harness {
    let mut h = Harness::new(&[]);
    h.act(Action::GitManageBranches);
    h.git_status(Vec::new());
    h.lsp_event(Event::Git(GitEvent::BranchesRead {
        result: Ok(vec![
            branch("main", false),
            branch("topic", false),
            branch("origin/topic", true),
        ]),
    }));
    h
}

#[test]
fn deleting_a_branch_starts_with_d_and_only_git_refusal_opens_the_way_to_force() {
    use crate::core::GitOp;
    let mut h = in_branches();
    assert!(h.ws.picker().is_none(), "the list is in the tab");
    h.run(Command::Move(Motion::Down));
    h.run(Command::Insert("f".into()));
    assert!(ops(&h).is_empty(), "no refusal yet, no force");
    h.run(Command::Insert("d".into()));
    let soft = GitOp::BranchDelete {
        name: "topic".into(),
        force: false,
    };
    assert_eq!(ops(&h), std::slice::from_ref(&soft));
    h.lsp_event(Event::Git(GitEvent::OpDone {
        op: soft.clone(),
        result: Err("error: The branch 'topic' is not fully merged.".into()),
    }));
    let text = &h.panel().notice.as_ref().unwrap().text;
    assert!(
        text.contains("not fully merged")
            && text.contains("Press F")
            && text.contains("unreachable")
    );
    h.run(Command::Move(Motion::Up));
    h.run(Command::Move(Motion::Down));
    h.run(Command::Insert("f".into()));
    assert_eq!(ops(&h).len(), 1);
    h.run(Command::Insert("d".into()));
    h.lsp_event(Event::Git(GitEvent::OpDone {
        op: soft,
        result: Err("error: The branch 'topic' is not fully merged.".into()),
    }));
    h.run(Command::Insert("f".into()));
    assert_eq!(
        ops(&h).last(),
        Some(&GitOp::BranchDelete {
            name: "topic".into(),
            force: true
        })
    );
}

#[test]
fn a_remote_branch_is_not_deleted_and_merge_takes_another_branch() {
    use crate::core::GitOp;
    let mut h = in_branches();
    h.run(Command::Insert("m".into()));
    assert!(ops(&h).is_empty(), "main into main");
    h.run(Command::Move(Motion::Down));
    h.run(Command::Move(Motion::Down));
    h.run(Command::Insert("d".into()));
    assert!(ops(&h).is_empty());
    assert!(
        h.panel()
            .notice
            .as_ref()
            .unwrap()
            .text
            .contains("that is a push")
    );
    h.run(Command::Insert("m".into()));
    assert_eq!(
        ops(&h),
        [GitOp::Merge {
            branch: "origin/topic".into()
        }]
    );
}

#[test]
fn a_merge_that_conflicts_leads_to_the_commit_view_with_git_words_and_reloads() {
    use crate::core::GitOp;
    let mut h = in_branches();
    h.run(Command::Move(Motion::Down));
    h.run(Command::Insert("m".into()));
    h.git.clear();
    h.lsp_event(Event::Git(GitEvent::OpDone {
        op: GitOp::Merge {
            branch: "topic".into(),
        },
        result: Err("CONFLICT (content): Merge conflict in f".into()),
    }));
    assert_eq!(h.panel().tab, GitTab::Commit);
    assert!(h.panel().notice.as_ref().unwrap().text.contains("CONFLICT"));
    assert!(!h.panel().busy);
}

#[test]
fn every_operation_rechecks_open_documents_on_disk() {
    use crate::core::GitOp;
    let mut h = opened("a.txt", "x\n");
    h.commit_view(Vec::new());
    let mut seen = Vec::new();
    let effects = h.ws.handle(Event::Git(GitEvent::OpDone {
        op: GitOp::Merge { branch: "t".into() },
        result: Ok("done".into()),
    }));
    seen.extend(effects);
    assert!(seen.iter().any(|e| matches!(e, Effect::CheckFile { .. })));
    assert!(
        seen.iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::Log { .. })))
    );
}

fn run_target() -> crate::core::RunTarget {
    crate::core::RunTarget {
        name: "the build".into(),
        command: crate::core::run::RunCommand::Shell("make".into()),
        cwd: PathBuf::from(ROOT),
        source: crate::core::run::RunSource::Detected,
    }
}

fn plain(text: &str) -> crate::core::RunLine {
    crate::core::RunLine {
        spans: Vec::new(),
        text: text.into(),
        location: None,
    }
}

fn located(text: &str, path: &str, line: usize, col: usize) -> crate::core::RunLine {
    crate::core::RunLine {
        spans: Vec::new(),
        text: text.into(),
        location: Some(crate::core::RunLocation {
            path: PathBuf::from(path),
            line,
            col,
            span: (0, 1),
        }),
    }
}

fn run_window() -> Harness {
    let (ws, effects) = Workspace::new_run_window(PathBuf::from(ROOT));
    assert_eq!(
        effects,
        [Effect::RunFollow {
            root: PathBuf::from(ROOT)
        }],
        "no documents, session, swaps or watcher of its own"
    );
    let mut h = Harness::new(&[]);
    h.ws = ws;
    h
}

fn stream_out(h: &Harness) -> Vec<RunStreamMessage> {
    h.git
        .iter()
        .filter_map(|e| match e {
            Effect::RunStreamSend(m) | Effect::RunStreamStart(m) => Some(m.clone()),
            _ => None,
        })
        .collect()
}

use crate::core::RunStreamMessage;

#[test]
fn the_main_window_tells_nobody_until_someone_follows() {
    let mut h = Harness::new(&[]);
    h.lsp_event(Event::Run(RunEvent::Output {
        id: 1,
        lines: vec![plain("x")],
        partial: String::new(),
        dropped: 0,
    }));
    assert!(stream_out(&h).is_empty(), "and pays nothing for it");
    h.lsp_event(Event::RunWatcherAttached);
    assert_eq!(stream_out(&h), [RunStreamMessage::Snapshot(None)]);
}

#[test]
fn a_follower_sees_what_already_ran_then_what_follows() {
    let mut h = Harness::new(&[]);
    h.lsp_event(Event::RunWatcherAttached);
    h.git.clear();
    h.ws.configure_run(vec![crate::core::ConfiguredRun {
        name: "own".into(),
        command: "true".into(),
        cwd: None,
        from_project: false,
    }]);
    h.act(Action::RunChoose);
    h.run(Command::Newline);
    let id = match stream_out(&h).first() {
        Some(RunStreamMessage::Started { id, target }) => {
            assert_eq!(target.name, "own");
            *id
        }
        other => panic!("{other:?}"),
    };
    h.lsp_event(Event::Run(RunEvent::Output {
        id,
        lines: vec![plain("line one")],
        partial: "par".into(),
        dropped: 0,
    }));
    assert!(matches!(
        stream_out(&h).last(),
        Some(RunStreamMessage::Output { lines, partial, .. })
            if lines[0].text == "line one" && partial == "par"
    ));
    h.lsp_event(Event::Run(RunEvent::Finished {
        id,
        outcome: crate::core::RunOutcome::Exited(0),
        duration_ms: 5,
    }));
    assert!(matches!(
        stream_out(&h).last(),
        Some(RunStreamMessage::Finished { .. })
    ));
    h.git.clear();
    h.lsp_event(Event::RunWatcherAttached);
    let Some(RunStreamMessage::Snapshot(Some(snapshot))) = stream_out(&h).pop() else {
        panic!("{:?}", stream_out(&h));
    };
    assert_eq!(snapshot.lines[0].text, "line one");
    assert!(matches!(
        snapshot.status,
        crate::core::RunStatus::Finished { .. }
    ));
}

#[test]
fn a_snapshot_keeps_line_numbers_and_status_on_the_other_side() {
    let mut panel = crate::core::RunPanel::new(4, run_target());
    panel.push(vec![plain("old")], String::new(), 7);
    panel.push(vec![plain("new")], "unfinished".into(), 0);
    let copy = crate::core::RunPanel::from_snapshot(panel.snapshot());
    assert_eq!(copy.first_line(), panel.first_line());
    assert_eq!(copy.end_line(), panel.end_line());
    assert_eq!(copy.line(8).unwrap().text, "new");
    assert_eq!(copy.partial, "unfinished");
    assert_eq!(copy.id, 4);
}

#[test]
fn the_run_window_shows_the_stream_and_enter_asks_the_main_window_at_the_place() {
    let mut h = run_window();
    h.lsp_event(Event::RunStreamState { connected: true });
    assert!(h.ws.run_connected());
    h.lsp_event(Event::RunStream(RunStreamMessage::Started {
        id: 9,
        target: run_target(),
    }));
    h.lsp_event(Event::RunStream(RunStreamMessage::Output {
        id: 9,
        lines: vec![
            plain("compiling"),
            located("a.rs:3:7: boom", "/p/a.rs", 2, 6),
        ],
        partial: String::new(),
        dropped: 0,
    }));
    h.lsp_event(Event::RunStream(RunStreamMessage::Output {
        id: 8,
        lines: vec![plain("stale")],
        partial: String::new(),
        dropped: 0,
    }));
    let panel = h.ws.run_panel().unwrap();
    assert_eq!(panel.end_line(), 2);
    h.git.clear();
    h.run(Command::Insert("n".into()));
    assert!(h.git.contains(&Effect::OpenInMainWindow {
        root: PathBuf::from(ROOT),
        path: PathBuf::from("/p/a.rs"),
        at: Some((2, 6)),
    }));
    assert!(h.ws.active().is_none(), "nothing opened here");
    h.lsp_event(Event::OpenedInMainWindow {
        result: Err("The main window is not running".into()),
    });
    assert!(h.ws.status().unwrap().text.contains("not running"));
    h.lsp_event(Event::RunStream(RunStreamMessage::Finished {
        id: 9,
        outcome: crate::core::RunOutcome::Exited(1),
        duration_ms: 1,
    }));
    assert!(!h.ws.run_panel().unwrap().is_active());
}

#[test]
fn the_run_window_reads_and_points_and_does_nothing_else() {
    let mut h = run_window();
    h.lsp_event(Event::RunStream(RunStreamMessage::Started {
        id: 1,
        target: run_target(),
    }));
    h.git.clear();
    for input in ["s", "r", "w"] {
        h.run(Command::Insert(input.into()));
    }
    for action in [
        Action::StopRun,
        Action::RerunLast,
        Action::RunChoose,
        Action::GitLog,
        Action::GitCommit,
        Action::Find,
        Action::OpenPalette,
        Action::RunOutputWindow,
    ] {
        h.act(action);
    }
    assert!(h.runs.is_empty(), "{:?}", h.runs);
    assert!(h.git.is_empty(), "{:?}", h.git);
    assert_eq!(h.ws.focus(), Focus::Run);
    assert!(h.ws.picker().is_none());
    h.act(Action::Quit);
    assert!(h.ws.should_quit(), "it can always be closed");
}

#[test]
fn the_action_asks_to_open_a_window_and_not_from_a_window() {
    let mut h = Harness::new(&[]);
    h.act(Action::RunOutputWindow);
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::OpenRunWindow { .. }))
    );
    let mut w = run_window();
    w.act(Action::RunOutputWindow);
    assert!(w.git.is_empty());
}

#[test]
fn in_a_git_window_a_conflict_goes_to_the_main_window_and_nothing_opens_here() {
    let (ws, _) = Workspace::new_git_window(PathBuf::from(ROOT));
    let mut h = Harness::new(&[]);
    h.ws = ws;
    h.git_status(vec![coded("c.txt", GitChange::Conflicted, 'U', 'U')]);
    h.act(Action::GitCommit);
    h.git_status(vec![coded("c.txt", GitChange::Conflicted, 'U', 'U')]);
    h.run(Command::Insert("n".into()));
    assert!(
        h.git
            .iter()
            .any(|e| matches!(e, Effect::OpenInMainWindow { path, .. } if path.ends_with("c.txt")))
    );
    assert!(h.ws.active().is_none());
}

#[test]
fn blame_that_does_not_line_up_with_the_text_shows_no_names() {
    let mut h = opened("a.txt", "one\rtwo\rthree\n");
    h.act(Action::GitBlame);
    let version = h.doc_version();
    h.blame_answer(version);
    let doc = h.current_doc();
    assert!(
        h.ws.blame(doc).unwrap().blame.is_none(),
        "still on, but silent"
    );
}

fn lines_hunk() -> crate::core::GitHunk {
    let row = |kind, text: &str| DiffRow {
        kind,
        old_line: None,
        new_line: None,
        text: text.to_owned(),
    };
    crate::core::GitHunk {
        side: crate::core::HunkSide::Unstaged,
        whole: false,
        patch: b"--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@\n x\n-o\n+n\n+m\n".to_vec(),
        title: "@@ -1,2 +1,2 @@".to_owned(),
        rows: vec![
            row(DiffKind::Same, "x"),
            row(DiffKind::Removed, "o"),
            row(DiffKind::Added, "n"),
            row(DiffKind::Added, "m"),
        ],
    }
}

impl Harness {
    fn pick_lines_of_a_hunk(&mut self) {
        self.commit_view(vec![coded("a", GitChange::Modified, ' ', 'M')]);
        self.hunks_arrive(vec![lines_hunk()]);
        self.run(Command::Indent);
        self.run(Command::Newline);
        assert_eq!(self.panel().field, GitField::HunkLines);
    }

    fn lines_sent(&self) -> Option<Vec<usize>> {
        self.git.iter().rev().find_map(|e| match e {
            Effect::Git(GitEffect::ApplyHunk { lines, .. }) => lines.clone(),
            _ => None,
        })
    }
}

#[test]
fn enter_goes_down_to_the_lines_and_space_ticks_them() {
    let mut h = Harness::new(&[]);
    h.pick_lines_of_a_hunk();
    assert_eq!(h.panel().changed_lines(), 3, "context cannot be picked");
    h.run(Command::Insert(" ".into()));
    assert_eq!(
        h.panel().line_pick.as_ref().map(|p| p.cursor),
        Some(1),
        "moves on"
    );
    h.run(Command::Move(Motion::Down));
    h.run(Command::Insert(" ".into()));
    h.run(Command::Newline);
    assert_eq!(h.lines_sent(), Some(vec![0, 2]));
    assert_eq!(h.panel().field, GitField::Hunks);
    assert!(h.panel().busy);
}

#[test]
fn with_nothing_ticked_enter_takes_the_line_under_the_cursor() {
    let mut h = Harness::new(&[]);
    h.pick_lines_of_a_hunk();
    h.run(Command::Move(Motion::Down));
    h.run(Command::Newline);
    assert_eq!(h.lines_sent(), Some(vec![1]));
}

#[test]
fn a_is_all_or_none_and_esc_goes_back_without_doing_anything() {
    let mut h = Harness::new(&[]);
    h.pick_lines_of_a_hunk();
    h.run(Command::Insert("a".into()));
    assert_eq!(
        h.panel().line_pick.as_ref().map(|p| p.picked.len()),
        Some(3)
    );
    h.run(Command::Insert("a".into()));
    assert_eq!(
        h.panel().line_pick.as_ref().map(|p| p.picked.len()),
        Some(0)
    );
    h.run(Command::Escape);
    assert_eq!(h.panel().field, GitField::Hunks);
    assert_eq!(h.lines_sent(), None);
}

#[test]
fn the_cursor_stays_inside_the_hunk() {
    let mut h = Harness::new(&[]);
    h.pick_lines_of_a_hunk();
    for _ in 0..9 {
        h.run(Command::Move(Motion::Down));
    }
    assert_eq!(h.panel().line_pick.as_ref().map(|p| p.cursor), Some(2));
    for _ in 0..9 {
        h.run(Command::Move(Motion::Up));
    }
    assert_eq!(h.panel().line_pick.as_ref().map(|p| p.cursor), Some(0));
}

#[test]
fn a_hunk_that_changed_under_the_picking_drops_the_ticks() {
    let mut h = Harness::new(&[]);
    h.pick_lines_of_a_hunk();
    h.run(Command::Insert(" ".into()));
    h.git_status(vec![coded("a", GitChange::Modified, ' ', 'M')]);
    h.hunks_arrive(vec![lines_hunk()]);
    assert_eq!(
        h.panel().line_pick.as_ref().map(|p| p.picked.len()),
        Some(1),
        "same hunk"
    );
    let mut other = lines_hunk();
    other.patch = b"--- a/a\n+++ b/a\n@@ -1 +1 @@\n-z\n+y\n".to_vec();
    h.git_status(vec![coded("a", GitChange::Modified, ' ', 'M')]);
    h.hunks_arrive(vec![other]);
    assert_eq!(h.panel().line_pick, None);
    assert_eq!(h.panel().field, GitField::Hunks);
}

#[test]
fn what_moves_as_a_whole_has_no_lines() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![coded("a", GitChange::Added, 'A', ' ')]);
    let mut whole = lines_hunk();
    whole.whole = true;
    h.hunks_arrive(vec![whole]);
    h.run(Command::Indent);
    h.run(Command::Newline);
    assert_eq!(h.panel().field, GitField::Hunks);
    assert!(
        h.panel()
            .notice
            .as_ref()
            .is_some_and(|n| n.text.contains("whole"))
    );
}

const TWO_CONFLICTS: &str = "top\n<<<<<<< HEAD\nmine\n=======\nyours\n>>>>>>> other\nmid\n<<<<<<< HEAD\nA\n=======\nB\n>>>>>>> other\nend\n";

impl Harness {
    fn merging(&mut self) {
        self.files
            .push((Path::new(ROOT).join("c.txt"), TWO_CONFLICTS.to_owned()));
        self.open("c.txt");
        self.act(Action::GitMergeView);
        assert!(self.ws.merge_view().is_some());
    }

    fn key(&mut self, key: &str) {
        self.run(Command::Insert(key.to_owned()));
    }
}

#[test]
fn each_choice_edits_the_real_text_and_moves_to_the_next_conflict() {
    let mut h = Harness::new(&[]);
    h.merging();
    h.key("o");
    assert_eq!(
        h.active_text(),
        "top\nmine\nmid\n<<<<<<< HEAD\nA\n=======\nB\n>>>>>>> other\nend\n"
    );
    let view = h.ws.merge_view().expect("still up");
    assert_eq!(view.selected, 1);
    assert_eq!(view.open_count(), 1);
    h.key("t");
    assert_eq!(h.active_text(), "top\nmine\nmid\nB\nend\n");
    assert_eq!(h.ws.merge_view().map(MergeView::open_count), Some(0));
}

#[test]
fn both_orders_leave_both_sides() {
    for (key, expected) in [("b", "mine\nyours\n"), ("r", "yours\nmine\n")] {
        let mut h = Harness::new(&[]);
        h.merging();
        h.key(key);
        assert!(
            h.active_text().starts_with(&format!("top\n{expected}mid")),
            "{}",
            h.active_text()
        );
    }
}

#[test]
fn deciding_the_second_conflict_first_finds_its_own_markers() {
    let mut h = Harness::new(&[]);
    h.merging();
    h.key("n");
    h.key("t");
    assert_eq!(
        h.active_text(),
        "top\n<<<<<<< HEAD\nmine\n=======\nyours\n>>>>>>> other\nmid\nB\nend\n"
    );
    h.key("o");
    assert_eq!(h.active_text(), "top\nmine\nmid\nB\nend\n");
}

#[test]
fn undo_reopens_the_last_choice_also_through_the_usual_key() {
    let mut h = Harness::new(&[]);
    h.merging();
    h.key("o");
    h.key("t");
    h.key("u");
    let view = h.ws.merge_view().expect("up");
    assert_eq!((view.open_count(), view.selected), (1, 1));
    assert!(h.active_text().contains("<<<<<<< HEAD\nA"));
    h.act(Action::Undo);
    assert_eq!(h.active_text(), TWO_CONFLICTS);
    assert_eq!(h.ws.merge_view().map(MergeView::open_count), Some(2));
    h.key("u");
    assert_eq!(
        h.active_text(),
        TWO_CONFLICTS,
        "nothing further back from here"
    );
}

#[test]
fn a_decided_conflict_is_not_decided_twice() {
    let mut h = Harness::new(&[]);
    h.merging();
    h.key("o");
    h.key("p");
    h.key("t");
    assert!(h.active_text().starts_with("top\nmine\nmid"));
    let notice = h.ws.merge_view().and_then(|v| v.notice.clone());
    assert!(notice.is_some_and(|n| n.text.contains("Already")));
}

#[test]
fn escape_closes_keeping_what_was_decided_and_nothing_reaches_the_editor_before() {
    let mut h = Harness::new(&[]);
    h.merging();
    h.key("z");
    assert_eq!(h.active_text(), TWO_CONFLICTS);
    h.key("o");
    h.run(Command::Escape);
    assert!(h.ws.merge_view().is_none());
    assert!(h.active_text().starts_with("top\nmine\nmid\n<<<<<<<"));
    assert!(h.dirty());
    h.act(Action::GitMergeView);
    assert_eq!(
        h.ws.merge_view().map(MergeView::open_count),
        Some(1),
        "the rest is still there"
    );
}

#[test]
fn edit_by_hand_lands_on_that_conflict_in_the_editor() {
    let mut h = Harness::new(&[]);
    h.merging();
    h.key("n");
    h.key("e");
    assert!(h.ws.merge_view().is_none());
    assert_eq!(h.ws.focus(), Focus::Editor);
    let at =
        h.ws.active()
            .map(|(v, d)| d.text().char_to_line(v.cursor.idx));
    assert_eq!(at, Some(7));
}

#[test]
fn finishing_needs_every_conflict_decided_and_then_stages_the_file() {
    let mut h = Harness::new(&[]);
    h.merging();
    h.run(Command::Newline);
    assert!(h.ws.merge_view().is_some());
    assert!(
        !h.git
            .iter()
            .any(|e| matches!(e, Effect::Git(GitEffect::StageFiles { .. })))
    );
    h.key("o");
    h.key("o");
    h.written.clear();
    h.run(Command::Newline);
    assert!(h.ws.merge_view().is_none());
    assert_eq!(h.written.len(), 1, "saved before git looks");
    let staged = h.git.iter().rev().find_map(|e| match e {
        Effect::Git(GitEffect::StageFiles {
            files, stage: true, ..
        }) => Some(files[0].path.clone()),
        _ => None,
    });
    assert!(staged.is_some_and(|p| p.ends_with("c.txt")));
    assert_eq!(h.panel().tab, GitTab::Commit);
}

#[test]
fn a_reload_under_the_view_lays_it_out_again_and_never_edits_stale_lines() {
    let mut h = Harness::new(&[]);
    h.merging();
    h.key("o");
    h.act(Action::Save);
    h.file_changed("<<<<<<< x\n1\n=======\n2\n>>>>>>> y\n");
    let view = h.ws.merge_view().expect("up");
    assert_eq!(view.conflict_count(), 1);
    assert!(view.undo.is_empty(), "what it could undo is gone");
    h.key("t");
    assert_eq!(h.active_text(), "2\n");
}

#[test]
fn enter_on_a_file_in_conflict_opens_the_view_also_when_it_must_be_read_first() {
    let mut h = Harness::new(&[]);
    h.files
        .push((PathBuf::from("/repo/c.txt"), TWO_CONFLICTS.to_owned()));
    h.commit_view(vec![coded("c.txt", GitChange::Conflicted, 'U', 'U')]);
    h.run(Command::Newline);
    let view = h.ws.merge_view().expect("opened after the read");
    assert_eq!(view.conflict_count(), 2);
}

#[test]
fn a_side_that_deleted_the_file_has_nothing_to_show_in_panes() {
    let mut h = Harness::new(&[]);
    h.commit_view(vec![coded("c.txt", GitChange::Conflicted, 'D', 'U')]);
    h.run(Command::Newline);
    assert!(h.ws.merge_view().is_none());
    assert!(
        h.panel()
            .notice
            .as_ref()
            .is_some_and(|n| n.text.contains("deleted"))
    );
}

#[test]
fn quitting_still_works_from_the_view() {
    let mut h = Harness::new(&[]);
    h.merging();
    h.act(Action::Quit);
    assert!(h.ws.should_quit());
}
