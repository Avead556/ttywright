use crate::core::git_command::GitEvent;
use std::collections::HashMap;
use std::path::{Path, PathBuf};

use super::git::{LOG_LIMIT, checked, known_top, parse_name_status};
use crate::core::{
    DocumentId, Event, GitBlame, GitBlameCommit, GitChange, GitCommit, GitFile, GitFileCommit,
};

const FOLLOW_FORMAT: &str = "--format=%x1e%H%x1f%P%x1f%an%x1f%at%x1f%s";

fn now() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

pub(super) fn relative_to(top: &Path, path: &Path) -> Result<PathBuf, String> {
    let resolved = std::fs::canonicalize(path).or_else(|_| {
        let parent = path.parent().ok_or(std::io::ErrorKind::NotFound)?;
        let name = path.file_name().ok_or(std::io::ErrorKind::NotFound)?;
        std::fs::canonicalize(parent).map(|p| p.join(name))
    });
    resolved
        .ok()
        .and_then(|p| p.strip_prefix(top).ok().map(Path::to_path_buf))
        .ok_or_else(|| format!("{} is not in this repository", path.display()))
}

fn parse_follow(text: &str, current: &Path) -> Vec<GitFileCommit> {
    let mut name = current.to_path_buf();
    let mut entries = Vec::new();
    for record in text.split('\u{1e}').filter(|r| !r.is_empty()) {
        let (head, status) = record.split_once('\0').unwrap_or((record, ""));
        let mut fields = head.split('\u{1f}');
        let (Some(hash), Some(parents), Some(author), Some(time), Some(subject)) = (
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
            fields.next(),
        ) else {
            continue;
        };
        let file = parse_name_status(status.trim_start_matches('\n'))
            .into_iter()
            .next()
            .unwrap_or_else(|| GitFile::new(name.clone(), GitChange::Modified));
        name = file.old_path.clone().unwrap_or_else(|| file.path.clone());
        entries.push(GitFileCommit {
            commit: GitCommit {
                hash: hash.to_owned(),
                parents: parents.split_whitespace().map(str::to_owned).collect(),
                author: author.to_owned(),
                time: time.parse().unwrap_or(0),
                refs: Vec::new(),
                subject: subject.to_owned(),
            },
            file,
        });
    }
    entries
}

fn file_history(top: &Path, path: &Path) -> Result<Vec<GitFileCommit>, String> {
    let relative = relative_to(top, path)?;
    let args = [
        "log".as_ref(),
        "--follow".as_ref(),
        "-n".as_ref(),
        LOG_LIMIT.as_ref(),
        FOLLOW_FORMAT.as_ref(),
        "--name-status".as_ref(),
        "-z".as_ref(),
        "--".as_ref(),
        relative.as_os_str(),
    ];
    let args: Vec<&std::ffi::OsStr> = args.to_vec();
    checked(top, args).map(|text| parse_follow(&text, &relative))
}

pub fn file_history_event(root: &Path, path: PathBuf) -> Event {
    let result = known_top(root).and_then(|top| file_history(&top, &path));
    Event::Git(GitEvent::FileHistoryRead {
        path,
        result,
        now: now(),
    })
}

fn parse_blame(text: &[u8]) -> GitBlame {
    let mut blame = GitBlame::default();
    let mut known: HashMap<String, usize> = HashMap::new();
    let mut current = None;
    for raw in text.split(|b| *b == b'\n') {
        if raw.first() == Some(&b'\t') {
            if let Some(index) = current {
                blame.lines.push(index);
            }
            continue;
        }
        let line = String::from_utf8_lossy(raw);
        let line = line.as_ref();
        if let Some(author) = line.strip_prefix("author ") {
            if let Some(commit) = current.and_then(|i: usize| blame.commits.get_mut(i)) {
                author.clone_into(&mut commit.author);
            }
        } else if let Some(time) = line.strip_prefix("author-time ") {
            if let Some(commit) = current.and_then(|i: usize| blame.commits.get_mut(i)) {
                commit.time = time.trim().parse().unwrap_or(0);
            }
        } else if let Some(hash) = line
            .split(' ')
            .next()
            .filter(|h| h.len() >= 40 && h.bytes().all(|b| b.is_ascii_hexdigit()))
        {
            current = Some(*known.entry(hash.to_owned()).or_insert_with(|| {
                blame.commits.push(GitBlameCommit {
                    hash: hash.to_owned(),
                    author: String::new(),
                    time: 0,
                });
                blame.commits.len() - 1
            }));
        }
    }
    blame
}

fn blame(top: &Path, path: &Path) -> Result<GitBlame, String> {
    let relative = relative_to(top, path)?;
    let args: Vec<&std::ffi::OsStr> = vec![
        "blame".as_ref(),
        "--porcelain".as_ref(),
        "--".as_ref(),
        relative.as_os_str(),
    ];
    let output = super::git::run(top, args)?;
    if output.status.success() {
        Ok(parse_blame(&output.stdout))
    } else {
        Err(super::git::failure(&output))
    }
}

