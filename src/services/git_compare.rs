use crate::core::git_command::GitEvent;
use std::ffi::OsStr;
use std::io::Read;
use std::os::unix::process::CommandExt;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

use rustix::process::{Pid, Signal, kill_process_group};

use super::git::{
    DIFF_ARGS, LOG_FORMAT, checked, known_top, parse_log, parse_name_status, parse_unified,
};
use crate::core::Event;
use crate::core::{DiffRow, GitCommit, GitCompareCommit, GitCompared, GitFile};

const COMPARE_LIMIT: &str = "-n500";
const CHECK_LIMIT: Duration = Duration::from_secs(120);
const MAX_UNTRACKED: u64 = 1024 * 1024;
const MAX_TODO_WARNINGS: usize = 3;
const MAX_OUTPUT: usize = 64 * 1024;

fn resolve(top: &Path, base: &str) -> Result<String, String> {
    if base.starts_with('-') {
        return Err(format!("Not a branch: {base}"));
    }
    let spec = format!("{base}^{{commit}}");
    checked(top, ["rev-parse", "--verify", "--quiet", &spec])
        .map(|hash| hash.trim().to_owned())
        .map_err(|_| format!("Not a branch: {base}"))
}

fn commits(top: &Path, range: &str) -> Result<Vec<GitCommit>, String> {
    let args = ["log", "--date-order", COMPARE_LIMIT, LOG_FORMAT, range];
    checked(top, args).map(|text| parse_log(&text))
}

fn compare(top: &Path, base: &str, path: Option<&Path>) -> Result<GitCompared, String> {
    let base = resolve(top, base)?;
    if let Some(path) = path {
        let relative = path.strip_prefix(top).unwrap_or(path);
        let args = [
            OsStr::new("diff"),
            OsStr::new("--name-status"),
            OsStr::new("-z"),
            OsStr::new("-M"),
            OsStr::new(&base),
            OsStr::new("--"),
            relative.as_os_str(),
        ];
        let files = checked(top, args).map(|t| parse_name_status(&t))?;
        return Ok(GitCompared {
            commits: Vec::new(),
            files,
        });
    }
    let side = |range: String, ours: bool| {
        commits(top, &range).map(|list| {
            list.into_iter()
                .map(|commit| GitCompareCommit { commit, ours })
                .collect::<Vec<_>>()
        })
    };
    let mut found = side(format!("{base}..HEAD"), true)?;
    found.extend(side(format!("HEAD..{base}"), false)?);
    let parted = format!("{base}...HEAD");
    let files = checked(top, ["diff", "--name-status", "-z", "-M", &parted])
        .map(|t| parse_name_status(&t))?;
    Ok(GitCompared {
        commits: found,
        files,
    })
}

pub fn compare_event(root: &Path, base: String, path: Option<PathBuf>) -> Event {
    let result = known_top(root).and_then(|top| compare(&top, &base, path.as_deref()));
    Event::Git(GitEvent::ComparedRead { base, path, result })
}

pub(super) fn whole_commit_diff(top: &Path, hash: &str) -> Result<Vec<DiffRow>, String> {
    let base = super::git::base_of(top, hash)?;
    let mut args = DIFF_ARGS.to_vec();
    args.extend(["-M", &base, hash]);
    checked(top, args).map(|text| parse_unified(&text))
}

pub(super) fn compare_diff(
    top: &Path,
    base: &str,
    worktree: bool,
    file: &GitFile,
) -> Result<Vec<DiffRow>, String> {
    let base = resolve(top, base)?;
    let range = if worktree {
        base
    } else {
        format!("{base}...HEAD")
    };
    let mut args: Vec<&OsStr> = DIFF_ARGS.iter().map(OsStr::new).collect();
    args.extend([OsStr::new("-M"), OsStr::new(&range), OsStr::new("--")]);
    args.push(file.path.as_os_str());
    if let Some(old) = &file.old_path {
        args.push(old.as_os_str());
    }
    checked(top, args).map(|text| parse_unified(&text))
}

fn has_todo(line: &str) -> bool {
    ["TODO", "FIXME"].iter().any(|word| {
        line.match_indices(word).any(|(at, _)| {
            let wordy = |c: char| c.is_alphanumeric() || c == '_';
            let before = line[..at].chars().next_back().is_none_or(|c| !wordy(c));
            let after = line[at + word.len()..]
                .chars()
                .next()
                .is_none_or(|c| !wordy(c));
            before && after
        })
    })
}

fn added_todos(diff: &str) -> Vec<(String, String)> {
    let mut file = String::new();
    let mut found = Vec::new();
    for line in diff.lines() {
        if let Some(name) = line.strip_prefix("+++ ") {
            file = name.strip_prefix("b/").unwrap_or(name).to_owned();
        } else if let Some(added) = line.strip_prefix('+').filter(|l| has_todo(l)) {
            found.push((file.clone(), clean(added)));
        }
    }
    found
}

