#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::sync::mpsc::{Receiver, channel};
use std::time::{Duration, Instant};
use ttywright::core::lsp_command::{LspEffect, LspEvent};

use ttywright::config::{Config, ServerSettings};
use ttywright::core::{
    Effect, Event, Language, LspAnswer, LspLocation, LspQuery, LspState, PositionEncoding,
    ServerKey,
};
use ttywright::services::Services;
use ttywright::services::lsp::provision::Download;
use ttywright::services::lsp::{Bundled, LspConfig, LspHandle, ServerCommand, find_on_path};

const WAIT: Duration = Duration::from_secs(60);

fn fake_server() -> String {
    let exe = std::env::current_exe().unwrap();
    let target = exe.parent().unwrap().parent().unwrap();
    target.join("examples/fake_lsp").display().to_string()
}

struct Fixture {
    dir: tempfile::TempDir,
    events: Receiver<Event>,
    handle: Option<LspHandle>,
}

fn start(mode: &str, initialize_timeout: Duration) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    start_in(dir, mode, initialize_timeout, Duration::from_secs(10))
}

fn start_in(
    dir: tempfile::TempDir,
    mode: &str,
    initialize_timeout: Duration,
    query_timeout: Duration,
) -> Fixture {
    let mut config = LspConfig::new("env".to_owned(), dir.path().to_path_buf());
    config.args = vec![
        format!("FAKE_LSP_MODE={mode}"),
        format!("FAKE_LSP_LOG={}", dir.path().join("log").display()),
        format!("FAKE_LSP_MARKER={}", dir.path().join("marker").display()),
        fake_server(),
    ];
    config.backoff = Duration::from_millis(10);
    config.initialize_timeout = initialize_timeout;
    config.query_timeout = query_timeout;
    let (tx, events) = channel();
    let handle = LspHandle::start(config, move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    Fixture {
        dir,
        events,
        handle: Some(handle),
    }
}

impl Fixture {
    fn handle(&self) -> &LspHandle {
        self.handle.as_ref().unwrap()
    }

    fn wait_for(&self, wanted: impl Fn(&Event) -> bool) -> Event {
        let deadline = Instant::now() + WAIT;
        loop {
            let left = deadline.saturating_duration_since(Instant::now());
            let event = self
                .events
                .recv_timeout(left)
                .expect("event did not arrive");
            if wanted(&event) {
                return event;
            }
        }
    }

    fn wait_ready(self) -> Self {
        self.ready();
        self
    }

    fn ready(&self) {
        self.wait_for(|e| matches!(e, Event::Lsp(LspEvent::Ready { .. })));
    }

    fn state(&self, state: LspState) {
        self.wait_for(
            |e| matches!(e, Event::Lsp(LspEvent::StateChanged { state: s, .. }) if *s == state),
        );
    }

    fn saw(&self, wanted: impl Fn(&Event) -> bool) -> bool {
        self.events.try_iter().any(|e| wanted(&e))
    }

    fn log(&self) -> String {
        std::fs::read_to_string(self.dir.path().join("log")).unwrap_or_default()
    }
}

fn file() -> PathBuf {
    PathBuf::from("/fake/a.rs")
}

#[test]
fn diagnostics_follow_the_document_versions() {
    let f = start("normal", WAIT);
    f.ready();
    f.handle().did_open(file(), "rust", 1, "fn".into());
    let event = f.wait_for(|e| matches!(e, Event::Lsp(LspEvent::Diagnostics { .. })));
    let Event::Lsp(LspEvent::Diagnostics { path, items, .. }) = event else {
        unreachable!();
    };
    assert_eq!(path, Path::new("/fake/a.rs"));
    assert_eq!(items[0].message, "fake error v1");

    f.handle().did_change(file(), 2, "fn x".into());
    let event = f.wait_for(|e| matches!(e, Event::Lsp(LspEvent::Diagnostics { .. })));
    let Event::Lsp(LspEvent::Diagnostics { items, .. }) = event else {
        unreachable!();
    };
    assert_eq!(items[0].message, "fake error v2");
}

#[test]
fn a_crash_restarts_the_server_and_it_works_again() {
    let f = start("crash-once", WAIT);
    f.ready();
    f.handle().did_open(file(), "rust", 1, "x".into());
    f.state(LspState::Crashed);
    f.ready();
    f.handle().did_open(file(), "rust", 1, "x".into());
    f.wait_for(|e| matches!(e, Event::Lsp(LspEvent::Diagnostics { .. })));
}

#[test]
fn repeated_crashes_stop_until_asked_to_restart() {
    let f = start("crash-on-open", WAIT);
    for _ in 0..5 {
        f.ready();
        f.handle().did_open(file(), "rust", 1, "x".into());
    }
    f.state(LspState::Stopped);
    f.handle().restart();
    f.ready();
}

#[test]
fn a_server_hanging_on_initialize_is_killed_and_retried() {
    let f = start("hang-init", Duration::from_millis(100));
    f.state(LspState::Crashed);
    f.state(LspState::Starting);
    f.state(LspState::Crashed);
}

#[test]
fn a_stream_of_garbage_counts_as_a_crash() {
    let f = start("garbage", WAIT);
    f.ready();
    f.state(LspState::Crashed);
}

#[test]
fn a_missing_binary_is_reported_without_retrying() {
    let dir = tempfile::tempdir().unwrap();
    let config = LspConfig::new("no-such-server-xyz".to_owned(), dir.path().to_path_buf());
    let (tx, rx) = channel();
    let _handle = LspHandle::start(config, move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    let mut states = Vec::new();
    while let Ok(event) = rx.recv_timeout(Duration::from_millis(300)) {
        states.push(event);
    }
    assert!(
        matches!(
            states.as_slice(),
            [
                Event::Lsp(LspEvent::StateChanged {
                    state: LspState::Starting,
                    ..
                }),
                Event::Lsp(LspEvent::StateChanged {
                    state: LspState::NotFound,
                    ..
                })
            ]
        ),
        "{states:?}"
    );
}

#[test]
fn dropping_the_handle_shuts_the_server_down_politely() {
    let mut f = start("normal", WAIT);
    f.ready();
    let started = Instant::now();
    f.handle.take();
    assert!(started.elapsed() < Duration::from_secs(5));
    let log = f.log();
    assert!(log.contains("shutdown") && log.contains("exit"), "{log}");
}

#[test]
fn a_manual_restart_begins_a_new_session() {
    let f = start("normal", WAIT);
    f.ready();
    f.handle().restart();
    f.ready();
    let deadline = Instant::now() + WAIT;
    while f.log().matches("initialized").count() < 2 {
        assert!(Instant::now() < deadline, "{}", f.log());
        std::thread::sleep(Duration::from_millis(5));
    }
    assert_eq!(f.log().matches("initialized").count(), 2);
}

fn definition(f: &Fixture, id: u64, line: u32) -> Result<LspAnswer, String> {
    f.handle()
        .query(id, file(), LspQuery::Definition { at: (line, 0) });
    let event =
        f.wait_for(|e| matches!(e, Event::Lsp(LspEvent::Answered { id: i, .. }) if *i == id));
    let Event::Lsp(LspEvent::Answered { result, .. }) = event else {
        unreachable!();
    };
    result
}

#[test]
fn definitions_come_back_in_every_shape_the_protocol_allows() {
    let f = start("normal", WAIT);
    f.ready();
    let at = |line, character| LspLocation {
        path: file(),
        start: (line, character),
    };
    assert_eq!(
        definition(&f, 1, 0),
        Ok(LspAnswer::Locations(vec![at(1, 2)])),
        "a link points at its selection range"
    );
    assert_eq!(
        definition(&f, 2, 1),
        Ok(LspAnswer::Locations(vec![at(3, 0), at(4, 0)]))
    );
    assert_eq!(definition(&f, 3, 2), Ok(LspAnswer::Locations(Vec::new())));
    assert_eq!(
        definition(&f, 4, 3),
        Ok(LspAnswer::Locations(vec![at(5, 0)])),
        "a single location"
    );
    assert_eq!(definition(&f, 5, 4), Err("fake failure".to_owned()));
}

#[test]
fn a_query_without_an_answer_times_out_and_is_cancelled() {
    let dir = tempfile::tempdir().unwrap();
    let f = start_in(dir, "normal", WAIT, Duration::from_millis(50));
    f.ready();
    assert_eq!(definition(&f, 1, 9), Err("no answer in time".to_owned()));
    assert_eq!(definition(&f, 2, 2), Ok(LspAnswer::Locations(Vec::new())));
    assert!(f.log().contains("$/cancelRequest"));
}

#[test]
fn a_cancelled_query_is_never_answered() {
    let f = start("normal", WAIT);
    f.ready();
    f.handle()
        .query(1, file(), LspQuery::Definition { at: (9, 0) });
    f.handle().cancel(1);
    assert_eq!(definition(&f, 2, 2), Ok(LspAnswer::Locations(Vec::new())));
    assert!(f.log().contains("$/cancelRequest"));
    assert!(!f.saw(|e| matches!(e, Event::Lsp(LspEvent::Answered { id: 1, .. }))));
}

#[test]
fn hover_comes_back_in_every_shape_the_protocol_allows() {
    let f = start("normal", WAIT);
    f.ready();
    let ask = |id: u64, line| {
        f.handle()
            .query(id, file(), LspQuery::Hover { at: (line, 0) });
        let event =
            f.wait_for(|e| matches!(e, Event::Lsp(LspEvent::Answered { id: i, .. }) if *i == id));
        let Event::Lsp(LspEvent::Answered { result, .. }) = event else {
            unreachable!();
        };
        result
    };
    let text = |s: &str| Ok(LspAnswer::Hover(Some(s.to_owned())));
    assert_eq!(
        ask(1, 0),
        text("```rust\nfn fake()\n```\n\n---\n\nFake docs")
    );
    assert_eq!(ask(2, 1), text("```rust\nfn old()\n```\n\nOld docs"));
    assert_eq!(ask(3, 2), Ok(LspAnswer::Hover(None)));
    assert_eq!(ask(4, 3), text("Plain docs"));
    assert_eq!(ask(5, 4), Err("fake failure".to_owned()));
}

#[test]
fn completions_come_back_with_the_protocols_defaults_filled_in() {
    let f = start("normal", WAIT);
    let Event::Lsp(LspEvent::Ready {
        completion_triggers,
        ..
    }) = f.wait_for(|e| matches!(e, Event::Lsp(LspEvent::Ready { .. })))
    else {
        unreachable!();
    };
    assert_eq!(completion_triggers, [".", ":"]);
    let ask = |id: u64, line| {
        f.handle()
            .query(id, file(), LspQuery::Completion { at: (line, 2) });
        let event =
            f.wait_for(|e| matches!(e, Event::Lsp(LspEvent::Answered { id: i, .. }) if *i == id));
        let Event::Lsp(LspEvent::Answered { result, .. }) = event else {
            unreachable!();
        };
        match result {
            Ok(LspAnswer::Completion { items, incomplete }) => Ok((items, incomplete)),
            Ok(other) => panic!("{other:?}"),
            Err(e) => Err(e),
        }
    };
    let (items, incomplete) = ask(1, 0).unwrap();
    assert!(!incomplete);
    let labels: Vec<_> = items.iter().map(|i| i.label.as_str()).collect();
    assert_eq!(
        labels,
        ["beta()", "alpha", "alpaca", "snip"],
        "an item that cannot be read costs itself, not the list"
    );
    assert_eq!(
        items[3].insert, "snip(x, )",
        "the plain text of the snippet"
    );
    assert_eq!(items[0].insert, "beta");
    assert_eq!(items[0].start, Some((0, 2)));
    assert_eq!(items[1].detail.as_deref(), Some("fn alpha()"));

    let (items, incomplete) = ask(2, 1).unwrap();
    assert!(!incomplete);
    assert_eq!(items[0].insert, "plain", "the label is the default text");
    assert_eq!(items[0].start, None);
    assert_eq!(items[1].insert, "inserted");

    assert_eq!(ask(3, 2), Ok((Vec::new(), false)));
    assert!(ask(4, 3).unwrap().1, "an incomplete list says so");
    assert_eq!(ask(5, 4), Err("fake failure".to_owned()));
}

fn write_script(dir: &Path, name: &str, body: &str) -> String {
    use std::os::unix::fs::PermissionsExt;
    let path = dir.join(name);
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
    path.display().to_string()
}

fn start_config(dir: tempfile::TempDir, config: LspConfig) -> Fixture {
    let (tx, events) = channel();
    let handle = LspHandle::start(config, move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    Fixture {
        dir,
        events,
        handle: Some(handle),
    }
}

fn fake_body(dir: &Path) -> String {
    format!(
        "FAKE_LSP_LOG='{}' exec '{}' \"$@\"",
        dir.join("log").display(),
        fake_server()
    )
}

#[test]
fn a_proxy_without_its_component_counts_as_not_found_until_it_is_there() {
    let dir = tempfile::tempdir().unwrap();
    let marker = dir.path().join("installed");
    let body = format!(
        "[ -f '{}' ] || exit 1\n{}",
        marker.display(),
        fake_body(dir.path())
    );
    let server = write_script(dir.path(), "proxy", &body);
    let mut config = LspConfig::new(server, dir.path().to_path_buf());
    config.check_first = true;
    let f = start_config(dir, config);
    f.state(LspState::NotFound);
    assert!(!f.log().contains("initialize"));
    std::fs::write(&marker, "").unwrap();
    f.handle().restart();
    f.ready();
}

#[test]
fn the_bundled_server_is_unpacked_when_none_is_installed() {
    let dir = tempfile::tempdir().unwrap();
    let body = format!("#!/bin/sh\n{}\n", fake_body(dir.path()));
    let compressed = zstd::encode_all(body.as_bytes(), 1).unwrap();
    let unpack = dir.path().join("lsp");
    let mut config = LspConfig::new("no-such-server".to_owned(), dir.path().to_path_buf());
    config.check_first = true;
    config.bundled = Some(Bundled {
        compressed: Box::leak(compressed.into_boxed_slice()),
        dir: unpack.clone(),
    });
    let f = start_config(dir, config);
    f.ready();
    assert_eq!(std::fs::read_dir(&unpack).unwrap().count(), 1);
    assert!(f.log().contains("initialize"));
}

#[test]
fn an_installed_server_wins_over_the_bundled_one() {
    let dir = tempfile::tempdir().unwrap();
    let command = write_script(dir.path(), "installed", &fake_body(dir.path()));
    let compressed = zstd::encode_all(&b"#!/bin/sh\nexit 1\n"[..], 1).unwrap();
    let unpack = dir.path().join("lsp");
    let mut config = LspConfig::new(command, dir.path().to_path_buf());
    config.check_first = true;
    config.bundled = Some(Bundled {
        compressed: Box::leak(compressed.into_boxed_slice()),
        dir: unpack.clone(),
    });
    let f = start_config(dir, config);
    f.ready();
    assert!(!unpack.exists(), "nothing was unpacked");
}

#[test]
fn a_bundled_server_that_cannot_run_ends_as_not_found() {
    let dir = tempfile::tempdir().unwrap();
    let compressed = zstd::encode_all(&b"#!/bin/sh\nexit 1\n"[..], 1).unwrap();
    let mut config = LspConfig::new("no-such-server".to_owned(), dir.path().to_path_buf());
    config.check_first = true;
    config.bundled = Some(Bundled {
        compressed: Box::leak(compressed.into_boxed_slice()),
        dir: dir.path().join("lsp"),
    });
    let f = start_config(dir, config);
    f.state(LspState::NotFound);
}

struct Machine {
    dir: tempfile::TempDir,
    download: Download,
}

impl Machine {
    fn new(rustup_works: bool, releases: &[&str]) -> Self {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let rustc = write_script(
            root,
            "rustc",
            "echo 'commit-date: 2025-12-08'\necho 'host: test-host'",
        );
        let marker = root.join("component");
        let rustup = write_script(
            root,
            "rustup",
            &format!(
                "echo \"$*\" >> '{}'\n{}",
                root.join("rustup.log").display(),
                if rustup_works {
                    format!("touch '{}'", marker.display())
                } else {
                    "exit 1".to_owned()
                }
            ),
        );
        let site = root.join("site");
        std::fs::create_dir(&site).unwrap();
        let served = site.join("server");
        std::fs::write(&served, format!("#!/bin/sh\n{}\n", fake_body(root))).unwrap();
        assert!(
            std::process::Command::new("gzip")
                .arg(&served)
                .status()
                .unwrap()
                .success()
        );
        let wanted = releases
            .iter()
            .map(|t| format!("*/{t}/rust-analyzer-test-host.gz) ;;"))
            .collect::<Vec<_>>()
            .join("\n  ");
        let curl = write_script(
            root,
            "curl",
            &format!(
                "for a; do [ \"$prev\" = --output ] && out=$a; prev=$a; url=$a; done\n\
                 echo \"$url\" >> '{}'\n\
                 case \"$url\" in\n  {wanted}\n  *) exit 22 ;;\nesac\n\
                 cp '{}.gz' \"$out\"",
                root.join("curl.log").display(),
                served.display()
            ),
        );
        let mut download = Download::new(root.join("downloaded"));
        download.rustc = rustc;
        download.rustup = rustup;
        download.curl = curl;
        download.releases = "https://example.test".to_owned();
        Self { dir, download }
    }

    fn proxy(&self) -> String {
        let root = self.dir.path();
        let body = format!(
            "[ -f '{}' ] || exit 1\n{}",
            root.join("component").display(),
            fake_body(root)
        );
        write_script(root, "proxy", &body)
    }

    fn config(&self) -> LspConfig {
        let mut config = LspConfig::new(self.proxy(), self.dir.path().to_path_buf());
        config.check_first = true;
        config.download = Some(self.download.clone());
        config
    }

    fn logged(&self, name: &str) -> String {
        std::fs::read_to_string(self.dir.path().join(name)).unwrap_or_default()
    }
}

fn running(config: LspConfig) -> Fixture {
    let dir = tempfile::tempdir().unwrap();
    start_config(dir, config)
}

#[test]
fn rustup_is_tried_first_because_it_matches_exactly() {
    let m = Machine::new(true, &["2025-12-08"]);
    let f = running(m.config());
    f.state(LspState::Installing);
    f.ready();
    assert_eq!(
        m.logged("rustup.log").trim(),
        "component add rust-analyzer rust-src"
    );
    assert_eq!(m.logged("curl.log"), "", "nothing was downloaded");
}

#[test]
fn without_rustup_the_release_of_the_toolchains_week_is_downloaded() {
    let m = Machine::new(false, &["2025-12-08"]);
    let f = running(m.config());
    f.state(LspState::Installing);
    f.ready();
    assert_eq!(
        m.logged("curl.log").trim(),
        "https://example.test/2025-12-08/rust-analyzer-test-host.gz"
    );
    assert!(
        m.dir
            .path()
            .join("downloaded/2025-12-08/rust-analyzer")
            .is_file()
    );
}

#[test]
fn a_skipped_week_moves_on_to_the_next_release() {
    let m = Machine::new(false, &["2025-12-22"]);
    let f = running(m.config());
    f.ready();
    let tried: Vec<String> = m.logged("curl.log").lines().map(str::to_owned).collect();
    assert_eq!(tried.len(), 3, "{tried:?}");
    assert!(tried[2].contains("/2025-12-22/"));
    assert!(
        !m.dir.path().join("downloaded/2025-12-08").exists(),
        "no empty folders"
    );
}

#[test]
fn a_second_run_uses_the_earlier_download() {
    let m = Machine::new(false, &["2025-12-08"]);
    drop(running(m.config()).wait_ready());
    let before = m.logged("curl.log");
    let f = running(m.config());
    f.ready();
    assert_eq!(m.logged("curl.log"), before);
    assert_eq!(
        m.logged("rustup.log").lines().count(),
        1,
        "rustup is not asked again either"
    );
}

#[test]
fn nothing_to_download_falls_back_on_the_bundled_server() {
    let m = Machine::new(false, &[]);
    let body = format!("#!/bin/sh\n{}\n", fake_body(m.dir.path()));
    let mut config = m.config();
    config.bundled = Some(Bundled {
        compressed: Box::leak(
            zstd::encode_all(body.as_bytes(), 1)
                .unwrap()
                .into_boxed_slice(),
        ),
        dir: m.dir.path().join("unpacked"),
    });
    let f = running(config);
    f.state(LspState::Installing);
    f.ready();
    assert!(m.dir.path().join("unpacked").exists());
}

#[test]
fn with_nothing_at_all_the_state_is_not_found_and_restart_tries_again() {
    let m = Machine::new(false, &[]);
    let f = running(m.config());
    f.state(LspState::NotFound);
    let attempts = m.logged("rustup.log").lines().count();
    f.handle().restart();
    f.state(LspState::NotFound);
    assert_eq!(m.logged("rustup.log").lines().count(), attempts + 1);
}

#[test]
fn a_download_that_does_not_run_is_not_kept() {
    let m = Machine::new(false, &["2025-12-08"]);
    let broken = m.dir.path().join("site/server.gz");
    std::fs::write(m.dir.path().join("site/server"), "#!/bin/sh\nexit 1\n").unwrap();
    std::fs::remove_file(&broken).unwrap();
    assert!(
        std::process::Command::new("gzip")
            .arg(m.dir.path().join("site/server"))
            .status()
            .unwrap()
            .success()
    );
    let f = running(m.config());
    f.state(LspState::NotFound);
    assert!(!m.dir.path().join("downloaded/2025-12-08").exists());
}

#[test]
fn quitting_during_a_download_does_not_wait_for_it() {
    let m = Machine::new(false, &["2025-12-08"]);
    let mut download = m.download.clone();
    download.rustup = write_script(m.dir.path(), "slow", "sleep 60");
    let mut config = m.config();
    config.download = Some(download);
    let mut f = running(config);
    f.state(LspState::Installing);
    let started = Instant::now();
    drop(f.handle.take());
    assert!(started.elapsed() < Duration::from_secs(3));
}

fn pinned(m: &Machine, tag: &str) -> LspConfig {
    let mut download = m.download.clone();
    download.pinned = Some(tag.to_owned());
    download.host = "test-host".to_owned();
    let mut config = LspConfig::new(String::new(), m.dir.path().to_path_buf());
    config.check_first = true;
    config.download = Some(download);
    config
}

#[test]
fn a_chosen_release_is_downloaded_even_when_rustup_could_serve() {
    let m = Machine::new(true, &["2024-07-22"]);
    let f = running(pinned(&m, "2024-07-22"));
    f.state(LspState::Installing);
    f.ready();
    assert_eq!(m.logged("rustup.log"), "");
    assert_eq!(
        m.logged("curl.log").trim(),
        "https://example.test/2024-07-22/rust-analyzer-test-host.gz"
    );
    drop(f);
    let f = running(pinned(&m, "2024-07-22"));
    f.ready();
    assert_eq!(
        m.logged("curl.log").lines().count(),
        1,
        "kept for next time"
    );
}

#[test]
fn a_chosen_release_that_does_not_exist_is_not_replaced_by_another() {
    let m = Machine::new(true, &["2025-12-08"]);
    let f = running(pinned(&m, "1999-01-04"));
    f.state(LspState::NotFound);
    assert_eq!(m.logged("curl.log").lines().count(), 1);
    assert_eq!(m.logged("rustup.log"), "");
}

#[test]
fn references_come_with_the_lines_they_are_on() {
    let f = start("normal", WAIT);
    f.ready();
    let path = f.dir.path().join("a.rs");
    std::fs::write(&path, "zero\none\ntwo\n").unwrap();
    let ask = |id: u64, line| {
        f.handle()
            .query(id, path.clone(), LspQuery::References { at: (line, 0) });
        let event =
            f.wait_for(|e| matches!(e, Event::Lsp(LspEvent::Answered { id: i, .. }) if *i == id));
        let Event::Lsp(LspEvent::Answered { result, .. }) = event else {
            unreachable!();
        };
        result
    };
    let Ok(LspAnswer::References(usages)) = ask(1, 0) else {
        panic!("no references");
    };
    let lines: Vec<_> = usages.iter().map(|u| u.disk_line.as_str()).collect();
    assert_eq!(lines, ["two", "one"]);
    assert_eq!(usages[0].location.start, (2, 1));
    assert_eq!(ask(2, 2), Ok(LspAnswer::References(Vec::new())));
}

#[test]
fn formatting_comes_back_as_edits_nothing_or_a_failure() {
    let f = start("normal", WAIT);
    f.ready();
    let ask = |id: u64, name: &str| {
        f.handle().query(
            id,
            PathBuf::from(name),
            LspQuery::Formatting { tab_size: 4 },
        );
        let event =
            f.wait_for(|e| matches!(e, Event::Lsp(LspEvent::Answered { id: i, .. }) if *i == id));
        let Event::Lsp(LspEvent::Answered { result, .. }) = event else {
            unreachable!();
        };
        result
    };
    let Ok(LspAnswer::Edits(edits)) = ask(1, "/fake/a.rs") else {
        panic!("no edits");
    };
    assert_eq!(edits[0].text, "// formatted\n");
    assert_eq!(ask(2, "/fake/clean.rs"), Ok(LspAnswer::Edits(Vec::new())));
    assert_eq!(ask(3, "/fake/bad.rs"), Err("rustfmt failed".to_owned()));
}

#[test]
fn code_actions_reach_the_server_and_come_back_with_their_edits() {
    let f = start("normal", WAIT);
    f.ready();
    let ask = |id: u64, line| {
        f.handle().query(
            id,
            file(),
            LspQuery::CodeActions {
                at: (line, 0),
                to: (line, 0),
            },
        );
        let event =
            f.wait_for(|e| matches!(e, Event::Lsp(LspEvent::Answered { id: i, .. }) if *i == id));
        let Event::Lsp(LspEvent::Answered { result, .. }) = event else {
            unreachable!();
        };
        result
    };
    let Ok(LspAnswer::CodeActions(actions)) = ask(1, 0) else {
        panic!("no actions");
    };
    assert_eq!(actions[0].title, "Fake fix");
    assert_eq!(actions[0].changes[0].path, file());
    assert_eq!(actions[0].changes[0].edits[0].text, "// fixed\n");
    assert_eq!(ask(2, 2), Ok(LspAnswer::CodeActions(Vec::new())));
}

fn candidate(command: &str, args: &[&str]) -> ServerCommand {
    ServerCommand {
        command: command.to_owned(),
        args: args.iter().map(|a| (*a).to_owned()).collect(),
        initialization: None,
    }
}

fn candidates_config(dir: &Path, bin: &Path, candidates: Vec<ServerCommand>) -> LspConfig {
    let mut config = LspConfig::new(String::new(), dir.to_path_buf());
    config.server = Language::Python;
    config.candidates = candidates;
    config.search_path = Some(bin.as_os_str().to_owned());
    config.backoff = Duration::from_millis(10);
    config
}

fn version_shy_server(bin: &Path, name: &str, log_dir: &Path) {
    let body = format!(
        "if [ \"$1\" = --version ]; then echo asked > '{}'; sleep 3600; fi\n{}",
        log_dir.join("version-asked").display(),
        fake_body(log_dir),
    );
    write_script(bin, name, &body);
}

#[test]
fn a_server_is_found_on_path_without_being_run_and_gets_its_arguments() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    version_shy_server(&bin, "second", dir.path());
    let config = candidates_config(
        dir.path(),
        &bin,
        vec![candidate("first", &[]), candidate("second", &["--stdio"])],
    );
    let root = dir.path().to_path_buf();
    let f = start_config(dir, config);
    let named = f.wait_for(|e| matches!(e, Event::Lsp(LspEvent::ServerNamed { .. })));
    let Event::Lsp(LspEvent::ServerNamed { server, name }) = named else {
        unreachable!();
    };
    assert_eq!(name, "second", "the second candidate is what runs");
    assert_eq!(server.server, Language::Python);
    assert_eq!(server.root, root);
    f.ready();
    assert!(!f.dir.path().join("version-asked").exists());
}

#[test]
fn a_missing_server_is_named_and_found_after_installing_and_restarting() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let config = candidates_config(dir.path(), &bin, vec![candidate("first", &[])]);
    let f = start_config(dir, config);
    let named = f.wait_for(|e| matches!(e, Event::Lsp(LspEvent::ServerNamed { .. })));
    assert!(matches!(named, Event::Lsp(LspEvent::ServerNamed { name, .. }) if name == "first"));
    f.state(LspState::NotFound);
    assert!(!f.saw(|e| matches!(
        e,
        Event::Lsp(LspEvent::StateChanged {
            state: LspState::Crashed,
            ..
        })
    )));
    version_shy_server(&f.dir.path().join("bin"), "first", f.dir.path());
    f.handle().restart();
    f.ready();
}

#[test]
fn a_relative_or_empty_path_entry_never_finds_a_program() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    let script = write_script(&bin, "tool", "exit 0");
    std::fs::write(bin.join("data"), "not executable").unwrap();
    std::fs::create_dir(bin.join("folder")).unwrap();
    let search = |paths: &[&Path]| Some(std::env::join_paths(paths).unwrap());
    assert_eq!(
        find_on_path("tool", search(&[&bin]).as_ref()),
        Some(PathBuf::from(&script))
    );
    assert_eq!(find_on_path("data", search(&[&bin]).as_ref()), None);
    assert_eq!(find_on_path("folder", search(&[&bin]).as_ref()), None);
    assert_eq!(
        find_on_path("tool", search(&[Path::new("bin"), Path::new("")]).as_ref()),
        None
    );
}

