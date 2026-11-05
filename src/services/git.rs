use crate::core::git_command::GitEvent;
use std::collections::HashMap;
use std::ffi::OsStr;
use std::path::{Path, PathBuf};
use std::process::{Command, Output, Stdio};
use std::sync::{Mutex, OnceLock};

use crate::core::{
    DiffRow, Event, GitBranch, GitChange, GitCode, GitCommit, GitDiffSource, GitFile, GitRef,
    GitRefKind, GitRemoteOp, GitRepoState,
};

pub(super) const LOG_LIMIT: &str = "2000";
pub(super) const LOG_FORMAT: &str = "--format=%H%x1f%P%x1f%an%x1f%at%x1f%D%x1f%s%x1e";

pub(super) fn run<I, S>(dir: &Path, args: I) -> Result<Output, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    Command::new("git")
        .current_dir(dir)
        .env("GIT_OPTIONAL_LOCKS", "0")
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("GIT_EDITOR", "true")
        .env("LC_ALL", "C")
        .stdin(Stdio::null())
        .args(args)
        .output()
        .map_err(|e| {
            if e.kind() == std::io::ErrorKind::NotFound {
                "git is not installed".to_owned()
            } else {
                e.to_string()
            }
        })
}

pub(super) fn failure(output: &Output) -> String {
    let stderr = String::from_utf8_lossy(&output.stderr);
    let stdout = String::from_utf8_lossy(&output.stdout);
    let text = if stderr.trim().is_empty() {
        stdout
    } else {
        stderr
    };
    let lines: Vec<&str> = text
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty())
        .collect();
    if lines.is_empty() {
        "git failed".to_owned()
    } else {
        lines.join("  ")
    }
}

pub(super) fn checked<I, S>(dir: &Path, args: I) -> Result<String, String>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let output = run(dir, args)?;
    if output.status.success() {
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    } else {
        Err(failure(&output))
    }
}

fn tops() -> &'static Mutex<HashMap<PathBuf, PathBuf>> {
    static TOPS: OnceLock<Mutex<HashMap<PathBuf, PathBuf>>> = OnceLock::new();
    TOPS.get_or_init(Mutex::default)
}

fn bases() -> &'static Mutex<HashMap<String, String>> {
    static BASES: OnceLock<Mutex<HashMap<String, String>>> = OnceLock::new();
    BASES.get_or_init(Mutex::default)
}

pub(super) fn known_top(root: &Path) -> Result<PathBuf, String> {
    if let Some(top) = tops().lock().ok().and_then(|t| t.get(root).cloned()) {
        return Ok(top);
    }
    top_level(root)
}

pub(super) fn top_level(root: &Path) -> Result<PathBuf, String> {
    let output = run(root, ["rev-parse", "--show-toplevel"])?;
    let mut known = tops().lock().ok();
    if output.status.success() {
        let top = PathBuf::from(String::from_utf8_lossy(&output.stdout).trim_end());
        if let Some(known) = known.as_mut() {
            known.insert(root.to_path_buf(), top.clone());
        }
        Ok(top)
    } else {
        if let Some(known) = known.as_mut() {
            known.remove(root);
        }
        Err("Not a git repository".to_owned())
    }
}

pub(super) fn has_commits(top: &Path) -> bool {
    run(top, ["rev-parse", "--quiet", "--verify", "HEAD"]).is_ok_and(|o| o.status.success())
}

fn branch(top: &Path) -> Option<String> {
    let output = run(top, ["symbolic-ref", "--short", "--quiet", "HEAD"]).ok()?;
    if output.status.success() {
        return Some(String::from_utf8_lossy(&output.stdout).trim().to_owned());
    }
    let short = checked(top, ["rev-parse", "--short", "HEAD"]).ok()?;
    Some(format!("detached at {}", short.trim()))
}

fn parse_status(text: &str) -> Vec<GitFile> {
    let mut files = Vec::new();
    let mut fields = text.split('\0').filter(|f| !f.is_empty());
    while let Some(entry) = fields.next() {
        let Some((code, path)) = entry.split_at_checked(3) else {
            continue;
        };
        let code = code.get(..2).unwrap_or(code);
        let change = match code.trim() {
            "??" => GitChange::Untracked,
            "DD" | "AU" | "UD" | "UA" | "DU" | "AA" | "UU" => GitChange::Conflicted,
            c if c.contains('R') || c.contains('C') => GitChange::Renamed,
            c if c.contains('D') => GitChange::Deleted,
            c if c.contains('A') => GitChange::Added,
            _ => GitChange::Modified,
        };
        let old_path = (change == GitChange::Renamed)
            .then(|| fields.next().map(PathBuf::from))
            .flatten();
        let mut letters = code.chars();
        let index = letters.next().unwrap_or(' ');
        let worktree = letters.next().unwrap_or(' ');
        files.push(GitFile {
            path: PathBuf::from(path),
            old_path,
            change,
            code: GitCode::new(index, worktree),
        });
    }
    files.sort_by(|a, b| a.path.cmp(&b.path));
    files
}

