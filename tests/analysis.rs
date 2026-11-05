#![allow(clippy::unwrap_used, clippy::expect_used)]

use std::path::Path;
use std::sync::mpsc::channel;
use std::time::Duration;

use ttywright::core::analysis::{AnalysisOutcome, AnalysisTool, Finding};
use ttywright::services::analysis::{AnalysisHost, on_path, plan};

fn analyze(root: &Path, tool: AnalysisTool) -> Vec<Finding> {
    let planned = plan(root, &on_path);
    let command = planned
        .commands
        .into_iter()
        .find(|c| c.tool == tool)
        .unwrap();
    let host = AnalysisHost::default();
    let (tx, rx) = channel();
    host.start(root, vec![command], move |_, outcome| {
        let _ = tx.send(outcome);
    });
    match rx.recv_timeout(Duration::from_secs(180)).unwrap() {
        AnalysisOutcome::Found(findings) => findings,
        other => panic!("{other:?}"),
    }
}

#[test]
#[ignore = "needs cargo clippy"]
fn real_clippy_finds_a_lint_and_a_compile_error() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::create_dir(root.join("src")).unwrap();
    std::fs::write(
        root.join("Cargo.toml"),
        "[package]\nname = \"demo\"\nversion = \"0.1.0\"\nedition = \"2021\"\n",
    )
    .unwrap();
    let main = root.join("src/main.rs");
    std::fs::write(
        &main,
        "fn main() {\n    let v = vec![1];\n    if v.len() == 0 {}\n}\n",
    )
    .unwrap();
    let found = analyze(&root, AnalysisTool::Clippy);
    assert!(
        found
            .iter()
            .any(|f| f.path == main && f.message.contains("clippy::len_zero")),
        "{found:?}"
    );

    std::fs::write(&main, "fn main() { nope(); }\n").unwrap();
    let found = analyze(&root, AnalysisTool::Clippy);
    assert!(
        found.iter().any(|f| f.message.contains("nope")),
        "{found:?}"
    );
}

#[test]
#[ignore = "needs go and staticcheck"]
fn real_staticcheck_finds_unused_code() {
    let dir = tempfile::tempdir().unwrap();
    let root = dir.path().canonicalize().unwrap();
    std::fs::write(root.join("go.mod"), "module demo\n\ngo 1.21\n").unwrap();
    let main = root.join("main.go");
    std::fs::write(
        &main,
        "package main\n\nfunc unused() {}\n\nfunc main() {}\n",
    )
    .unwrap();
    let found = analyze(&root, AnalysisTool::Staticcheck);
    assert_eq!(found.len(), 1, "{found:?}");
    assert_eq!(found[0].path, main);
    assert_eq!(found[0].line, 2);
}
