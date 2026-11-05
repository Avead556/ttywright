#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::{Path, PathBuf};
use std::time::{Duration, Instant};

use ttywright::core::{Action, Command, PluginContext};
use ttywright::services::plugin::{self, Helper, Limits, LoadedPlugin};

const ALLOC: &str = r#"
  (memory (export "memory") 2)
  (global $next (mut i32) (i32.const 4096))
  (func (export "alloc") (param i32) (result i32)
    (local $at i32)
    (local.set $at (global.get $next))
    (global.set $next (i32.add (local.get $at) (local.get 0)))
    (local.get $at))
"#;

fn roomy() -> String {
    ALLOC.replace(
        r#"(memory (export "memory") 2)"#,
        r#"(memory (export "memory") 64)"#,
    )
}

fn plugin_host() -> Helper {
    let exe = std::env::current_exe().unwrap();
    let target = exe.parent().unwrap().parent().unwrap();
    Helper {
        program: target.join("examples/plugin_host"),
        args: Vec::new(),
    }
}

struct Setup {
    dir: tempfile::TempDir,
}

impl Setup {
    fn new() -> Self {
        Self {
            dir: tempfile::tempdir().unwrap(),
        }
    }

    fn plugins(&self) -> PathBuf {
        self.dir.path().join("plugins")
    }

    fn install(&self, name: &str, wat: &str) {
        let dir = self.plugins().join(name);
        std::fs::create_dir_all(&dir).unwrap();
        let manifest = format!("name = \"{name}\"\n\n[[commands]]\nid = \"go\"\ntitle = \"Go\"\n");
        std::fs::write(dir.join("plugin.toml"), manifest).unwrap();
        std::fs::write(dir.join("plugin.wasm"), wat::parse_str(wat).unwrap()).unwrap();
    }

    fn only(&self) -> LoadedPlugin {
        let (mut loaded, problems) = plugin::load(&self.plugins());
        assert_eq!(problems, Vec::<String>::new());
        assert_eq!(loaded.len(), 1);
        loaded.remove(0).with_helper(plugin_host())
    }

    fn root(&self) -> PathBuf {
        let root = self.dir.path().join("project");
        std::fs::create_dir_all(&root).unwrap();
        root
    }
}

fn run(plugin: &LoadedPlugin, root: &Path) -> Result<ttywright::core::PluginReport, String> {
    plugin::run(
        plugin,
        "go",
        root,
        PluginContext::default(),
        Limits::default(),
        None,
    )
}

fn answering(json: &str) -> String {
    let packed = (16_i64 << 32) | json.len() as i64;
    let data = json.replace('"', "\\\"");
    format!(
        r#"(module {ALLOC}
          (data (i32.const 16) "{data}")
          (func (export "handle") (param i32 i32) (result i64) (i64.const {packed})))"#
    )
}

const INSERT: &str = r#"{"commands":[{"Insert":"hi"}],"status":"done"}"#;

#[test]
fn the_shipped_example_loads_and_says_hello() {
    let s = Setup::new();
    let dir = s.plugins().join("hello");
    std::fs::create_dir_all(&dir).unwrap();
    let source = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/plugins/hello");
    std::fs::copy(source.join("plugin.toml"), dir.join("plugin.toml")).unwrap();
    std::fs::write(
        dir.join("plugin.wasm"),
        wat::parse_file(source.join("plugin.wat")).unwrap(),
    )
    .unwrap();
    let plugin = s.only();
    assert_eq!(plugin.info.commands[0].title, "Say Hello");
    let report = plugin::run(
        &plugin,
        "say",
        &s.root(),
        PluginContext::default(),
        Limits::default(),
        None,
    )
    .unwrap();
    assert_eq!(
        report.commands,
        vec![Command::Insert("hello from a plugin".to_owned())]
    );
    assert_eq!(report.status.as_deref(), Some("Said hello"));
}

#[test]
fn a_loop_is_stopped_and_reported() {
    let s = Setup::new();
    s.install(
        "p",
        &format!(
            r#"(module {ALLOC}
              (func (export "handle") (param i32 i32) (result i64)
                (loop $l (br $l)) (i64.const 0)))"#
        ),
    );
    let started = Instant::now();
    let error = run(&s.only(), &s.root()).unwrap_err();
    assert!(error.contains("too long"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(30));
}

#[test]
fn a_crash_is_an_error_not_a_crash_of_ours() {
    let s = Setup::new();
    s.install(
        "p",
        &format!(
            r#"(module {ALLOC}
              (func (export "handle") (param i32 i32) (result i64) unreachable))"#
        ),
    );
    let error = run(&s.only(), &s.root()).unwrap_err();
    assert!(error.contains("crashed"), "{error}");
}