fn clean(text: &str) -> String {
    let text: String = text
        .trim()
        .chars()
        .filter(|c| !c.is_control())
        .take(120)
        .collect();
    text
}

fn todo_warnings(found: Vec<(String, String)>) -> Vec<String> {
    let count = found.len();
    let mut warnings: Vec<String> = found
        .into_iter()
        .take(MAX_TODO_WARNINGS)
        .map(|(file, text)| format!("{file}: {text}"))
        .collect();
    if count > MAX_TODO_WARNINGS {
        warnings.push(format!("{} more TODOs", count - MAX_TODO_WARNINGS));
    }
    warnings
}

fn being_added(
    top: &Path,
    staged: bool,
    paths: &[PathBuf],
    untracked: &[PathBuf],
) -> Vec<(String, String)> {
    let mut args: Vec<&OsStr> = ["diff", "--no-color", "--no-ext-diff", "-U0"]
        .iter()
        .map(OsStr::new)
        .collect();
    if staged {
        args.push(OsStr::new("--cached"));
    } else {
        args.extend([OsStr::new("HEAD"), OsStr::new("--")]);
        args.extend(
            paths
                .iter()
                .filter(|p| !untracked.contains(p))
                .map(|p| p.as_os_str()),
        );
    }
    let tracked_chosen = staged || paths.iter().any(|p| !untracked.contains(p));
    let mut found = if tracked_chosen {
        checked(top, args).map_or_else(|_| Vec::new(), |diff| added_todos(&diff))
    } else {
        Vec::new()
    };
    for path in untracked {
        let full = top.join(path);
        let small = std::fs::metadata(&full).is_ok_and(|m| m.len() <= MAX_UNTRACKED);
        if small && let Ok(text) = std::fs::read_to_string(&full) {
            found.extend(
                text.lines()
                    .filter(|l| has_todo(l))
                    .map(|l| (path.display().to_string(), clean(l))),
            );
        }
    }
    found
}

fn run_check(top: &Path, command: &str, limit: Duration) -> Option<String> {
    let script = format!("( {command}\n) 2>&1");
    let child = Command::new("sh")
        .args(["-c", &script])
        .current_dir(top)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .process_group(0)
        .spawn();
    let mut child = match child {
        Ok(child) => child,
        Err(error) => return Some(format!("Check could not start: {error}")),
    };
    let group = Pid::from_raw(i32::try_from(child.id()).unwrap_or(0));
    let mut stdout = child.stdout.take();
    let reader = std::thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(out) = stdout.as_mut() {
            let mut chunk = [0_u8; 8192];
            while let Ok(n) = out.read(&mut chunk) {
                if n == 0 {
                    break;
                }
                text.extend_from_slice(&chunk[..n]);
                if text.len() > MAX_OUTPUT {
                    text.drain(..text.len() - MAX_OUTPUT);
                }
            }
        }
        text
    });
    let started = Instant::now();
    let status = loop {
        match child.try_wait() {
            Ok(Some(status)) => break Some(status),
            Ok(None) if started.elapsed() < limit => {
                std::thread::sleep(Duration::from_millis(20));
            }
            _ => break None,
        }
    };
    if let Some(group) = group {
        let _ = kill_process_group(group, Signal::KILL);
    }
    let _ = child.wait();
    let text = reader.join().unwrap_or_default();
    let text = String::from_utf8_lossy(&text);
    let last = text
        .lines()
        .rev()
        .find(|l| !l.trim().is_empty())
        .map_or_else(String::new, clean);
    match status {
        None => Some(format!(
            "Check still running after {}s, stopped",
            limit.as_secs()
        )),
        Some(status) if status.success() => None,
        Some(status) => Some(format!(
            "Check failed ({}): {last}",
            status
                .code()
                .map_or_else(|| "killed".to_owned(), |c| format!("code {c}")),
        )),
    }
}

pub fn checks_event(
    root: &Path,
    staged: bool,
    paths: &[PathBuf],
    untracked: &[PathBuf],
    todo: bool,
    command: Option<&str>,
) -> Event {
    let mut warnings = Vec::new();
    match known_top(root) {
        Ok(top) => {
            if todo {
                warnings.extend(todo_warnings(being_added(&top, staged, paths, untracked)));
            }
            if let Some(command) = command {
                warnings.extend(run_check(&top, command, CHECK_LIMIT));
            }
        }
        Err(text) => warnings.push(text),
    }
    Event::Git(GitEvent::CommitChecked { warnings })
}

#[cfg(test)]
mod tests {
    use super::super::git::tests::Repo;
    use super::*;

    fn commit(repo: &Repo, name: &str, text: &str) {
        std::fs::write(repo.path().join(name), text).unwrap();
        repo.git(&["add", "-A"]);
        repo.git(&["commit", "-qm", name]);
    }

