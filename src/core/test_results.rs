use super::run::{RunCommand, RunTarget};
use super::run_output::RunLine;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum TestState {
    Failed,
    Passed,
    Ignored,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestRow {
    pub name: String,
    pub state: TestState,
    pub line: usize,
}

const MAX_ROWS: usize = 20_000;

fn state_word(word: &str) -> Option<TestState> {
    match word {
        "ok" | "PASS" | "PASSED" => Some(TestState::Passed),
        "FAILED" | "FAIL" | "ERROR" => Some(TestState::Failed),
        "ignored" | "SKIP" | "SKIPPED" | "XFAIL" | "XPASS" => Some(TestState::Ignored),
        _ => None,
    }
}

fn cargo_line(line: &str) -> Option<(String, TestState)> {
    let (name, result) = line.strip_prefix("test ")?.split_once(" ... ")?;
    let word = result.split([',', ' ']).next()?;
    Some((name.trim().to_owned(), state_word(word)?))
}

fn go_line(line: &str) -> Option<(String, TestState)> {
    let rest = line.trim_start().strip_prefix("--- ")?;
    let (word, rest) = rest.split_once(": ")?;
    let name = rest.split_whitespace().next()?;
    Some((name.to_owned(), state_word(word)?))
}

fn pytest_line(line: &str) -> Option<(String, TestState)> {
    let mut words = line.split_whitespace();
    let name = words.next()?;
    if !name.contains("::") {
        return None;
    }
    Some((name.to_owned(), state_word(words.next()?)?))
}

fn jest_line(line: &str) -> Option<(String, TestState)> {
    let line = line.trim_start();
    let mut chars = line.chars();
    let state = match chars.next()? {
        '✓' | '✔' => TestState::Passed,
        '✕' | '✗' | '×' => TestState::Failed,
        '○' => TestState::Ignored,
        _ => return None,
    };
    let rest = chars.as_str();
    let rest = rest.strip_prefix(' ')?;
    let name = match rest.rfind(" (") {
        Some(at) if rest.ends_with(" ms)") || rest.ends_with("ms)") => &rest[..at],
        _ => rest,
    };
    let name = name.trim();
    (!name.is_empty()).then(|| (name.to_owned(), state))
}

pub fn parse_line(line: &str) -> Option<(String, TestState)> {
    cargo_line(line)
        .or_else(|| go_line(line))
        .or_else(|| pytest_line(line))
        .or_else(|| jest_line(line))
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct TestPanel {
    pub run_id: Option<u64>,
    pub rows: Vec<TestRow>,
    pub selected: usize,
}

impl TestPanel {
    pub fn feed(&mut self, run_id: u64, first: usize, lines: &[RunLine]) {
        for (n, line) in lines.iter().enumerate() {
            let number = first + n;
            if self.run_id == Some(run_id)
                && let Some(name) = line
                    .text
                    .strip_prefix("---- ")
                    .and_then(|t| t.strip_suffix(" stdout ----"))
                && let Some(row) = self.rows.iter_mut().find(|r| r.name == name)
            {
                row.line = number;
                continue;
            }
            let Some((name, state)) = parse_line(&line.text) else {
                continue;
            };
            if self.run_id != Some(run_id) {
                *self = Self {
                    run_id: Some(run_id),
                    ..Self::default()
                };
            }
            if let Some(row) = self.rows.iter_mut().find(|r| r.name == name) {
                row.state = state;
                row.line = number;
            } else if self.rows.len() < MAX_ROWS {
                self.rows.push(TestRow {
                    name,
                    state,
                    line: number,
                });
            }
        }
        self.rows.sort_by_key(|r| r.state);
    }

    pub fn count(&self, state: TestState) -> usize {
        self.rows.iter().filter(|r| r.state == state).count()
    }

    pub fn failed(&self) -> Vec<String> {
        self.rows
            .iter()
            .filter(|r| r.state == TestState::Failed)
            .map(|r| r.name.clone())
            .collect()
    }
}

pub fn only_failed(target: &RunTarget, names: &[String]) -> Option<RunTarget> {
    if names.is_empty() || names.iter().any(|n| n.starts_with('-')) {
        return None;
    }
    let RunCommand::Argv(argv) = &target.command else {
        return None;
    };
    let program = std::path::Path::new(argv.first()?).file_name()?.to_str()?;
    let has = |word: &str| argv.iter().any(|a| a == word);
    let mut out = argv.clone();
    match program {
        "cargo" if has("test") => {
            if !has("--") {
                out.push("--".to_owned());
            }
            out.push("--exact".to_owned());
            out.extend(names.iter().cloned());
        }
        "go" if has("test") => {
            if let Some(at) = out.iter().position(|a| a == "-run") {
                out.drain(at..(at + 2).min(out.len()));
            }
            let plain = |n: &String| n.chars().all(|c| c.is_alphanumeric() || "_/".contains(c));
            if !names.iter().all(plain) {
                return None;
            }
            let at = out.iter().position(|a| a == "test")? + 1;
            out.insert(at, format!("^({})$", names.join("|")));
            out.insert(at, "-run".to_owned());
        }
        "pytest" => out.extend(names.iter().cloned()),
        "python" | "python3" if has("pytest") => out.extend(names.iter().cloned()),
        _ => return None,
    }
    Some(RunTarget {
        name: format!("{} (failed)", target.name),
        command: RunCommand::Argv(out),
        cwd: target.cwd.clone(),
        source: target.source,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::run::RunSource;

    fn line(text: &str) -> RunLine {
        RunLine {
            text: text.to_owned(),
            location: None,
            spans: Vec::new(),
        }
    }

    #[test]
    fn each_tool_s_lines_are_understood() {
        use TestState::{Failed, Ignored, Passed};
        let cases = [
            ("test a::b ... ok", "a::b", Passed),
            ("test a::b ... FAILED", "a::b", Failed),
            ("test slow ... ignored, takes long", "slow", Ignored),
            ("--- PASS: TestAdd (0.00s)", "TestAdd", Passed),
            ("    --- FAIL: TestAdd/neg (0.01s)", "TestAdd/neg", Failed),
            ("--- SKIP: TestNet (0.00s)", "TestNet", Ignored),
            (
                "tests/a.py::test_x PASSED   [ 50%]",
                "tests/a.py::test_x",
                Passed,
            ),
            (
                "tests/a.py::C::test_y FAILED",
                "tests/a.py::C::test_y",
                Failed,
            ),
            ("  ✓ adds numbers (3 ms)", "adds numbers", Passed),
            ("  ✕ adds strings", "adds strings", Failed),
        ];
        for (text, name, state) in cases {
            assert_eq!(parse_line(text), Some((name.to_owned(), state)), "{text}");
        }
    }

    #[test]
    fn ordinary_output_is_not_taken_for_a_result() {
        for text in [
            "test result: ok. 3 passed; 0 failed",
            "running 3 tests",
            "FAIL",
            "ok  \texample.com/a\t0.1s",
            "the test ... was fine",
            "--- a/file.rs",
        ] {
            assert_eq!(parse_line(text), None, "{text}");
        }
    }

    #[test]
    fn failures_come_first_and_lead_to_what_they_printed() {
        let mut panel = TestPanel::default();
        panel.feed(1, 0, &[line("test a ... ok"), line("test b ... FAILED")]);
        assert_eq!(panel.rows[0].name, "b");
        assert_eq!(panel.rows[0].line, 1);
        panel.feed(1, 2, &[line("failures:"), line("---- b stdout ----")]);
        assert_eq!(panel.rows[0].line, 3);
        assert_eq!(panel.count(TestState::Failed), 1);
        assert_eq!(panel.failed(), ["b"]);
    }

    #[test]
    fn a_new_run_starts_clean_once_it_reports_something() {
        let mut panel = TestPanel::default();
        panel.feed(1, 0, &[line("test a ... FAILED")]);
        panel.feed(2, 0, &[line("compiling")]);
        assert_eq!(panel.rows.len(), 1, "a build's output leaves the results");
        panel.feed(2, 1, &[line("test a ... ok")]);
        assert_eq!(panel.run_id, Some(2));
        assert_eq!(panel.count(TestState::Failed), 0);
    }

    fn target(argv: &[&str]) -> RunTarget {
        RunTarget {
            name: "t".to_owned(),
            command: RunCommand::Argv(argv.iter().map(|&a| a.to_owned()).collect()),
            cwd: std::path::PathBuf::from("/p"),
            source: RunSource::UserConfig,
        }
    }

    fn argv(target: Option<RunTarget>) -> Vec<String> {
        match target.expect("narrowed").command {
            RunCommand::Argv(argv) => argv,
            other => panic!("{other:?}"),
        }
    }

    #[test]
    fn only_the_failed_are_asked_for_in_each_tool_s_own_way() {
        let names = ["a::b".to_owned()];
        assert_eq!(
            argv(only_failed(&target(&["cargo", "test"]), &names)),
            ["cargo", "test", "--", "--exact", "a::b"]
        );
        let names = ["TestA".to_owned(), "TestB/neg".to_owned()];
        assert_eq!(
            argv(only_failed(
                &target(&["go", "test", "-run", "X", "./..."]),
                &names
            )),
            ["go", "test", "-run", "^(TestA|TestB/neg)$", "./..."]
        );
        let names = ["t.py::x".to_owned()];
        assert_eq!(
            argv(only_failed(&target(&["pytest", "-v"]), &names)),
            ["pytest", "-v", "t.py::x"]
        );
    }

    #[test]
    fn nothing_printed_by_a_test_can_become_a_command() {
        let shell = RunTarget {
            command: RunCommand::Shell("cargo test".to_owned()),
            ..target(&[])
        };
        let names = ["a".to_owned()];
        assert_eq!(only_failed(&shell, &names), None, "not into a shell");
        let option = ["--config=x".to_owned()];
        assert_eq!(only_failed(&target(&["pytest"]), &option), None);
        let regex = ["a|b)$;".to_owned()];
        assert_eq!(only_failed(&target(&["go", "test"]), &regex), None);
        assert_eq!(only_failed(&target(&["make"]), &names), None, "unknown");
        assert_eq!(only_failed(&target(&["cargo", "test"]), &[]), None);
    }
}
