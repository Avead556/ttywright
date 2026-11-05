use std::collections::HashSet;
use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::mpsc::{self, Sender};
use std::thread::{self, JoinHandle};
use std::time::{SystemTime, UNIX_EPOCH};

use super::fs::write_atomic;
use super::paths::path_hash;
use crate::core::{DocumentId, Event, HistoryEntry, SnapshotLabel};

const MERGE_WINDOW_SECS: u64 = 10;
const COMPRESSION_LEVEL: i32 = 3;

#[derive(Debug, Clone)]
pub struct HistoryStore {
    dir: PathBuf,
}

impl HistoryStore {
    pub fn new(dir: PathBuf) -> Self {
        Self { dir }
    }

    fn blobs(&self) -> PathBuf {
        self.dir.join("blobs")
    }

    fn indexes(&self) -> PathBuf {
        self.dir.join("index")
    }

    fn blob(&self, hash: &str) -> PathBuf {
        self.blobs().join(format!("{hash}.zst"))
    }

    fn index(&self, path: &Path) -> PathBuf {
        self.indexes().join(format!("{}.jsonl", path_hash(path)))
    }

    pub fn record(
        &self,
        path: &Path,
        bytes: &[u8],
        label: SnapshotLabel,
        now: u64,
    ) -> io::Result<()> {
        let index = self.index(path);
        let mut entries = read_index(&index);
        let hash = blake3::hash(bytes).to_hex().to_string();
        if entries.last().is_some_and(|last| last.hash == hash) {
            return Ok(());
        }
        fs::create_dir_all(self.blobs())?;
        fs::create_dir_all(self.indexes())?;
        let blob = self.blob(&hash);
        if !blob.exists() {
            write_atomic(&blob, &zstd::encode_all(bytes, COMPRESSION_LEVEL)?)?;
        }
        let merges = entries.last().is_some_and(|last| {
            label.is_routine_save()
                && last.label.is_routine_save()
                && now.saturating_sub(last.timestamp) < MERGE_WINDOW_SECS
        });
        if merges {
            entries.pop();
        }
        entries.push(HistoryEntry {
            timestamp: now,
            path: path.to_path_buf(),
            hash,
            label,
        });
        write_index(&index, &entries)
    }

    pub fn list(&self, path: &Path) -> Vec<HistoryEntry> {
        let mut entries = read_index(&self.index(path));
        entries.reverse();
        entries
    }

    pub fn read(&self, hash: &str) -> io::Result<Vec<u8>> {
        zstd::decode_all(fs::File::open(self.blob(hash))?)
    }

    pub fn prune(&self, now: u64, max_age_secs: u64, max_size: u64) -> io::Result<()> {
        let Ok(files) = fs::read_dir(self.indexes()) else {
            return Ok(());
        };
        let mut indexes: Vec<(PathBuf, Vec<HistoryEntry>)> = files
            .filter_map(Result::ok)
            .map(|f| (f.path(), read_index(&f.path())))
            .collect();
        for (_, entries) in &mut indexes {
            entries.retain(|e| now.saturating_sub(e.timestamp) <= max_age_secs);
        }
        while self.referenced_size(&indexes) > max_size {
            let oldest = indexes
                .iter_mut()
                .filter(|(_, entries)| !entries.is_empty())
                .min_by_key(|(_, entries)| entries[0].timestamp);
            match oldest {
                Some((_, entries)) => {
                    entries.remove(0);
                }
                None => break,
            }
        }
        let referenced: HashSet<String> = indexes
            .iter()
            .flat_map(|(_, entries)| entries.iter().map(|e| format!("{}.zst", e.hash)))
            .collect();
        for (file, entries) in &indexes {
            if entries.is_empty() {
                fs::remove_file(file)?;
            } else {
                write_index(file, entries)?;
            }
        }
        for blob in fs::read_dir(self.blobs())?.filter_map(Result::ok) {
            if !referenced.contains(&*blob.file_name().to_string_lossy()) {
                fs::remove_file(blob.path())?;
            }
        }
        Ok(())
    }

    fn referenced_size(&self, indexes: &[(PathBuf, Vec<HistoryEntry>)]) -> u64 {
        let hashes: HashSet<&str> = indexes
            .iter()
            .flat_map(|(_, entries)| entries.iter().map(|e| e.hash.as_str()))
            .collect();
        hashes
            .into_iter()
            .filter_map(|h| fs::metadata(self.blob(h)).ok())
            .map(|m| m.len())
            .sum()
    }
}

fn read_index(file: &Path) -> Vec<HistoryEntry> {
    let Ok(text) = fs::read_to_string(file) else {
        return Vec::new();
    };
    text.lines()
        .filter_map(|line| serde_json::from_str(line).ok())
        .collect()
}

fn write_index(file: &Path, entries: &[HistoryEntry]) -> io::Result<()> {
    let mut text = Vec::new();
    for entry in entries {
        serde_json::to_writer(&mut text, entry)?;
        text.push(b'\n');
    }
    write_atomic(file, &text)
}

fn unix_now() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |d| d.as_secs())
}

