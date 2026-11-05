use super::*;
use crate::core::git_command::{GitEffect, GitEvent};
use crate::core::run_state::{RunEffect, RunEvent};
use crate::core::terminal::{TerminalEffect, TerminalEvent};

fn commit(hash: &str, subject: &str) -> crate::core::GitCommit {
    crate::core::GitCommit {
        hash: hash.into(),
        parents: Vec::new(),
        author: "Ann".into(),
        time: 0,
        refs: Vec::new(),
        subject: subject.into(),
    }
}

fn cell_of(row: &str, needle: &str) -> usize {
    row[..row.find(needle).unwrap()].chars().count()
}

fn joined(rows: &[String]) -> String {
    rows.join("\n")
}

#[test]
fn file_history_lists_commits_with_the_files_name() {
    let mut s = Screen::new();
    s.open("a.txt", "one\n");
    s.ws.dispatch(Command::Run(Action::GitFileHistory));
    let waiting = joined(&s.draw(120, 40));
    assert!(waiting.contains("Loading"), "{waiting}");
    s.ws.handle(Event::Git(GitEvent::FileHistoryRead {
        path: PathBuf::from("/proj/a.txt"),
        result: Ok(vec![crate::core::GitFileCommit {
            commit: commit("deadbeef99", "made it better"),
            file: crate::core::GitFile::new("a.txt".into(), crate::core::GitChange::Modified),
        }]),
        now: 7200,
    }));
    let screen = joined(&s.draw(120, 40));
    assert!(screen.contains("deadbeef"), "{screen}");
    assert!(screen.contains("made it better"));
    assert!(screen.contains("2 h ago"));
    assert!(screen.contains("History"));
}

fn blame_of(s: &mut Screen, author: &str) {
    let (doc, version) = {
        let (view, d) = s.ws.active().unwrap();
        (view.doc, d.version())
    };
    s.ws.handle(Event::Git(GitEvent::BlameRead {
        doc,
        version,
        result: Ok(crate::core::GitBlame {
            commits: vec![
                crate::core::GitBlameCommit {
                    hash: "abcdef1234567".into(),
                    author: author.into(),
                    time: 0,
                },
                crate::core::GitBlameCommit {
                    hash: "0".repeat(40),
                    author: "Not Committed Yet".into(),
                    time: 0,
                },
            ],
            lines: vec![0, 1],
        }),
        now: 7200,
    }));
}

#[test]
fn blame_sits_beside_the_text_and_clicks_still_land_on_the_right_character() {
    let mut s = Screen::new();
    s.open("a.txt", "first\nsecond\n");
    s.ws.dispatch(Command::Run(Action::GitBlame));
    blame_of(&mut s, "Ann");
    let screen = s.draw(120, 30);
    let line = screen.iter().find(|l| l.contains("first")).unwrap();
    assert!(line.contains("Ann"), "{line}");
    assert!(line.contains("2 h ago"));
    assert!(line.contains("abcdef1"));
    assert!(line.find("abcdef1") < line.find("first"), "beside the text");
    let second = screen.iter().find(|l| l.contains("second")).unwrap();
    assert!(second.contains("not committed"), "{second}");
    assert_eq!(cell_of(line, "first"), cell_of(second, "second"));
}

#[test]
fn blame_turns_stale_with_the_first_keystroke_and_shows_no_old_names() {
    let mut s = Screen::new();
    s.open("a.txt", "first\nsecond\n");
    s.ws.dispatch(Command::Run(Action::GitBlame));
    blame_of(&mut s, "Ann");
    s.ws.dispatch(Command::Insert("x".into()));
    let screen = joined(&s.draw(120, 30));
    assert!(!screen.contains("abcdef1"), "{screen}");
    assert!(!screen.contains("Ann"));
    assert!(screen.contains("stale"));
    assert!(screen.contains("xfirst"), "typing goes on");
}

#[test]
fn in_a_narrow_pane_blame_keeps_only_the_hash_and_wide_names_do_not_shift_text() {
    let mut s = Screen::new();
    s.open("a.txt", "first\nsecond\n");
    s.ws.dispatch(Command::Run(Action::GitBlame));
    blame_of(&mut s, "Ann");
    let narrow = joined(&s.draw(60, 20));
    assert!(narrow.contains("abcdef1"), "{narrow}");
    assert!(!narrow.contains("Ann"));
    blame_of(&mut s, "日本語の長い名前です");
    let wide = s.draw(120, 30);
    let a = wide.iter().find(|l| l.contains("first")).unwrap();
    let b = wide.iter().find(|l| l.contains("second")).unwrap();
    assert_eq!(cell_of(a, "first"), cell_of(b, "second"));
}

#[test]
fn the_history_tab_never_panics_on_small_screens() {
    for (w, h) in [(40, 10), (41, 16), (60, 20), (200, 60)] {
        let mut s = Screen::new();
        s.open("a.txt", "x\n");
        s.ws.dispatch(Command::Run(Action::GitFileHistory));
        s.ws.dispatch(Command::Run(Action::GitBlame));
        blame_of(&mut s, "Ann");
        s.draw(w, h);
    }
}

