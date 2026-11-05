use std::collections::{BTreeMap, HashMap};
use std::io::Read;
use std::path::{Component, Path, PathBuf};
use std::process::{Command, Stdio};
use std::sync::{Arc, Mutex, PoisonError};
use std::thread;
use std::time::Duration;

use rustix::process::{Pid, Signal, kill_process_group};
use serde_json::Value;

use crate::core::Severity;
use crate::core::analysis::{
    AnalysisCommand, AnalysisEffect, AnalysisEvent, AnalysisOutcome, AnalysisPlan, AnalysisTool,
    Finding,
};
use crate::services::fs::write_atomic;

const MAX_OUTPUT: u64 = 256 * 1024 * 1024;
const MESSAGE_CHARS: usize = 1000;
const GRACE: Duration = Duration::from_millis(1500);

pub fn on_path(program: &str) -> Option<PathBuf> {
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .filter(|dir| dir.is_absolute())
        .map(|dir| dir.join(program))
        .find(|file| file.is_file())
}

fn any_file(root: &Path, names: &[&str]) -> bool {
    names.iter().any(|name| root.join(name).is_file())
}

fn wants_eslint(root: &Path) -> bool {
    any_file(
        root,
        &[
            "eslint.config.js",
            "eslint.config.mjs",
            "eslint.config.cjs",
            "eslint.config.ts",
            ".eslintrc",
            ".eslintrc.js",
            ".eslintrc.cjs",
            ".eslintrc.json",
            ".eslintrc.yml",
            ".eslintrc.yaml",
        ],
    ) || std::fs::read_to_string(root.join("package.json"))
        .is_ok_and(|text| text.contains("\"eslintConfig\""))
}

pub fn plan(root: &Path, lookup: &dyn Fn(&str) -> Option<PathBuf>) -> AnalysisPlan {
    let wanted: [(AnalysisTool, bool, &str, &[&str]); 4] = [
        (
            AnalysisTool::Clippy,
            any_file(root, &["Cargo.toml"]),
            "cargo",
            &["clippy", "--message-format=json", "--all-targets"],
        ),
        (
            AnalysisTool::Staticcheck,
            any_file(root, &["go.mod"]),
            "staticcheck",
            &["-f", "json", "./..."],
        ),
        (
            AnalysisTool::Ruff,
            any_file(
                root,
                &[
                    "ruff.toml",
                    ".ruff.toml",
                    "pyproject.toml",
                    "requirements.txt",
                    "setup.py",
                ],
            ),
            "ruff",
            &["check", "--output-format=json", "."],
        ),
        (
            AnalysisTool::Eslint,
            wants_eslint(root),
            "eslint",
            &["--format", "json", "."],
        ),
    ];
    let mut out = AnalysisPlan::default();
    for (tool, wanted, program, args) in wanted {
        if !wanted {
            continue;
        }
        let local = root.join("node_modules/.bin/eslint");
        let found = lookup(program)
            .or_else(|| (tool == AnalysisTool::Eslint && local.is_file()).then_some(local));
        match found {
            Some(found) => out.commands.push(AnalysisCommand {
                tool,
                from_project: found.starts_with(root),
                program: found,
                args: args.iter().map(|a| (*a).to_owned()).collect(),
            }),
            None => out.missing.push(program.to_owned()),
        }
    }
    out
}

type Grants = BTreeMap<String, Vec<String>>;