    fn feature_repo() -> Repo {
        let repo = Repo::new();
        commit(&repo, "a.txt", "one\n");
        repo.git(&["branch", "-M", "main"]);
        repo.git(&["checkout", "-qb", "feature"]);
        commit(&repo, "b.txt", "new\n");
        repo.git(&["checkout", "-q", "main"]);
        commit(&repo, "c.txt", "theirs\n");
        repo.git(&["checkout", "-q", "feature"]);
        repo
    }

    #[test]
    fn two_branches_list_each_sides_commits_and_this_sides_files() {
        let repo = feature_repo();
        let found = compare(&repo.path(), "main", None).unwrap();
        let side = |ours| {
            found
                .commits
                .iter()
                .filter(|c| c.ours == ours)
                .map(|c| c.commit.subject.as_str())
                .collect::<Vec<_>>()
        };
        assert_eq!(side(true), ["b.txt"]);
        assert_eq!(side(false), ["c.txt"]);
        let names: Vec<_> = found.files.iter().map(|f| f.path.clone()).collect();
        assert_eq!(
            names,
            [PathBuf::from("b.txt")],
            "not what main did after parting"
        );
        let rows = compare_diff(&repo.path(), "main", false, &found.files[0]).unwrap();
        assert!(!rows.is_empty());
        let hash = &found.commits[0].commit.hash;
        assert!(!whole_commit_diff(&repo.path(), hash).unwrap().is_empty());
    }

    #[test]
    fn a_path_compares_as_it_is_now_and_only_that_path() {
        let repo = feature_repo();
        std::fs::write(repo.path().join("a.txt"), "unsaved in no commit\n").unwrap();
        std::fs::write(repo.path().join("b.txt"), "changed too\n").unwrap();
        let path = repo.path().join("a.txt");
        let found = compare(&repo.path(), "main", Some(&path)).unwrap();
        assert!(found.commits.is_empty());
        assert_eq!(found.files.len(), 1);
        assert_eq!(found.files[0].path, PathBuf::from("a.txt"));
        let rows = compare_diff(&repo.path(), "main", true, &found.files[0]).unwrap();
        assert!(!rows.is_empty());
    }

    #[test]
    fn an_option_or_an_unknown_name_is_no_branch() {
        let repo = feature_repo();
        for bad in ["--output=/tmp/x", "no-such-branch"] {
            assert!(compare(&repo.path(), bad, None).is_err(), "{bad}");
        }
    }

    #[test]
    fn only_added_lines_with_the_whole_word_count_as_todo() {
        let diff = "+++ b/a.rs\n+// TODO: x\n-// TODO: gone\n+TODOS\n+// FIXME\n+++ b/c.rs\n+ok\n";
        let found = added_todos(diff);
        assert_eq!(
            found,
            [
                ("a.rs".to_owned(), "// TODO: x".to_owned()),
                ("a.rs".to_owned(), "// FIXME".to_owned())
            ]
        );
    }

    #[test]
    fn the_todo_check_looks_at_what_is_being_committed() {
        let repo = feature_repo();
        std::fs::write(repo.path().join("a.txt"), "one\n// TODO staged\n").unwrap();
        repo.git(&["add", "a.txt"]);
        std::fs::write(repo.path().join("b.txt"), "new\n// TODO not staged\n").unwrap();
        std::fs::write(repo.path().join("n.txt"), "// TODO new file\n").unwrap();
        let staged = being_added(&repo.path(), true, &[], &[]);
        assert_eq!(staged.len(), 1, "{staged:?}");
        assert!(staged[0].1.contains("staged"));
        let ticked = being_added(
            &repo.path(),
            false,
            &[PathBuf::from("b.txt"), PathBuf::from("n.txt")],
            &[PathBuf::from("n.txt")],
        );
        assert_eq!(ticked.len(), 2, "{ticked:?}");
    }

    #[test]
    fn a_failing_command_warns_with_its_last_line_and_a_passing_one_is_silent() {
        let repo = feature_repo();
        let limit = Duration::from_secs(20);
        assert_eq!(run_check(&repo.path(), "true", limit), None);
        let warning = run_check(
            &repo.path(),
            "echo one; echo 'two\x1b[31m' >&2; exit 3",
            limit,
        )
        .unwrap();
        assert!(warning.contains("code 3"), "{warning}");
        assert!(warning.contains("two"), "{warning}");
        assert!(!warning.contains('\x1b'), "{warning}");
    }

    #[test]
    fn a_hanging_command_is_stopped_with_what_it_started() {
        let repo = feature_repo();
        let marker = repo.path().join("late");
        let command = format!("(sleep 2; touch {}) & sleep 60", marker.display());
        let started = Instant::now();
        let warning = run_check(&repo.path(), &command, Duration::from_millis(300)).unwrap();
        assert!(warning.contains("stopped"), "{warning}");
        assert!(started.elapsed() < Duration::from_secs(2));
        std::thread::sleep(Duration::from_millis(2200));
        assert!(!marker.exists(), "the child went too");
    }
}
