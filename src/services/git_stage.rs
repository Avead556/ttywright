use crate::core::git_command::GitEvent;
use std::ffi::OsStr;
use std::io::Write;
use std::path::Path;
use std::process::{Command, Stdio};

use super::git::{
    checked, failure, has_commits, known_top, parse_unified, run, top_level, worktree_diff,
};
use crate::core::{Event, GitChange, GitFile, GitHunk, GitOperation, GitRepoState, HunkSide};

const PATCH_DIFF: [&str; 8] = [
    "diff",
    "--no-color",
    "--no-ext-diff",
    "--no-textconv",
    "--src-prefix=a/",
    "--dst-prefix=b/",
    "-U3",
    "--inter-hunk-context=0",
];

const APPLY: [&str; 7] = [
    "-c",
    "apply.whitespace=nowarn",
    "-c",
    "apply.ignoreWhitespace=no",
    "apply",
    "--cached",
    "--whitespace=nowarn",
];

const CHANGED: &str = "The file changed since this was shown: look again";
const WHOLE: &str = "This moves as a whole: it has no lines to pick";
const NO_ENDING: &str =
    "The file's last line has no line ending, so this needs that line taken too";

struct ChangedLine<'a> {
    ordinal: usize,
    line: &'a [u8],
    no_newline: Option<&'a [u8]>,
}

impl<'a> ChangedLine<'a> {
    fn new(line: &'a [u8], next: &mut usize) -> Self {
        let ordinal = *next;
        *next += 1;
        Self {
            ordinal,
            line,
            no_newline: None,
        }
    }
}

#[derive(Default)]
struct ChangeBlock<'a> {
    removed: Vec<ChangedLine<'a>>,
    added: Vec<ChangedLine<'a>>,
}

impl<'a> ChangeBlock<'a> {
    fn mark_last(&mut self, marker: &'a [u8], body: &mut Vec<u8>) {
        match self.added.last_mut().or(self.removed.last_mut()) {
            Some(line) => line.no_newline = Some(marker),
            None => body.extend_from_slice(marker),
        }
    }
}

struct PickedBody<'a> {
    picked: &'a [usize],
    staged: bool,
    ordinal: usize,
    body: Vec<u8>,
    old: usize,
    new: usize,
    changing: usize,
    old_ended: bool,
    new_ended: bool,
    misplaced: bool,
}

impl<'a> PickedBody<'a> {
    fn new(picked: &'a [usize], staged: bool) -> Self {
        Self {
            picked,
            staged,
            ordinal: 0,
            body: Vec::new(),
            old: 0,
            new: 0,
            changing: 0,
            old_ended: false,
            new_ended: false,
            misplaced: false,
        }
    }

    fn push(&mut self, sign: u8, line: &[u8], marker: Option<&[u8]>) {
        let (in_old, in_new) = (sign != b'+', sign != b'-');
        self.misplaced |= (in_old && self.old_ended) || (in_new && self.new_ended);
        self.old += usize::from(in_old);
        self.new += usize::from(in_new);
        self.old_ended |= in_old && marker.is_some();
        self.new_ended |= in_new && marker.is_some();
        self.body.push(sign);
        self.body
            .extend_from_slice(line.get(1..).unwrap_or_default());
        if let Some(marker) = marker {
            self.body.extend_from_slice(marker);
        }
    }

    fn take(&mut self, line: &ChangedLine<'_>, sign: u8, stays: bool) {
        if self.picked.contains(&line.ordinal) {
            self.changing += 1;
            self.push(sign, line.line, line.no_newline);
        } else if stays {
            self.push(b' ', line.line, line.no_newline);
        }
    }

    fn flush(&mut self, block: &mut ChangeBlock<'_>) {
        let (removed, added) = (
            std::mem::take(&mut block.removed),
            std::mem::take(&mut block.added),
        );
        for n in 0..removed.len().max(added.len()) {
            if let Some(line) = removed.get(n) {
                self.take(line, b'-', !self.staged);
            }
            if let Some(line) = added.get(n) {
                self.take(line, b'+', self.staged);
            }
        }
    }
}

type HunkSpan = (usize, usize);

fn hunk_numbers(line: &[u8]) -> Option<(HunkSpan, HunkSpan, Vec<u8>)> {
    let rest = line.strip_prefix(b"@@ ")?;
    let close = rest.windows(3).position(|w| w == b" @@")?;
    let text = std::str::from_utf8(&rest[..close]).ok()?;
    let (old, new) = text.split_once(' ')?;
    let side = |part: &str, sign: char| -> Option<(usize, usize)> {
        let part = part.strip_prefix(sign)?;
        match part.split_once(',') {
            Some((start, count)) => Some((start.parse().ok()?, count.parse().ok()?)),
            None => Some((part.parse().ok()?, 1)),
        }
    };
    Some((side(old, '-')?, side(new, '+')?, rest[close + 3..].to_vec()))
}