#[test]
fn runaway_recursion_is_an_error_too() {
    let s = Setup::new();
    s.install(
        "p",
        &format!(
            r#"(module {ALLOC}
              (func $f (export "handle") (param i32 i32) (result i64)
                (call $f (local.get 0) (local.get 1))))"#
        ),
    );
    assert!(run(&s.only(), &s.root()).is_err());
}

#[test]
fn memory_past_the_limit_is_refused() {
    let s = Setup::new();
    s.install(
        "p",
        &format!(
            r#"(module {ALLOC}
              (func (export "handle") (param i32 i32) (result i64)
                (if (i32.eq (memory.grow (i32.const 2048)) (i32.const -1))
                  (then unreachable))
                (i64.const 0)))"#
        ),
    );
    let error = run(&s.only(), &s.root()).unwrap_err();
    assert!(error.contains("crashed"), "{error}");
}

#[test]
fn an_answer_pointing_outside_memory_is_refused() {
    let s = Setup::new();
    let packed = (0x7fff_0000_i64 << 32) | 64;
    s.install(
        "p",
        &format!(
            r#"(module {ALLOC}
              (func (export "handle") (param i32 i32) (result i64) (i64.const {packed})))"#
        ),
    );
    assert!(run(&s.only(), &s.root()).is_err());
}

#[test]
fn asking_for_system_access_fails_to_start_because_none_exists() {
    let s = Setup::new();
    s.install(
        "p",
        &format!(
            r#"(module
              (import "wasi_snapshot_preview1" "proc_exit" (func (param i32)))
              {ALLOC}
              (data (i32.const 16) "{{}}") (func (export "handle") (param i32 i32) (result i64) (i64.const 68719476738)))"#
        ),
    );
    let error = run(&s.only(), &s.root()).unwrap_err();
    assert!(error.contains("proc_exit"), "{error}");
}

fn reader(path: &str) -> String {
    let len = path.len();
    format!(
        r#"(module
          (import "idea" "read_file" (func $read (param i32 i32) (result i64)))
          {ALLOC}
          (data (i32.const 16) "{path}")
          (data (i32.const 1024) "{{\"status\":\"read\"}}")
          (data (i32.const 1100) "{{\"status\":\"no\"}}")
          (func (export "handle") (param i32 i32) (result i64)
            (if (result i64) (i64.ge_s (call $read (i32.const 16) (i32.const {len})) (i64.const 0))
              (then (i64.const {read}))
              (else (i64.const {no})))))"#,
        read = (1024_i64 << 32) | 17,
        no = (1100_i64 << 32) | 15,
    )
}