fn parse_refs(decoration: &str) -> Vec<GitRef> {
    decoration
        .split(", ")
        .filter(|r| !r.is_empty())
        .map(|entry| {
            if let Some(branch) = entry.strip_prefix("HEAD -> ") {
                let name = branch.strip_prefix("refs/heads/").unwrap_or(branch);
                return GitRef {
                    name: name.to_owned(),
                    kind: GitRefKind::Head,
                };
            }
            let (name, kind) = if let Some(name) = entry.strip_prefix("tag: refs/tags/") {
                (name, GitRefKind::Tag)
            } else if let Some(name) = entry.strip_prefix("refs/heads/") {
                (name, GitRefKind::Local)
            } else if let Some(name) = entry.strip_prefix("refs/remotes/") {
                (name, GitRefKind::Remote)
            } else {
                (entry, GitRefKind::Head)
            };
            GitRef {
                name: name.to_owned(),
                kind,
            }
        })
        .collect()
}

pub(super) fn parse_log(text: &str) -> Vec<GitCommit> {
    text.split('\u{1e}')
        .filter_map(|record| {
            let record = record.trim_start_matches('\n');
            let mut fields = record.split('\u{1f}');
            let hash = fields.next().filter(|h| !h.is_empty())?.to_owned();
            let parents = fields.next()?;
            let author = fields.next()?.to_owned();
            let time = fields.next()?.parse().unwrap_or(0);
            let refs = parse_refs(fields.next()?);
            let subject = fields.next()?.to_owned();
            Some(GitCommit {
                hash,
                parents: parents.split_whitespace().map(str::to_owned).collect(),
                author,
                time,
                refs,
                subject,
            })
        })
        .collect()
}

pub(super) fn parse_name_status(text: &str) -> Vec<GitFile> {
    let mut files = Vec::new();
    let mut fields = text.split('\0').filter(|f| !f.is_empty());
    while let Some(code) = fields.next() {
        let renamed = code.starts_with(['R', 'C']);
        let old_path = if renamed {
            fields.next().map(PathBuf::from)
        } else {
            None
        };
        let Some(path) = fields.next() else {
            break;
        };
        let change = match code.chars().next() {
            Some('A') => GitChange::Added,
            Some('D') => GitChange::Deleted,
            Some('R' | 'C') => GitChange::Renamed,
            Some('U') => GitChange::Conflicted,
            _ => GitChange::Modified,
        };
        files.push(GitFile {
            path: PathBuf::from(path),
            old_path,
            change,
            code: GitCode::default(),
        });
    }
    files
}

pub(super) use crate::core::parse_unified;

pub(super) const DIFF_ARGS: [&str; 3] = ["diff", "--no-color", "--no-ext-diff"];

pub(super) fn worktree_diff(top: &Path, file: &GitFile) -> Result<Vec<DiffRow>, String> {
    let mut args: Vec<&OsStr> = DIFF_ARGS.iter().map(OsStr::new).collect();
    if file.change == GitChange::Untracked || !has_commits(top) {
        args.extend(["--no-index", "--", "/dev/null"].map(OsStr::new));
        args.push(file.path.as_os_str());
        let output = run(top, args)?;
        return Ok(parse_unified(&String::from_utf8_lossy(&output.stdout)));
    }
    args.extend(["-M", "HEAD", "--"].map(OsStr::new));
    args.push(file.path.as_os_str());
    if let Some(old) = &file.old_path {
        args.push(old.as_os_str());
    }
    checked(top, args).map(|text| parse_unified(&text))
}

pub(super) fn base_of(top: &Path, hash: &str) -> Result<String, String> {
    if let Some(base) = bases().lock().ok().and_then(|b| b.get(hash).cloned()) {
        return Ok(base);
    }
    let base = read_base(top, hash)?;
    if let Ok(mut known) = bases().lock() {
        known.insert(hash.to_owned(), base.clone());
    }
    Ok(base)
}

fn read_base(top: &Path, hash: &str) -> Result<String, String> {
    match run(
        top,
        ["rev-parse", "--quiet", "--verify", &format!("{hash}^1")],
    ) {
        Ok(output) if output.status.success() => {
            Ok(String::from_utf8_lossy(&output.stdout).trim().to_owned())
        }
        _ => checked(top, ["hash-object", "-t", "tree", "/dev/null"]).map(|h| h.trim().to_owned()),
    }
}

fn commit_diff(top: &Path, hash: &str, file: &GitFile) -> Result<Vec<DiffRow>, String> {
    let base = base_of(top, hash)?;
    let mut args: Vec<&OsStr> = DIFF_ARGS.iter().map(OsStr::new).collect();
    args.push(OsStr::new("-M"));
    args.push(OsStr::new(&base));
    args.push(OsStr::new(hash));
    args.push(OsStr::new("--"));
    args.push(file.path.as_os_str());
    if let Some(old) = &file.old_path {
        args.push(old.as_os_str());
    }
    checked(top, args).map(|text| parse_unified(&text))
}