fn coded(path: &str, index: char, worktree: char) -> crate::core::GitFile {
    let mut file = crate::core::GitFile::new(path.into(), crate::core::GitChange::Modified);
    file.code = crate::core::GitCode::new(index, worktree);
    file
}

fn status(s: &mut Screen, files: Vec<crate::core::GitFile>) {
    s.ws.handle(Event::Git(GitEvent::StatusRead {
        branch: Some("main".into()),
        top: None,
        files: Ok(files),
        state: crate::core::GitRepoState::default(),
    }));
}

#[test]
fn the_screen_says_in_words_what_a_commit_would_take() {
    let mut s = Screen::new();
    s.ws.dispatch(Command::Run(Action::GitCommit));
    status(
        &mut s,
        vec![coded("a.rs", ' ', 'M'), coded("b.rs", ' ', 'M')],
    );
    let rows = s.draw(120, 40);
    assert!(find_row(&rows, "TICKED, whole files").contains("(2)"));
    assert!(find_row(&rows, "a.rs").contains("[x]"));

    status(
        &mut s,
        vec![coded("a.rs", 'M', 'M'), coded("b.rs", ' ', 'M')],
    );
    let rows = s.draw(120, 40);
    assert!(find_row(&rows, "STAGED only").contains("(1 files)"));
    assert!(find_row(&rows, "a.rs").contains("[~]"), "partly staged");
    assert!(find_row(&rows, "b.rs").contains("[ ]"));
    assert!(find_row(&rows, "Space stage file").contains("S ticked"));

    s.ws.dispatch(Command::Insert("s".into()));
    let rows = s.draw(120, 40);
    assert!(find_row(&rows, "TICKED").contains("(2)"));
    assert!(find_row(&rows, "partly staged").contains("takes all of it"));
    assert!(find_row(&rows, "a.rs").contains("[x] ~"));
}

#[test]
fn hunks_show_under_their_side_and_the_chosen_one_is_lit() {
    use crate::core::HunkSide::{Staged, Unstaged};
    let mut s = Screen::new();
    s.ws.dispatch(Command::Run(Action::GitCommit));
    status(&mut s, vec![coded("a.rs", 'M', 'M')]);
    let id = s.ws.git_panel().unwrap().diff_request.expect("asked");
    let hunk = |side, title: &str, text: &str| crate::core::GitHunk {
        side,
        whole: false,
        patch: Vec::new(),
        title: title.into(),
        rows: vec![crate::core::DiffRow {
            kind: crate::core::DiffKind::Added,
            old_line: None,
            new_line: Some(1),
            text: text.into(),
        }],
    };
    s.ws.handle(Event::Git(GitEvent::HunksRead {
        id,
        result: Ok(vec![
            hunk(Unstaged, "@@ -1 +1 @@", "later"),
            hunk(Staged, "@@ -9 +9 @@", "committed next"),
        ]),
    }));
    s.ws.dispatch(Command::Indent);
    let rows = s.draw(120, 40);
    let not_staged = rows
        .iter()
        .position(|r| r.contains("Not staged (1)"))
        .unwrap();
    let staged = rows.iter().position(|r| r.contains("Staged (1)")).unwrap();
    let later = rows.iter().position(|r| r.contains("later")).unwrap();
    let next = rows
        .iter()
        .position(|r| r.contains("committed next"))
        .unwrap();
    assert!(
        not_staged < later && later < staged && staged < next,
        "{rows:?}"
    );
    assert!(find_row(&rows, "stage/unstage").contains("Space"));
}

#[test]
fn a_merge_shows_its_state_conflicted_files_and_the_keys_to_finish() {
    let mut s = Screen::new();
    s.open("c.txt", "<<<<<<< HEAD\nmine\n=======\nyours\n>>>>>>> o\n");
    s.ws.dispatch(Command::Run(Action::GitCommit));
    let mut file = crate::core::GitFile::new("c.txt".into(), crate::core::GitChange::Conflicted);
    file.code = crate::core::GitCode::new('U', 'U');
    s.ws.handle(Event::Git(GitEvent::StatusRead {
        branch: Some("main".into()),
        top: Some(PathBuf::from("/proj")),
        files: Ok(vec![file]),
        state: crate::core::GitRepoState {
            operation: Some(crate::core::GitOperation::Merging),
            staged_id: String::new(),
        },
    }));
    let rows = s.draw(150, 40);
    assert!(rows[0].contains("MERGING"), "{}", rows[0]);
    assert!(find_row(&rows, "1 conflicted").contains("MERGING"));
    assert!(find_row(&rows, "! c.txt").contains("[!]"));
    let hints = find_row(&rows, "X abort");
    assert!(hints.contains("T theirs") && hints.contains("B both") && hints.contains("X abort"));
    assert!(
        find_row(&rows, "Conflict 1 of 1").contains("O ours"),
        "over the text"
    );
    assert!(find_row(&rows, "Finish merge").contains("STAGED"));
}