#[test]
fn any_command_comes_back_as_it_was_sent() {
    let s = Setup::new();
    s.install(
        "p",
        &answering(r#"{"commands":[{"Run":"Quit"},{"Run":"GitPush"},{"Insert":"ok"}]}"#),
    );
    let report = run(&s.only(), &s.root()).unwrap();
    assert_eq!(
        report.commands,
        vec![
            Command::Run(Action::Quit),
            Command::Run(Action::GitPush),
            Command::Insert("ok".to_owned()),
        ]
    );
}

#[test]
fn a_leftover_capabilities_key_is_ignored() {
    let s = Setup::new();
    s.install("p", &answering(INSERT));
    let manifest = s.plugins().join("p/plugin.toml");
    let old = std::fs::read_to_string(&manifest).unwrap();
    std::fs::write(&manifest, format!("capabilities = [\"telepathy\"]\n{old}")).unwrap();
    assert_eq!(s.only().info.commands[0].id, "go");
}

#[test]
fn more_commands_than_the_editor_should_take_at_once_is_an_error() {
    let s = Setup::new();
    let many = vec![r#"\"Newline\""#; 5000].join(",");
    let json = format!(r#"{{\"commands\":[{many}]}}"#);
    let len = json.replace('\\', "").len();
    let far = 1_i64 << 20;
    s.install(
        "p",
        &format!(
            r#"(module {roomy}
              (data (i32.const {far}) "{json}")
              (func (export "handle") (param i32 i32) (result i64) (i64.const {packed})))"#,
            roomy = roomy(),
            packed = (far << 32) | len as i64,
        ),
    );
    let error = run(&s.only(), &s.root()).unwrap_err();
    assert!(error.contains("more than"), "{error}");
}

#[test]
fn the_document_is_always_given() {
    let wat = format!(
        r#"(module {ALLOC}
          (data (i32.const 1024) "{{\"status\":\"long\"}}")
          (data (i32.const 1100) "{{\"status\":\"short\"}}")
          (func (export "handle") (param i32 i32) (result i64)
            (if (result i64) (i32.gt_u (local.get 1) (i32.const 1000))
              (then (i64.const {long}))
              (else (i64.const {short})))))"#,
        long = (1024_i64 << 32) | 17,
        short = (1100_i64 << 32) | 18,
    );
    let s = Setup::new();
    s.install("p", &wat);
    let context = PluginContext {
        text: Some("x".repeat(5000)),
        ..PluginContext::default()
    };
    let report = plugin::run(&s.only(), "go", &s.root(), context, Limits::default(), None).unwrap();
    assert_eq!(report.status.as_deref(), Some("long"));
}

#[test]
fn any_file_can_be_read_inside_the_project_or_out_and_a_missing_one_is_an_answer() {
    let s = Setup::new();
    let root = s.root();
    std::fs::write(root.join("in.txt"), "x").unwrap();
    let outside = s.dir.path().join("out.txt");
    std::fs::write(&outside, "x").unwrap();
    for (path, status) in [
        ("in.txt", "read"),
        ("../out.txt", "read"),
        (outside.to_str().unwrap(), "read"),
        ("missing.txt", "no"),
    ] {
        let s2 = Setup::new();
        s2.install("p", &reader(path));
        let report = run(&s2.only(), &root).unwrap();
        assert_eq!(report.status.as_deref(), Some(status), "{path}");
    }
}

fn writer(path: &str, data: &str) -> String {
    let (path_len, data_len) = (path.len(), data.len());
    format!(
        r#"(module
          (import "idea" "write_file" (func $write (param i32 i32 i32 i32) (result i64)))
          {ALLOC}
          (data (i32.const 16) "{path}")
          (data (i32.const 512) "{data}")
          (data (i32.const 1024) "{{\"status\":\"wrote\"}}")
          (data (i32.const 1100) "{{\"status\":\"no\"}}")
          (func (export "handle") (param i32 i32) (result i64)
            (if (result i64)
              (i64.eq (call $write (i32.const 16) (i32.const {path_len})
                                   (i32.const 512) (i32.const {data_len})) (i64.const 0))
              (then (i64.const {wrote}))
              (else (i64.const {no})))))"#,
        wrote = (1024_i64 << 32) | 18,
        no = (1100_i64 << 32) | 15,
    )
}

#[test]
fn a_file_can_be_written_anywhere_making_its_folders() {
    let s = Setup::new();
    let root = s.root();
    s.install("p", &writer("deep/er/a.txt", "written"));
    let report = run(&s.only(), &root).unwrap();
    assert_eq!(report.status.as_deref(), Some("wrote"));
    assert_eq!(
        std::fs::read_to_string(root.join("deep/er/a.txt")).unwrap(),
        "written"
    );

    let s = Setup::new();
    let root = s.root();
    let outside = s.dir.path().join("outside.txt");
    s.install("p", &writer(outside.to_str().unwrap(), "out"));
    run(&s.only(), &root).unwrap();
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), "out");

    let s = Setup::new();
    let root = s.root();
    std::fs::write(s.dir.path().join("a-file"), "").unwrap();
    s.install("p", &writer("../a-file/under.txt", "x"));
    let report = run(&s.only(), &root).unwrap();
    assert_eq!(
        report.status.as_deref(),
        Some("no"),
        "a failure is an answer"
    );
}

fn runner(request: &str) -> String {
    let len = request.len();
    let data = request.replace('\\', "\\\\").replace('"', "\\\"");
    let name = "result.json";
    let (roomy, far) = (roomy(), 2 << 20);
    format!(
        r#"(module
          (import "idea" "run" (func $run (param i32 i32) (result i64)))
          (import "idea" "write_file" (func $write (param i32 i32 i32 i32) (result i64)))
          {roomy}
          (data (i32.const 16) "{name}")
          (data (i32.const {far}) "{data}")
          (data (i32.const 3072) "{{\"status\":\"ran\"}}")
          (func (export "handle") (param i32 i32) (result i64)
            (local $r i64)
            (local.set $r (call $run (i32.const {far}) (i32.const {len})))
            (if (i64.lt_s (local.get $r) (i64.const 0)) (then unreachable))
            (drop (call $write
              (i32.const 16) (i32.const {name_len})
              (i32.wrap_i64 (i64.shr_u (local.get $r) (i64.const 32)))
              (i32.wrap_i64 (local.get $r))))
            (i64.const {ran})))"#,
        name_len = name.len(),
        ran = (3072_i64 << 32) | 16,
    )
}