pub(super) fn pick_lines(patch: &[u8], picked: &[usize], staged: bool) -> Result<Vec<u8>, String> {
    let lines: Vec<&[u8]> = patch.split_inclusive(|b| *b == b'\n').collect();
    let at = lines
        .iter()
        .position(|l| l.starts_with(b"@@ "))
        .ok_or("This has no lines to pick")?;
    let moves_whole =
        |l: &&[u8]| l.starts_with(b"new file mode") || l.starts_with(b"deleted file mode");
    if lines[..at].iter().any(moves_whole) {
        return Err(WHOLE.to_owned());
    }
    let ((old_start, old_count), (new_start, new_count), tail) =
        hunk_numbers(lines[at]).ok_or("Git wrote a hunk this cannot read")?;
    let mut out = PickedBody::new(picked, staged);
    let mut block = ChangeBlock::default();
    for line in &lines[at + 1..] {
        match line.first() {
            Some(b'-') => block.removed.push(ChangedLine::new(line, &mut out.ordinal)),
            Some(b'+') => block.added.push(ChangedLine::new(line, &mut out.ordinal)),
            Some(b'\\') => block.mark_last(line, &mut out.body),
            _ => {
                out.flush(&mut block);
                out.push(b' ', line, None);
            }
        }
    }
    out.flush(&mut block);
    if out.misplaced {
        return Err(NO_ENDING.to_owned());
    }
    let PickedBody {
        body,
        old,
        new,
        changing,
        ..
    } = out;
    if changing == 0 {
        return Err("Pick at least one changed line".to_owned());
    }
    let start = |start: usize, was: usize, now: usize| match (was, now) {
        (0, n) if n > 0 => start + 1,
        (w, 0) if w > 0 => start.saturating_sub(1),
        _ => start,
    };
    let mut out: Vec<u8> = lines[..at].concat();
    out.extend_from_slice(
        format!(
            "@@ -{},{old} +{},{new} @@",
            start(old_start, old_count, old),
            start(new_start, new_count, new),
        )
        .as_bytes(),
    );
    out.extend_from_slice(&tail);
    out.extend_from_slice(&body);
    Ok(out)
}

fn title_of(header: &str) -> String {
    let has = |what: &str| header.lines().any(|l| l.starts_with(what));
    if has("Binary files ") || has("GIT binary patch") {
        "Binary file: moves as a whole".to_owned()
    } else if has("rename from") {
        "Renamed, text unchanged".to_owned()
    } else if has("new file mode") {
        "New empty file".to_owned()
    } else if has("deleted file mode") {
        "Deleted empty file".to_owned()
    } else if has("old mode") {
        "File mode changed".to_owned()
    } else {
        "Whole file".to_owned()
    }
}

fn plain_header(header: &[u8]) -> Option<Vec<u8>> {
    let plus = header
        .split_inclusive(|b| *b == b'\n')
        .find(|l| l.starts_with(b"+++ "))?;
    let name = &plus[4..];
    let minus_name: Vec<u8> = if name.starts_with(b"\"b/") {
        [&b"\"a/"[..], &name[3..]].concat()
    } else if name.starts_with(b"b/") {
        [&b"a/"[..], &name[2..]].concat()
    } else {
        return None;
    };
    Some([&b"--- "[..], &minus_name, plus].concat())
}

fn starts_section(line: &[u8]) -> bool {
    line.starts_with(b"diff --git ") || line.starts_with(b"diff --cc ")
}

pub(super) fn split_patch(patch: &[u8], side: HunkSide) -> Vec<GitHunk> {
    let mut sections: Vec<Vec<&[u8]>> = Vec::new();
    for line in patch.split_inclusive(|b| *b == b'\n') {
        if starts_section(line) || sections.is_empty() {
            sections.push(Vec::new());
        }
        if let Some(section) = sections.last_mut() {
            section.push(line);
        }
    }
    let mut hunks = Vec::new();
    for lines in sections {
        let first_hunk = lines.iter().position(|l| l.starts_with(b"@@ "));
        let header: Vec<u8> = lines[..first_hunk.unwrap_or(lines.len())].concat();
        let Some(first_hunk) = first_hunk else {
            if !header.is_empty() {
                let text = String::from_utf8_lossy(&header);
                hunks.push(GitHunk {
                    side,
                    whole: true,
                    title: title_of(&text),
                    rows: Vec::new(),
                    patch: header,
                });
            }
            continue;
        };
        let text = String::from_utf8_lossy(&header).into_owned();
        let keeps_header = text
            .lines()
            .any(|l| l.starts_with("new file mode") || l.starts_with("deleted file mode"));
        let start: Option<Vec<u8>> = if keeps_header {
            Some(header)
        } else {
            plain_header(&header)
        };
        let Some(start) = start else { continue };
        let mut at = first_hunk;
        while at < lines.len() {
            let end = lines[at + 1..]
                .iter()
                .position(|l| l.starts_with(b"@@ "))
                .map_or(lines.len(), |n| at + 1 + n);
            let body = lines[at..end].concat();
            let shown = String::from_utf8_lossy(&body);
            hunks.push(GitHunk {
                side,
                whole: false,
                title: shown.lines().next().unwrap_or_default().to_owned(),
                rows: parse_unified(&shown),
                patch: [start.as_slice(), &body].concat(),
            });
            at = end;
        }
    }
    hunks
}

