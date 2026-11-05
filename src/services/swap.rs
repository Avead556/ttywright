use std::fs::{self, File, TryLockError};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use ropey::Rope;
use serde::{Deserialize, Serialize};

use super::fs::{disk_state, read_if_exists, write_atomic};
use super::paths::path_hash;
use crate::core::{ChangeSet, DiskState, Event, RecoveredSwap};

const FORMAT_VERSION: u32 = 1;
const JOURNAL_VERSION: u32 = 1;
const JOURNAL_EXTENSION: &str = "journal";
const UTF8_BOM: &[u8] = b"\xEF\xBB\xBF";

#[derive(Debug, Serialize, Deserialize)]
struct JournalStart {
    journal: u32,
    path: PathBuf,
    disk: Option<DiskState>,
    text: String,
}

#[derive(Debug, Clone, Copy)]
pub struct Journal<'a> {
    dir: &'a Path,
}

impl Journal<'_> {
    fn file_for(&self, path: &Path) -> PathBuf {
        self.dir
            .join(format!("{}.{JOURNAL_EXTENSION}", path_hash(path)))
    }

    pub fn append(
        &self,
        path: &Path,
        disk: Option<DiskState>,
        start: Option<&[u8]>,
        changes: &[ChangeSet],
    ) -> io::Result<()> {
        let mut lines = Vec::new();
        if let Some(text) = start {
            let first = JournalStart {
                journal: JOURNAL_VERSION,
                path: path.to_path_buf(),
                disk,
                text: String::from_utf8_lossy(text).into_owned(),
            };
            serde_json::to_writer(&mut lines, &first)?;
            lines.push(b'\n');
        }
        for change in changes {
            serde_json::to_writer(&mut lines, change)?;
            lines.push(b'\n');
        }
        let mut file = File::options()
            .create(true)
            .append(start.is_none())
            .write(true)
            .truncate(start.is_some())
            .open(self.file_for(path))?;
        file.write_all(&lines)
    }

    pub fn remove(&self, path: &Path) -> io::Result<()> {
        match fs::remove_file(self.file_for(path)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }
}

fn replay(journal: &[u8]) -> Option<(JournalStart, Vec<u8>)> {
    let text = std::str::from_utf8(journal).ok()?;
    let mut lines = text.split_inclusive('\n');
    let start: JournalStart = serde_json::from_str(lines.next()?).ok()?;
    if start.journal != JOURNAL_VERSION {
        return None;
    }
    let has_bom = start.text.as_bytes().starts_with(UTF8_BOM);
    let body = start
        .text
        .strip_prefix(std::str::from_utf8(UTF8_BOM).ok()?)
        .unwrap_or(&start.text);
    let mut rope = Rope::from_str(body);
    let mut lines = lines.peekable();
    while let Some(line) = lines.next() {
        let parsed: Result<ChangeSet, _> = serde_json::from_str(line);
        let Ok(change) = parsed else {
            if lines.peek().is_none() {
                break;
            }
            return None;
        };
        if !change.fits(rope.len_chars()) {
            return None;
        }
        change.apply(&mut rope);
    }
    let mut bytes = Vec::with_capacity(rope.len_bytes() + UTF8_BOM.len());
    if has_bom {
        bytes.extend_from_slice(UTF8_BOM);
    }
    for chunk in rope.chunks() {
        bytes.extend_from_slice(chunk.as_bytes());
    }
    Some((start, bytes))
}

#[derive(Debug, Serialize, Deserialize)]
struct SwapFile {
    version: u32,
    path: PathBuf,
    base: Option<DiskState>,
    contents: String,
}

#[derive(Debug, Clone)]
pub struct SwapDir {
    dir: PathBuf,
}

impl SwapDir {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn file_for(&self, path: &Path) -> PathBuf {
        self.dir.join(format!("{}.json", path_hash(path)))
    }