#[test]
fn stash_and_branch_tabs_show_their_lists_and_never_panic_when_small() {
    for (w, h) in [(40, 10), (41, 16), (60, 20), (160, 50)] {
        let mut s = Screen::new();
        s.ws.dispatch(Command::Run(Action::GitStash));
        s.ws.handle(Event::Git(GitEvent::StashesRead {
            result: Ok(vec![crate::core::GitStash {
                index: 0,
                hash: "abc".into(),
                time: 0,
                subject: "On main: an idea".into(),
            }]),
            now: 7200,
        }));
        let rows = s.draw(w, h);
        if w > 100 {
            assert!(find_row(&rows, "an idea").contains("{0}"));
            assert!(find_row(&rows, "D drop").contains("O pop"));
        }
        s.ws.dispatch(Command::Insert("n".into()));
        let rows = s.draw(w, h);
        if w > 100 {
            assert!(find_row(&rows, "Stash tracked files").contains("untracked stay"));
        }
        s.ws.dispatch(Command::Run(Action::GitManageBranches));
        s.ws.handle(Event::Git(GitEvent::StatusRead {
            branch: Some("main".into()),
            top: None,
            files: Ok(Vec::new()),
            state: crate::core::GitRepoState::default(),
        }));
        s.ws.handle(Event::Git(GitEvent::BranchesRead {
            result: Ok(vec![
                crate::core::GitBranch {
                    name: "main".into(),
                    remote: false,
                },
                crate::core::GitBranch {
                    name: "origin/x".into(),
                    remote: true,
                },
            ]),
        }));
        let rows = s.draw(w, h);
        if w > 100 {
            assert!(
                find_row(&rows, "main").contains("* main")
                    || rows.iter().any(|r| r.contains("* main"))
            );
            assert!(find_row(&rows, "M merge").contains("D delete"));
        }
    }
}

fn added(text: &str, line: usize) -> crate::core::DiffRow {
    crate::core::DiffRow {
        kind: crate::core::DiffKind::Added,
        old_line: None,
        new_line: Some(line),
        text: text.into(),
    }
}

fn removed(text: &str, line: usize) -> crate::core::DiffRow {
    crate::core::DiffRow {
        kind: crate::core::DiffKind::Removed,
        old_line: Some(line),
        new_line: None,
        text: text.into(),
    }
}

fn history_with_diff(s: &mut Screen) {
    s.open("a.txt", "x\n");
    s.ws.dispatch(Command::Run(Action::GitFileHistory));
    s.ws.handle(Event::Git(GitEvent::FileHistoryRead {
        path: PathBuf::from("/proj/a.txt"),
        result: Ok(vec![crate::core::GitFileCommit {
            commit: commit("abc", "s"),
            file: crate::core::GitFile::new("a.txt".into(), crate::core::GitChange::Modified),
        }]),
        now: 1,
    }));
    let id = s.ws.git_panel().unwrap().diff_request.expect("asked");
    s.ws.handle(Event::Git(GitEvent::DiffRead {
        id,
        result: Ok(vec![removed("the old line", 3), added("the new line", 3)]),
    }));
}

#[test]
fn a_wide_panel_faces_old_and_new_and_a_narrow_one_stays_inline() {
    let mut s = Screen::new();
    history_with_diff(&mut s);
    let wide = s.draw(170, 40);
    let row = find_row(&wide, "the old line");
    assert!(row.contains("the new line"), "on one row: {row}");
    assert!(row.find("the old line") < row.find("the new line"));

    let narrow = s.draw(100, 40);
    assert!(!find_row(&narrow, "the old line").contains("the new line"));
    assert!(find_row(&narrow, "the new line").contains("+ the new line"));
}

#[test]
fn v_turns_side_by_side_off_and_on() {
    let mut s = Screen::new();
    history_with_diff(&mut s);
    s.ws.dispatch(Command::Insert("v".into()));
    let rows = s.draw(170, 40);
    assert!(!find_row(&rows, "the old line").contains("the new line"));
    s.ws.dispatch(Command::Run(Action::GitSideBySide));
    let rows = s.draw(170, 40);
    assert!(find_row(&rows, "the old line").contains("the new line"));
}

#[test]
fn v_while_typing_a_message_is_a_letter() {
    let mut s = Screen::new();
    s.ws.dispatch(Command::Run(Action::GitCommit));
    s.ws.dispatch(Command::Indent);
    s.ws.dispatch(Command::Insert("v".into()));
    let panel = s.ws.git_panel().unwrap();
    assert_eq!(panel.message.text(), "v");
    assert!(panel.side_by_side);
}

fn with_markdown(s: &mut Screen, text: &str) {
    s.open("notes.md", text);
}