enum Job {
    Record {
        path: PathBuf,
        bytes: Vec<u8>,
        label: SnapshotLabel,
    },
    List {
        doc: DocumentId,
        path: PathBuf,
    },
    Read {
        hash: String,
    },
    Prune,
    RecordConfirmed {
        path: PathBuf,
        bytes: Vec<u8>,
        label: SnapshotLabel,
        done: Sender<Result<(), String>>,
    },
}

#[derive(Clone)]
pub struct HistoryRecorder {
    jobs: Sender<Job>,
}

impl HistoryRecorder {
    pub fn record(&self, path: &Path, bytes: &[u8], label: SnapshotLabel) -> Result<(), String> {
        let (done, answer) = mpsc::channel();
        self.jobs
            .send(Job::RecordConfirmed {
                path: path.to_path_buf(),
                bytes: bytes.to_vec(),
                label,
                done,
            })
            .map_err(|e| e.to_string())?;
        answer.recv().map_err(|e| e.to_string())?
    }
}

pub struct HistoryWorker {
    jobs: Option<Sender<Job>>,
    thread: Option<JoinHandle<()>>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct HistoryLimits {
    pub max_age_secs: u64,
    pub max_size_bytes: u64,
}

impl HistoryWorker {
    pub fn start(
        dir: PathBuf,
        limits: HistoryLimits,
        emit: impl Fn(Event) + Send + 'static,
    ) -> Self {
        let store = HistoryStore::new(dir);
        let (jobs, rx) = mpsc::channel();
        let thread = thread::spawn(move || {
            let mut failing = false;
            for job in rx {
                let result = match job {
                    Job::Record { path, bytes, label } => {
                        store.record(&path, &bytes, label, unix_now())
                    }
                    Job::RecordConfirmed {
                        path,
                        bytes,
                        label,
                        done,
                    } => {
                        let result = store.record(&path, &bytes, label, unix_now());
                        let _ = done.send(result.map_err(|e| e.to_string()));
                        Ok(())
                    }
                    Job::List { doc, path } => {
                        emit(Event::HistoryListed {
                            doc,
                            entries: store.list(&path),
                            now: unix_now(),
                        });
                        Ok(())
                    }
                    Job::Read { hash } => {
                        let result = store.read(&hash).map_err(|e| e.to_string());
                        emit(Event::HistoryVersion { hash, result });
                        Ok(())
                    }
                    Job::Prune => {
                        store.prune(unix_now(), limits.max_age_secs, limits.max_size_bytes)
                    }
                };
                if let Err(e) = &result
                    && !failing
                {
                    emit(Event::HistoryFailed {
                        message: e.to_string(),
                    });
                }
                failing = result.is_err();
            }
        });
        let worker = Self {
            jobs: Some(jobs),
            thread: Some(thread),
        };
        worker.send(Job::Prune);
        worker
    }

    pub fn record(&self, path: PathBuf, bytes: Vec<u8>, label: SnapshotLabel) {
        self.send(Job::Record { path, bytes, label });
    }

    pub fn recorder(&self) -> Option<HistoryRecorder> {
        self.jobs.clone().map(|jobs| HistoryRecorder { jobs })
    }

    pub fn list(&self, doc: DocumentId, path: PathBuf) {
        self.send(Job::List { doc, path });
    }

    pub fn read(&self, hash: String) {
        self.send(Job::Read { hash });
    }

    fn send(&self, job: Job) {
        if let Some(jobs) = &self.jobs {
            let _ = jobs.send(job);
        }
    }
}

impl Drop for HistoryWorker {
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
    use crate::core::SnapshotLabel::{Autosave, BeforeRevert, ExternalChange, Opened, Save};

    const LIMITS: HistoryLimits = HistoryLimits {
        max_age_secs: 14 * 24 * 60 * 60,
        max_size_bytes: 256 * 1024 * 1024,
    };
    const FILE: &str = "/p/a.rs";
    const NOW: u64 = 1_000_000;

    fn store() -> (tempfile::TempDir, HistoryStore) {
        let tmp = tempfile::tempdir().unwrap();
        let store = HistoryStore::new(tmp.path().join("h"));
        (tmp, store)
    }

    fn texts(store: &HistoryStore) -> Vec<String> {
        store
            .list(Path::new(FILE))
            .iter()
            .map(|e| String::from_utf8(store.read(&e.hash).unwrap()).unwrap())
            .collect()
    }

    fn add(store: &HistoryStore, text: &str, label: SnapshotLabel, at: u64) {
        store
            .record(Path::new(FILE), text.as_bytes(), label, at)
            .unwrap();
    }

    #[test]
    fn versions_come_back_newest_first_with_their_contents() {
        let (_tmp, s) = store();
        add(&s, "один", Opened, NOW);
        add(&s, "два", Save, NOW + 60);
        assert_eq!(texts(&s), ["два", "один"]);
        let entries = s.list(Path::new(FILE));
        assert_eq!((entries[0].label, entries[0].timestamp), (Save, NOW + 60));
        assert_eq!(entries[0].path, Path::new(FILE));
    }