fn commit_files(top: &Path, hash: &str) -> Result<Vec<GitFile>, String> {
    let base = base_of(top, hash)?;
    checked(top, ["diff", "--name-status", "-z", "-M", &base, hash]).map(|t| parse_name_status(&t))
}

fn read_log(top: &Path) -> Result<Vec<GitCommit>, String> {
    let any_ref = checked(top, ["for-each-ref", "--count=1"])?;
    if any_ref.trim().is_empty() && !has_commits(top) {
        return Ok(Vec::new());
    }
    let args = [
        "log",
        "--all",
        "--date-order",
        "--decorate=full",
        "-n",
        LOG_LIMIT,
        LOG_FORMAT,
    ];
    checked(top, args).map(|text| parse_log(&text))
}

fn commit(
    top: &Path,
    message: &str,
    paths: &[PathBuf],
    untracked: &[PathBuf],
    amend: bool,
) -> Result<String, String> {
    if !untracked.is_empty() {
        let mut add: Vec<&OsStr> = vec![OsStr::new("add"), OsStr::new("--")];
        add.extend(untracked.iter().map(|p| p.as_os_str()));
        checked(top, add)?;
    }
    let mut args: Vec<&OsStr> = ["commit", "--only", "-m", message]
        .into_iter()
        .map(OsStr::new)
        .collect();
    if amend {
        args.push(OsStr::new("--amend"));
    }
    args.push(OsStr::new("--"));
    args.extend(paths.iter().map(|p| p.as_os_str()));
    checked(top, args)?;
    checked(top, ["log", "-1", "--format=%h %s"]).map(|s| s.trim().to_owned())
}

fn remote(top: &Path, op: GitRemoteOp) -> Result<String, String> {
    let args: &[&str] = match op {
        GitRemoteOp::Push => &["push"],
        GitRemoteOp::Pull => &["pull", "--ff-only"],
    };
    let output = run(top, args)?;
    if !output.status.success() {
        return Err(failure(&output));
    }
    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    let text = if stdout.trim().is_empty() {
        stderr
    } else {
        stdout
    };
    Ok(text.lines().last().unwrap_or("Done").trim().to_owned())
}

pub fn status_event(root: &Path) -> Event {
    match top_level(root) {
        Err(message) => Event::Git(GitEvent::StatusRead {
            branch: None,
            top: None,
            files: Err(message),
            state: GitRepoState::default(),
        }),
        Ok(top) => {
            let files = checked(
                &top,
                ["status", "--porcelain=v1", "-z", "--untracked-files=all"],
            )
            .map(|text| parse_status(&text));
            Event::Git(GitEvent::StatusRead {
                branch: branch(&top),
                state: super::git_stage::repo_state(&top),
                top: Some(top),
                files,
            })
        }
    }
}

pub fn log_event(root: &Path) -> Event {
    Event::Git(GitEvent::LogRead {
        result: top_level(root).and_then(|top| read_log(&top)),
        now: std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_secs()),
    })
}

pub fn commit_files_event(root: &Path, hash: String) -> Event {
    let result = known_top(root).and_then(|top| commit_files(&top, &hash));
    Event::Git(GitEvent::CommitFilesRead { hash, result })
}

pub fn diff_event(root: &Path, id: u64, source: &GitDiffSource) -> Event {
    let result = known_top(root).and_then(|top| match source {
        GitDiffSource::Commit { hash, file } => commit_diff(&top, hash, file),
        GitDiffSource::Stash(hash) => super::git_ops::stash_diff(&top, hash),
        GitDiffSource::WholeCommit(hash) => super::git_compare::whole_commit_diff(&top, hash),
        GitDiffSource::Compare {
            base,
            worktree,
            file,
        } => super::git_compare::compare_diff(&top, base, *worktree, file),
    });
    Event::Git(GitEvent::DiffRead { id, result })
}

pub fn commit_event(
    root: &Path,
    message: &str,
    paths: &[PathBuf],
    untracked: &[PathBuf],
    amend: bool,
) -> Event {
    Event::Git(GitEvent::Committed {
        result: top_level(root).and_then(|top| commit(&top, message, paths, untracked, amend)),
    })
}

fn last_commit(top: &Path) -> Result<(String, bool), String> {
    let message = checked(top, ["log", "-1", "--format=%B"])?;
    let remotes = checked(top, ["branch", "--remotes", "--contains", "HEAD"])?;
    Ok((message, !remotes.trim().is_empty()))
}

pub fn last_commit_event(root: &Path) -> Event {
    Event::Git(GitEvent::LastCommit {
        result: top_level(root).and_then(|top| last_commit(&top)),
    })
}