#[test]
fn the_preview_sits_beside_the_text_and_follows_typing() {
    let mut s = Screen::new();
    with_markdown(&mut s, "# Title here\n\nSome **bold** words\n");
    let rows = s.draw(140, 30);
    let row = find_row(&rows, "# Title here");
    assert!(
        row.matches("Title here").count() == 2,
        "text and preview: {row}"
    );
    assert!(
        rows.iter().any(|r| r.contains("════")),
        "the heading's rule"
    );
    assert!(find_row(&rows, "Some **bold** words").contains("Some **bold** words"));
    assert!(find_row(&rows, "Some bold words").contains("Some bold words"));
    s.ws.dispatch(Command::Insert("New ".into()));
    let rows = s.draw(140, 30);
    let row = find_row(&rows, "New # Title");
    assert!(
        row.contains("New # Title here"),
        "typed text reads as a paragraph now: {row}"
    );
    assert!(row.matches("New # Title here").count() == 2, "{row}");
}

#[test]
fn the_tab_row_offers_the_switch_only_for_markdown_and_clicking_it_switches() {
    let mut s = Screen::new();
    s.open("a.rs", "fn main() {}\n");
    assert!(!joined(&s.draw(140, 30)).contains("Text only"));
    s.open("notes.md", "# Title\n");
    let rows = s.draw(140, 30);
    let y = rows
        .iter()
        .position(|r| r.contains("Text only"))
        .expect("offered");
    let x = rows[y].chars().position(|c| c == '☰').expect("the mark");
    assert_eq!(
        s.ui.hits.at(x as u16, y as u16),
        Some(&Hit::MarkdownSwitch),
        "over the tabs"
    );
    s.ws.dispatch(Command::Run(Action::MarkdownPreview));
    assert!(joined(&s.draw(140, 30)).contains("◧ Preview"));
}

#[test]
fn a_picture_in_a_preview_shows_as_characters_or_says_why_not() {
    use crate::core::image_art::{ArtCell, AsciiImage, ImageRequest};
    let mut s = Screen::new();
    s.open("notes.md", "![the cat](cat.png)\n\n![gone](gone.png)\n");
    assert!(joined(&s.draw(140, 30)).contains("decoding image"));
    let mut wants = std::mem::take(&mut s.ui.image_wants);
    wants.dedup();
    wants.sort();
    wants.dedup();
    assert_eq!(wants.len(), 2, "both are in view: {wants:?}");
    wants.reverse();
    let request: ImageRequest = wants[0].clone();
    for want in wants {
        s.ws.dispatch(Command::WantImage(want));
    }
    assert!(s.ui.image_wants.is_empty());
    s.ws.handle(crate::core::Event::ImageDecoded {
        request,
        result: Ok(AsciiImage {
            cols: 3,
            rows: 1,
            cells: vec![
                ArtCell {
                    ch: '\u{2588}',
                    dots: 0xFF,
                    ink: Some([255; 3]),
                    paper: None,
                },
                ArtCell {
                    ch: ' ',
                    dots: 0,
                    ink: None,
                    paper: None,
                },
                ArtCell {
                    ch: '\u{258C}',
                    dots: 0x0F,
                    ink: Some([255; 3]),
                    paper: Some([0; 3]),
                },
            ],
            width: 6,
            height: 1,
        }),
    });
    let rows = joined(&s.draw(140, 30));
    assert!(rows.contains("the cat"), "its caption");
    assert!(rows.contains("\u{2588} \u{258C}"), "{rows}");
    assert!(rows.contains("decoding image"), "the other still waits");
}

#[test]
fn the_preview_only_opens_on_markdown_and_toggles_off() {
    let mut s = Screen::new();
    s.open("a.rs", "fn main() {}\n");
    s.ws.dispatch(Command::Run(Action::MarkdownPreview));
    assert!(s.ws.status().unwrap().text.contains("Markdown"));
    let doc = s.ws.active().unwrap().0.doc;
    assert!(!s.ws.markdown_preview(doc));
    with_markdown(&mut s, "# T\n");
    let doc = s.ws.active().unwrap().0.doc;
    assert!(s.ws.markdown_preview(doc));
    s.ws.dispatch(Command::Run(Action::MarkdownPreview));
    assert!(!s.ws.markdown_preview(doc));
}

#[test]
fn the_preview_follows_the_cursor_down_a_long_document() {
    let text: String = (0..200).map(|n| format!("para number {n}\n\n")).collect();
    let mut s = Screen::new();
    with_markdown(&mut s, &text);
    let rows = s.draw(140, 30);
    assert!(find_row(&rows, "para number 0").contains("para number 0"));
    s.ws.dispatch(Command::Move(crate::core::Motion::DocEnd));
    let rows = s.draw(140, 30);
    let shown: usize = rows
        .iter()
        .map(|r| r.matches("para number 199").count())
        .sum();
    assert_eq!(shown, 2, "both follow:\n{}", rows.join("\n"));
}

