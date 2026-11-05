#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::fs::{self, File, FileTimes};
use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, SystemTime};

use ttywright::core::{Event, FindOptions, Query};
use ttywright::services::index::{IndexLimits, ProjectIndex};
use ttywright::services::search;

const WAIT: Duration = Duration::from_secs(30);

fn write_old(path: &Path, bytes: &[u8]) {
    fs::create_dir_all(path.parent().unwrap()).unwrap();
    fs::write(path, bytes).unwrap();
    age(path);
}

fn age(path: &Path) {
    let old = SystemTime::now() - Duration::from_secs(3600);
    File::options()
        .write(true)
        .open(path)
        .unwrap()
        .set_times(FileTimes::new().set_modified(old))
        .unwrap();
}

struct Started {
    index: ProjectIndex,
    events: Receiver<Event>,
}

impl Started {
    fn wait_done(&self) -> (usize, usize) {
        let deadline = std::time::Instant::now() + WAIT;
        let mut seen = 0;
        loop {
            let left = deadline.saturating_duration_since(std::time::Instant::now());
            match self.events.recv_timeout(left).expect("indexing finished") {
                Event::IndexProgress { done, total } if done >= total => {
                    return (seen.max(total), total);
                }
                Event::IndexProgress { total, .. } => seen = total,
                _ => {}
            }
        }
    }
}

fn start(root: &Path, cache: Option<PathBuf>) -> Started {
    start_limited(root, cache, IndexLimits::default())
}

fn start_limited(root: &Path, cache: Option<PathBuf>, limits: IndexLimits) -> Started {
    let (tx, events) = channel();
    let index = ProjectIndex::start(root.to_path_buf(), cache, limits, move |e| {
        let _ = tx.send(e);
    });
    Started { index, events }
}

fn project() -> tempfile::TempDir {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    write_old(&root.join(".gitignore"), b"ignored/\n*.log\n");
    write_old(&root.join("src/a.rs"), b"fn alpha() {}\n// needle here\n");
    write_old(&root.join("src/b.py"), b"def beta():\n    return NEEDLE\n");
    write_old(&root.join("src/crlf.txt"), b"first\r\nneedle crlf\r\nlast");
    write_old(
        &root.join("src/ru.txt"),
        "Привет, Мир\nигла тут\n".as_bytes(),
    );
    write_old(
        &root.join("src/odd.txt"),
        "\u{212A}iss \u{17F}o\n".as_bytes(),
    );
    write_old(&root.join("src/latin1.txt"), b"caf\xE9 needle\n");
    write_old(&root.join("src/bin.dat"), b"needle\0needle\n");
    write_old(&root.join("ignored/x.rs"), b"needle in ignored\n");
    write_old(&root.join("debug.log"), b"needle in log\n");
    let big = vec![b'n'; 6 * 1024 * 1024];
    write_old(&root.join("big.txt"), &big);
    for i in 0..40 {
        let name = format!("many/f{i}.txt");
        write_old(
            &root.join(name),
            format!("line {i}\nword{i} common\n").as_bytes(),
        );
    }
    dir
}

fn options(regex: bool, case_sensitive: bool, whole_word: bool) -> FindOptions {
    FindOptions {
        regex,
        case_sensitive,
        whole_word,
        structural: false,
    }
}

const QUERIES: &[(&str, bool)] = &[
    ("needle", false),
    ("NEEDLE", false),
    ("absent", false),
    ("игла", false),
    ("ИГЛА", false),
    ("мир", false),
    ("kiss", false),
    ("so", false),
    ("caf\u{FFFD}", false),
    ("common", false),
    ("n", false),
    ("ne", false),
    (r"need\w+", true),
    (r"needle|absent", true),
    (r"(?i)NEEDLE\s", true),
    (r"\d+", true),
    (r"^line", true),
    (r"first\r?\nneedle", true),
    (r"word(1|2)\b", true),
    (r"[a-z]+\s[a-z]+", true),
];

fn assert_same_as_scan(root: &Path, started: &Started, when: &str) {
    let reader = started.index.reader();
    for (query, regex) in QUERIES {
        for case in [false, true] {
            for word in [false, true] {
                let options = options(*regex, case, word);
                let scan = search::search(1, root, query, options);
                let indexed = search::search_with(1, root, query, options, Some(&reader));
                assert_eq!(scan, indexed, "{when}: {query:?} {options:?}");
            }
        }
    }
}

#[test]
fn results_are_those_of_a_full_scan_however_stale_the_index_is() {
    let dir = project();
    let root = dir.path();
    let started = start(root, None);
    started.wait_done();
    assert_same_as_scan(root, &started, "fresh");

    fs::write(root.join("src/a.rs"), b"fn alpha() {}\n// absent now\n").unwrap();
    fs::write(root.join("many/new.txt"), b"absent needle\n").unwrap();
    fs::remove_file(root.join("src/b.py")).unwrap();
    fs::write(root.join("src/bin.dat"), b"no longer binary needle\n").unwrap();
    assert_same_as_scan(root, &started, "changed, not told");

    write_old(
        &root.join("src/crlf.txt"),
        b"absent on this branch and longer\n",
    );
    assert_same_as_scan(root, &started, "old time, other size");
}

