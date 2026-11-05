use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{Receiver, RecvTimeoutError, Sender, channel};
use std::sync::{Arc, RwLock};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use crate::core::trigram::{TextFilter, TrigramFilter};
use crate::core::{Event, ProjectSymbol};
use crate::lang::languages::Language;
use crate::lang::symbols;
use crate::services::fs::{walker, write_atomic};
use crate::services::search::MAX_FILE_BYTES;

const FORMAT: &str = "ttywright index 1";
const RACY: Duration = Duration::from_secs(2);
const MIN_RESCAN_GAP: Duration = Duration::from_secs(2);
const SAVE_EVERY: Duration = Duration::from_secs(30);
const PROGRESS_EVERY: Duration = Duration::from_millis(200);
const STOP_WAIT: Duration = Duration::from_secs(1);

#[derive(Debug, Clone, Copy)]
pub struct IndexLimits {
    pub max_files: usize,
    pub max_filter_bytes: usize,
    pub max_symbols: usize,
}

impl Default for IndexLimits {
    fn default() -> Self {
        Self {
            max_files: 500_000,
            max_filter_bytes: 128 * 1024 * 1024,
            max_symbols: 3_000_000,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct Stat {
    mtime_ns: u64,
    size: u64,
}

fn stat_of(metadata: &fs::Metadata) -> Option<Stat> {
    let since = metadata.modified().ok()?.duration_since(UNIX_EPOCH).ok()?;
    Some(Stat {
        mtime_ns: u64::try_from(since.as_nanos()).ok()?,
        size: metadata.len(),
    })
}

fn now_ns() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|d| u64::try_from(d.as_nanos()).ok())
        .unwrap_or(0)
}

fn racy_ns() -> u64 {
    u64::try_from(RACY.as_nanos()).unwrap_or(u64::MAX)
}

#[derive(Debug)]
enum Content {
    Text(TrigramFilter),
    Binary,
}

#[derive(Debug, Clone, Copy)]
struct StoredSymbol {
    name_at: u32,
    line: u32,
    col: u32,
    kind: u8,
}

#[derive(Debug)]
struct Record {
    stat: Stat,
    read_at_ns: u64,
    content: Content,
    names: Box<str>,
    symbols: Box<[StoredSymbol]>,
}

impl Record {
    fn name(&self, index: usize) -> &str {
        let from = self.symbols.get(index).map_or(0, |s| s.name_at as usize);
        let to = self
            .symbols
            .get(index + 1)
            .map_or(self.names.len(), |s| s.name_at as usize);
        self.names.get(from..to).unwrap_or("")
    }

    fn filter_bytes(&self) -> usize {
        match &self.content {
            Content::Text(filter) => filter.as_bytes().len(),
            Content::Binary => 0,
        }
    }

    fn proves(&self, stat: Stat) -> bool {
        self.stat == stat && self.settled()
    }

    fn settled(&self) -> bool {
        self.stat.mtime_ns.saturating_add(racy_ns()) < self.read_at_ns
    }
}

#[derive(Debug, Default)]
struct Data {
    files: HashMap<String, Record>,
    filter_bytes: usize,
    symbols: usize,
}

impl Data {
    fn insert(&mut self, path: String, record: Record) {
        self.remove(&path);
        self.filter_bytes += record.filter_bytes();
        self.symbols += record.symbols.len();
        self.files.insert(path, record);
    }