#[test]
fn a_narrow_pane_shows_the_preview_alone_and_small_screens_do_not_panic() {
    let mut s = Screen::new();
    with_markdown(
        &mut s,
        "# T\n\n| a |\n|---|\n| b |\n\n```rust\nfn x() {}\n```\n",
    );
    let rows = s.draw(50, 20);
    assert!(rows.iter().any(|r| r.contains('┌')), "rendered");
    assert!(!rows.iter().any(|r| r.contains("# T")), "and alone");
    assert!(s.ui.preview_alone.is_some());
    let rows = s.draw(140, 30);
    assert!(rows.iter().any(|r| r.contains('┌')), "rendered");
    assert!(rows.iter().any(|r| r.contains("# T")), "beside the text");
    assert!(s.ui.preview_alone.is_none());
    for (w, h) in [(40, 10), (50, 11), (90, 20), (91, 21), (120, 30), (250, 60)] {
        s.draw(w, h);
    }
}

#[test]
fn a_run_window_fills_the_screen_and_says_what_it_is_waiting_for() {
    let (ws, _) = Workspace::new_run_window(PathBuf::from("/proj"));
    let mut s = Screen {
        ws,
        ui: UiState::default(),
        keymap: Keymap::new(true),
        clicks: 0,
    };
    let rows = joined(&s.draw(100, 24));
    assert!(
        rows.contains("main window of this project is not running"),
        "{rows}"
    );
    assert!(!rows.contains("Project"), "no tree");
    s.ws.handle(Event::RunStreamState { connected: true });
    assert!(joined(&s.draw(100, 24)).contains("Waiting for a run"));
    s.ws.handle(Event::RunStream(crate::core::RunStreamMessage::Started {
        id: 1,
        target: crate::core::RunTarget {
            name: "t".into(),
            command: crate::core::run::RunCommand::Shell("make it".into()),
            cwd: "/proj".into(),
            source: crate::core::run::RunSource::Detected,
        },
    }));
    s.ws.handle(Event::RunStream(crate::core::RunStreamMessage::Output {
        id: 1,
        lines: vec![crate::core::RunLine {
            spans: Vec::new(),
            text: "hello from the run".into(),
            location: None,
        }],
        partial: String::new(),
        dropped: 0,
    }));
    let rows = joined(&s.draw(100, 24));
    assert!(rows.contains("hello from the run"));
    assert!(rows.contains("make it"));
    assert!(rows.contains("open in the main window"));
    s.ws.handle(Event::RunStreamState { connected: false });
    let rows = joined(&s.draw(100, 24));
    assert!(rows.contains("the main window is gone"));
    assert!(rows.contains("hello from the run"), "what it said stays");
    for (w, h) in [(40, 10), (41, 11), (200, 60)] {
        s.draw(w, h);
    }
}

#[test]
fn three_panes_show_each_side_and_line_up() {
    let mut s = Screen::new();
    s.open(
        "c.txt",
        "top\n<<<<<<< HEAD\nmine\n=======\nyours\n>>>>>>> other\nend\n",
    );
    s.ws.dispatch(Command::Run(Action::GitMergeView));
    let rows = s.draw(120, 24);
    assert!(
        rows[0].contains("c.txt") && rows[0].contains("conflict 1 of 1"),
        "{}",
        rows[0]
    );
    assert!(
        rows[1].contains("Ours: HEAD")
            && rows[1].contains("Result")
            && rows[1].contains("Theirs: other"),
        "{}",
        rows[1]
    );
    let row = rows.iter().find(|r| r.contains("mine")).expect("a row");
    assert!(row.contains("yours") && row.contains("undecided"), "{row}");
    assert!(
        row.find("mine") < row.find("undecided") && row.find("undecided") < row.find("yours"),
        "{row}"
    );
    assert!(
        !rows.iter().any(|r| r.contains("<<<<")),
        "markers are not shown"
    );
    s.ws.dispatch(Command::Insert("t".into()));
    let rows = s.draw(120, 24);
    assert!(rows[0].contains("all decided"), "{}", rows[0]);
    let row = rows.iter().find(|r| r.contains("mine")).expect("a row");
    assert!(!row.contains("undecided"), "{row}");
    assert_eq!(row.matches("yours").count(), 2, "in the result too: {row}");
}

#[test]
fn three_panes_never_panic_at_any_size_or_on_wide_or_long_lines() {
    let text = format!(
        "<<<<<<< H\n{}\n\tтаб\n=======\n{}\n>>>>>>> o\n",
        "長い".repeat(200),
        "y".repeat(500)
    );
    for (w, h) in [(40, 10), (41, 11), (60, 12), (61, 12), (200, 60)] {
        let mut s = Screen::new();
        s.open("c.txt", &text);
        s.ws.dispatch(Command::Run(Action::GitMergeView));
        s.draw(w, h);
        s.ws.dispatch(Command::Move(Motion::Right));
        s.draw(w, h);
    }
}