fn rollback(top: &Path, file: &GitFile) -> Result<(), String> {
    fn with<'a>(start: &'a [&'a str], file: &'a GitFile) -> Vec<&'a OsStr> {
        let mut args: Vec<&OsStr> = start.iter().map(OsStr::new).collect();
        args.push(OsStr::new("--"));
        args.push(file.path.as_os_str());
        if let Some(old) = &file.old_path {
            args.push(old.as_os_str());
        }
        args
    }
    match file.change {
        GitChange::Added => checked(top, with(&["reset", "-q"], file)).map(drop),
        GitChange::Renamed => {
            checked(top, with(&["reset", "-q"], file))?;
            let old = file.old_path.as_deref().unwrap_or(&file.path);
            checked(
                top,
                [OsStr::new("restore"), OsStr::new("--"), old.as_os_str()],
            )
            .map(drop)
        }
        _ => checked(
            top,
            with(
                &["restore", "--source=HEAD", "--staged", "--worktree"],
                file,
            ),
        )
        .map(drop),
    }
}

pub fn rollback_event(root: &Path, file: &GitFile) -> Event {
    Event::Git(GitEvent::RolledBack {
        path: file.path.clone(),
        result: top_level(root).and_then(|top| rollback(&top, file)),
    })
}

fn branches(top: &Path) -> Result<Vec<GitBranch>, String> {
    let text = checked(
        top,
        [
            "for-each-ref",
            "--format=%(refname)",
            "refs/heads",
            "refs/remotes",
        ],
    )?;
    let mut list: Vec<GitBranch> = text
        .lines()
        .filter(|r| !r.ends_with("/HEAD"))
        .filter_map(|r| {
            if let Some(name) = r.strip_prefix("refs/heads/") {
                Some(GitBranch {
                    name: name.to_owned(),
                    remote: false,
                })
            } else {
                r.strip_prefix("refs/remotes/").map(|name| GitBranch {
                    name: name.to_owned(),
                    remote: true,
                })
            }
        })
        .collect();
    list.sort_by_key(|b| b.remote);
    Ok(list)
}

pub fn branches_event(root: &Path) -> Event {
    Event::Git(GitEvent::BranchesRead {
        result: top_level(root).and_then(|top| branches(&top)),
    })
}

fn switch(top: &Path, branch: &GitBranch, create: bool) -> Result<(), String> {
    if create {
        return checked(top, ["switch", "-c", &branch.name]).map(drop);
    }
    let name = if branch.remote {
        branch
            .name
            .split_once('/')
            .map_or(branch.name.as_str(), |(_, n)| n)
    } else {
        &branch.name
    };
    checked(top, ["switch", name]).map(drop)
}

pub fn switch_event(root: &Path, branch: &GitBranch, create: bool) -> Event {
    Event::Git(GitEvent::Switched {
        name: branch.name.clone(),
        result: top_level(root).and_then(|top| switch(&top, branch, create)),
    })
}