    pub fn write(&self, path: &Path, base: Option<DiskState>, contents: &[u8]) -> io::Result<()> {
        let swap = SwapFile {
            version: FORMAT_VERSION,
            path: path.to_path_buf(),
            base,
            contents: String::from_utf8_lossy(contents).into_owned(),
        };
        write_atomic(&self.file_for(path), &serde_json::to_vec(&swap)?)
    }

    pub fn remove(&self, path: &Path) -> io::Result<()> {
        match fs::remove_file(self.file_for(path)) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    pub fn journal(&self) -> Journal<'_> {
        Journal { dir: &self.dir }
    }

    fn fold_journals(&self) {
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return;
        };
        for file in entries.filter_map(Result::ok).map(|e| e.path()) {
            if file.extension().is_none_or(|e| e != JOURNAL_EXTENSION) {
                continue;
            }
            let replayed = fs::read(&file).ok().and_then(|bytes| replay(&bytes));
            if let Some((start, text)) = replayed
                && self.write(&start.path, start.disk, &text).is_err()
            {
                continue;
            }
            let _ = fs::remove_file(&file);
        }
    }

    pub fn recover(&self) -> Vec<RecoveredSwap> {
        self.fold_journals();
        let Ok(entries) = fs::read_dir(&self.dir) else {
            return Vec::new();
        };
        let mut swaps = Vec::new();
        for entry in entries.filter_map(Result::ok) {
            let file = entry.path();
            if entry.file_name().to_string_lossy().contains(".idea-tmp-") {
                let _ = fs::remove_file(&file);
                continue;
            }
            if file.extension().is_some_and(|e| e == "json")
                && let Some(swap) = read_swap(&file)
                && let Ok(on_disk) = read_if_exists(&swap.path)
            {
                if on_disk.as_deref() == Some(swap.contents.as_bytes()) {
                    let _ = fs::remove_file(&file);
                } else {
                    swaps.push(RecoveredSwap {
                        current: disk_state(&swap.path).unwrap_or(None),
                        path: swap.path,
                        base: swap.base,
                        on_disk,
                        contents: swap.contents.into_bytes(),
                    });
                }
            }
        }
        swaps.sort_by(|a, b| a.path.cmp(&b.path));
        swaps
    }
}

fn read_swap(file: &Path) -> Option<SwapFile> {
    let swap: SwapFile = serde_json::from_slice(&fs::read(file).ok()?).ok()?;
    (swap.version == FORMAT_VERSION).then_some(swap)
}

const LOCK_GRACE: Duration = Duration::from_millis(300);

fn claim(dir: &Path) -> io::Result<(File, bool)> {
    let lock = File::options()
        .create(true)
        .truncate(false)
        .write(true)
        .open(dir.join(".lock"))?;
    let deadline = Instant::now() + LOCK_GRACE;
    let alone = loop {
        match lock.try_lock() {
            Ok(()) => break true,
            Err(TryLockError::WouldBlock) if Instant::now() >= deadline => break false,
            Err(TryLockError::WouldBlock) => thread::sleep(Duration::from_millis(5)),
            Err(TryLockError::Error(e)) => return Err(e),
        }
    };
    match lock.try_lock_shared() {
        Ok(()) | Err(TryLockError::WouldBlock) => Ok((lock, alone)),
        Err(TryLockError::Error(e)) => Err(e),
    }
}

enum Job {
    Write {
        path: PathBuf,
        base: Option<DiskState>,
        contents: Vec<u8>,
    },
    Remove(PathBuf),
}

pub struct SwapWorker {
    dir: SwapDir,
    may_recover: bool,
    jobs: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
    _lock: File,
}