pub fn blame_event(root: &Path, doc: DocumentId, path: &Path, version: u64) -> Event {
    Event::Git(GitEvent::BlameRead {
        doc,
        version,
        result: known_top(root).and_then(|top| blame(&top, path)),
        now: now(),
    })
}

#[cfg(test)]
mod tests {
    use super::super::git::tests::Repo;
    use super::*;

    #[test]
    fn history_follows_a_rename_and_each_commit_says_what_the_file_was_called() {
        let repo = Repo::new();
        repo.write("a b.txt", "one\ntwo\nthree\n");
        repo.commit_all("first");
        repo.write("a b.txt", "one\ntwo\nthree\nfour\n");
        repo.commit_all("second");
        repo.git(&["mv", "a b.txt", "ы c.txt"]);
        repo.git(&["commit", "-qm", "moved"]);
        repo.write("other", "unrelated");
        repo.commit_all("elsewhere");
        repo.write("ы c.txt", "ONE\ntwo\nthree\nfour\n");
        repo.commit_all("fourth");
        let top = repo.path();
        let list = file_history(&top, &top.join("ы c.txt")).unwrap();
        let subjects: Vec<&str> = list.iter().map(|e| e.commit.subject.as_str()).collect();
        assert_eq!(subjects, ["fourth", "moved", "second", "first"]);
        assert_eq!(list[0].file.path, Path::new("ы c.txt"));
        assert_eq!(list[1].file.change, GitChange::Renamed);
        assert_eq!(list[1].file.old_path.as_deref(), Some(Path::new("a b.txt")));
        assert_eq!(list[2].file.path, Path::new("a b.txt"));
        assert_eq!(list[3].file.change, GitChange::Added);
        let first_parent = &list[1].commit.parents[0];
        assert_eq!(first_parent, &list[2].commit.hash);
        assert_eq!(list[0].commit.author, "Test");
        assert!(list[0].commit.time > 0);
    }

    #[test]
    fn a_merge_without_a_status_keeps_the_name_the_newer_commit_gave() {
        let text = "\u{1e}c3\u{1f}c2\u{1f}A\u{1f}3\u{1f}move\0\nR100\0old\0new\0\
                    \u{1e}c2\u{1f}c1 c0\u{1f}A\u{1f}2\u{1f}merge\n\
                    \u{1e}c1\u{1f}\u{1f}A\u{1f}1\u{1f}start\0\nA\0old\0";
        let list = parse_follow(text, Path::new("new"));
        assert_eq!(list.len(), 3);
        assert_eq!(list[1].commit.parents, ["c1", "c0"]);
    }

    #[test]
    fn a_file_outside_the_repository_or_unknown_to_it_gets_git_words() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.commit_all("base");
        let top = repo.path();
        let outside = tempfile::tempdir().unwrap();
        let err = file_history(&top, &outside.path().join("x")).unwrap_err();
        assert!(err.contains("not in this repository"), "{err}");
        assert!(file_history(&top, &top.join("never")).unwrap().is_empty());
        repo.write("untracked", "x\n");
        let err = blame(&top, &top.join("untracked")).unwrap_err();
        assert!(err.contains("no such path"), "{err}");
    }

    #[test]
    fn blame_names_each_lines_commit_with_odd_text_and_uncommitted_lines() {
        let repo = Repo::new();
        repo.write("f", "author fake\n\tTabbed\nthird\n");
        repo.commit_all("first");
        let mut bytes = std::fs::read(repo.path().join("f")).unwrap();
        bytes.extend_from_slice(b"caf\xe9\n");
        std::fs::write(repo.path().join("f"), &bytes).unwrap();
        repo.commit_all("second");
        std::fs::write(repo.path().join("f"), [&bytes[..], b"new\n"].concat()).unwrap();
        let top = repo.path();
        let found = blame(&top, &top.join("f")).unwrap();
        assert_eq!(found.lines.len(), 5);
        let subject = |hash: &str| repo.git(&["log", "-1", "--format=%s", hash]);
        let line = |n: usize| found.line(n).unwrap();
        assert_eq!(line(0).author, "Test");
        assert!(line(0).time > 0);
        assert_eq!(subject(&line(2).hash).trim(), "first");
        assert_eq!(subject(&line(3).hash).trim(), "second");
        assert!(line(4).is_uncommitted());
        assert!(!line(3).is_uncommitted());
        assert_eq!(found.line(5), None);
    }

    #[test]
    fn an_author_named_after_a_hash_is_still_an_author() {
        let hash = "a".repeat(40);
        let text =
            format!("{hash} 1 1 1\nauthor {hash} x\nauthor-time 5\n\tone\n{hash} 2 2\n\ttwo\n");
        let found = parse_blame(text.as_bytes());
        assert_eq!(found.commits.len(), 1);
        assert_eq!(found.lines, [0, 0]);
        assert_eq!(found.commits[0].author, format!("{hash} x"));
    }

    #[test]
    fn garbage_from_blame_gives_no_lines_and_no_panic() {
        assert_eq!(
            parse_blame(b"\xff\xfe nonsense\nauthor x\n\torphan\n")
                .lines
                .len(),
            0
        );
    }
}