fn base_text(root: &Path, path: &Path) -> Option<String> {
    let top = known_top(root).ok()?;
    let resolved = std::fs::canonicalize(path).ok()?;
    let relative = resolved.strip_prefix(&top).ok()?;
    let spec = format!("HEAD:{}", relative.display());
    let output = run(&top, ["show", &spec]).ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

pub fn base_event(root: &Path, path: PathBuf) -> Event {
    let text = base_text(root, &path);
    Event::Git(GitEvent::BaseRead { path, text })
}

pub fn remote_event(root: &Path, op: GitRemoteOp) -> Event {
    Event::Git(GitEvent::RemoteDone {
        op,
        result: top_level(root).and_then(|top| remote(&top, op)),
    })
}

#[cfg(test)]
pub(super) mod tests {
    use super::*;
    use crate::core::{DiffKind, parse_hunk_header};

    #[test]
    fn status_reads_each_kind_of_change_and_odd_names() {
        let text =
            " M a b.rs\0?? new/ы.txt\0R  to.rs\0from.rs\0 D gone\0A  added\0UU both\0MM twice\0";
        let files = parse_status(text);
        let found = |name: &str| files.iter().find(|f| f.path == Path::new(name)).unwrap();
        assert_eq!(found("a b.rs").change, GitChange::Modified);
        assert_eq!(found("new/ы.txt").change, GitChange::Untracked);
        assert_eq!(found("to.rs").change, GitChange::Renamed);
        assert_eq!(
            found("to.rs").old_path.as_deref(),
            Some(Path::new("from.rs"))
        );
        assert_eq!(found("gone").change, GitChange::Deleted);
        assert_eq!(found("added").change, GitChange::Added);
        assert_eq!(found("both").change, GitChange::Conflicted);
        assert_eq!(found("twice").change, GitChange::Modified);
        assert!(parse_status("").is_empty());
    }

    #[test]
    fn the_log_format_comes_apart_with_pipes_and_merges() {
        let text = "aaa\u{1f}bbb ccc\u{1f}Ann | Lee\u{1f}1700\u{1f}HEAD -> refs/heads/main, tag: refs/tags/v1, refs/remotes/origin/main, refs/heads/feature/x\u{1f}fix: a | b\u{1e}\nbbb\u{1f}\u{1f}Bo\u{1f}1600\u{1f}\u{1f}first\u{1e}\n";
        let log = parse_log(text);
        assert_eq!(log.len(), 2);
        assert_eq!(log[0].parents, ["bbb", "ccc"]);
        assert_eq!(log[0].author, "Ann | Lee");
        assert_eq!(log[0].time, 1700);
        let refs: Vec<_> = log[0]
            .refs
            .iter()
            .map(|r| (r.name.as_str(), r.kind))
            .collect();
        assert_eq!(
            refs,
            [
                ("main", GitRefKind::Head),
                ("v1", GitRefKind::Tag),
                ("origin/main", GitRefKind::Remote),
                ("feature/x", GitRefKind::Local),
            ],
            "a slash does not make a branch remote"
        );
        assert_eq!(parse_refs("HEAD")[0].kind, GitRefKind::Head, "detached");
        assert_eq!(log[0].subject, "fix: a | b");
        assert!(log[1].parents.is_empty() && log[1].refs.is_empty());
    }

    #[test]
    fn name_status_reads_renames_with_both_paths() {
        let files = parse_name_status("M\0a\0R100\0old\0new\0A\0b\0D\0c\0");
        assert_eq!(files.len(), 4);
        assert_eq!(files[1].path, Path::new("new"));
        assert_eq!(files[1].old_path.as_deref(), Some(Path::new("old")));
        assert_eq!(files[3].change, GitChange::Deleted);
    }

    #[test]
    fn a_unified_diff_becomes_numbered_rows_with_gaps() {
        let text = "diff --git a/a b/a\nindex 1..2 100644\n--- a/a\n+++ b/a\n@@ -1,2 +1,2 @@ fn x\n same\n-old\n+new\n@@ -10 +10,2 @@\n ten\n+- dash\n\\ No newline at end of file\n";
        let rows = parse_unified(text);
        let shape: Vec<_> = rows
            .iter()
            .map(|r| (r.kind, r.old_line, r.new_line, r.text.as_str()))
            .collect();
        assert_eq!(
            shape,
            [
                (DiffKind::Same, Some(1), Some(1), "same"),
                (DiffKind::Removed, Some(2), None, "old"),
                (DiffKind::Added, None, Some(2), "new"),
                (DiffKind::Gap, None, None, ""),
                (DiffKind::Same, Some(10), Some(10), "ten"),
                (DiffKind::Added, None, Some(11), "- dash"),
            ]
        );
    }

    #[test]
    fn a_binary_file_says_so() {
        let rows = parse_unified("Binary files a/x and b/x differ\n");
        assert_eq!(rows[0].text, "Binary file");
    }

    #[test]
    fn garbage_from_git_never_panics_the_parsers() {
        assert_eq!(parse_hunk_header("@@ "), None);
        assert_eq!(parse_hunk_header("@@  "), None);
        assert_eq!(parse_hunk_header("@@ é é"), None);
        assert!(parse_status("é\0").is_empty() || !parse_status("é\0").is_empty());
        let files = parse_status("éa path\0");
        assert_eq!(files.len(), 1, "a code no version of git writes");
        assert!(parse_unified("@@ \n@@ -x +y @@\n+z\n").is_empty());
    }

    #[test]
    fn hunk_headers_without_counts_parse() {
        assert_eq!(parse_hunk_header("@@ -3 +7 @@"), Some((3, 7)));
        assert_eq!(parse_hunk_header("@@ -0,0 +1,2 @@"), Some((0, 1)));
        assert_eq!(parse_hunk_header("not"), None);
    }

    pub(crate) struct Repo(tempfile::TempDir);

    impl Repo {
        pub(crate) fn new() -> Self {
            let repo = Self(tempfile::tempdir().unwrap());
            repo.git(&["init", "-q", "-b", "main"]);
            repo.git(&["config", "user.name", "Test"]);
            repo.git(&["config", "user.email", "t@example.com"]);
            repo.git(&["config", "commit.gpgsign", "false"]);
            repo
        }

        pub(crate) fn path(&self) -> PathBuf {
            std::fs::canonicalize(self.0.path()).unwrap()
        }

        pub(crate) fn git(&self, args: &[&str]) -> String {
            checked(&self.path(), args).unwrap()
        }

        pub(crate) fn write(&self, name: &str, text: &str) {
            let file = self.path().join(name);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            std::fs::write(file, text).unwrap();
        }

        pub(crate) fn commit_all(&self, subject: &str) {
            self.git(&["add", "-A"]);
            self.git(&["commit", "-qm", subject]);
        }

        pub(crate) fn status(&self) -> (Option<String>, Vec<GitFile>) {
            let Event::Git(GitEvent::StatusRead { branch, files, .. }) = status_event(&self.path())
            else {
                panic!();
            };
            (branch, files.unwrap())
        }

        fn log(&self) -> Vec<GitCommit> {
            let Event::Git(GitEvent::LogRead { result, .. }) = log_event(&self.path()) else {
                panic!();
            };
            result.unwrap()
        }
    }

    #[test]
    fn outside_a_repository_it_says_so() {
        let dir = tempfile::tempdir().unwrap();
        let Event::Git(GitEvent::StatusRead {
            branch, files, top, ..
        }) = status_event(dir.path())
        else {
            panic!();
        };
        assert_eq!((branch, top), (None, None));
        assert_eq!(files, Err("Not a git repository".to_owned()));
    }

    #[test]
    fn a_new_repository_has_a_branch_and_an_empty_log() {
        let repo = Repo::new();
        repo.write("a.txt", "x\n");
        let (branch, files) = repo.status();
        assert_eq!(branch.as_deref(), Some("main"));
        assert_eq!(files[0].change, GitChange::Untracked);
        assert!(repo.log().is_empty());
        let rows = worktree_diff(&repo.path(), &files[0]).unwrap();
        assert_eq!(rows[0].text, "x", "an untracked file shows as all new");
        assert_eq!(rows[0].kind, DiffKind::Added);
    }

    #[test]
    fn the_first_commit_works_and_takes_only_the_chosen_files() {
        let repo = Repo::new();
        repo.write("a.txt", "a\n");
        repo.write("b.txt", "b\n");
        let chosen = [PathBuf::from("a.txt")];
        let Event::Git(GitEvent::Committed { result }) =
            commit_event(&repo.path(), "first", &chosen, &chosen, false)
        else {
            panic!();
        };
        assert!(result.unwrap().ends_with("first"));
        let (_, files) = repo.status();
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].path, Path::new("b.txt"));
    }

    #[test]
    fn a_commit_of_some_paths_leaves_other_staged_work_staged() {
        let repo = Repo::new();
        repo.write("a.txt", "1\n");
        repo.write("b.txt", "1\n");
        repo.commit_all("base");
        repo.write("a.txt", "2\n");
        repo.write("b.txt", "2\n");
        repo.git(&["add", "b.txt"]);
        commit(
            &repo.path(),
            "only a",
            &[PathBuf::from("a.txt")],
            &[],
            false,
        )
        .unwrap();
        assert_eq!(
            repo.git(&["diff", "--cached", "--name-only"]).trim(),
            "b.txt"
        );
        assert_eq!(
            repo.git(&["show", "--format=", "--name-only", "HEAD"])
                .trim(),
            "a.txt"
        );
    }

    #[test]
    fn deleted_and_renamed_files_can_be_committed() {
        let repo = Repo::new();
        repo.write("gone.txt", "bye\n");
        repo.write("old.txt", "same text long enough to be seen as a rename\n");
        repo.commit_all("base");
        std::fs::remove_file(repo.path().join("gone.txt")).unwrap();
        repo.git(&["mv", "old.txt", "new.txt"]);
        let (_, files) = repo.status();
        let renamed = files
            .iter()
            .find(|f| f.change == GitChange::Renamed)
            .unwrap();
        assert_eq!(renamed.old_path.as_deref(), Some(Path::new("old.txt")));
        let paths: Vec<PathBuf> = ["gone.txt", "new.txt", "old.txt"]
            .iter()
            .map(PathBuf::from)
            .collect();
        commit(&repo.path(), "tidy", &paths, &[], false).unwrap();
        assert!(repo.status().1.is_empty());
    }

    #[test]
    fn a_rejecting_hook_is_respected_and_its_words_are_shown() {
        let repo = Repo::new();
        repo.write("a.txt", "1\n");
        repo.commit_all("base");
        let hook = repo.path().join(".git/hooks/pre-commit");
        std::fs::write(&hook, "#!/bin/sh\necho 'tests are red' >&2\nexit 1\n").unwrap();
        use std::os::unix::fs::PermissionsExt;
        std::fs::set_permissions(&hook, std::fs::Permissions::from_mode(0o755)).unwrap();
        repo.write("a.txt", "2\n");
        let result = commit(&repo.path(), "nope", &[PathBuf::from("a.txt")], &[], false);
        assert!(result.unwrap_err().contains("tests are red"));
        assert_eq!(repo.log().len(), 1);
    }

    #[test]
    fn the_log_shows_branches_merges_and_each_commits_files() {
        let repo = Repo::new();
        repo.write("a.txt", "1\n");
        repo.commit_all("base");
        repo.git(&["checkout", "-qb", "feature"]);
        repo.write("f.txt", "f\n");
        repo.commit_all("on feature");
        repo.git(&["checkout", "-q", "main"]);
        repo.write("a.txt", "2\n");
        repo.commit_all("on main");
        repo.git(&["merge", "-q", "--no-edit", "feature"]);
        let log = repo.log();
        assert_eq!(log.len(), 4);
        assert_eq!(log[0].parents.len(), 2, "the merge is newest");
        assert!(
            log[0]
                .refs
                .iter()
                .any(|r| r.name == "main" && r.kind == GitRefKind::Head)
        );
        assert!(log.iter().any(|c| {
            c.refs
                .iter()
                .any(|r| r.name == "feature" && r.kind == GitRefKind::Local)
        }));

        let top = repo.path();
        let base = log.iter().find(|c| c.subject == "base").unwrap();
        let files = commit_files(&top, &base.hash).unwrap();
        assert_eq!(files[0].path, Path::new("a.txt"), "a root commit has files");
        assert_eq!(files[0].change, GitChange::Added);
        assert_eq!(
            commit_diff(&top, &base.hash, &files[0]).unwrap()[0].text,
            "1"
        );

        let change = log.iter().find(|c| c.subject == "on main").unwrap();
        let files = commit_files(&top, &change.hash).unwrap();
        let rows = commit_diff(&top, &change.hash, &files[0]).unwrap();
        assert_eq!(rows.len(), 2);
        assert_eq!(
            (rows[0].kind, rows[1].kind),
            (DiffKind::Removed, DiffKind::Added)
        );
    }

    #[test]
    fn it_works_from_a_folder_inside_the_repository() {
        let repo = Repo::new();
        repo.write("sub/a.txt", "1\n");
        let sub = repo.path().join("sub");
        let Event::Git(GitEvent::StatusRead { files, .. }) = status_event(&sub) else {
            panic!();
        };
        let files = files.unwrap();
        assert_eq!(files[0].path, Path::new("sub/a.txt"), "relative to the top");
        let chosen = [files[0].path.clone()];
        let Event::Git(GitEvent::Committed { result }) =
            commit_event(&sub, "from sub", &chosen, &chosen, false)
        else {
            panic!();
        };
        assert!(result.is_ok());
    }

    #[test]
    fn a_detached_head_names_its_commit() {
        let repo = Repo::new();
        repo.write("a", "1");
        repo.commit_all("base");
        repo.git(&["checkout", "-q", "--detach"]);
        assert!(repo.status().0.unwrap().starts_with("detached at "));
    }

    #[test]
    fn push_and_pull_reach_a_remote_and_failures_come_back_in_words() {
        let remote_dir = tempfile::tempdir().unwrap();
        checked(remote_dir.path(), ["init", "-q", "--bare", "-b", "main"]).unwrap();
        let repo = Repo::new();
        repo.write("a", "1");
        repo.commit_all("base");
        let top = repo.path();
        assert!(remote(&top, GitRemoteOp::Push).is_err(), "no remote yet");
        repo.git(&[
            "remote",
            "add",
            "origin",
            remote_dir.path().to_str().unwrap(),
        ]);
        repo.git(&["push", "-q", "-u", "origin", "main"]);
        repo.write("a", "2");
        repo.commit_all("next");
        assert!(remote(&top, GitRemoteOp::Push).is_ok());
        assert_eq!(
            checked(remote_dir.path(), ["log", "-1", "--format=%s", "main"])
                .unwrap()
                .trim(),
            "next"
        );
        assert!(remote(&top, GitRemoteOp::Pull).is_ok());
    }

    fn modified(path: &str) -> GitFile {
        GitFile::new(PathBuf::from(path), GitChange::Modified)
    }

    #[test]
    fn amending_replaces_the_commit_and_with_no_files_only_rewrites_the_message() {
        let repo = Repo::new();
        repo.write("a", "1");
        repo.write("b", "1");
        repo.commit_all("base");
        repo.write("a", "2");
        commit(&repo.path(), "typo", &[PathBuf::from("a")], &[], false).unwrap();
        repo.write("b", "2");
        repo.git(&["add", "b"]);
        commit(&repo.path(), "fixed\n\nbody line", &[], &[], true).unwrap();
        assert_eq!(repo.log().len(), 2, "replaced, not added");
        assert_eq!(
            repo.git(&["log", "-1", "--format=%B"]).trim(),
            "fixed\n\nbody line"
        );
        assert_eq!(
            repo.git(&["show", "--format=", "--name-only", "HEAD"])
                .trim(),
            "a"
        );
        assert_eq!(
            repo.git(&["diff", "--cached", "--name-only"]).trim(),
            "b",
            "stage kept"
        );
        commit(&repo.path(), "fixed", &[PathBuf::from("b")], &[], true).unwrap();
        assert_eq!(
            repo.git(&["show", "--format=", "--name-only", "HEAD"])
                .trim(),
            "a\nb"
        );
    }

    #[test]
    fn the_last_commit_says_whether_a_remote_has_it() {
        let remote_dir = tempfile::tempdir().unwrap();
        checked(remote_dir.path(), ["init", "-q", "--bare", "-b", "main"]).unwrap();
        let repo = Repo::new();
        repo.write("a", "1");
        repo.commit_all("hello");
        assert!(!last_commit(&repo.path()).unwrap().1);
        assert!(last_commit(&repo.path()).unwrap().0.starts_with("hello"));
        repo.git(&[
            "remote",
            "add",
            "origin",
            remote_dir.path().to_str().unwrap(),
        ]);
        repo.git(&["push", "-q", "-u", "origin", "main"]);
        assert!(last_commit(&repo.path()).unwrap().1);
    }

    #[test]
    fn rollback_restores_modified_and_deleted_files_and_clears_their_stage() {
        let repo = Repo::new();
        repo.write("m", "old\n");
        repo.write("d", "old\n");
        repo.commit_all("base");
        repo.write("m", "new\n");
        repo.git(&["add", "m"]);
        std::fs::remove_file(repo.path().join("d")).unwrap();
        let top = repo.path();
        rollback(&top, &modified("m")).unwrap();
        let mut deleted = modified("d");
        deleted.change = GitChange::Deleted;
        rollback(&top, &deleted).unwrap();
        assert_eq!(std::fs::read_to_string(top.join("m")).unwrap(), "old\n");
        assert_eq!(std::fs::read_to_string(top.join("d")).unwrap(), "old\n");
        assert!(repo.status().1.is_empty());
    }

    #[test]
    fn rollback_of_a_new_or_moved_file_never_deletes_its_text() {
        let repo = Repo::new();
        repo.write("old", "same text long enough to be seen as a rename\n");
        repo.commit_all("base");
        repo.write("added", "keep me\n");
        repo.git(&["add", "added"]);
        let mut added = modified("added");
        added.change = GitChange::Added;
        let top = repo.path();
        rollback(&top, &added).unwrap();
        assert_eq!(
            std::fs::read_to_string(top.join("added")).unwrap(),
            "keep me\n"
        );

        repo.git(&["mv", "old", "new"]);
        let renamed = repo
            .status()
            .1
            .into_iter()
            .find(|f| f.change == GitChange::Renamed)
            .unwrap();
        rollback(&top, &renamed).unwrap();
        assert!(top.join("old").exists(), "back");
        assert!(top.join("new").exists(), "and nothing lost");
        let left = repo.status().1;
        assert!(
            left.iter().all(|f| f.change == GitChange::Untracked),
            "{left:?}"
        );
    }

    #[test]
    fn branches_list_switch_create_and_refuse_to_overwrite() {
        let remote_dir = tempfile::tempdir().unwrap();
        checked(remote_dir.path(), ["init", "-q", "--bare", "-b", "main"]).unwrap();
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.commit_all("base");
        repo.git(&[
            "remote",
            "add",
            "origin",
            remote_dir.path().to_str().unwrap(),
        ]);
        repo.git(&["push", "-q", "-u", "origin", "main"]);
        repo.git(&["push", "-q", "origin", "main:shared"]);
        repo.git(&["fetch", "-q"]);
        let top = repo.path();
        let new = GitBranch {
            name: "topic".into(),
            remote: false,
        };
        switch(&top, &new, true).unwrap();
        assert_eq!(repo.status().0.as_deref(), Some("topic"));
        let list = branches(&top).unwrap();
        let names: Vec<_> = list.iter().map(|b| (b.name.as_str(), b.remote)).collect();
        assert_eq!(names[0], ("main", false));
        assert_eq!(names[1], ("topic", false));
        assert!(names.contains(&("origin/shared", true)));
        assert!(!names.iter().any(|(n, _)| n.ends_with("HEAD")));

        let remote = list.iter().find(|b| b.name == "origin/shared").unwrap();
        switch(&top, remote, false).unwrap();
        assert_eq!(
            repo.status().0.as_deref(),
            Some("shared"),
            "a local one tracking it"
        );

        repo.write("a", "2\n");
        repo.commit_all("on shared");
        let main = GitBranch {
            name: "main".into(),
            remote: false,
        };
        repo.write("a", "unsaved work\n");
        assert!(switch(&top, &main, false).is_err());
        assert_eq!(
            std::fs::read_to_string(top.join("a")).unwrap(),
            "unsaved work\n"
        );
    }

    #[test]
    fn a_files_base_is_its_text_in_the_last_commit() {
        let repo = Repo::new();
        repo.write("sub/a b.txt", "committed\n");
        repo.commit_all("base");
        repo.write("sub/a b.txt", "changed\n");
        repo.write("new", "x");
        let top = repo.path();
        assert_eq!(
            base_text(&top, &top.join("sub/a b.txt")).as_deref(),
            Some("committed\n")
        );
        assert_eq!(base_text(&top, &top.join("new")), None);
        assert_eq!(base_text(&top, Path::new("/no/such")), None);
    }
}