#[test]
fn random_files_and_queries_agree_with_a_scan() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path();
    let alphabet = [
        "kit",
        "KIT",
        "\u{212A}it",
        "so",
        "\u{17F}o",
        "я",
        "Я",
        "foo",
        "Bar",
        "\n",
        "\r\n",
        " ",
        "_",
        "é",
    ];
    let mut x = 88_172_645_463_325_252_u64;
    let mut below = move |n: usize| {
        x ^= x << 13;
        x ^= x >> 7;
        x ^= x << 17;
        usize::try_from(x % n as u64).unwrap()
    };
    for i in 0..60 {
        let text: String = (0..below(60))
            .map(|_| alphabet[below(alphabet.len())])
            .collect();
        write_old(&root.join(format!("d{}/f{i}.txt", i % 5)), text.as_bytes());
    }
    let started = start(root, None);
    started.wait_done();
    let reader = started.index.reader();
    for _ in 0..300 {
        let raw: String = (0..1 + below(4))
            .map(|_| alphabet[below(alphabet.len())])
            .collect();
        let options = options(below(2) == 0, below(2) == 0, below(3) == 0);
        let raw = if options.regex {
            format!("{raw}\\b")
        } else {
            raw
        };
        if Query::new(&raw, options).is_err() {
            continue;
        }
        let scan = search::search(1, root, &raw, options);
        let indexed = search::search_with(1, root, &raw, options, Some(&reader));
        assert_eq!(scan, indexed, "{raw:?} {options:?}");
    }
}

fn skips(started: &Started, path: &Path, query: &str) -> bool {
    let query = Query::new(query, FindOptions::default()).unwrap();
    let meta = fs::symlink_metadata(path).unwrap();
    started.index.reader().can_skip(path, &meta, query.needs())
}

#[test]
fn a_file_is_skipped_only_when_known_unchanged_and_without_the_text() {
    let dir = project();
    let root = dir.path();
    let started = start(root, None);
    started.wait_done();
    let file = root.join("src/a.rs");
    assert!(skips(&started, &file, "zzabsentzz"));
    assert!(!skips(&started, &file, "needle"));
    assert!(
        skips(&started, &root.join("src/bin.dat"), "needle"),
        "binary is never searched"
    );

    fs::write(&file, b"fn alpha() {}\n// other text here\n").unwrap();
    assert!(!skips(&started, &file, "zzabsentzz"), "changed, so read it");
    let unknown = root.join("src/unknown.rs");
    fs::write(&unknown, b"x").unwrap();
    assert!(
        !skips(&started, &unknown, "zzabsentzz"),
        "unknown, so read it"
    );
}

#[test]
fn a_file_written_just_now_is_not_trusted_as_its_time_may_not_change_again() {
    let dir = tempfile::tempdir().unwrap();
    let file = dir.path().join("a.txt");
    fs::write(&file, b"first text\n").unwrap();
    let started = start(dir.path(), None);
    started.wait_done();
    assert!(!skips(&started, &file, "zzabsentzz"));
}

#[test]
fn a_rescan_after_a_change_reads_only_what_changed() {
    let dir = project();
    let root = dir.path();
    let started = start(root, None);
    started.wait_done();
    write_old(&root.join("src/a.rs"), b"fn gamma() {}\n");
    fs::remove_file(root.join("src/b.py")).unwrap();
    started.index.rescan();
    let (_, total) = started.wait_done();
    assert_eq!(total, 1);
    let reader = started.index.reader();
    assert!(
        reader
            .symbols("gamma", 10)
            .iter()
            .any(|s| s.name == "gamma")
    );
    assert!(reader.symbols("alpha", 10).is_empty());
    assert!(reader.symbols("beta", 10).is_empty());
}

#[test]
fn symbols_are_found_with_where_they_are_and_ignored_files_are_left_out() {
    let dir = project();
    let root = dir.path();
    write_old(&root.join("ignored/hidden.rs"), b"fn secret_name() {}\n");
    let started = start(root, None);
    started.wait_done();
    let reader = started.index.reader();
    let found = reader.symbols("alph", 10);
    assert_eq!(found.len(), 1);
    assert_eq!(found[0].name, "alpha");
    assert_eq!(found[0].kind, "function");
    assert_eq!(found[0].path, root.join("src/a.rs"));
    assert_eq!(found[0].start, (0, 3));
    assert!(reader.symbols("secret_name", 10).is_empty());
    assert!(reader.symbols("", 10).is_empty());
}