impl SwapWorker {
    pub fn start(dir: PathBuf, emit: impl Fn(Event) + Send + 'static) -> io::Result<Self> {
        fs::create_dir_all(&dir)?;
        let (lock, may_recover) = claim(&dir)?;
        let dir = SwapDir::new(dir);
        let (jobs, rx) = mpsc::channel();
        let worker_dir = dir.clone();
        let thread = thread::spawn(move || {
            let mut failing = false;
            for job in rx {
                let result = match job {
                    Job::Write {
                        path,
                        base,
                        contents,
                    } => worker_dir.write(&path, base, &contents),
                    Job::Remove(path) => worker_dir.remove(&path),
                };
                if let Err(e) = &result
                    && !failing
                {
                    emit(Event::SwapFailed {
                        message: e.to_string(),
                    });
                }
                failing = result.is_err();
            }
        });
        Ok(Self {
            dir,
            may_recover,
            jobs: Some(jobs),
            thread: Some(thread),
            _lock: lock,
        })
    }

    pub fn journal(&self) -> Journal<'_> {
        self.dir.journal()
    }

    pub fn write(&self, path: PathBuf, base: Option<DiskState>, contents: Vec<u8>) {
        self.send(Job::Write {
            path,
            base,
            contents,
        });
    }

    pub fn remove(&self, path: PathBuf) {
        self.send(Job::Remove(path));
    }

    fn send(&self, job: Job) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(job);
        }
    }

    pub fn recover(&self) -> Vec<RecoveredSwap> {
        if self.may_recover {
            self.dir.recover()
        } else {
            Vec::new()
        }
    }
}

impl Drop for SwapWorker {
    fn drop(&mut self) {
        self.jobs = None;
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn setup() -> (tempfile::TempDir, SwapDir, PathBuf) {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("swap");
        fs::create_dir(&dir).unwrap();
        let file = tmp.path().join("a.txt");
        (tmp, SwapDir::new(dir), file)
    }

    #[test]
    fn a_lock_held_for_a_moment_by_a_dead_editors_child_does_not_cost_recovery() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("swap");
        fs::create_dir(&dir).unwrap();
        let copy = File::options()
            .create(true)
            .truncate(false)
            .write(true)
            .open(dir.join(".lock"))
            .unwrap();
        copy.try_lock().unwrap();
        let release = thread::spawn(move || {
            thread::sleep(std::time::Duration::from_millis(30));
            drop(copy);
        });
        let worker = SwapWorker::start(dir, |_| {}).unwrap();
        release.join().unwrap();
        assert!(worker.may_recover, "nobody is running: the swaps are ours");
    }

