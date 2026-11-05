use crate::core::git_command::GitEvent;
use std::path::Path;
use std::process::Output;

use super::git::{DIFF_ARGS, checked, known_top, parse_unified, run, top_level};
use crate::core::{DiffKind, DiffRow, Event, GitOp, GitOperation, GitStash};

fn words(output: &Output) -> String {
    let out = String::from_utf8_lossy(&output.stdout);
    let err = String::from_utf8_lossy(&output.stderr);
    let lines: Vec<&str> = out
        .lines()
        .chain(err.lines())
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        "git failed".to_owned()
    } else {
        lines.join("  ")
    }
}

fn spoken(top: &Path, args: &[&str]) -> Result<String, String> {
    let output = run(top, args)?;
    if output.status.success() {
        Ok(words(&output))
    } else {
        Err(words(&output))
    }
}

fn operation_word(operation: GitOperation) -> &'static str {
    match operation {
        GitOperation::Merging => "merge",
        GitOperation::Rebasing => "rebase",
        GitOperation::CherryPicking => "cherry-pick",
        GitOperation::Reverting => "revert",
    }
}

fn checked_ref(top: &Path, entry: &GitStash) -> Result<String, String> {
    let name = format!("stash@{{{}}}", entry.index);
    let at = checked(top, ["rev-parse", "--verify", "--quiet", name.as_str()])
        .map_err(|_| "The stash list changed: look again".to_owned())?;
    if at.trim() == entry.hash {
        Ok(name)
    } else {
        Err("The stash list changed: look again".to_owned())
    }
}

fn perform(top: &Path, op: &GitOp) -> Result<String, String> {
    match op {
        GitOp::Continue(operation) => spoken(top, &[operation_word(*operation), "--continue"]),
        GitOp::Abort(operation) => spoken(top, &[operation_word(*operation), "--abort"]),
        GitOp::StashPush { message, untracked } => {
            let mut args = vec!["stash", "push"];
            if *untracked {
                args.push("--include-untracked");
            }
            if !message.trim().is_empty() {
                args.extend(["-m", message.as_str()]);
            }
            spoken(top, &args)
        }
        GitOp::StashApply(entry) => {
            let name = checked_ref(top, entry)?;
            spoken(top, &["stash", "apply", &name])
        }
        GitOp::StashPop(entry) => {
            let name = checked_ref(top, entry)?;
            spoken(top, &["stash", "pop", &name])
        }
        GitOp::StashDrop(entry) => {
            let name = checked_ref(top, entry)?;
            spoken(top, &["stash", "drop", &name])
        }
        GitOp::BranchDelete { name, force } => {
            let flag = if *force { "-D" } else { "-d" };
            spoken(top, &["branch", flag, "--", name])
        }
        GitOp::Merge { branch } => spoken(top, &["merge", "--no-edit", branch]),
    }
}

pub fn op_event(root: &Path, op: GitOp) -> Event {
    let result = top_level(root).and_then(|top| perform(&top, &op));
    Event::Git(GitEvent::OpDone { op, result })
}

fn parse_stashes(text: &str) -> Vec<GitStash> {
    text.lines()
        .enumerate()
        .filter_map(|(index, line)| {
            let mut fields = line.split('\u{1f}');
            let hash = fields.next().filter(|h| !h.is_empty())?.to_owned();
            let time = fields.next()?.parse().unwrap_or(0);
            let subject = fields.next()?.to_owned();
            Some(GitStash {
                index,
                hash,
                time,
                subject,
            })
        })
        .collect()
}

fn stashes(top: &Path) -> Result<Vec<GitStash>, String> {
    checked(top, ["stash", "list", "--format=%H%x1f%ct%x1f%gs"]).map(|t| parse_stashes(&t))
}

pub fn stashes_event(root: &Path) -> Event {
    Event::Git(GitEvent::StashesRead {
        result: known_top(root).and_then(|top| stashes(&top)),
        now: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    })
}

fn rows_by_file(text: &str) -> Vec<DiffRow> {
    let mut rows = Vec::new();
    for section in text.split("\ndiff --git ").filter(|s| !s.trim().is_empty()) {
        let name = section
            .lines()
            .find_map(|l| {
                l.strip_prefix("+++ b/")
                    .or_else(|| l.strip_prefix("--- a/"))
            })
            .unwrap_or("");
        rows.push(DiffRow {
            kind: DiffKind::Section,
            old_line: None,
            new_line: None,
            text: name.trim_end().to_owned(),
        });
        rows.extend(parse_unified(section));
    }
    rows
}

pub(super) fn stash_diff(top: &Path, hash: &str) -> Result<Vec<DiffRow>, String> {
    let base = format!("{hash}^1");
    let mut args = DIFF_ARGS.to_vec();
    args.extend([base.as_str(), hash]);
    checked(top, args).map(|text| rows_by_file(&text))
}