#[test]
fn language_id_and_initialization_options_reach_the_server() {
    let dir = tempfile::tempdir().unwrap();
    let bin = dir.path().join("bin");
    std::fs::create_dir(&bin).unwrap();
    version_shy_server(&bin, "server", dir.path());
    let mut wanted = candidate("server", &[]);
    wanted.initialization = Some(serde_json::json!({"storagePath": "/s"}));
    let config = candidates_config(dir.path(), &bin, vec![wanted]);
    let f = start_config(dir, config);
    f.ready();
    f.handle()
        .did_open(file(), "javascriptreact", 1, "x".into());
    f.wait_for(|e| matches!(e, Event::Lsp(LspEvent::Diagnostics { .. })));
    let log = f.log();
    assert!(log.contains("languageId=javascriptreact"), "{log}");
    assert!(log.contains(r#"options={"storagePath":"/s"}"#), "{log}");
}

#[test]
fn a_server_that_asks_before_it_answers_initialize_still_starts() {
    let f = start("asks-first", WAIT);
    f.ready();
}

fn encoding_server(dir: &Path, encoding: &str) -> LspConfig {
    let mut config = LspConfig::new("env".to_owned(), dir.to_path_buf());
    config.args = vec![
        format!("FAKE_LSP_ENCODING={encoding}"),
        format!("FAKE_LSP_LOG={}", dir.join("log").display()),
        fake_server(),
    ];
    config
}

#[test]
fn the_encoding_a_server_picks_or_does_not_pick_is_what_the_core_hears() {
    for (picked, expected) in [
        ("utf-8", PositionEncoding::Utf8),
        ("utf-16", PositionEncoding::Utf16),
        ("none", PositionEncoding::Utf16),
    ] {
        let dir = tempfile::tempdir().unwrap();
        let config = encoding_server(dir.path(), picked);
        let f = start_config(dir, config);
        let ready = f.wait_for(|e| matches!(e, Event::Lsp(LspEvent::Ready { .. })));
        let Event::Lsp(LspEvent::Ready { encoding, .. }) = ready else {
            unreachable!();
        };
        assert_eq!(encoding, expected, "{picked}");
    }
}

fn services_with(
    dir: &Path,
    modes: &[(Language, &str)],
) -> (
    Services<impl Fn(Event) + Clone + Send + 'static>,
    Receiver<Event>,
) {
    let mut config = Config::default();
    for (language, mode) in modes {
        let log = dir.join(format!("{language:?}.log"));
        config.servers.insert(
            *language,
            ServerSettings {
                command: Some("env".to_owned()),
                args: Some(vec![
                    format!("FAKE_LSP_MODE={mode}"),
                    format!("FAKE_LSP_LOG={}", log.display()),
                    fake_server(),
                ]),
                enabled: true,
            },
        );
    }
    let (tx, rx) = channel();
    let services = Services::new(
        move |e| {
            let _ = tx.send(e);
        },
        None,
        &config,
    );
    (services, rx)
}

fn wait_event(rx: &Receiver<Event>, wanted: impl Fn(&Event) -> bool) -> Event {
    let deadline = Instant::now() + WAIT;
    loop {
        let left = deadline.saturating_duration_since(Instant::now());
        let event = rx.recv_timeout(left).expect("event did not arrive");
        if wanted(&event) {
            return event;
        }
    }
}

fn is_ready(event: &Event, key: &ServerKey) -> bool {
    matches!(event, Event::Lsp(LspEvent::Ready { server, .. }) if server == key)
}

#[test]
fn sessions_are_kept_apart_and_a_crash_concerns_only_its_own() {
    let dir = tempfile::tempdir().unwrap();
    let key = |server| ServerKey {
        server,
        root: dir.path().to_path_buf(),
    };
    let (go, python) = (key(Language::Go), key(Language::Python));
    let (mut services, rx) = services_with(
        dir.path(),
        &[
            (Language::Go, "crash-on-open"),
            (Language::Python, "normal"),
        ],
    );
    services.run(Effect::Lsp(LspEffect::Start { server: go.clone() }));
    services.run(Effect::Lsp(LspEffect::Start {
        server: python.clone(),
    }));
    let mut waiting = vec![go.clone(), python.clone()];
    while !waiting.is_empty() {
        let event = wait_event(&rx, |e| matches!(e, Event::Lsp(LspEvent::Ready { .. })));
        waiting.retain(|key| !is_ready(&event, key));
    }

    let open = |server: &ServerKey, language, name: &str| {
        Effect::Lsp(LspEffect::DidOpen {
            server: server.clone(),
            language,
            path: PathBuf::from(name),
            version: 1,
            text: "x".to_owned(),
        })
    };
    services.run(open(&go, Language::Go, "/fake/a.go"));
    let crashed = wait_event(&rx, |e| {
        matches!(
            e,
            Event::Lsp(LspEvent::StateChanged {
                state: LspState::Crashed,
                ..
            })
        )
    });
    let Event::Lsp(LspEvent::StateChanged { server, .. }) = crashed else {
        unreachable!();
    };
    assert_eq!(server, go);

    services.run(open(&python, Language::Python, "/fake/b.py"));
    let diagnosed = wait_event(
        &rx,
        |e| matches!(e, Event::Lsp(LspEvent::Diagnostics { server, .. }) if *server == python),
    );
    let Event::Lsp(LspEvent::Diagnostics { path, .. }) = diagnosed else {
        unreachable!();
    };
    assert_eq!(path, Path::new("/fake/b.py"));
    let python_log = std::fs::read_to_string(dir.path().join("Python.log")).unwrap();
    assert!(python_log.contains("languageId=python"), "{python_log}");
    assert!(!python_log.contains("a.go"), "{python_log}");
    let go_log = std::fs::read_to_string(dir.path().join("Go.log")).unwrap();
    assert!(go_log.contains("languageId=go"), "{go_log}");
}

#[test]
fn a_disabled_server_never_starts_and_says_so() {
    let mut config = Config::default();
    config.servers.insert(
        Language::Go,
        ServerSettings {
            enabled: false,
            ..ServerSettings::default()
        },
    );
    let (tx, _rx) = channel();
    let mut services = Services::new(
        move |e| {
            let _ = tx.send(e);
        },
        None,
        &config,
    );
    let server = ServerKey {
        server: Language::Go,
        root: PathBuf::from("/fake"),
    };
    let answer = services.run(Effect::Lsp(LspEffect::Start {
        server: server.clone(),
    }));
    assert_eq!(
        answer.map(|e| format!("{e:?}")),
        Some(format!(
            "{:?}",
            Event::Lsp(LspEvent::StateChanged {
                server,
                state: LspState::Disabled
            })
        ))
    );
}

#[test]
fn quitting_with_hung_servers_takes_a_bounded_total_time() {
    let dir = tempfile::tempdir().unwrap();
    let languages = [
        Language::Go,
        Language::Python,
        Language::Php,
        Language::Java,
        Language::Bash,
    ];
    let modes: Vec<_> = languages.iter().map(|l| (*l, "ignore-shutdown")).collect();
    let (mut services, rx) = services_with(dir.path(), &modes);
    let keys: Vec<_> = languages
        .iter()
        .map(|server| ServerKey {
            server: *server,
            root: dir.path().to_path_buf(),
        })
        .collect();
    for server in &keys {
        services.run(Effect::Lsp(LspEffect::Start {
            server: server.clone(),
        }));
    }
    let mut ready = 0;
    while ready < keys.len() {
        if matches!(
            wait_event(&rx, |_| true),
            Event::Lsp(LspEvent::Ready { .. })
        ) {
            ready += 1;
        }
    }
    let logs: Vec<_> = languages
        .iter()
        .map(|l| dir.path().join(format!("{l:?}.log")))
        .collect();
    let started = Instant::now();
    drop(services);
    let took = started.elapsed();
    assert!(took < Duration::from_secs(5), "took {took:?}");
    for log in logs {
        let deadline = Instant::now() + Duration::from_secs(5);
        while !std::fs::read_to_string(&log).unwrap().contains("shutdown") {
            assert!(Instant::now() < deadline, "not asked: {}", log.display());
            std::thread::sleep(Duration::from_millis(10));
        }
        let text = std::fs::read_to_string(&log).unwrap();
        let pid = text.lines().find_map(|l| l.strip_prefix("pid=")).unwrap();
        while std::process::Command::new("kill")
            .args(["-0", pid])
            .stderr(std::process::Stdio::null())
            .status()
            .unwrap()
            .success()
        {
            assert!(Instant::now() < deadline, "still running: {pid}");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

fn ask_until(
    f: &Fixture,
    path: &Path,
    query: &LspQuery,
    done: impl Fn(&LspAnswer) -> bool,
) -> LspAnswer {
    let deadline = Instant::now() + Duration::from_secs(120);
    let mut id = 1000;
    loop {
        id += 1;
        f.handle().query(id, path.to_path_buf(), query.clone());
        let event =
            f.wait_for(|e| matches!(e, Event::Lsp(LspEvent::Answered { id: i, .. }) if *i == id));
        let Event::Lsp(LspEvent::Answered { result, .. }) = event else {
            unreachable!();
        };
        match result {
            Ok(answer) if done(&answer) => return answer,
            last => assert!(Instant::now() < deadline, "gave up, last answer: {last:?}"),
        }
        std::thread::sleep(Duration::from_millis(500));
    }
}

#[test]
#[ignore = "needs rust-analyzer on PATH"]
fn real_rust_analyzer_renames_across_files_and_extracts_a_selection() {
    let dir = tempfile::tempdir().unwrap();
    let write = |name: &str, text: &str| {
        let path = dir.path().join(name);
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(&path, text).unwrap();
        path
    };
    write(
        "Cargo.toml",
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    );
    let main_text =
        "mod util;\nfn main() {\n    let n = util::helper() + 1 + 2;\n    println!(\"{n}\");\n}\n";
    let main = write("src/main.rs", main_text);
    let util_text = "pub fn helper() -> i32 {\n    3\n}\n";
    let util = write("src/util.rs", util_text);

    let mut config = LspConfig::new("rust-analyzer".to_owned(), dir.path().to_path_buf());
    config.query_timeout = Duration::from_secs(60);
    let (tx, events) = channel();
    let handle = LspHandle::start(config, move |e| {
        let _ = tx.send(e);
    })
    .unwrap();
    let f = Fixture {
        dir,
        events,
        handle: Some(handle),
    };
    f.ready();
    f.handle()
        .did_open(util.clone(), "rust", 1, util_text.to_owned());
    f.handle()
        .did_open(main.clone(), "rust", 1, main_text.to_owned());

    let rename = LspQuery::Rename {
        at: (0, 8),
        new_name: "assist".to_owned(),
    };
    let answer = ask_until(
        &f,
        &util,
        &rename,
        |a| matches!(a, LspAnswer::Rename { changes: Some(c), .. } if c.len() == 2),
    );
    let LspAnswer::Rename {
        changes: Some(changes),
        places,
    } = answer
    else {
        unreachable!();
    };
    assert_eq!(places.len(), 2, "{changes:?}");
    assert!(changes.iter().all(|c| c.edits[0].text == "assist"));

    let extract = LspQuery::CodeActions {
        at: (2, 12),
        to: (2, 34),
    };
    let wanted = |a: &LspAnswer, title: &str| matches!(a, LspAnswer::CodeActions(list) if list.iter().any(|x| x.title.contains(title)));
    ask_until(&f, &main, &extract, |a| wanted(a, "Extract into variable"));
    ask_until(&f, &main, &extract, |a| wanted(a, "Extract into function"));
    let refactor = LspQuery::Refactorings {
        at: (2, 12),
        to: (2, 34),
    };
    let refactorings = ask_until(
        &f,
        &main,
        &refactor,
        |a| matches!(a, LspAnswer::Refactorings(list) if list.iter().any(|o| o.action.title.contains("Extract into variable"))),
    );
    let LspAnswer::Refactorings(list) = refactorings else {
        unreachable!();
    };
    assert!(list.iter().all(|o| !o.places.is_empty()), "{list:?}");
    let usages = LspQuery::SafeDeleteUsages { at: (0, 8) };
    ask_until(
        &f,
        &util,
        &usages,
        |a| matches!(a, LspAnswer::SafeDeleteUsages(list) if list.len() == 2),
    );
}

#[test]
fn a_failed_start_says_how_it_ended_and_what_the_server_printed() {
    let dir = tempfile::tempdir().unwrap();
    let script = write_script(
        dir.path(),
        "dies",
        "echo first >&2\necho boom >&2\nexit 3\n",
    );
    let mut config = LspConfig::new(script, dir.path().to_path_buf());
    config.backoff = Duration::from_secs(30);
    let f = start_config(dir, config);
    let failed = f.wait_for(|e| matches!(e, Event::Lsp(LspEvent::Failed { .. })));
    let Event::Lsp(LspEvent::Failed { reason, .. }) = failed else {
        unreachable!();
    };
    assert!(reason.contains("code 3"), "{reason}");
    assert!(reason.contains("boom"), "{reason}");
}