    fn remove(&mut self, path: &str) {
        if let Some(old) = self.files.remove(path) {
            self.filter_bytes -= old.filter_bytes();
            self.symbols -= old.symbols.len();
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct IndexStats {
    pub files: usize,
    pub filter_bytes: usize,
    pub symbols: usize,
}

#[derive(Debug, Clone)]
pub struct IndexReader {
    root: PathBuf,
    data: Arc<RwLock<Data>>,
}

impl IndexReader {
    pub fn can_skip(&self, path: &Path, metadata: &fs::Metadata, needs: &TextFilter) -> bool {
        let Some(key) = path.strip_prefix(&self.root).ok().and_then(Path::to_str) else {
            return false;
        };
        let Some(stat) = stat_of(metadata) else {
            return false;
        };
        let Ok(data) = self.data.read() else {
            return false;
        };
        data.files
            .get(key)
            .filter(|record| record.proves(stat))
            .is_some_and(|record| match &record.content {
                Content::Binary => true,
                Content::Text(filter) => !needs.may_match(filter),
            })
    }

    pub fn stats(&self) -> IndexStats {
        self.data.read().map_or(
            IndexStats {
                files: 0,
                filter_bytes: 0,
                symbols: 0,
            },
            |d| IndexStats {
                files: d.files.len(),
                filter_bytes: d.filter_bytes,
                symbols: d.symbols,
            },
        )
    }

    pub fn symbols(&self, query: &str, limit: usize) -> Vec<ProjectSymbol> {
        if query.trim().is_empty() {
            return Vec::new();
        }
        let mut matcher = Matcher::new(Config::DEFAULT);
        let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
        let mut buffer = Vec::new();
        let Ok(data) = self.data.read() else {
            return Vec::new();
        };
        let mut scored: Vec<(u32, &str, &str, StoredSymbol)> = Vec::new();
        for (path, record) in &data.files {
            for (at, symbol) in record.symbols.iter().enumerate() {
                let name = record.name(at);
                if let Some(score) = pattern.score(Utf32Str::new(name, &mut buffer), &mut matcher) {
                    scored.push((score, name, path, *symbol));
                }
            }
        }
        scored.sort_by(|a, b| {
            (b.0, a.1, a.2, a.3.line, a.3.col).cmp(&(a.0, b.1, b.2, b.3.line, b.3.col))
        });
        let mut still_true: HashMap<&str, bool> = HashMap::new();
        let mut found = Vec::new();
        for (_, name, path, symbol) in scored {
            if found.len() == limit {
                break;
            }
            let full = self.root.join(path);
            let current = *still_true.entry(path).or_insert_with(|| {
                let stat = fs::symlink_metadata(&full).ok().and_then(|m| stat_of(&m));
                stat == data.files.get(path).map(|r| r.stat)
            });
            if current {
                found.push(ProjectSymbol {
                    name: name.to_owned(),
                    kind: symbols::kind_name(symbol.kind).unwrap_or("").to_owned(),
                    path: full,
                    start: (symbol.line, symbol.col),
                });
            }
        }
        found
    }
}

enum Job {
    Rescan,
    Stop,
}

pub struct ProjectIndex {
    reader: IndexReader,
    jobs: Sender<Job>,
    stop: Arc<AtomicBool>,
    finished: Receiver<()>,
}

impl ProjectIndex {
    pub fn start(
        root: PathBuf,
        cache: Option<PathBuf>,
        limits: IndexLimits,
        emit: impl Fn(Event) + Send + 'static,
    ) -> Self {
        let data = Arc::new(RwLock::new(Data::default()));
        let reader = IndexReader {
            root: root.clone(),
            data: Arc::clone(&data),
        };
        let (jobs, inbox) = channel();
        let (done, finished) = channel();
        let stop = Arc::new(AtomicBool::new(false));
        let worker = Worker {
            root,
            cache,
            limits,
            data,
            stop: Arc::clone(&stop),
            emit: Box::new(emit),
            dirty: false,
            last_save: Instant::now(),
        };
        thread::spawn(move || {
            worker.run(&inbox);
            let _ = done.send(());
        });
        Self {
            reader,
            jobs,
            stop,
            finished,
        }
    }

    pub fn reader(&self) -> IndexReader {
        self.reader.clone()
    }

    pub fn rescan(&self) {
        let _ = self.jobs.send(Job::Rescan);
    }
}

impl Drop for ProjectIndex {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        let _ = self.jobs.send(Job::Stop);
        let _ = self.finished.recv_timeout(STOP_WAIT);
    }
}

struct Worker {
    root: PathBuf,
    cache: Option<PathBuf>,
    limits: IndexLimits,
    data: Arc<RwLock<Data>>,
    stop: Arc<AtomicBool>,
    emit: Box<dyn Fn(Event) + Send>,
    dirty: bool,
    last_save: Instant,
}

impl Worker {
    fn run(mut self, inbox: &Receiver<Job>) {
        if let Some(loaded) = self.cache.as_deref().and_then(|c| load(c, &self.root))
            && let Ok(mut data) = self.data.write()
        {
            *data = loaded;
        }
        let mut again = self.rescan();
        let mut last_scan = Instant::now();
        loop {
            let wait = if again {
                RACY + Duration::from_secs(1)
            } else {
                Duration::from_secs(3600)
            };
            match inbox.recv_timeout(wait) {
                Ok(Job::Stop) | Err(RecvTimeoutError::Disconnected) => break,
                Ok(Job::Rescan) => {
                    let gap = MIN_RESCAN_GAP.saturating_sub(last_scan.elapsed());
                    if self.stopped_within(gap) {
                        break;
                    }
                    let stop = inbox.try_iter().any(|job| matches!(job, Job::Stop));
                    if stop {
                        break;
                    }
                }
                Err(RecvTimeoutError::Timeout) if !again => continue,
                Err(RecvTimeoutError::Timeout) => {}
            }
            again = self.rescan();
            last_scan = Instant::now();
        }
        self.save();
    }

    fn stopped_within(&self, wait: Duration) -> bool {
        let until = Instant::now() + wait;
        while Instant::now() < until {
            if self.stop.load(Ordering::Relaxed) {
                return true;
            }
            thread::sleep(Duration::from_millis(20));
        }
        self.stop.load(Ordering::Relaxed)
    }

    fn rescan(&mut self) -> bool {
        let mut seen = Vec::new();
        let mut unsettled = false;
        for entry in walker(&self.root).build().filter_map(Result::ok) {
            if self.stop.load(Ordering::Relaxed) {
                return false;
            }
            let Some(key) = entry
                .path()
                .strip_prefix(&self.root)
                .ok()
                .and_then(Path::to_str)
                .map(str::to_owned)
            else {
                continue;
            };
            if !entry.file_type().is_some_and(|t| t.is_file()) {
                continue;
            }
            let stat = entry.metadata().ok().and_then(|m| stat_of(&m));
            if let Some(stat) = stat.filter(|s| s.size <= MAX_FILE_BYTES) {
                seen.push((key, stat));
            }
        }
        let now = now_ns();
        let mut to_read = Vec::new();
        let Ok(mut data) = self.data.write() else {
            return false;
        };
        let walked: std::collections::HashSet<&str> =
            seen.iter().map(|(k, _)| k.as_str()).collect();
        let gone: Vec<String> = data
            .files
            .keys()
            .filter(|k| !walked.contains(k.as_str()))
            .cloned()
            .collect();
        self.dirty |= !gone.is_empty();
        for key in &gone {
            data.remove(key);
        }
        for (key, stat) in &seen {
            let known = data.files.get(key);
            match known {
                Some(record) if record.proves(*stat) => {}
                Some(record)
                    if record.stat == *stat && stat.mtime_ns.saturating_add(racy_ns()) >= now =>
                {
                    unsettled = true;
                }
                _ => to_read.push((key.clone(), *stat)),
            }
        }
        drop(data);
        let total = to_read.len();
        let mut shown = Instant::now();
        (self.emit)(Event::IndexProgress { done: 0, total });
        for (done, (key, stat)) in to_read.into_iter().enumerate() {
            if self.stop.load(Ordering::Relaxed) {
                return false;
            }
            if shown.elapsed() >= PROGRESS_EVERY {
                shown = Instant::now();
                (self.emit)(Event::IndexProgress { done, total });
            }
            unsettled |= self.read_file(key, stat);
            if self.last_save.elapsed() >= SAVE_EVERY {
                self.save();
            }
        }
        (self.emit)(Event::IndexProgress { done: total, total });
        self.save();
        unsettled
    }

    fn read_file(&mut self, key: String, stat: Stat) -> bool {
        let read_at_ns = now_ns();
        let Ok(bytes) = fs::read(self.root.join(&key)) else {
            if let Ok(mut data) = self.data.write() {
                data.remove(&key);
            }
            return false;
        };
        let record = read_record(&key, stat, read_at_ns, &bytes);
        let settled = record.settled();
        let Ok(mut data) = self.data.write() else {
            return false;
        };
        data.remove(&key);
        let fits = data.files.len() < self.limits.max_files
            && data.filter_bytes + record.filter_bytes() <= self.limits.max_filter_bytes;
        if fits {
            let mut record = record;
            if data.symbols + record.symbols.len() > self.limits.max_symbols {
                record.symbols = Box::default();
                record.names = Box::default();
            }
            data.insert(key, record);
        }
        self.dirty = true;
        !settled
    }

    fn save(&mut self) {
        self.last_save = Instant::now();
        if !self.dirty {
            return;
        }
        let Some(cache) = self.cache.clone() else {
            return;
        };
        let encoded = self.data.read().ok().and_then(|d| encode(&d, &self.root));
        if let Some(encoded) = encoded {
            if let Some(dir) = cache.parent() {
                let _ = fs::create_dir_all(dir);
            }
            let _ = write_atomic(&cache, &encoded);
            self.dirty = false;
        }
    }
}

fn read_record(key: &str, stat: Stat, read_at_ns: u64, bytes: &[u8]) -> Record {
    if bytes.contains(&0) {
        return Record {
            stat,
            read_at_ns,
            content: Content::Binary,
            names: Box::default(),
            symbols: Box::default(),
        };
    }
    let text = String::from_utf8_lossy(bytes);
    let first_line = text.lines().next().unwrap_or("");
    let language = Language::detect(Path::new(key), first_line);
    let mut names = String::new();
    let mut stored = Vec::new();
    for found in symbols::extract(language, &text) {
        let Ok(name_at) = u32::try_from(names.len()) else {
            break;
        };
        names.push_str(&found.name);
        stored.push(StoredSymbol {
            name_at,
            line: found.start.0,
            col: found.start.1,
            kind: symbols::kind_index(found.kind),
        });
    }
    Record {
        stat,
        read_at_ns,
        content: Content::Text(TrigramFilter::of_text(&text)),
        names: names.into_boxed_str(),
        symbols: stored.into_boxed_slice(),
    }
}

fn put_bytes(out: &mut Vec<u8>, bytes: &[u8]) -> Option<()> {
    out.extend_from_slice(&u32::try_from(bytes.len()).ok()?.to_le_bytes());
    out.extend_from_slice(bytes);
    Some(())
}

fn encode(data: &Data, root: &Path) -> Option<Vec<u8>> {
    let mut out = Vec::new();
    put_bytes(&mut out, FORMAT.as_bytes())?;
    put_bytes(&mut out, env!("CARGO_PKG_VERSION").as_bytes())?;
    put_bytes(&mut out, root.to_str()?.as_bytes())?;
    put_bytes(
        &mut out,
        crate::lang::syntax::modules_fingerprint().as_bytes(),
    )?;
    out.extend_from_slice(&u32::try_from(data.files.len()).ok()?.to_le_bytes());
    for (path, record) in &data.files {
        put_bytes(&mut out, path.as_bytes())?;
        out.extend_from_slice(&record.stat.mtime_ns.to_le_bytes());
        out.extend_from_slice(&record.stat.size.to_le_bytes());
        out.extend_from_slice(&record.read_at_ns.to_le_bytes());
        match &record.content {
            Content::Binary => out.push(0),
            Content::Text(filter) => {
                out.push(1);
                put_bytes(&mut out, filter.as_bytes())?;
            }
        }
        out.extend_from_slice(&u32::try_from(record.symbols.len()).ok()?.to_le_bytes());
        put_bytes(&mut out, record.names.as_bytes())?;
        for symbol in &record.symbols {
            out.extend_from_slice(&symbol.name_at.to_le_bytes());
            out.extend_from_slice(&symbol.line.to_le_bytes());
            out.extend_from_slice(&symbol.col.to_le_bytes());
            out.push(symbol.kind);
        }
    }
    let packed = zstd::encode_all(out.as_slice(), 1).ok()?;
    let mut file = blake3::hash(&packed).as_bytes().to_vec();
    file.extend_from_slice(&packed);
    Some(file)
}

struct Reader<'a>(&'a [u8]);

impl<'a> Reader<'a> {
    fn take(&mut self, n: usize) -> Option<&'a [u8]> {
        if n > self.0.len() {
            return None;
        }
        let (head, rest) = self.0.split_at(n);
        self.0 = rest;
        Some(head)
    }