    #[test]
    fn recovers_unsaved_text_with_what_it_was_based_on() {
        let (_tmp, swaps, file) = setup();
        fs::write(&file, "disk").unwrap();
        let base = disk_state(&file).unwrap();
        swaps.write(&file, base, "мои правки".as_bytes()).unwrap();

        let found = swaps.recover();
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].path, file);
        assert_eq!(found[0].base, base);
        assert_eq!(found[0].current, base);
        assert_eq!(found[0].on_disk.as_deref(), Some(b"disk".as_slice()));
        assert_eq!(found[0].contents, "мои правки".as_bytes());
    }

    #[test]
    fn recovers_text_of_a_file_that_is_gone() {
        let (_tmp, swaps, file) = setup();
        swaps.write(&file, None, b"new").unwrap();
        let found = swaps.recover();
        assert_eq!(found[0].on_disk, None);
        assert_eq!(found[0].current, None);
    }

    #[test]
    fn a_swap_matching_the_file_is_removed_without_recovery() {
        let (tmp, swaps, file) = setup();
        fs::write(&file, "same").unwrap();
        swaps.write(&file, None, b"same").unwrap();
        assert!(swaps.recover().is_empty());
        assert_eq!(fs::read_dir(tmp.path().join("swap")).unwrap().count(), 0);
    }

    #[test]
    fn removed_swaps_stay_gone_and_removing_twice_is_fine() {
        let (_tmp, swaps, file) = setup();
        swaps.write(&file, None, b"x").unwrap();
        swaps.remove(&file).unwrap();
        swaps.remove(&file).unwrap();
        assert!(swaps.recover().is_empty());
    }

    #[test]
    fn damaged_swaps_are_skipped_and_kept() {
        let (tmp, swaps, file) = setup();
        let damaged = tmp.path().join("swap/damaged.json");
        fs::write(&damaged, "{not json").unwrap();
        swaps.write(&file, None, b"good").unwrap();
        assert_eq!(swaps.recover().len(), 1);
        assert!(damaged.exists());
    }

    #[test]
    fn interrupted_swap_writes_are_cleaned_up() {
        let (tmp, swaps, _file) = setup();
        let leftover = tmp.path().join("swap/.x.json.idea-tmp-1-1");
        fs::write(&leftover, "partial").unwrap();
        assert!(swaps.recover().is_empty());
        assert!(!leftover.exists());
    }

    #[test]
    fn a_second_instance_does_not_recover_while_the_first_runs() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("swap");
        let file = tmp.path().join("a.txt");
        let first = SwapWorker::start(dir.clone(), |_| {}).unwrap();
        first.write(file.clone(), None, b"live".to_vec());
        drop(first);

        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(10);
        let first = loop {
            let worker = SwapWorker::start(dir.clone(), |_| {}).unwrap();
            if worker.may_recover || std::time::Instant::now() > deadline {
                break worker;
            }
            drop(worker);
            std::thread::sleep(std::time::Duration::from_millis(5));
        };
        assert_eq!(first.recover().len(), 1, "no other instance is running");
        let second = SwapWorker::start(dir.clone(), |_| {}).unwrap();
        assert!(second.recover().is_empty());
        drop(first);
        let third = SwapWorker::start(dir, |_| {}).unwrap();
        assert!(third.recover().is_empty(), "the second is still running");
    }

    #[test]
    fn worker_finishes_its_queue_in_order_when_dropped() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("swap");
        let file = tmp.path().join("a.txt");
        let worker = SwapWorker::start(dir.clone(), |_| {}).unwrap();
        worker.write(file.clone(), None, b"one".to_vec());
        worker.write(file, None, b"two".to_vec());
        drop(worker);
        assert_eq!(SwapDir::new(dir).recover()[0].contents, b"two");
    }

    #[test]
    fn worker_reports_a_run_of_failures_once() {
        let tmp = tempfile::tempdir().unwrap();
        let dir = tmp.path().join("swap");
        let (tx, rx) = mpsc::channel();
        let worker = SwapWorker::start(dir.clone(), move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        fs::remove_dir_all(&dir).unwrap();
        worker.write("/x/a".into(), None, b"1".to_vec());
        worker.write("/x/a".into(), None, b"2".to_vec());
        drop(worker);
        assert_eq!(rx.iter().count(), 1);
    }

    fn change(len: usize, from: usize, to: usize, text: &str) -> ChangeSet {
        ChangeSet::replace(len, from, to, text)
    }

    fn file_in(tmp: &tempfile::TempDir, text: &str) -> PathBuf {
        let path = tmp.path().join("a.txt");
        std::fs::write(&path, text).unwrap();
        path
    }

    fn journal_dir(tmp: &tempfile::TempDir) -> SwapDir {
        let dir = tmp.path().join("swap");
        std::fs::create_dir_all(&dir).unwrap();
        SwapDir::new(dir)
    }

    #[test]
    fn a_journal_alone_brings_back_every_change() {
        let tmp = tempfile::tempdir().unwrap();
        let path = file_in(&tmp, "abc");
        let dir = journal_dir(&tmp);
        let journal = dir.journal();
        journal
            .append(&path, None, Some(b"abc"), &[change(3, 3, 3, "d")])
            .unwrap();
        journal
            .append(
                &path,
                None,
                None,
                &[change(4, 0, 1, "ы"), change(4, 4, 4, "\n")],
            )
            .unwrap();
        let swaps = dir.recover();
        assert_eq!(swaps.len(), 1);
        assert_eq!(swaps[0].contents, "ыbcd\n".as_bytes());
        assert_eq!(swaps[0].path, path);
    }

    #[test]
    fn the_journal_wins_over_an_older_swap_and_recovery_is_repeatable() {
        let tmp = tempfile::tempdir().unwrap();
        let path = file_in(&tmp, "abc");
        let dir = journal_dir(&tmp);
        dir.write(&path, None, b"abcd").unwrap();
        dir.journal()
            .append(&path, None, Some(b"abc"), &[change(3, 3, 3, "de")])
            .unwrap();
        assert_eq!(dir.recover()[0].contents, b"abcde");
        assert_eq!(
            dir.recover()[0].contents,
            b"abcde",
            "killed again, nothing is lost"
        );
    }

    #[test]
    fn a_torn_last_line_costs_only_that_line() {
        let tmp = tempfile::tempdir().unwrap();
        let path = file_in(&tmp, "abc");
        let dir = journal_dir(&tmp);
        let journal = dir.journal();
        journal
            .append(&path, None, Some(b"abc"), &[change(3, 3, 3, "d")])
            .unwrap();
        let mut file = File::options()
            .append(true)
            .open(journal.file_for(&path))
            .unwrap();
        file.write_all(b"{\"ops\":[{\"Ins").unwrap();
        assert_eq!(dir.recover()[0].contents, b"abcd");
    }

    #[test]
    fn a_damaged_journal_falls_back_on_the_swap() {
        for damage in [
            "garbage\n{}\n".to_owned(),
            format!(
                "{}\n{}\n",
                serde_json::json!({"journal": 1, "path": "/x", "disk": null, "text": "abc"}),
                serde_json::to_string(&change(99, 0, 1, "")).unwrap()
            ),
        ] {
            let tmp = tempfile::tempdir().unwrap();
            let path = file_in(&tmp, "abc");
            let dir = journal_dir(&tmp);
            dir.write(&path, None, b"from swap").unwrap();
            std::fs::write(dir.journal().file_for(&path), damage).unwrap();
            assert_eq!(dir.recover()[0].contents, b"from swap");
        }
    }

    #[test]
    fn a_journal_that_adds_up_to_the_file_means_nothing_to_recover() {
        let tmp = tempfile::tempdir().unwrap();
        let path = file_in(&tmp, "abcd");
        let dir = journal_dir(&tmp);
        dir.journal()
            .append(&path, None, Some(b"abc"), &[change(3, 3, 3, "d")])
            .unwrap();
        assert!(dir.recover().is_empty());
        assert_eq!(
            std::fs::read_dir(&dir.dir).unwrap().count(),
            0,
            "nothing left"
        );
    }

    #[test]
    fn a_new_journal_replaces_one_left_behind() {
        let tmp = tempfile::tempdir().unwrap();
        let path = file_in(&tmp, "x");
        let dir = journal_dir(&tmp);
        let journal = dir.journal();
        journal
            .append(&path, None, Some(b"old"), &[change(3, 0, 3, "o")])
            .unwrap();
        journal
            .append(&path, None, Some(b"new"), &[change(3, 3, 3, "!")])
            .unwrap();
        assert_eq!(dir.recover()[0].contents, b"new!");
    }

    #[test]
    fn a_byte_order_mark_stays_outside_the_counted_text() {
        let tmp = tempfile::tempdir().unwrap();
        let path = file_in(&tmp, "x");
        let dir = journal_dir(&tmp);
        dir.journal()
            .append(
                &path,
                None,
                Some(b"\xEF\xBB\xBFab"),
                &[change(2, 2, 2, "c")],
            )
            .unwrap();
        assert_eq!(dir.recover()[0].contents, b"\xEF\xBB\xBFabc");
    }

    #[test]
    fn removing_a_journal_is_idempotent_and_saved_text_stays_gone() {
        let tmp = tempfile::tempdir().unwrap();
        let path = file_in(&tmp, "abc");
        let dir = journal_dir(&tmp);
        let journal = dir.journal();
        journal
            .append(&path, None, Some(b"abc"), &[change(3, 3, 3, "d")])
            .unwrap();
        journal.remove(&path).unwrap();
        journal.remove(&path).unwrap();
        assert!(dir.recover().is_empty());
    }
}