#[test]
fn a_symbol_of_a_file_changed_behind_its_back_is_not_offered() {
    let dir = project();
    let root = dir.path();
    let started = start(root, None);
    started.wait_done();
    fs::write(root.join("src/a.rs"), b"fn other_name() {}\n").unwrap();
    assert!(started.index.reader().symbols("alpha", 10).is_empty());
}

#[test]
fn a_restart_reads_only_what_changed_since() {
    let dir = project();
    let root = dir.path();
    let store = tempfile::tempdir().unwrap();
    let cache = store.path().join("sub/index.bin");
    let first = start(root, Some(cache.clone()));
    let (_, built) = first.wait_done();
    assert!(built > 40, "{built}");
    drop(first);
    assert!(cache.exists());
    assert!(!root.join(".ttywright").exists(), "nothing in the project");

    let second = start(root, Some(cache.clone()));
    let (_, total) = second.wait_done();
    assert_eq!(total, 0, "all known");
    assert_eq!(
        second.index.reader().symbols("alpha", 5).len(),
        1,
        "from the cache"
    );
    drop(second);

    write_old(&root.join("src/a.rs"), b"fn delta() {}\n");
    let third = start(root, Some(cache));
    let (_, total) = third.wait_done();
    assert_eq!(total, 1);
    assert_eq!(third.index.reader().symbols("delta", 5).len(), 1);
    assert_same_as_scan(root, &third, "after a restart");
}

#[test]
fn a_damaged_or_foreign_cache_just_rebuilds() {
    let dir = project();
    let root = dir.path();
    let store = tempfile::tempdir().unwrap();
    let cache = store.path().join("index.bin");
    let first = start(root, Some(cache.clone()));
    let (_, built) = first.wait_done();
    drop(first);
    let good = fs::read(&cache).unwrap();

    let mut flipped = good.clone();
    let middle = flipped.len() / 2;
    flipped[middle] ^= 0x40;
    let cases: [(&str, Vec<u8>); 4] = [
        ("garbage", b"not a cache at all".to_vec()),
        ("empty", Vec::new()),
        ("cut short", good[..good.len() / 2].to_vec()),
        ("one bit", flipped),
    ];
    for (name, bytes) in cases {
        fs::write(&cache, &bytes).unwrap();
        let again = start(root, Some(cache.clone()));
        let (_, total) = again.wait_done();
        assert_eq!(total, built, "{name}");
        assert_same_as_scan(root, &again, name);
    }

    let other = project();
    fs::write(&cache, &good).unwrap();
    let foreign = start(other.path(), Some(cache));
    let (_, total) = foreign.wait_done();
    assert_eq!(total, built);
    assert_eq!(
        foreign.index.reader().symbols("alpha", 5)[0].path,
        other.path().join("src/a.rs")
    );
}

#[test]
fn two_instances_on_one_cache_leave_it_readable() {
    let dir = project();
    let root = dir.path();
    let store = tempfile::tempdir().unwrap();
    let cache = store.path().join("index.bin");
    for _ in 0..3 {
        let a = start(root, Some(cache.clone()));
        let b = start(root, Some(cache.clone()));
        a.wait_done();
        b.wait_done();
    }
    let last = start(root, Some(cache));
    let (_, total) = last.wait_done();
    assert_eq!(total, 0);
}

#[test]
fn past_its_limits_it_stops_knowing_and_search_still_answers() {
    let dir = project();
    let root = dir.path();
    let limits = IndexLimits {
        max_files: 5,
        ..IndexLimits::default()
    };
    let started = start_limited(root, None, limits);
    started.wait_done();
    assert!(started.index.reader().stats().files <= 5);
    assert_same_as_scan(root, &started, "over the limit");

    let limits = IndexLimits {
        max_filter_bytes: 16,
        max_symbols: 0,
        ..IndexLimits::default()
    };
    let small = start_limited(root, None, limits);
    small.wait_done();
    assert!(small.index.reader().stats().filter_bytes <= 16);
    assert_eq!(small.index.reader().stats().symbols, 0);
    assert_same_as_scan(root, &small, "over the byte limit");
}

#[test]
fn a_stop_during_a_build_is_prompt_and_what_was_done_is_kept() {
    let dir = tempfile::tempdir().unwrap();
    for i in 0..3000 {
        write_old(&dir.path().join(format!("d/f{i}.rs")), b"fn x() {}\n");
    }
    let store = tempfile::tempdir().unwrap();
    let cache = store.path().join("index.bin");
    let started = start(dir.path(), Some(cache.clone()));
    let begun = std::time::Instant::now();
    drop(started);
    assert!(begun.elapsed() < Duration::from_secs(3));
    let again = start(dir.path(), Some(cache));
    again.wait_done();
    assert_eq!(again.index.reader().stats().files, 3000);
}