    #[test]
    fn the_same_contents_are_not_recorded_twice_in_a_row() {
        let (_tmp, s) = store();
        add(&s, "x", Opened, NOW);
        add(&s, "x", Save, NOW + 60);
        assert_eq!(s.list(Path::new(FILE)).len(), 1);
        add(&s, "y", Save, NOW + 120);
        add(&s, "x", Save, NOW + 180);
        assert_eq!(
            s.list(Path::new(FILE)).len(),
            3,
            "going back is a new version"
        );
    }

    #[test]
    fn frequent_saves_collapse_into_the_latest() {
        let (_tmp, s) = store();
        add(&s, "1", Autosave, NOW);
        add(&s, "2", Autosave, NOW + 3);
        add(&s, "3", Save, NOW + 6);
        assert_eq!(texts(&s), ["3"]);
        add(&s, "4", Autosave, NOW + 6 + MERGE_WINDOW_SECS);
        assert_eq!(texts(&s), ["4", "3"]);
    }

    #[test]
    fn versions_that_matter_are_never_collapsed() {
        let (_tmp, s) = store();
        add(&s, "opened", Opened, NOW);
        add(&s, "saved", Autosave, NOW + 1);
        add(&s, "theirs", ExternalChange, NOW + 2);
        add(&s, "again", Autosave, NOW + 3);
        add(&s, "before", BeforeRevert, NOW + 4);
        assert_eq!(texts(&s), ["before", "again", "theirs", "saved", "opened"]);
    }

    #[test]
    fn files_have_separate_histories_and_share_blobs() {
        let (tmp, s) = store();
        add(&s, "same", Opened, NOW);
        s.record(Path::new("/p/b.rs"), b"same", Opened, NOW)
            .unwrap();
        assert_eq!(s.list(Path::new("/p/b.rs")).len(), 1);
        assert_eq!(s.list(Path::new(FILE)).len(), 1);
        assert_eq!(fs::read_dir(tmp.path().join("h/blobs")).unwrap().count(), 1);
    }

    #[test]
    fn a_damaged_index_line_costs_only_that_line() {
        let (_tmp, s) = store();
        add(&s, "1", Opened, NOW);
        let index = s.index(Path::new(FILE));
        let mut text = fs::read_to_string(&index).unwrap();
        text.push_str("{broken\n");
        fs::write(&index, text).unwrap();
        add(&s, "2", Save, NOW + 60);
        assert_eq!(texts(&s), ["2", "1"]);
    }

    #[test]
    fn pruning_drops_old_versions_and_their_unshared_blobs() {
        let (tmp, s) = store();
        add(&s, "old", Opened, NOW);
        add(&s, "new", Save, NOW + 100);
        s.prune(NOW + 150, 60, u64::MAX).unwrap();
        assert_eq!(texts(&s), ["new"]);
        assert_eq!(fs::read_dir(tmp.path().join("h/blobs")).unwrap().count(), 1);
    }

    #[test]
    fn pruning_keeps_a_blob_another_file_still_uses() {
        let (_tmp, s) = store();
        add(&s, "shared", Opened, NOW);
        s.record(Path::new("/p/b.rs"), b"shared", Opened, NOW + 100)
            .unwrap();
        s.prune(NOW + 150, 60, u64::MAX).unwrap();
        assert!(s.list(Path::new(FILE)).is_empty());
        let kept = s.list(Path::new("/p/b.rs"));
        assert_eq!(s.read(&kept[0].hash).unwrap(), b"shared");
    }

    #[test]
    fn pruning_to_a_size_removes_the_oldest_first() {
        let (_tmp, s) = store();
        add(&s, "first", Opened, NOW);
        add(&s, "second", Save, NOW + 100);
        add(&s, "third", Save, NOW + 200);
        let one_blob = fs::metadata(s.blob(&s.list(Path::new(FILE))[0].hash))
            .unwrap()
            .len();
        s.prune(NOW + 200, u64::MAX, one_blob + one_blob / 2 + 2)
            .unwrap();
        assert!(texts(&s).len() < 3);
        assert_eq!(texts(&s)[0], "third");
    }

    #[test]
    fn pruning_an_empty_store_does_nothing() {
        let (_tmp, s) = store();
        s.prune(NOW, 60, 0).unwrap();
    }

    #[test]
    fn worker_answers_after_earlier_saves_and_reports_contents() {
        let tmp = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel();
        let worker = HistoryWorker::start(tmp.path().join("h"), LIMITS, move |e| {
            let _ = tx.send(e);
        });
        worker.record(FILE.into(), b"text".to_vec(), Save);
        worker.list(DocumentId(7), FILE.into());
        let Ok(Event::HistoryListed { doc, entries, .. }) = rx.recv() else {
            panic!("no list");
        };
        assert_eq!((doc, entries.len()), (DocumentId(7), 1));
        worker.read(entries[0].hash.clone());
        let Ok(Event::HistoryVersion { result, .. }) = rx.recv() else {
            panic!("no version");
        };
        assert_eq!(result, Ok(b"text".to_vec()));
    }
}