#[test]
fn picking_lines_marks_each_changed_one() {
    let mut s = Screen::new();
    s.ws.dispatch(Command::Run(Action::GitCommit));
    let fx = s.ws.handle(Event::Git(GitEvent::StatusRead {
        branch: None,
        top: None,
        files: Ok(vec![crate::core::GitFile::new(
            "a".into(),
            crate::core::GitChange::Modified,
        )]),
        state: crate::core::GitRepoState::default(),
    }));
    let id = fx
        .iter()
        .find_map(|e| match e {
            Effect::Git(GitEffect::Hunks { id, .. }) => Some(*id),
            _ => None,
        })
        .expect("hunks were asked for");
    let row = |kind, text: &str| crate::core::DiffRow {
        kind,
        old_line: None,
        new_line: None,
        text: text.to_owned(),
    };
    s.ws.handle(Event::Git(GitEvent::HunksRead {
        id,
        result: Ok(vec![crate::core::GitHunk {
            side: crate::core::HunkSide::Unstaged,
            whole: false,
            patch: b"--- a/a\n+++ b/a\n@@ -1 +1,2 @@\n-gone\n+came\n+more\n".to_vec(),
            title: "@@ -1 +1,2 @@".into(),
            rows: vec![
                row(crate::core::DiffKind::Removed, "gone"),
                row(crate::core::DiffKind::Added, "came"),
                row(crate::core::DiffKind::Added, "more"),
            ],
        }]),
    }));
    s.ws.dispatch(Command::Indent);
    s.ws.dispatch(Command::Newline);
    s.ws.dispatch(Command::Insert(" ".into()));
    let rows = s.draw(120, 24);
    assert!(
        find_row(&rows, "gone").contains("[x]"),
        "{}",
        rows.join("\n")
    );
    assert!(
        find_row(&rows, "came").contains("▌[ ]"),
        "{}",
        rows.join("\n")
    );
    assert!(find_row(&rows, "more").contains("[ ]"));
    assert!(
        rows.iter().any(|r| r.contains("Space tick")),
        "{}",
        rows.join(
            "
"
        )
    );
}

#[test]
fn the_title_row_has_the_menu_and_a_dropdown_shows_its_keys() {
    let mut s = Screen::new();
    let rows = s.draw(120, 30);
    assert!(
        rows[0].starts_with(" File  Edit  View  Navigate  Code  Git  Run  Debug  Claude  Help "),
        "{}",
        rows[0]
    );
    s.ws.dispatch(Command::Run(Action::MainMenu));
    let rows = s.draw(120, 30);
    let save = find_row(&rows, "Save All");
    assert!(
        save.contains("Ctrl+Shift+S") || save.contains("Save All"),
        "{save}"
    );
    assert!(
        find_row(&rows, "New File").contains("Alt+Ins"),
        "{}",
        rows.join("\n")
    );
    assert!(rows.iter().any(|r| r.contains("Delete File")));
}

#[test]
fn every_menu_draws_at_any_size_without_a_panic() {
    for menu in 0..crate::core::menu::MENUS.len() {
        for (w, h) in [(40, 10), (60, 12), (80, 24), (200, 60)] {
            let mut s = Screen::new();
            s.ws.dispatch(Command::OpenMenu { menu });
            for _ in 0..20 {
                s.draw(w, h);
                s.ws.dispatch(Command::Move(Motion::Down));
            }
        }
    }
}

#[test]
fn the_file_prompt_shows_the_place_and_what_was_typed() {
    let mut s = Screen::new();
    s.ws.dispatch(Command::Run(Action::NewFile));
    s.ws.dispatch(Command::Insert("имя.rs".into()));
    let rows = s.draw(100, 30);
    assert!(
        rows.iter().any(|r| r.contains("New File")),
        "{}",
        rows.join("\n")
    );
    assert!(find_row(&rows, "имя.rs").contains('❯') || rows.iter().any(|r| r.contains("имя.rs")));
    assert!(rows.iter().any(|r| r.contains("Esc cancel")));
}

#[test]
fn a_proposal_shows_in_the_title_and_its_diff_in_the_review() {
    let mut s = Screen::new();
    s.open("a.rs", "one\ntwo\n");
    s.ws.handle(Event::McpCalled {
        id: 1,
        tool: "propose_edit".into(),
        args: serde_json::json!({
            "summary": "rename it",
            "edits": [{ "path": "/proj/a.rs", "old_text": "two", "new_text": "deux" }],
        }),
    });
    let rows = s.draw(120, 24);
    assert!(rows[0].contains("Claude: 1 to review"), "{}", rows[0]);
    s.ws.dispatch(Command::Run(Action::ClaudeReview));
    let rows = s.draw(120, 24);
    assert!(rows[0].contains("rename it"), "{}", rows[0]);
    assert!(find_row(&rows, "- two").contains("[ ]") || rows.iter().any(|r| r.contains("[ ]")));
    assert!(
        rows.iter().any(|r| r.contains("+ deux")),
        "{}",
        rows.join("\n")
    );
    assert!(rows.iter().any(|r| r.contains("A accept file")));
    for (w, h) in [(40, 10), (61, 12), (200, 50)] {
        s.draw(w, h);
    }
}