fn run_program(request: &str, limits: Limits) -> (serde_json::Value, Duration) {
    let s = Setup::new();
    let root = s.root();
    s.install("p", &runner(request));
    let started = Instant::now();
    plugin::run(
        &s.only(),
        "go",
        &root,
        PluginContext::default(),
        limits,
        None,
    )
    .unwrap();
    let took = started.elapsed();
    let text = std::fs::read_to_string(root.join("result.json")).unwrap();
    (serde_json::from_str(&text).unwrap(), took)
}

fn short(limits: Limits) -> Limits {
    Limits {
        process_time: Duration::from_millis(500),
        ..limits
    }
}

#[test]
fn a_program_runs_with_arguments_and_input_and_its_output_comes_back() {
    let (result, _) = run_program(
        r#"{"program":"sh","args":["-c","pwd; cat; echo oops >&2; exit 3"],"stdin":"typed"}"#,
        Limits::default(),
    );
    assert_eq!(result["code"], 3);
    let stdout = result["stdout"].as_str().unwrap();
    assert!(
        stdout.contains("project"),
        "the project by default: {stdout}"
    );
    assert!(stdout.ends_with("typed"), "{stdout}");
    assert_eq!(result["stderr"], "oops\n");
    assert_eq!(result["timed_out"], false);
}