    fn u8(&mut self) -> Option<u8> {
        self.take(1).map(|b| b[0])
    }

    fn u32(&mut self) -> Option<u32> {
        Some(u32::from_le_bytes(self.take(4)?.try_into().ok()?))
    }

    fn u64(&mut self) -> Option<u64> {
        Some(u64::from_le_bytes(self.take(8)?.try_into().ok()?))
    }

    fn bytes(&mut self) -> Option<&'a [u8]> {
        let len = self.u32()? as usize;
        self.take(len)
    }

    fn text(&mut self) -> Option<&'a str> {
        std::str::from_utf8(self.bytes()?).ok()
    }
}

fn load(cache: &Path, root: &Path) -> Option<Data> {
    let file = fs::read(cache).ok()?;
    let packed = file.get(32..)?;
    if file.get(..32)? != blake3::hash(packed).as_bytes() {
        return None;
    }
    let plain = zstd::decode_all(packed).ok()?;
    decode(&plain, root)
}

fn decode(plain: &[u8], root: &Path) -> Option<Data> {
    let mut r = Reader(plain);
    if r.text()? != FORMAT
        || r.text()? != env!("CARGO_PKG_VERSION")
        || r.text()? != root.to_str()?
        || r.text()? != crate::lang::syntax::modules_fingerprint()
    {
        return None;
    }
    let mut data = Data::default();
    for _ in 0..r.u32()? {
        let path = r.text()?.to_owned();
        let stat = Stat {
            mtime_ns: r.u64()?,
            size: r.u64()?,
        };
        let read_at_ns = r.u64()?;
        let content = match r.u8()? {
            0 => Content::Binary,
            1 => Content::Text(TrigramFilter::from_bytes(r.bytes()?.to_vec())?),
            _ => return None,
        };
        let count = r.u32()?;
        let names: Box<str> = r.text()?.into();
        let mut stored = Vec::new();
        for _ in 0..count {
            let symbol = StoredSymbol {
                name_at: r.u32()?,
                line: r.u32()?,
                col: r.u32()?,
                kind: r.u8()?,
            };
            symbols::kind_name(symbol.kind)?;
            stored.push(symbol);
        }
        data.insert(
            path,
            Record {
                stat,
                read_at_ns,
                content,
                names,
                symbols: stored.into_boxed_slice(),
            },
        );
    }
    r.0.is_empty().then_some(data)
}