#[test]
fn the_claude_panel_draws_the_program_s_screen_with_colour_and_wide_letters() {
    let mut s = Screen::new();
    s.ws.dispatch(Command::Run(Action::ClaudeOpen));
    s.draw(100, 30);
    let (rows, columns) = s.ui.claude_size.expect("measured");
    s.ws.dispatch(Command::ClaudeSize { rows, columns });
    s.ws.handle(Event::ClaudeOutput {
        bytes: "hello \x1b[38;2;1;2;3mпривет 日本\x1b[0m"
            .as_bytes()
            .to_vec(),
    });
    s.ws.handle(Event::IdeConnected);
    let rows = s.draw(100, 30);
    assert!(
        rows.iter().any(|r| r.contains("hello привет 日")),
        "{}",
        rows.join("\n")
    );
    assert!(rows.iter().any(|r| r.contains("connected to the editor")));
    assert!(rows[4].contains('✳'), "{}", rows[4]);
    for (w, h) in [(40, 10), (60, 12), (200, 60)] {
        s.draw(w, h);
    }
}

#[test]
fn dragged_edges_move_on_screen_within_limits() {
    let mut s = Screen::new();
    s.open("a.rs", "x\n");
    s.ws.dispatch(Command::ResizeProject { columns: 50 });
    let rows = s.draw(120, 30);
    let border = |r: &str| r.chars().skip(4).position(|c| c == '│');
    assert_eq!(border(&rows[5]), Some(50), "{}", rows[5]);
    s.ws.dispatch(Command::ResizeProject { columns: 500 });
    let rows = s.draw(120, 30);
    assert_eq!(
        border(&rows[5]),
        Some(75),
        "text keeps its room: {}",
        rows[5]
    );
    s.ws.dispatch(Command::ResizeProject { columns: 1 });
    let rows = s.draw(120, 30);
    assert_eq!(border(&rows[5]), Some(12), "{}", rows[5]);
    s.ws.dispatch(Command::Run(Action::ShowProblems));
    s.draw(120, 30);
    s.draw(120, 30);
    s.ws.dispatch(Command::ToolRoom {
        rows: 8,
        min: 4,
        max: 20,
    });
    s.ws.dispatch(Command::ResizeTool { rows: 15 });
    assert_eq!(s.ws.tool_height(), Some(15));
    s.ws.dispatch(Command::ResizeTool { rows: 0 });
    assert_eq!(s.ws.tool_height(), Some(4));
    s.ws.dispatch(Command::ResizeTool { rows: 99 });
    assert_eq!(s.ws.tool_height(), Some(20));
    s.draw(60, 12);
}

#[test]
fn nerd_icons_are_a_choice_and_show_a_file_s_logo() {
    use crate::core::DirEntry;
    let entries = |names: &[(&str, bool)]| {
        names
            .iter()
            .map(|(name, is_dir)| DirEntry {
                name: (*name).to_owned(),
                path: PathBuf::from("/proj").join(name),
                is_dir: *is_dir,
            })
            .collect::<Vec<_>>()
    };
    let mut s = Screen::new();
    s.ws.handle(Event::DirListed {
        path: PathBuf::from("/proj"),
        result: Ok(entries(&[("src", true), ("a.rs", false), ("b.php", false)])),
    });
    let rows = s.draw(100, 20);
    assert!(
        find_row(&rows, "a.rs").contains("▪ a.rs"),
        "{}",
        rows.join("\n")
    );
    s.ui.nerd_icons = true;
    let rows = s.draw(100, 20);
    assert!(
        find_row(&rows, "a.rs").contains("\u{e7a8} a.rs"),
        "{}",
        rows.join("\n")
    );
    assert!(find_row(&rows, "b.php").contains("\u{e73d} b.php"));
    assert!(find_row(&rows, "src").contains("▸ \u{f07b} src"));
}

#[test]
fn terminal_tabs_show_their_names_and_the_active_tab_s_screen() {
    let mut s = Screen::new();
    s.ws.dispatch(Command::Run(Action::TerminalToggle));
    s.draw(100, 30);
    let (rows, columns) = s.ui.terminal_size.expect("measured");
    let fx = s.ws.dispatch(Command::TerminalSize { rows, columns });
    let Some(Effect::Terminal(TerminalEffect::Start { id, .. })) = fx.first() else {
        panic!("{fx:?}");
    };
    s.ws.handle(Event::Terminal(TerminalEvent::Output {
        id: *id,
        bytes: b"$ first tab".to_vec(),
    }));
    s.ws.dispatch(Command::Run(Action::TerminalNew));
    let rows = s.draw(100, 30);
    assert!(
        find_row(&rows, "Local ×").contains("Local (2) × "),
        "{}",
        rows.join("\n")
    );
    assert!(find_row(&rows, "Local ×").trim_end().ends_with('+'));
    assert!(
        !rows.iter().any(|r| r.contains("first tab")),
        "the other tab's"
    );
    s.ws.dispatch(Command::SelectTerminalTab { index: 0 });
    let rows = s.draw(100, 30);
    assert!(
        rows.iter().any(|r| r.contains("$ first tab")),
        "{}",
        rows.join("\n")
    );
    assert!(rows[5].contains('❯'), "{}", rows[5]);
    for (w, h) in [(40, 10), (60, 12), (200, 60)] {
        s.draw(w, h);
    }
}