fn read_grants(file: &Path) -> Grants {
    std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn is_approved(file: &Path, root: &Path, commands: &[AnalysisCommand]) -> bool {
    let grants = read_grants(file);
    let Some(known) = grants.get(&root.display().to_string()) else {
        return false;
    };
    !commands.is_empty()
        && commands
            .iter()
            .all(|c| !c.from_project && known.contains(&c.command_line()))
}

pub fn remember(file: &Path, root: &Path, commands: &[AnalysisCommand]) -> Result<(), String> {
    let mut grants = read_grants(file);
    let known = grants.entry(root.display().to_string()).or_default();
    for command in commands.iter().filter(|c| !c.from_project) {
        let line = command.command_line();
        if !known.contains(&line) {
            known.push(line);
        }
    }
    let bytes = serde_json::to_vec_pretty(&grants).map_err(|e| e.to_string())?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    write_atomic(file, &bytes).map_err(|e| e.to_string())
}

fn clean(message: &str, code: Option<&str>) -> String {
    let text: String = message
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(MESSAGE_CHARS)
        .collect();
    match code.filter(|c| !c.is_empty() && !text.starts_with(c)) {
        Some(code) => {
            let code: String = code.chars().filter(|c| !c.is_control()).take(80).collect();
            format!("{text} ({code})")
        }
        None => text,
    }
}

fn place(name: &str, root: &Path, exists: &dyn Fn(&Path) -> bool) -> Option<PathBuf> {
    let path = Path::new(name);
    if path.components().any(|c| matches!(c, Component::ParentDir)) {
        return None;
    }
    let full = if path.is_absolute() {
        path.to_path_buf()
    } else {
        root.ancestors()
            .map(|dir| dir.join(path))
            .find(|p| exists(p))
            .unwrap_or_else(|| root.join(path))
    };
    full.starts_with(root).then_some(full)
}

fn number(value: &Value, key: &str) -> usize {
    let n = value.get(key).and_then(Value::as_u64).unwrap_or(1);
    usize::try_from(n).unwrap_or(1).saturating_sub(1)
}

fn severity_named(name: &str) -> Severity {
    match name {
        "error" | "error: internal compiler error" => Severity::Error,
        "warning" => Severity::Warning,
        _ => Severity::Info,
    }
}

fn read_clippy(stdout: &str, root: &Path, exists: &dyn Fn(&Path) -> bool) -> Vec<Finding> {
    let mut out = Vec::new();
    for line in stdout.lines() {
        let Ok(value) = serde_json::from_str::<Value>(line) else {
            continue;
        };
        if value.get("reason").and_then(Value::as_str) != Some("compiler-message") {
            continue;
        }
        let Some(message) = value.get("message") else {
            continue;
        };
        let spans = message.get("spans").and_then(Value::as_array);
        let primary = spans
            .into_iter()
            .flatten()
            .find(|s| s.get("is_primary").and_then(Value::as_bool) == Some(true));
        let Some(span) = primary else {
            continue;
        };
        let path = span
            .get("file_name")
            .and_then(Value::as_str)
            .and_then(|name| place(name, root, exists));
        let Some(path) = path else {
            continue;
        };
        out.push(Finding {
            path,
            line: number(span, "line_start"),
            col: number(span, "column_start"),
            severity: severity_named(message.get("level").and_then(Value::as_str).unwrap_or("")),
            message: clean(
                message.get("message").and_then(Value::as_str).unwrap_or(""),
                message.pointer("/code/code").and_then(Value::as_str),
            ),
        });
    }
    out
}

fn read_staticcheck(stdout: &str, root: &Path, exists: &dyn Fn(&Path) -> bool) -> Vec<Finding> {
    stdout
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter_map(|value| {
            let location = value.get("location")?;
            Some(Finding {
                path: place(location.get("file")?.as_str()?, root, exists)?,
                line: number(location, "line"),
                col: number(location, "column"),
                severity: severity_named(value.get("severity")?.as_str()?),
                message: clean(
                    value.get("message")?.as_str()?,
                    value.get("code").and_then(Value::as_str),
                ),
            })
        })
        .collect()
}

fn read_ruff(
    stdout: &str,
    root: &Path,
    exists: &dyn Fn(&Path) -> bool,
) -> Result<Vec<Finding>, String> {
    let list: Vec<Value> = serde_json::from_str(stdout).map_err(|e| e.to_string())?;
    Ok(list
        .iter()
        .filter_map(|item| {
            let location = item.get("location")?;
            let code = item.get("code").and_then(Value::as_str);
            Some(Finding {
                path: place(item.get("filename")?.as_str()?, root, exists)?,
                line: number(location, "row"),
                col: number(location, "column"),
                severity: if code.is_some() {
                    Severity::Warning
                } else {
                    Severity::Error
                },
                message: clean(item.get("message")?.as_str()?, code),
            })
        })
        .collect())
}

fn read_eslint(
    stdout: &str,
    root: &Path,
    exists: &dyn Fn(&Path) -> bool,
) -> Result<Vec<Finding>, String> {
    let list: Vec<Value> = serde_json::from_str(stdout).map_err(|e| e.to_string())?;
    let mut out = Vec::new();
    for file in &list {
        let Some(path) = file
            .get("filePath")
            .and_then(Value::as_str)
            .and_then(|name| place(name, root, exists))
        else {
            continue;
        };
        for message in file
            .get("messages")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
        {
            let Some(text) = message.get("message").and_then(Value::as_str) else {
                continue;
            };
            out.push(Finding {
                path: path.clone(),
                line: number(message, "line"),
                col: number(message, "column"),
                severity: if message.get("severity").and_then(Value::as_u64) == Some(2) {
                    Severity::Error
                } else {
                    Severity::Warning
                },
                message: clean(text, message.get("ruleId").and_then(Value::as_str)),
            });
        }
    }
    Ok(out)
}

fn complaint(stderr: &str) -> Option<String> {
    let lines = || stderr.lines().map(str::trim).filter(|l| !l.is_empty());
    let line = lines()
        .find(|l| l.starts_with("error"))
        .or_else(|| lines().next_back())?;
    Some(clean(line, None))
}

pub fn outcome(
    tool: AnalysisTool,
    exit_ok: bool,
    stdout: &str,
    stderr: &str,
    root: &Path,
    exists: &dyn Fn(&Path) -> bool,
) -> AnalysisOutcome {
    let parsed = match tool {
        AnalysisTool::Clippy => Ok(read_clippy(stdout, root, exists)),
        AnalysisTool::Staticcheck => Ok(read_staticcheck(stdout, root, exists)),
        AnalysisTool::Ruff => read_ruff(stdout, root, exists),
        AnalysisTool::Eslint => read_eslint(stdout, root, exists),
    };
    match parsed {
        Ok(mut findings) if !findings.is_empty() || exit_ok => {
            let mut seen = std::collections::HashSet::new();
            findings.retain(|f| seen.insert((f.path.clone(), f.line, f.col, f.message.clone())));
            AnalysisOutcome::Found(findings)
        }
        Ok(_) => AnalysisOutcome::Failed(
            complaint(stderr).unwrap_or_else(|| "it failed without saying why".to_owned()),
        ),
        Err(_) if exit_ok && stdout.trim().is_empty() => AnalysisOutcome::Found(Vec::new()),
        Err(parse) => AnalysisOutcome::Failed(
            complaint(stderr).unwrap_or_else(|| format!("unreadable report: {parse}")),
        ),
    }
}

struct Running {
    group: Pid,
    stopped: bool,
}

type Table = Arc<Mutex<HashMap<AnalysisTool, Running>>>;

#[derive(Default)]
pub struct AnalysisHost {
    running: Table,
}

fn signal_all(table: &Table, signal: Signal, mark: bool) {
    let mut running = table.lock().unwrap_or_else(PoisonError::into_inner);
    for entry in running.values_mut() {
        entry.stopped |= mark;
        let _ = kill_process_group(entry.group, signal);
    }
}

impl AnalysisHost {
    pub fn run<E>(&self, effect: AnalysisEffect, grants: Option<PathBuf>, emit: E)
    where
        E: Fn(AnalysisEvent) + Clone + Send + 'static,
    {
        match effect {
            AnalysisEffect::Detect { root } => {
                thread::spawn(move || {
                    let mut plan = plan(&root, &on_path);
                    plan.approved =
                        grants.is_some_and(|file| is_approved(&file, &root, &plan.commands));
                    emit(AnalysisEvent::Planned { plan });
                });
            }
            AnalysisEffect::Start { root, commands } => {
                if let Some(file) = grants {
                    let _ = remember(&file, &root, &commands);
                }
                self.start(&root, commands, move |tool, outcome| {
                    emit(AnalysisEvent::Finished { tool, outcome });
                });
            }
            AnalysisEffect::Stop => self.stop(),
        }
    }

    pub fn start<E>(&self, root: &Path, commands: Vec<AnalysisCommand>, emit: E)
    where
        E: Fn(AnalysisTool, AnalysisOutcome) + Clone + Send + 'static,
    {
        for command in commands {
            let (table, root, emit) = (self.running.clone(), root.to_path_buf(), emit.clone());
            thread::spawn(move || {
                let tool = command.tool;
                emit(tool, run_one(&command, &root, &table));
            });
        }
    }

    pub fn stop(&self) {
        signal_all(&self.running, Signal::TERM, true);
        let table = self.running.clone();
        thread::spawn(move || {
            thread::sleep(GRACE);
            signal_all(&table, Signal::KILL, true);
        });
    }
}

impl Drop for AnalysisHost {
    fn drop(&mut self) {
        signal_all(&self.running, Signal::KILL, true);
    }
}

fn run_one(command: &AnalysisCommand, root: &Path, table: &Table) -> AnalysisOutcome {
    use std::os::unix::process::CommandExt;
    let spawned = Command::new(&command.program)
        .args(&command.args)
        .current_dir(root)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .process_group(0)
        .spawn();
    let mut child = match spawned {
        Ok(child) => child,
        Err(e) => return AnalysisOutcome::Failed(format!("could not start: {e}")),
    };
    let Some(group) = i32::try_from(child.id()).ok().and_then(Pid::from_raw) else {
        let _ = child.kill();
        let _ = child.wait();
        return AnalysisOutcome::Failed("could not track it".to_owned());
    };
    table.lock().unwrap_or_else(PoisonError::into_inner).insert(
        command.tool,
        Running {
            group,
            stopped: false,
        },
    );
    let (stdout, stderr) = (child.stdout.take(), child.stderr.take());
    let errors = thread::spawn(move || {
        let mut text = Vec::new();
        if let Some(pipe) = stderr {
            let _ = pipe.take(MAX_OUTPUT).read_to_end(&mut text);
        }
        text
    });
    let mut report = Vec::new();
    if let Some(pipe) = stdout {
        let _ = pipe.take(MAX_OUTPUT).read_to_end(&mut report);
    }
    let too_much = report.len() as u64 >= MAX_OUTPUT;
    if too_much {
        let _ = kill_process_group(group, Signal::KILL);
    }
    let entry = table
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .remove(&command.tool);
    let status = child.wait();
    let errors = errors.join().unwrap_or_default();
    if entry.is_some_and(|e| e.stopped) {
        return AnalysisOutcome::Stopped;
    }
    if too_much {
        return AnalysisOutcome::Failed("it wrote too much and was stopped".to_owned());
    }
    outcome(
        command.tool,
        status.is_ok_and(|s| s.success()),
        &String::from_utf8_lossy(&report),
        &String::from_utf8_lossy(&errors),
        root,
        &|p| p.is_file(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::os::unix::fs::PermissionsExt;
    use std::sync::mpsc::channel;

    const ROOT: &str = "/project";

    fn found(o: AnalysisOutcome) -> Vec<Finding> {
        match o {
            AnalysisOutcome::Found(f) => f,
            other => panic!("expected findings, got {other:?}"),
        }
    }

    fn report(tool: AnalysisTool, exit_ok: bool, stdout: &str, stderr: &str) -> AnalysisOutcome {
        outcome(tool, exit_ok, stdout, stderr, Path::new(ROOT), &|_| true)
    }

    #[test]
    fn real_clippy_output_gives_each_place_once_with_its_lint() {
        let text = include_str!("../../tests/fixtures/analysis/clippy.jsonl");
        let list = found(report(AnalysisTool::Clippy, true, text, ""));
        assert_eq!(list.len(), 3, "{list:?}");
        assert_eq!(list[0].path, Path::new("/project/src/main.rs"));
        assert_eq!((list[0].line, list[0].col), (1, 8));
        assert_eq!(list[0].severity, Severity::Warning);
        assert!(list[0].message.starts_with("unused variable"));
        assert!(list.iter().any(|f| f.message.contains("clippy::len_zero")));
    }

    #[test]
    fn real_staticcheck_output_is_read() {
        let text = include_str!("../../tests/fixtures/analysis/staticcheck.jsonl");
        let list = found(report(AnalysisTool::Staticcheck, false, text, ""));
        assert_eq!(list.len(), 2);
        assert_eq!(list[0].path, Path::new("/project/main.go"));
        assert_eq!((list[0].line, list[0].col), (4, 5));
        assert_eq!(list[0].message, "func unused is unused (U1000)");
    }

    #[test]
    fn ruff_rules_warn_and_syntax_errors_are_errors() {
        let text = r#"[
            {"code":"F401","filename":"/project/a.py","location":{"row":2,"column":8},"message":"unused"},
            {"code":null,"filename":"/project/b.py","location":{"row":1,"column":1},"message":"bad"}
        ]"#;
        let list = found(report(AnalysisTool::Ruff, false, text, ""));
        assert_eq!(list[0].severity, Severity::Warning);
        assert_eq!((list[0].line, list[0].col), (1, 7));
        assert_eq!(list[0].message, "unused (F401)");
        assert_eq!(list[1].severity, Severity::Error);
    }

    #[test]
    fn eslint_severity_two_is_an_error() {
        let text = r#"[{"filePath":"/project/a.js","messages":[
            {"ruleId":"no-undef","severity":2,"message":"x","line":3,"column":4},
            {"ruleId":null,"severity":1,"message":"y","line":1,"column":1}
        ]}]"#;
        let list = found(report(AnalysisTool::Eslint, false, text, ""));
        assert_eq!(list[0].severity, Severity::Error);
        assert_eq!(list[0].message, "x (no-undef)");
        assert_eq!(list[1].severity, Severity::Warning);
    }

    #[test]
    fn a_bad_exit_without_findings_is_a_failure_with_the_tools_words() {
        let stderr = "note: x\nerror: no such command: `clippy`\n\nhelp: install it\n";
        assert_eq!(
            report(AnalysisTool::Clippy, false, "", stderr),
            AnalysisOutcome::Failed("error: no such command: `clippy`".to_owned())
        );
        assert!(matches!(
            report(AnalysisTool::Eslint, false, "", "config broke"),
            AnalysisOutcome::Failed(m) if m == "config broke"
        ));
        assert!(matches!(
            report(AnalysisTool::Ruff, false, "not json", ""),
            AnalysisOutcome::Failed(m) if m.contains("unreadable")
        ));
        assert_eq!(found(report(AnalysisTool::Clippy, true, "", "")), vec![]);
        assert_eq!(found(report(AnalysisTool::Ruff, true, "[]", "")), vec![]);
    }

    #[test]
    fn places_outside_the_project_are_dropped_and_control_codes_never_pass() {
        let text = r#"[{"code":"A","filename":"/etc/passwd","location":{"row":1,"column":1},"message":"m"},
            {"code":"B","filename":"../up.py","location":{"row":1,"column":1},"message":"m"},
            {"code":"C","filename":"/project/ok.py","location":{"row":1,"column":1},"message":"a\u001b[31mb"}]"#;
        let list = found(report(AnalysisTool::Ruff, true, text, ""));
        assert_eq!(list.len(), 1);
        assert_eq!(list[0].message, "a [31mb (C)");
    }

    #[test]
    fn cargo_paths_resolve_against_the_workspace_above_the_project() {
        let exists = |p: &Path| p == Path::new("/w/member/src/a.rs");
        let name = "member/src/a.rs";
        assert_eq!(
            place("src/a.rs", Path::new("/w/member"), &|p| p
                == Path::new("/w/member/src/a.rs")),
            Some(PathBuf::from("/w/member/src/a.rs"))
        );
        assert_eq!(
            place(name, Path::new("/w/member"), &exists),
            Some(PathBuf::from("/w/member/src/a.rs")),
        );
    }

    fn touch(path: &Path) {
        std::fs::create_dir_all(path.parent().unwrap()).unwrap();
        std::fs::write(path, "").unwrap();
    }

    #[test]
    fn the_plan_follows_the_projects_files_and_names_what_is_missing() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert_eq!(plan(root, &|_| None), AnalysisPlan::default());
        touch(&root.join("Cargo.toml"));
        touch(&root.join("go.mod"));
        let lookup = |p: &str| (p == "cargo").then(|| PathBuf::from("/bin/cargo"));
        let planned = plan(root, &lookup);
        assert_eq!(planned.commands.len(), 1);
        assert_eq!(
            planned.commands[0].command_line(),
            "/bin/cargo clippy --message-format=json --all-targets"
        );
        assert_eq!(planned.missing, ["staticcheck"]);
        assert!(!planned.approved);
    }

    #[test]
    fn only_eslint_may_come_from_the_project_and_it_is_marked() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        touch(&root.join("eslint.config.js"));
        touch(&root.join("node_modules/.bin/eslint"));
        touch(&root.join("node_modules/.bin/ruff"));
        touch(&root.join("ruff.toml"));
        let planned = plan(root, &|_| None);
        assert_eq!(
            planned.missing,
            ["ruff"],
            "no other tool is taken from there"
        );
        assert_eq!(
            planned.commands[0].program,
            root.join("node_modules/.bin/eslint")
        );
        assert!(planned.commands[0].from_project);
        let from_path = plan(root, &|p| {
            (p == "eslint").then(|| PathBuf::from("/bin/eslint"))
        });
        assert!(!from_path.commands[0].from_project, "PATH comes first");
    }

    #[test]
    fn approval_is_for_these_exact_commands_in_this_project_only() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("deep/grants.json");
        let command = |program: &str, from_project| AnalysisCommand {
            tool: AnalysisTool::Ruff,
            program: PathBuf::from(program),
            args: vec!["check".to_owned()],
            from_project,
        };
        let (a, b) = (Path::new("/a"), Path::new("/b"));
        let ruff = [command("/bin/ruff", false)];
        assert!(!is_approved(&file, a, &ruff), "nothing yet");
        remember(&file, a, &ruff).unwrap();
        assert!(is_approved(&file, a, &ruff));
        assert!(!is_approved(&file, b, &ruff), "another project");
        assert!(
            !is_approved(&file, a, &[command("/tmp/ruff", false)]),
            "another program"
        );
        assert!(!is_approved(&file, a, &[]));
        let local = [command("/a/node_modules/.bin/eslint", true)];
        remember(&file, a, &local).unwrap();
        assert!(
            !is_approved(&file, a, &local),
            "from the project: always asked"
        );
    }

    fn script(dir: &Path, body: &str) -> PathBuf {
        let path = dir.join("tool");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
        path
    }

    fn command(program: PathBuf) -> AnalysisCommand {
        AnalysisCommand {
            tool: AnalysisTool::Ruff,
            program,
            args: Vec::new(),
            from_project: false,
        }
    }

    #[test]
    fn a_tool_is_run_in_the_project_and_its_report_comes_back() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let tool = script(
            &root,
            "echo '[{\"code\":\"X\",\"filename\":\"'\"$PWD\"'/a.py\",\"location\":{\"row\":1,\"column\":1},\"message\":\"m\"}]'\nexit 1",
        );
        let host = AnalysisHost::default();
        let (tx, rx) = channel();
        host.start(&root, vec![command(tool)], move |t, o| {
            let _ = tx.send((t, o));
        });
        let (tool, result) = rx.recv_timeout(Duration::from_secs(20)).unwrap();
        assert_eq!(tool, AnalysisTool::Ruff);
        assert_eq!(found(result)[0].path, root.join("a.py"));
    }

    #[test]
    fn stop_ends_the_tool_and_its_children_even_if_it_ignores_term() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        let pid_file = root.join("child");
        let tool = script(
            &root,
            &format!(
                "trap '' TERM\n(trap '' TERM; echo $$ > /dev/null; sleep 600) &\necho $! > {}\nwait\nwait\nsleep 600",
                pid_file.display()
            ),
        );
        let host = AnalysisHost::default();
        let (tx, rx) = channel();
        host.start(&root, vec![command(tool)], move |_, o| {
            let _ = tx.send(o);
        });
        while std::fs::read_to_string(&pid_file).map_or(true, |t| t.trim().is_empty()) {
            thread::sleep(Duration::from_millis(20));
        }
        host.stop();
        let result = rx.recv_timeout(Duration::from_secs(20)).unwrap();
        assert_eq!(result, AnalysisOutcome::Stopped);
        let child = std::fs::read_to_string(&pid_file).unwrap();
        let child = Pid::from_raw(child.trim().parse().unwrap()).unwrap();
        for _ in 0..200 {
            if rustix::process::test_kill_process(child).is_err() {
                return;
            }
            thread::sleep(Duration::from_millis(20));
        }
        panic!("the tool's child outlived Stop");
    }

    #[test]
    fn a_program_that_cannot_start_says_so() {
        let host = AnalysisHost::default();
        let (tx, rx) = channel();
        host.start(
            Path::new("/"),
            vec![command(PathBuf::from("/nonexistent/tool"))],
            move |_, o| {
                let _ = tx.send(o);
            },
        );
        assert!(matches!(
            rx.recv_timeout(Duration::from_secs(20)).unwrap(),
            AnalysisOutcome::Failed(m) if m.contains("could not start")
        ));
    }
}