fn file_paths<'a>(file: &'a GitFile, args: &[&'static str]) -> Vec<&'a OsStr> {
    let mut all: Vec<&OsStr> = args.iter().map(|a| OsStr::new(*a)).collect();
    all.push(OsStr::new("--"));
    all.push(file.path.as_os_str());
    if let Some(old) = &file.old_path {
        all.push(old.as_os_str());
    }
    all
}

fn diff_bytes(top: &Path, file: &GitFile, extra: &[&'static str]) -> Result<Vec<u8>, String> {
    let start: Vec<&str> = PATCH_DIFF.iter().chain(extra).copied().collect();
    let output = run(top, file_paths(file, &start))?;
    if output.status.success() {
        Ok(output.stdout)
    } else {
        Err(failure(&output))
    }
}

pub(super) fn read_hunks(top: &Path, file: &GitFile) -> Result<Vec<GitHunk>, String> {
    match file.change {
        GitChange::Conflicted => return Ok(Vec::new()),
        GitChange::Untracked => {
            let rows = worktree_diff(top, file)?;
            let binary = rows.first().is_some_and(|r| r.text == "Binary file");
            return Ok(vec![GitHunk {
                side: HunkSide::Unstaged,
                whole: true,
                title: if binary {
                    "New binary file: moves as a whole"
                } else {
                    "New file: moves as a whole"
                }
                .to_owned(),
                patch: Vec::new(),
                rows,
            }]);
        }
        _ => {}
    }
    let mut hunks = split_patch(
        &diff_bytes(top, file, &["--no-renames"])?,
        HunkSide::Unstaged,
    );
    hunks.extend(split_patch(
        &diff_bytes(top, file, &["--cached", "-M"])?,
        HunkSide::Staged,
    ));
    Ok(hunks)
}

pub fn hunks_event(root: &Path, id: u64, file: &GitFile) -> Event {
    Event::Git(GitEvent::HunksRead {
        id,
        result: known_top(root).and_then(|top| read_hunks(&top, file)),
    })
}

fn apply_patch(top: &Path, patch: &[u8], reverse: bool) -> Result<(), String> {
    let mut args = APPLY.to_vec();
    if reverse {
        args.push("--reverse");
    }
    let mut child = Command::new("git")
        .current_dir(top)
        .env("GIT_TERMINAL_PROMPT", "0")
        .env("LC_ALL", "C")
        .args(args)
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|e| e.to_string())?;
    if let Some(mut stdin) = child.stdin.take() {
        let _ = stdin.write_all(patch);
    }
    let output = child.wait_with_output().map_err(|e| e.to_string())?;
    if output.status.success() {
        Ok(())
    } else {
        Err(failure(&output))
    }
}

fn add_files(top: &Path, files: &[GitFile]) -> Result<(), String> {
    let mut args: Vec<&OsStr> = ["add", "-A", "--"].map(OsStr::new).to_vec();
    args.extend(files.iter().map(|f| f.path.as_os_str()));
    checked(top, args).map(drop)
}

fn remove_from_index(top: &Path, files: &[GitFile]) -> Result<(), String> {
    let start: &[&str] = if has_commits(top) {
        &["reset", "-q", "--"]
    } else {
        &["rm", "--cached", "-q", "-r", "--"]
    };
    let mut args: Vec<&OsStr> = start.iter().map(OsStr::new).collect();
    for file in files {
        args.push(file.path.as_os_str());
        if let Some(old) = &file.old_path {
            args.push(old.as_os_str());
        }
    }
    checked(top, args).map(drop)
}

pub(super) fn apply_hunk(
    top: &Path,
    file: &GitFile,
    hunk: &GitHunk,
    lines: Option<&[usize]>,
) -> Result<(), String> {
    if !read_hunks(top, file)?.contains(hunk) {
        return Err(CHANGED.to_owned());
    }
    let staged = hunk.side == HunkSide::Staged;
    if let Some(lines) = lines {
        if hunk.whole {
            return Err(WHOLE.to_owned());
        }
        return apply_patch(top, &pick_lines(&hunk.patch, lines, staged)?, staged);
    }
    if hunk.whole {
        let one = std::slice::from_ref(file);
        return if staged {
            remove_from_index(top, one)
        } else {
            add_files(top, one)
        };
    }
    apply_patch(top, &hunk.patch, staged)
}

pub fn apply_event(root: &Path, file: &GitFile, hunk: &GitHunk, lines: Option<&[usize]>) -> Event {
    Event::Git(GitEvent::StageDone {
        result: top_level(root).and_then(|top| apply_hunk(&top, file, hunk, lines)),
    })
}

pub fn stage_files_event(root: &Path, files: &[GitFile], stage: bool) -> Event {
    Event::Git(GitEvent::StageDone {
        result: top_level(root).and_then(|top| {
            if stage {
                add_files(&top, files)
            } else {
                remove_from_index(&top, files)
            }
        }),
    })
}

fn staged_id(top: &Path) -> Result<String, String> {
    checked(
        top,
        [
            "diff",
            "--cached",
            "--raw",
            "-z",
            "--no-renames",
            "--no-ext-diff",
            "--abbrev=40",
        ],
    )
}

fn operation(top: &Path) -> Option<GitOperation> {
    let dir = checked(top, ["rev-parse", "--absolute-git-dir"]).ok()?;
    let dir = Path::new(dir.trim_end());
    let has = |name: &str| dir.join(name).exists();
    if has("rebase-merge") || has("rebase-apply") {
        Some(GitOperation::Rebasing)
    } else if has("MERGE_HEAD") {
        Some(GitOperation::Merging)
    } else if has("CHERRY_PICK_HEAD") {
        Some(GitOperation::CherryPicking)
    } else if has("REVERT_HEAD") {
        Some(GitOperation::Reverting)
    } else {
        None
    }
}

pub(super) fn repo_state(top: &Path) -> GitRepoState {
    GitRepoState {
        operation: operation(top),
        staged_id: staged_id(top).unwrap_or_default(),
    }
}

pub(super) fn commit_staged(
    top: &Path,
    message: &str,
    amend: bool,
    expected: &str,
) -> Result<String, String> {
    if staged_id(top)? != expected {
        return Err("What is staged changed since it was shown: look again".to_owned());
    }
    let mut args = vec!["commit", "-m", message];
    if amend {
        args.push("--amend");
    }
    checked(top, args)?;
    checked(top, ["log", "-1", "--format=%h %s"]).map(|s| s.trim().to_owned())
}

pub fn commit_staged_event(root: &Path, message: &str, amend: bool, expected: &str) -> Event {
    Event::Git(GitEvent::Committed {
        result: top_level(root).and_then(|top| commit_staged(&top, message, amend, expected)),
    })
}

#[cfg(test)]
mod tests {
    use super::super::git::tests::Repo;
    use super::*;

    const TWELVE: &str = "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\n11\n12\n13\n14\n15\n16\n17\n18\n19\n20\n";

    fn only(repo: &Repo, name: &str) -> GitFile {
        repo.status()
            .1
            .into_iter()
            .find(|f| f.path == Path::new(name))
            .unwrap_or_else(|| panic!("{name} is not changed"))
    }

    fn hunks(repo: &Repo, name: &str) -> Vec<GitHunk> {
        read_hunks(&repo.path(), &only(repo, name)).unwrap()
    }

    fn index_text(repo: &Repo, name: &str) -> Vec<u8> {
        let spec = format!(":{name}");
        run(&repo.path(), ["show", spec.as_str()]).unwrap().stdout
    }

    fn file_bytes(repo: &Repo, name: &str) -> Vec<u8> {
        std::fs::read(repo.path().join(name)).unwrap()
    }

    fn side(hunks: &[GitHunk], side: HunkSide) -> Vec<&GitHunk> {
        hunks.iter().filter(|h| h.side == side).collect()
    }

    fn two_hunks(repo: &Repo, name: &str, base: &[u8], changed: &[u8]) {
        std::fs::write(repo.path().join(name), base).unwrap();
        repo.commit_all("base");
        std::fs::write(repo.path().join(name), changed).unwrap();
    }

    #[test]
    fn one_hunk_stages_and_unstages_leaving_the_other_alone() {
        let repo = Repo::new();
        two_hunks(
            &repo,
            "f",
            TWELVE.as_bytes(),
            TWELVE
                .replace("\n2\n", "\ntwo\n")
                .replace("19\n", "nineteen\n")
                .as_bytes(),
        );
        let list = hunks(&repo, "f");
        assert_eq!(list.len(), 2, "{list:?}");
        assert!(
            list.iter()
                .all(|h| h.side == HunkSide::Unstaged && !h.whole)
        );
        assert!(
            list[0].title.starts_with("@@ -1,5 +1,5 @@"),
            "{}",
            list[0].title
        );
        let top = repo.path();
        let file = only(&repo, "f");
        apply_hunk(&top, &file, &list[1], None).unwrap();
        let staged = String::from_utf8(index_text(&repo, "f")).unwrap();
        assert!(
            staged.contains("nineteen") && !staged.contains("two"),
            "{staged}"
        );
        assert!(String::from_utf8_lossy(&file_bytes(&repo, "f")).contains("two"));

        let file = only(&repo, "f");
        assert!(file.code.staged() && file.code.unstaged());
        let list = hunks(&repo, "f");
        assert_eq!(side(&list, HunkSide::Unstaged).len(), 1);
        let on_index = side(&list, HunkSide::Staged)[0].clone();
        apply_hunk(&top, &file, &on_index, None).unwrap();
        assert_eq!(index_text(&repo, "f"), TWELVE.as_bytes(), "back as it was");
        assert!(String::from_utf8_lossy(&file_bytes(&repo, "f")).contains("nineteen"));
    }

    #[test]
    fn a_hunk_moved_over_an_earlier_unstaged_one_still_lands_on_its_own_lines() {
        let repo = Repo::new();
        let changed = TWELVE
            .replace("\n2\n", "\ntwo\nand\nmore\n")
            .replace("19\n", "nineteen\n");
        two_hunks(&repo, "f", TWELVE.as_bytes(), changed.as_bytes());
        let list = hunks(&repo, "f");
        apply_hunk(&repo.path(), &only(&repo, "f"), &list[1], None).unwrap();
        let list = hunks(&repo, "f");
        apply_hunk(
            &repo.path(),
            &only(&repo, "f"),
            side(&list, HunkSide::Unstaged)[0],
            None,
        )
        .unwrap();
        assert_eq!(index_text(&repo, "f"), changed.as_bytes());
        assert!(side(&hunks(&repo, "f"), HunkSide::Unstaged).is_empty());
    }

    #[test]
    fn crlf_lines_reach_the_index_with_their_carriage_returns() {
        let repo = Repo::new();
        repo.git(&["config", "core.autocrlf", "false"]);
        let base = TWELVE.replace('\n', "\r\n");
        let changed = base
            .replace("\n2\r\n", "\ntwo\r\n")
            .replace("19\r\n", "nineteen\r\n");
        two_hunks(&repo, "f", base.as_bytes(), changed.as_bytes());
        let list = hunks(&repo, "f");
        assert!(
            list[0].rows.iter().all(|r| !r.text.ends_with('\r')),
            "shown clean"
        );
        apply_hunk(&repo.path(), &only(&repo, "f"), &list[0], None).unwrap();
        let expected = base.replace("\n2\r\n", "\ntwo\r\n");
        assert_eq!(index_text(&repo, "f"), expected.as_bytes());
        let list = hunks(&repo, "f");
        apply_hunk(
            &repo.path(),
            &only(&repo, "f"),
            side(&list, HunkSide::Staged)[0],
            None,
        )
        .unwrap();
        assert_eq!(index_text(&repo, "f"), base.as_bytes());
    }

    #[test]
    fn with_autocrlf_the_index_keeps_its_own_line_endings() {
        let repo = Repo::new();
        repo.git(&["config", "core.autocrlf", "true"]);
        std::fs::write(repo.path().join("f"), TWELVE.replace('\n', "\r\n")).unwrap();
        repo.commit_all("base");
        let changed = TWELVE
            .replace("\n2\n", "\ntwo\n")
            .replace("19\n", "nineteen\n");
        std::fs::write(repo.path().join("f"), changed.replace('\n', "\r\n")).unwrap();
        let list = hunks(&repo, "f");
        assert_eq!(list.len(), 2);
        apply_hunk(&repo.path(), &only(&repo, "f"), &list[1], None).unwrap();
        let staged = index_text(&repo, "f");
        assert!(
            !staged.contains(&b'\r'),
            "{}",
            String::from_utf8_lossy(&staged)
        );
        assert!(String::from_utf8_lossy(&staged).contains("nineteen"));
    }

    #[test]
    fn a_last_line_without_a_newline_stages_in_both_directions() {
        let repo = Repo::new();
        two_hunks(
            &repo,
            "f",
            b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\nend",
            b"one\n2\n3\n4\n5\n6\n7\n8\n9\n10\nEND",
        );
        let list = hunks(&repo, "f");
        assert_eq!(list.len(), 2, "{list:?}");
        apply_hunk(&repo.path(), &only(&repo, "f"), &list[1], None).unwrap();
        assert_eq!(
            index_text(&repo, "f"),
            b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\nEND"
        );
        let list = hunks(&repo, "f");
        apply_hunk(
            &repo.path(),
            &only(&repo, "f"),
            side(&list, HunkSide::Staged)[0],
            None,
        )
        .unwrap();
        assert_eq!(
            index_text(&repo, "f"),
            b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\nend"
        );

        std::fs::write(
            repo.path().join("f"),
            "1\n2\n3\n4\n5\n6\n7\n8\n9\n10\nend\n",
        )
        .unwrap();
        let list = hunks(&repo, "f");
        apply_hunk(&repo.path(), &only(&repo, "f"), &list[0], None).unwrap();
        assert_eq!(
            index_text(&repo, "f"),
            b"1\n2\n3\n4\n5\n6\n7\n8\n9\n10\nend\n"
        );
    }

    #[test]
    fn an_untracked_file_moves_whole_and_a_new_file_unstages_keeping_its_text() {
        let repo = Repo::new();
        repo.write("base", "x\n");
        repo.commit_all("base");
        repo.write("new файл.txt", "a\nb\n");
        let list = hunks(&repo, "new файл.txt");
        assert_eq!(list.len(), 1);
        assert!(list[0].whole);
        assert_eq!(list[0].rows[0].text, "a");
        apply_hunk(&repo.path(), &only(&repo, "new файл.txt"), &list[0], None).unwrap();
        assert!(only(&repo, "new файл.txt").code.staged());

        let list = hunks(&repo, "new файл.txt");
        assert_eq!(list.len(), 1);
        assert!(!list[0].whole, "{list:?}");
        apply_hunk(&repo.path(), &only(&repo, "new файл.txt"), &list[0], None).unwrap();
        assert_eq!(only(&repo, "new файл.txt").change, GitChange::Untracked);
        assert_eq!(file_bytes(&repo, "new файл.txt"), b"a\nb\n");
    }

    #[test]
    fn empty_and_binary_files_and_deletions_move_too() {
        let repo = Repo::new();
        repo.write("gone", "bye\n");
        repo.commit_all("base");
        repo.write("empty", "");
        std::fs::write(repo.path().join("bin"), [0u8, 1, 2, 0, 200]).unwrap();
        std::fs::remove_file(repo.path().join("gone")).unwrap();
        let top = repo.path();
        for name in ["empty", "bin", "gone"] {
            let list = hunks(&repo, name);
            assert_eq!(list.len(), 1, "{name}: {list:?}");
            apply_hunk(&top, &only(&repo, name), &list[0], None).unwrap();
            assert!(only(&repo, name).code.staged(), "{name}");
            let list = hunks(&repo, name);
            assert_eq!(list.len(), 1, "{name}: {list:?}");
            assert_eq!(list[0].side, HunkSide::Staged);
            apply_hunk(&top, &only(&repo, name), &list[0], None).unwrap();
            assert!(!only(&repo, name).code.staged(), "{name}");
        }
        assert!(!top.join("gone").exists(), "still deleted in the folder");
        assert_eq!(file_bytes(&repo, "bin"), [0u8, 1, 2, 0, 200]);
    }

    #[test]
    fn a_rename_keeps_moving_while_its_edits_stage_separately() {
        let repo = Repo::new();
        repo.write("old name", TWELVE);
        repo.commit_all("base");
        repo.git(&["mv", "old name", "новое имя"]);
        repo.write("новое имя", &TWELVE.replace("\n2\n", "\ntwo\n"));
        repo.git(&["add", "новое имя"]);
        repo.write(
            "новое имя",
            &TWELVE
                .replace("\n2\n", "\ntwo\n")
                .replace("19\n", "nineteen\n"),
        );
        let file = only(&repo, "новое имя");
        assert_eq!(file.change, GitChange::Renamed);
        let list = hunks(&repo, "новое имя");
        assert_eq!(side(&list, HunkSide::Staged).len(), 1, "{list:?}");
        assert_eq!(side(&list, HunkSide::Unstaged).len(), 1);

        apply_hunk(&repo.path(), &file, side(&list, HunkSide::Staged)[0], None).unwrap();
        let file = only(&repo, "новое имя");
        assert_eq!(file.change, GitChange::Renamed, "still a rename");
        assert_eq!(index_text(&repo, "новое имя"), TWELVE.as_bytes());

        apply_hunk(
            &repo.path(),
            &file,
            side(&hunks(&repo, "новое имя"), HunkSide::Unstaged)[1],
            None,
        )
        .unwrap();
        let staged = String::from_utf8(index_text(&repo, "новое имя")).unwrap();
        assert!(staged.contains("nineteen") && !staged.contains("two"));

        let file = only(&repo, "новое имя");
        let list = hunks(&repo, "новое имя");
        apply_hunk(&repo.path(), &file, side(&list, HunkSide::Staged)[0], None).unwrap();
        let file = only(&repo, "новое имя");
        let list = hunks(&repo, "новое имя");
        let bare = side(&list, HunkSide::Staged)[0];
        assert!(bare.whole, "{list:?}");
        assert_eq!(bare.title, "Renamed, text unchanged");
        apply_hunk(&repo.path(), &file, bare, None).unwrap();
        assert_eq!(only(&repo, "новое имя").change, GitChange::Untracked);
        assert_eq!(only(&repo, "old name").change, GitChange::Deleted);
    }

    #[test]
    fn non_ascii_names_with_spaces_and_text_in_another_encoding_stage() {
        let repo = Repo::new();
        let base: Vec<u8> = TWELVE.bytes().chain(*b"caf\xe9\n").collect();
        let changed: Vec<u8> = TWELVE
            .replace("\n2\n", "\ntwo\n")
            .bytes()
            .chain(*b"caf\xe9 au lait\n")
            .collect();
        two_hunks(&repo, "ф а й л.txt", &base, &changed);
        let list = hunks(&repo, "ф а й л.txt");
        assert_eq!(list.len(), 2);
        apply_hunk(&repo.path(), &only(&repo, "ф а й л.txt"), &list[1], None).unwrap();
        let staged = index_text(&repo, "ф а й л.txt");
        assert!(
            staged.ends_with(b"caf\xe9 au lait\n"),
            "bytes, not their display"
        );
    }

    #[test]
    fn hostile_settings_do_not_change_what_is_staged() {
        let repo = Repo::new();
        for (key, value) in [
            ("diff.noprefix", "true"),
            ("diff.mnemonicPrefix", "true"),
            ("apply.whitespace", "fix"),
            ("apply.ignoreWhitespace", "change"),
            ("diff.context", "0"),
            ("diff.interHunkLines", "20"),
            ("color.ui", "always"),
        ] {
            repo.git(&["config", key, value]);
        }
        let changed = TWELVE
            .replace("\n2\n", "\ntwo  \n")
            .replace("19\n", "nineteen  \n");
        two_hunks(&repo, "f", TWELVE.as_bytes(), changed.as_bytes());
        let list = hunks(&repo, "f");
        assert_eq!(list.len(), 2, "{list:?}");
        apply_hunk(&repo.path(), &only(&repo, "f"), &list[0], None).unwrap();
        assert_eq!(
            index_text(&repo, "f"),
            TWELVE.replace("\n2\n", "\ntwo  \n").as_bytes()
        );
    }

    #[test]
    fn a_textconv_driver_does_not_leak_into_the_patch() {
        let repo = Repo::new();
        repo.git(&["config", "diff.upper.textconv", "tr a-z A-Z <"]);
        repo.write(".gitattributes", "*.txt diff=upper\n");
        repo.write("a.txt", "lower\n");
        repo.commit_all("base");
        repo.write("a.txt", "changed\n");
        let list = hunks(&repo, "a.txt");
        apply_hunk(&repo.path(), &only(&repo, "a.txt"), &list[0], None).unwrap();
        assert_eq!(index_text(&repo, "a.txt"), b"changed\n");
    }

    #[test]
    fn a_hunk_that_changed_since_it_was_shown_is_refused_and_nothing_moves() {
        let repo = Repo::new();
        two_hunks(
            &repo,
            "f",
            TWELVE.as_bytes(),
            TWELVE.replace("\n2\n", "\ntwo\n").as_bytes(),
        );
        let list = hunks(&repo, "f");
        repo.write("f", &TWELVE.replace("2\n", "TWO\n"));
        let err = apply_hunk(&repo.path(), &only(&repo, "f"), &list[0], None).unwrap_err();
        assert_eq!(err, CHANGED);
        assert_eq!(index_text(&repo, "f"), TWELVE.as_bytes());
    }

    #[test]
    fn a_patch_git_refuses_reports_its_words_and_changes_nothing() {
        let repo = Repo::new();
        repo.write("f", TWELVE);
        repo.commit_all("base");
        let bad = b"--- a/f\n+++ b/f\n@@ -1,2 +1,2 @@\n not there\n-at all\n+nope\n";
        let err = apply_patch(&repo.path(), bad, false).unwrap_err();
        assert!(err.contains("patch does not apply"), "{err}");
        assert!(repo.status().1.is_empty(), "the index is as it was");
    }

    #[test]
    fn staging_before_the_first_commit_works_both_ways() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        let file = only(&repo, "a");
        let Event::Git(GitEvent::StageDone { result }) =
            stage_files_event(&repo.path(), std::slice::from_ref(&file), true)
        else {
            panic!();
        };
        result.unwrap();
        assert!(only(&repo, "a").code.staged());
        let Event::Git(GitEvent::StageDone { result }) =
            stage_files_event(&repo.path(), std::slice::from_ref(&file), false)
        else {
            panic!();
        };
        result.unwrap();
        assert_eq!(only(&repo, "a").change, GitChange::Untracked);
    }

    #[test]
    fn a_commit_of_the_index_takes_only_what_is_staged_and_shown() {
        let repo = Repo::new();
        let changed = TWELVE
            .replace("\n2\n", "\ntwo\n")
            .replace("19\n", "nineteen\n");
        two_hunks(&repo, "f", TWELVE.as_bytes(), changed.as_bytes());
        let list = hunks(&repo, "f");
        let top = repo.path();
        apply_hunk(&top, &only(&repo, "f"), &list[0], None).unwrap();
        let shown = repo_state(&top).staged_id;
        assert!(!shown.is_empty());

        repo.write("other", "x\n");
        repo.git(&["add", "other"]);
        let err = commit_staged(&top, "msg", false, &shown).unwrap_err();
        assert!(err.contains("changed since"), "{err}");
        assert_eq!(
            repo.git(&["log", "--format=%s"]).trim(),
            "base",
            "no commit"
        );
        repo.git(&["reset", "-q", "--", "other"]);

        commit_staged(&top, "part", false, &shown).unwrap();
        let committed = repo.git(&["show", "HEAD:f"]);
        assert!(
            committed.contains("two") && !committed.contains("nineteen"),
            "{committed}"
        );
        let left = only(&repo, "f");
        assert!(
            !left.code.staged() && left.code.unstaged(),
            "the rest waits"
        );
        assert!(String::from_utf8_lossy(&file_bytes(&repo, "f")).contains("nineteen"));
    }

    #[test]
    fn the_first_commit_can_come_from_the_index_and_an_amend_takes_it_too() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.write("b", "1\n");
        repo.git(&["add", "a"]);
        let top = repo.path();
        commit_staged(&top, "first", false, &repo_state(&top).staged_id).unwrap();
        assert_eq!(
            repo.git(&["show", "--format=", "--name-only", "HEAD"])
                .trim(),
            "a"
        );
        repo.git(&["add", "b"]);
        commit_staged(&top, "first, better", true, &repo_state(&top).staged_id).unwrap();
        assert_eq!(repo.git(&["log", "--format=%s"]).trim(), "first, better");
        assert_eq!(
            repo.git(&["show", "--format=", "--name-only", "HEAD"])
                .trim(),
            "a\nb"
        );
    }

    #[test]
    fn git_says_so_when_there_is_nothing_to_commit() {
        let repo = Repo::new();
        repo.write("a", "1\n");
        repo.commit_all("base");
        let top = repo.path();
        let err = commit_staged(&top, "m", false, &repo_state(&top).staged_id).unwrap_err();
        assert!(
            err.contains("nothing to commit") || err.contains("no changes"),
            "{err}"
        );
    }

    #[test]
    fn the_status_letters_say_what_is_staged_and_what_is_not() {
        let repo = Repo::new();
        repo.write("s", "1\n");
        repo.write("m", "1\n");
        repo.write("u", "1\n");
        repo.commit_all("base");
        repo.write("s", "2\n");
        repo.git(&["add", "s"]);
        repo.write("m", "2\n");
        repo.git(&["add", "m"]);
        repo.write("m", "3\n");
        repo.write("u", "2\n");
        repo.write("n", "new\n");
        for (name, staged, unstaged) in [
            ("s", true, false),
            ("m", true, true),
            ("u", false, true),
            ("n", false, true),
        ] {
            let code = only(&repo, name).code;
            assert_eq!(
                (code.staged(), code.unstaged()),
                (staged, unstaged),
                "{name}"
            );
        }
    }

    fn index_of(repo: &Repo, name: &str) -> String {
        String::from_utf8_lossy(&index_text(repo, name)).into_owned()
    }

    fn five_changes(repo: &Repo) {
        repo.write("f", "a\nb\nc\n");
        repo.commit_all("base");
        repo.write("f", "A\nB\nnew\nc\n");
    }

    #[test]
    fn a_single_added_line_stages_alone() {
        let repo = Repo::new();
        five_changes(&repo);
        let list = hunks(&repo, "f");
        apply_hunk(&repo.path(), &only(&repo, "f"), &list[0], Some(&[4])).unwrap();
        assert_eq!(index_of(&repo, "f"), "a\nb\nnew\nc\n");
        assert_eq!(
            std::fs::read_to_string(repo.path().join("f")).unwrap(),
            "A\nB\nnew\nc\n"
        );
    }

    #[test]
    fn a_removal_with_its_replacement_stages_as_a_change_of_that_line() {
        let repo = Repo::new();
        five_changes(&repo);
        let list = hunks(&repo, "f");
        apply_hunk(&repo.path(), &only(&repo, "f"), &list[0], Some(&[0, 2])).unwrap();
        assert_eq!(index_of(&repo, "f"), "A\nb\nc\n");
    }

    #[test]
    fn staged_lines_unstage_one_at_a_time_leaving_the_rest_staged() {
        let repo = Repo::new();
        five_changes(&repo);
        repo.git(&["add", "f"]);
        let list = hunks(&repo, "f");
        let on_index = side(&list, HunkSide::Staged)[0];
        apply_hunk(&repo.path(), &only(&repo, "f"), on_index, Some(&[1, 3])).unwrap();
        assert_eq!(index_of(&repo, "f"), "A\nb\nnew\nc\n");
        assert_eq!(
            std::fs::read_to_string(repo.path().join("f")).unwrap(),
            "A\nB\nnew\nc\n"
        );
    }

    #[test]
    fn crlf_and_non_utf8_lines_keep_their_bytes() {
        let repo = Repo::new();
        std::fs::write(repo.path().join("f"), b"a\r\nb\r\n").unwrap();
        repo.commit_all("base");
        std::fs::write(repo.path().join("f"), b"a\r\nb\r\n\xFFnew\r\nnext\r\n").unwrap();
        let list = hunks(&repo, "f");
        apply_hunk(&repo.path(), &only(&repo, "f"), &list[0], Some(&[0])).unwrap();
        assert_eq!(index_text(&repo, "f"), b"a\r\nb\r\n\xFFnew\r\n");
    }

    #[test]
    fn a_file_without_a_final_newline_can_be_picked_from() {
        let repo = Repo::new();
        repo.write("f", "a\nb");
        repo.commit_all("base");
        repo.write("f", "a\nb2\nc");
        let list = hunks(&repo, "f");
        apply_hunk(&repo.path(), &only(&repo, "f"), &list[0], Some(&[0, 1])).unwrap();
        assert_eq!(index_text(&repo, "f"), b"a\nb2\n");
        let list = hunks(&repo, "f");
        assert!(
            list.iter().any(|h| h.side == HunkSide::Unstaged),
            "`c` is left"
        );
    }

    #[test]
    fn a_line_git_cannot_place_after_a_last_line_without_newline_changes_nothing() {
        let repo = Repo::new();
        repo.write("f", "a\nb");
        repo.commit_all("base");
        repo.write("f", "a\nb\nc");
        let list = hunks(&repo, "f");
        assert_eq!(
            apply_hunk(&repo.path(), &only(&repo, "f"), &list[0], Some(&[2])),
            Err(NO_ENDING.to_owned())
        );
        assert_eq!(index_text(&repo, "f"), b"a\nb");
        apply_hunk(&repo.path(), &only(&repo, "f"), &list[0], Some(&[0, 1])).unwrap();
        assert_eq!(index_text(&repo, "f"), b"a\nb\n");
    }

    #[test]
    fn lines_of_a_file_that_was_empty_can_be_picked() {
        let repo = Repo::new();
        repo.write("f", "");
        repo.commit_all("base");
        repo.write("f", "one\ntwo\n");
        let list = hunks(&repo, "f");
        apply_hunk(&repo.path(), &only(&repo, "f"), &list[0], Some(&[1])).unwrap();
        assert_eq!(index_text(&repo, "f"), b"two\n");
    }

    #[test]
    fn a_new_file_refuses_lines_as_it_moves_whole() {
        let repo = Repo::new();
        repo.write("base", "x\n");
        repo.commit_all("base");
        repo.write("new", "one\n");
        repo.git(&["add", "new"]);
        let on_index = hunks(&repo, "new");
        let staged = side(&on_index, HunkSide::Staged)[0];
        assert_eq!(
            apply_hunk(&repo.path(), &only(&repo, "new"), staged, Some(&[0])),
            Err(WHOLE.to_owned())
        );
        assert_eq!(index_text(&repo, "new"), b"one\n");
        let untracked = Repo::new();
        untracked.write("base", "x\n");
        untracked.commit_all("base");
        untracked.write("u", "one\n");
        let list = hunks(&untracked, "u");
        assert!(
            apply_hunk(
                &untracked.path(),
                &only(&untracked, "u"),
                &list[0],
                Some(&[0])
            )
            .is_err()
        );
    }

    #[test]
    fn a_stale_hunk_or_an_empty_choice_changes_nothing() {
        let repo = Repo::new();
        five_changes(&repo);
        let list = hunks(&repo, "f");
        let file = only(&repo, "f");
        assert!(apply_hunk(&repo.path(), &file, &list[0], Some(&[99])).is_err());
        repo.write("f", "A\nB\nchanged after\nc\n");
        let err = apply_hunk(&repo.path(), &file, &list[0], Some(&[0])).unwrap_err();
        assert_eq!(err, CHANGED);
        assert_eq!(index_of(&repo, "f"), "a\nb\nc\n");
    }

    #[test]
    fn numbers_stay_right_in_a_later_hunk() {
        let repo = Repo::new();
        two_hunks(
            &repo,
            "f",
            TWELVE.as_bytes(),
            TWELVE
                .replace("\n2\n", "\ntwo\n")
                .replace("19\n", "nineteen\nextra\n")
                .as_bytes(),
        );
        let list = hunks(&repo, "f");
        apply_hunk(&repo.path(), &only(&repo, "f"), &list[1], Some(&[2])).unwrap();
        let index = index_of(&repo, "f");
        assert!(index.contains("19\nextra"), "{index}");
        assert!(
            !index.contains("nineteen") && !index.contains("two"),
            "{index}"
        );
    }
}