#[test]
fn a_program_s_colors_reach_the_screen_and_a_link_stays_a_link() {
    use crate::core::run_output::{RunColor, RunSpan, RunStyle};
    let mut s = Screen::new();
    s.ws.configure_run(vec![crate::core::ConfiguredRun {
        name: "a".into(),
        command: "a".into(),
        cwd: None,
        from_project: false,
    }]);
    s.ws.handle(Event::Run(RunEvent::FactsRead {
        facts: Default::default(),
    }));
    let fx = s.ws.dispatch(Command::SelectPickerItem {
        index: 0,
        accept: true,
    });
    let id = fx
        .iter()
        .find_map(|e| match e {
            Effect::Run(RunEffect::Start { id, .. }) => Some(*id),
            _ => None,
        })
        .expect("started");
    let red = RunStyle {
        fg: Some(RunColor::Rgb(200, 1, 2)),
        ..RunStyle::default()
    };
    s.ws.handle(Event::Run(RunEvent::Output {
        id,
        lines: vec![crate::core::RunLine {
            text: "ok failed".into(),
            location: None,
            spans: vec![RunSpan {
                start: 3,
                end: 9,
                style: red,
            }],
        }],
        partial: String::new(),
        dropped: 0,
    }));
    let buf = s.buffer(100, 30);
    assert_eq!(
        color_of(&buf, "failed"),
        ratatui::style::Color::Rgb(200, 1, 2)
    );
    assert_ne!(
        color_of(&buf, "ok failed"),
        ratatui::style::Color::Rgb(200, 1, 2)
    );
}

#[test]
fn a_bookmarked_line_stands_out_with_or_without_color() {
    let mut s = Screen::new();
    s.open("a.rs", "one\ntwo\n");
    s.ws.dispatch(Command::Run(Action::ToggleBookmark));
    let buf = s.buffer(100, 20);
    let rows = s.draw(100, 20);
    let (y, row) = rows
        .iter()
        .enumerate()
        .find(|(_, r)| r.contains("one"))
        .unwrap();
    let x = row.chars().position(|c| c == 'o').unwrap() - 1;
    let number_x = (0..x)
        .rev()
        .find(|n| row.chars().nth(*n) == Some('1'))
        .unwrap();
    let cell = &buf[(number_x as u16, y as u16)];
    assert!(cell.modifier.contains(ratatui::style::Modifier::REVERSED));
    let (y2, row2) = rows
        .iter()
        .enumerate()
        .find(|(_, r)| r.contains("two"))
        .unwrap();
    let n2 = row2.chars().position(|c| c == '2').unwrap();
    assert!(
        !buf[(n2 as u16, y2 as u16)]
            .modifier
            .contains(ratatui::style::Modifier::REVERSED)
    );
}

#[test]
fn the_cross_closes_its_tab_and_the_name_only_switches_to_it() {
    let mut s = Screen::new();
    s.open("a.rs", "a\n");
    s.open("b.rs", "b\n");
    let rows = s.draw(140, 30);
    let y = rows.iter().position(|r| r.contains("b.rs ×")).expect("tab");
    let x = rows[y].chars().position(|c| c == '×').expect("cross") as u16;
    let y = y as u16;
    let view = s.ws.active_view().expect("active");
    assert_eq!(s.ui.hits.at(x, y), Some(&Hit::CloseTab { view }));
    assert_eq!(
        s.ui.hits.at(x + 1, y),
        Some(&Hit::CloseTab { view }),
        "room around it"
    );
    assert_eq!(s.ui.hits.at(x - 4, y), Some(&Hit::Tab { view }), "the name");
    s.ws.dispatch(Command::CloseView { view });
    let rows = joined(&s.draw(140, 30));
    assert!(!rows.contains("b.rs"), "{rows}");
    assert!(rows.contains("a.rs"), "the other stays");
}

#[test]
fn a_tab_with_unsaved_changes_closes_from_its_dot_too() {
    let mut s = Screen::new();
    s.open("a.rs", "a\n");
    s.ws.dispatch(Command::Insert("x".to_owned()));
    let rows = s.draw(140, 30);
    let y = rows.iter().position(|r| r.contains("a.rs ●")).expect("tab");
    let x = rows[y].chars().position(|c| c == '●').expect("dot") as u16;
    let view = s.ws.active_view().expect("active");
    assert_eq!(s.ui.hits.at(x, y as u16), Some(&Hit::CloseTab { view }));
}

#[test]
fn the_title_says_while_a_macro_is_being_recorded() {
    let mut s = Screen::new();
    s.open("a.rs", "a\n");
    assert!(!joined(&s.draw(140, 30)).contains("REC"));
    s.ws.dispatch(Command::Run(Action::MacroRecord));
    assert!(joined(&s.draw(140, 30)).contains("● REC"));
    s.ws.dispatch(Command::Run(Action::MacroRecord));
    assert!(!joined(&s.draw(140, 30)).contains("REC"));
}