#[cfg(test)]
mod tests {
    use super::super::git::tests::Repo;
    use super::super::git_stage::repo_state;
    use super::*;
    use crate::core::GitChange;

    fn diverged(repo: &Repo) {
        repo.write("f", "base\n");
        repo.commit_all("base");
        repo.git(&["checkout", "-qb", "other"]);
        repo.write("f", "theirs\n");
        repo.commit_all("on other");
        repo.git(&["checkout", "-q", "main"]);
        repo.write("f", "ours\n");
        repo.commit_all("on main");
    }

    fn merge_other(repo: &Repo) -> Result<String, String> {
        perform(
            &repo.path(),
            &GitOp::Merge {
                branch: "other".into(),
            },
        )
    }

    #[test]
    fn a_clean_merge_is_committed_with_gits_own_message() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.commit_all("base");
        repo.git(&["checkout", "-qb", "other"]);
        repo.write("b", "2\n");
        repo.commit_all("on other");
        repo.git(&["checkout", "-q", "main"]);
        repo.write("c", "3\n");
        repo.commit_all("on main");
        merge_other(&repo).unwrap();
        assert!(repo.path().join("b").exists());
        assert_eq!(
            repo.git(&["log", "-1", "--format=%s"]).trim(),
            "Merge branch 'other'"
        );
        assert_eq!(repo_state(&repo.path()).operation, None);
    }

    #[test]
    fn a_conflicted_merge_says_so_and_continues_once_resolved() {
        let repo = Repo::new();
        diverged(&repo);
        let err = merge_other(&repo).unwrap_err();
        assert!(err.contains("CONFLICT"), "{err}");
        let top = repo.path();
        assert_eq!(repo_state(&top).operation, Some(GitOperation::Merging));
        let (_, files) = repo.status();
        assert_eq!(files[0].change, GitChange::Conflicted);

        let err = perform(&top, &GitOp::Continue(GitOperation::Merging)).unwrap_err();
        assert!(
            err.contains("unmerged") || err.contains("not possible"),
            "{err}"
        );

        repo.write("f", "both\n");
        repo.git(&["add", "f"]);
        perform(&top, &GitOp::Continue(GitOperation::Merging)).unwrap();
        assert_eq!(repo_state(&top).operation, None);
        assert_eq!(
            repo.git(&["log", "-1", "--format=%s"]).trim(),
            "Merge branch 'other'"
        );
        assert_eq!(
            repo.git(&["log", "-1", "--format=%P"])
                .split_whitespace()
                .count(),
            2
        );
    }

    #[test]
    fn aborting_a_merge_goes_back_to_before_it() {
        let repo = Repo::new();
        diverged(&repo);
        merge_other(&repo).unwrap_err();
        let top = repo.path();
        perform(&top, &GitOp::Abort(GitOperation::Merging)).unwrap();
        assert_eq!(repo_state(&top).operation, None);
        assert_eq!(std::fs::read_to_string(top.join("f")).unwrap(), "ours\n");
    }

    #[test]
    fn a_rebase_shows_as_rebasing_and_goes_on_and_aborts() {
        let repo = Repo::new();
        diverged(&repo);
        let top = repo.path();
        let run_rebase = |repo: &Repo| run(&repo.path(), ["rebase", "other"]).unwrap();
        assert!(!run_rebase(&repo).status.success());
        assert_eq!(repo_state(&top).operation, Some(GitOperation::Rebasing));
        perform(&top, &GitOp::Abort(GitOperation::Rebasing)).unwrap();
        assert_eq!(repo_state(&top).operation, None);
        assert_eq!(std::fs::read_to_string(top.join("f")).unwrap(), "ours\n");

        assert!(!run_rebase(&repo).status.success());
        repo.write("f", "together\n");
        repo.git(&["add", "f"]);
        perform(&top, &GitOp::Continue(GitOperation::Rebasing)).unwrap();
        assert_eq!(repo_state(&top).operation, None);
        assert_eq!(repo.git(&["log", "-1", "--format=%s"]).trim(), "on main");
        assert_eq!(
            std::fs::read_to_string(top.join("f")).unwrap(),
            "together\n"
        );
    }

    #[test]
    fn stashes_push_list_show_apply_pop_and_drop() {
        let repo = Repo::new();
        repo.write("f", "1\n");
        repo.commit_all("base");
        repo.write("f", "2\n");
        let top = repo.path();
        perform(
            &top,
            &GitOp::StashPush {
                message: "first idea".into(),
                untracked: false,
            },
        )
        .unwrap();
        assert_eq!(std::fs::read_to_string(top.join("f")).unwrap(), "1\n");
        repo.write("g", "new\n");
        perform(
            &top,
            &GitOp::StashPush {
                message: String::new(),
                untracked: true,
            },
        )
        .unwrap();
        assert!(!top.join("g").exists(), "untracked went along when asked");
        let list = stashes(&top).unwrap();
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].index, 0);
        assert!(list[1].subject.contains("first idea"));
        assert!(list[0].time > 0);

        let rows = stash_diff(&top, &list[1].hash).unwrap();
        assert_eq!(rows[0].kind, DiffKind::Section);
        assert_eq!(rows[0].text, "f");
        assert!(
            rows.iter()
                .any(|r| r.text == "2" && r.kind == DiffKind::Added)
        );

        perform(&top, &GitOp::StashApply(list[1].clone())).unwrap();
        assert_eq!(std::fs::read_to_string(top.join("f")).unwrap(), "2\n");
        assert_eq!(stashes(&top).unwrap().len(), 2, "apply keeps it");
        repo.git(&["checkout", "-q", "--", "f"]);
        perform(&top, &GitOp::StashPop(list[1].clone())).unwrap();
        assert_eq!(stashes(&top).unwrap().len(), 1, "pop does not");
        perform(&top, &GitOp::StashDrop(stashes(&top).unwrap()[0].clone())).unwrap();
        assert!(stashes(&top).unwrap().is_empty());
    }

    #[test]
    fn a_stash_list_that_moved_underneath_refuses_instead_of_hitting_another() {
        let repo = Repo::new();
        repo.write("f", "1\n");
        repo.commit_all("base");
        repo.write("f", "2\n");
        repo.git(&["stash", "push", "-q", "-m", "keep me"]);
        let top = repo.path();
        let seen = stashes(&top).unwrap()[0].clone();
        repo.write("f", "3\n");
        repo.git(&["stash", "push", "-q", "-m", "newer"]);
        let err = perform(&top, &GitOp::StashDrop(seen)).unwrap_err();
        assert!(err.contains("changed"), "{err}");
        assert_eq!(stashes(&top).unwrap().len(), 2, "nothing dropped");
    }

    #[test]
    fn a_stash_pop_with_a_conflict_keeps_the_stash_and_marks_the_file() {
        let repo = Repo::new();
        repo.write("f", "1\n");
        repo.commit_all("base");
        repo.write("f", "stashed\n");
        repo.git(&["stash", "push", "-q"]);
        repo.write("f", "committed\n");
        repo.commit_all("next");
        let top = repo.path();
        let entry = stashes(&top).unwrap()[0].clone();
        let err = perform(&top, &GitOp::StashPop(entry)).unwrap_err();
        assert!(
            err.contains("CONFLICT") || err.contains("conflict"),
            "{err}"
        );
        assert_eq!(
            stashes(&top).unwrap().len(),
            1,
            "git keeps it on a conflict"
        );
        assert_eq!(repo.status().1[0].change, GitChange::Conflicted);
        assert_eq!(repo_state(&top).operation, None, "no operation to abort");
    }

    #[test]
    fn branch_delete_refuses_unmerged_work_with_git_words_until_forced() {
        let repo = Repo::new();
        diverged(&repo);
        let top = repo.path();
        let err = perform(
            &top,
            &GitOp::BranchDelete {
                name: "other".into(),
                force: false,
            },
        )
        .unwrap_err();
        assert!(err.contains("not fully merged"), "{err}");
        assert!(repo.git(&["branch", "--list", "other"]).contains("other"));
        perform(
            &top,
            &GitOp::BranchDelete {
                name: "other".into(),
                force: true,
            },
        )
        .unwrap();
        assert!(repo.git(&["branch", "--list", "other"]).trim().is_empty());
        let err = perform(
            &top,
            &GitOp::BranchDelete {
                name: "main".into(),
                force: true,
            },
        )
        .unwrap_err();
        assert!(err.contains("main"), "the current one: {err}");
    }

    #[test]
    fn a_merged_branch_deletes_without_force() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.commit_all("base");
        repo.git(&["branch", "done"]);
        let result = perform(
            &repo.path(),
            &GitOp::BranchDelete {
                name: "done".into(),
                force: false,
            },
        );
        assert!(result.unwrap().contains("Deleted branch"));
    }

    #[test]
    fn a_merge_blocked_by_local_changes_is_git_refusal_and_keeps_the_text() {
        let repo = Repo::new();
        diverged(&repo);
        repo.write("f", "uncommitted\n");
        let err = merge_other(&repo).unwrap_err();
        assert!(err.contains("overwritten"), "{err}");
        assert_eq!(
            std::fs::read_to_string(repo.path().join("f")).unwrap(),
            "uncommitted\n"
        );
        assert_eq!(repo_state(&repo.path()).operation, None);
    }

    #[test]
    fn garbage_stash_lines_are_skipped() {
        assert!(parse_stashes("nonsense\n\n").is_empty());
    }
}