#[test]
fn a_program_can_run_in_another_folder() {
    let (result, _) = run_program(r#"{"program":"pwd","cwd":"/"}"#, Limits::default());
    assert_eq!(result["stdout"], "/\n");
}

#[test]
fn a_program_that_cannot_start_is_an_answer() {
    let (result, _) = run_program(r#"{"program":"/no/such/program"}"#, Limits::default());
    assert!(
        result["error"]
            .as_str()
            .unwrap()
            .contains("/no/such/program")
    );
    let (result, _) = run_program(r#"{"nonsense":1}"#, Limits::default());
    assert!(result["error"].as_str().unwrap().contains("bad request"));
}

fn alive(pid: &str) -> bool {
    std::process::Command::new("kill")
        .args(["-0", pid])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success()
}

#[test]
fn a_hung_program_is_killed_with_its_children_at_the_limit() {
    let (result, took) = run_program(
        r#"{"program":"sh","args":["-c","sleep 100 & echo $!; trap '' TERM; wait"]}"#,
        short(Limits::default()),
    );
    assert_eq!(result["timed_out"], true);
    assert!(result["code"].is_null());
    assert!(took < Duration::from_secs(10), "{took:?}");
    let child = result["stdout"].as_str().unwrap().trim().to_owned();
    assert!(!child.is_empty());
    std::thread::sleep(Duration::from_millis(100));
    assert!(!alive(&child), "the child of the program is gone too");
}

#[test]
fn a_program_flooding_its_output_is_stopped_at_the_cap() {
    let (result, took) = run_program(
        r#"{"program":"yes"}"#,
        Limits {
            output_bytes: 100_000,
            ..Limits::default()
        },
    );
    assert_eq!(result["truncated"], true);
    assert_eq!(result["stdout"].as_str().unwrap().len(), 100_000);
    assert!(took < Duration::from_secs(10), "{took:?}");
}

#[test]
fn a_program_full_of_input_it_never_reads_cannot_hang_the_call() {
    let big = "x".repeat(1 << 20);
    let (result, _) = run_program(
        &format!(r#"{{"program":"true","stdin":"{big}"}}"#),
        Limits::default(),
    );
    assert_eq!(result["code"], 0);
}

#[test]
fn a_child_left_running_after_the_program_ends_does_not_hold_the_call() {
    let (result, took) = run_program(
        r#"{"program":"sh","args":["-c","sleep 100 & echo $!"]}"#,
        Limits::default(),
    );
    assert_eq!(result["code"], 0);
    assert!(took < Duration::from_secs(5), "{took:?}");
    let child = result["stdout"].as_str().unwrap().trim().to_owned();
    std::thread::sleep(Duration::from_millis(100));
    assert!(!alive(&child));
}

#[test]
fn all_programs_of_a_call_share_the_time_limit() {
    let double = |request: &str| {
        let len = request.len();
        let data = request.replace('"', "\\\"");
        format!(
            r#"(module
              (import "idea" "run" (func $run (param i32 i32) (result i64)))
              {ALLOC}
              (data (i32.const 64) "{data}")
              (data (i32.const 3072) "{{\"status\":\"ran\"}}")
              (func (export "handle") (param i32 i32) (result i64)
                (drop (call $run (i32.const 64) (i32.const {len})))
                (drop (call $run (i32.const 64) (i32.const {len})))
                (drop (call $run (i32.const 64) (i32.const {len})))
                (i64.const {ran})))"#,
            ran = (3072_i64 << 32) | 16,
        )
    };
    let s = Setup::new();
    s.install("p", &double(r#"{"program":"sleep","args":["30"]}"#));
    let started = Instant::now();
    plugin::run(
        &s.only(),
        "go",
        &s.root(),
        PluginContext::default(),
        short(Limits::default()),
        None,
    )
    .unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[test]
fn a_broken_manifest_only_costs_that_plugin() {
    let s = Setup::new();
    s.install("good", &answering(INSERT));
    let bad = s.plugins().join("bad");
    std::fs::create_dir_all(&bad).unwrap();
    std::fs::write(bad.join("plugin.toml"), "not toml [").unwrap();
    let (loaded, problems) = plugin::load(&s.plugins());
    assert_eq!(loaded.len(), 1);
    assert_eq!(problems.len(), 1);
}

#[test]
fn no_plugin_folder_is_simply_no_plugins() {
    let s = Setup::new();
    let (loaded, problems) = plugin::load(&s.plugins());
    assert!(loaded.is_empty() && problems.is_empty());
}

fn with_helper(helper: Helper) -> Result<ttywright::core::PluginReport, String> {
    let s = Setup::new();
    s.install(
        "fine",
        &format!(r#"(module {ALLOC} (data (i32.const 16) "{{}}") (func (export "handle") (param i32 i32) (result i64) (i64.const 68719476738)))"#),
    );
    let plugin = s.only().with_helper(helper);
    plugin::run(
        &plugin,
        "go",
        &s.root(),
        PluginContext::default(),
        Limits::default(),
        None,
    )
}

fn shell(script: &str) -> Helper {
    Helper {
        program: "/bin/sh".into(),
        args: vec!["-c".to_owned(), script.to_owned()],
    }
}

#[test]
fn a_plugin_process_that_dies_is_only_a_message() {
    for script in ["exit 3", "kill -SEGV $$", "echo garbage"] {
        let error = with_helper(shell(script)).unwrap_err();
        assert!(error.contains("stopped unexpectedly"), "{script}: {error}");
    }
}

#[test]
fn a_plugin_process_that_hangs_is_killed_after_the_limit() {
    let s = Setup::new();
    s.install(
        "fine",
        &format!(r#"(module {ALLOC} (data (i32.const 16) "{{}}") (func (export "handle") (param i32 i32) (result i64) (i64.const 68719476738)))"#),
    );
    let plugin = s.only().with_helper(shell("sleep 100"));
    let started = Instant::now();
    let error = plugin::run(
        &plugin,
        "go",
        &s.root(),
        PluginContext::default(),
        Limits {
            process_time: Duration::from_millis(100),
            process_patience: Duration::from_millis(100),
            ..Limits::default()
        },
        None,
    )
    .unwrap_err();
    assert!(error.contains("stopped unexpectedly"), "{error}");
    assert!(started.elapsed() < Duration::from_secs(20));
}

#[test]
fn a_missing_plugin_process_is_only_a_message() {
    let error = with_helper(Helper {
        program: "/no/such/ttywright-plugins".into(),
        args: Vec::new(),
    })
    .unwrap_err();
    assert!(error.contains("cannot start"), "{error}");
}

#[test]
fn the_process_is_gone_after_the_call_with_nothing_left_running() {
    let s = Setup::new();
    let mark = s.dir.path().join("pid");
    let script = format!(
        "echo $$ > {} ; exec {}",
        mark.display(),
        plugin_host().program.display()
    );
    s.install(
        "fine",
        &format!(r#"(module {ALLOC} (data (i32.const 16) "{{}}") (func (export "handle") (param i32 i32) (result i64) (i64.const 68719476738)))"#),
    );
    let plugin = s.only().with_helper(shell(&script));
    plugin::run(
        &plugin,
        "go",
        &s.root(),
        PluginContext::default(),
        Limits::default(),
        None,
    )
    .unwrap_or_else(|e| panic!("{e}"));
    let pid = std::fs::read_to_string(mark).unwrap();
    let alive = std::process::Command::new("kill")
        .args(["-0", pid.trim()])
        .stderr(std::process::Stdio::null())
        .status()
        .unwrap()
        .success();
    assert!(!alive);
}
