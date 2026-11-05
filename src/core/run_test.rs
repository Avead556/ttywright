use std::path::Path;

use regex::Regex;

use super::run::{RunCommand, RunFacts, RunSource, RunTarget};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TestAt {
    pub name: String,
    pub class: Option<String>,
}

fn capture(pattern: &str, line: &str) -> Option<String> {
    let found = Regex::new(pattern).ok()?.captures(line)?;
    Some(found.get(1)?.as_str().to_owned())
}

fn above(lines: &[&str], from: usize, pattern: &str) -> Option<(usize, String)> {
    (0..=from)
        .rev()
        .find_map(|n| capture(pattern, lines.get(n)?).map(|name| (n, name)))
}

fn after_mark(lines: &[&str], from: usize, mark: &str, name: &str) -> Option<(usize, String)> {
    let (start, _) = above(lines, from, &format!("({mark})"))?;
    (start..lines.len()).find_map(|n| capture(name, lines[n]).map(|found| (n, found)))
}

const CLASS: &str = r"\bclass\s+(\w+)";

pub fn test_at(path: &Path, text: &str, line: usize) -> Option<TestAt> {
    let lines: Vec<&str> = text.lines().collect();
    let from = line.min(lines.len().checked_sub(1)?);
    let named = |name: String| TestAt { name, class: None };
    let in_class = |at: usize, name: String| TestAt {
        class: above(&lines, at, CLASS).map(|(_, class)| class),
        name,
    };
    match path.extension()?.to_str()? {
        "rs" => {
            let (_, name) = after_mark(&lines, from, r"#\[(?:\w+::)?test\b", r"\bfn\s+(\w+)")?;
            Some(named(name))
        }
        "go" => above(&lines, from, r"^func\s+(Test\w*|Benchmark\w*)\s*\(").map(|(_, n)| named(n)),
        "py" => {
            let (at, name) = above(&lines, from, r"^\s*(?:async\s+)?def\s+(test\w*)")?;
            let indented = lines[at].starts_with([' ', '\t']);
            Some(if indented {
                in_class(at, name)
            } else {
                named(name)
            })
        }
        "php" => {
            let (at, name) = above(&lines, from, r"\bfunction\s+(test\w*)").or_else(|| {
                after_mark(&lines, from, r"#\[Test\b|@test\b", r"\bfunction\s+(\w+)")
            })?;
            Some(in_class(at, name))
        }
        "java" | "kt" => {
            let (at, name) = after_mark(
                &lines,
                from,
                r"@(?:Test|ParameterizedTest)\b",
                r"\b(?:void|fun)\s+(\w+)\s*\(",
            )?;
            Some(in_class(at, name))
        }
        "js" | "jsx" | "mjs" | "cjs" | "ts" | "tsx" | "mts" | "cts" => above(
            &lines,
            from,
            r#"\b(?:it|test)(?:\.\w+)?\(\s*['"`]([^'"`]+)['"`]"#,
        )
        .map(|(_, n)| named(n)),
        _ => None,
    }
}

fn owned(argv: &[&str]) -> Vec<String> {
    argv.iter().map(|a| (*a).to_owned()).collect()
}

pub fn test_target(root: &Path, path: &Path, test: &TestAt, facts: &RunFacts) -> Option<RunTarget> {
    let relative = path.strip_prefix(root).unwrap_or(path);
    let file = relative.display().to_string();
    let name = test.name.as_str();
    let present = |f: &str| facts.present.contains(f);
    let argv = match path.extension()?.to_str()? {
        "rs" => owned(&["cargo", "test", name]),
        "go" => {
            let folder = relative
                .parent()
                .map_or(String::new(), |p| p.display().to_string());
            let package = if folder.is_empty() {
                ".".to_owned()
            } else {
                format!("./{folder}")
            };
            owned(&["go", "test", "-run", &format!("^{name}$"), &package])
        }
        "py" => {
            let place = match &test.class {
                Some(class) => format!("{file}::{class}::{name}"),
                None => format!("{file}::{name}"),
            };
            owned(&["python3", "-m", "pytest", &place])
        }
        "php" => {
            let phpunit = if present("vendor/bin/phpunit") {
                "vendor/bin/phpunit"
            } else {
                "phpunit"
            };
            owned(&[phpunit, "--filter", name, &file])
        }
        "java" | "kt" => {
            let class = test.class.as_deref()?;
            if present("gradlew") || present("build.gradle") || present("build.gradle.kts") {
                let gradle = if present("gradlew") {
                    "./gradlew"
                } else {
                    "gradle"
                };
                owned(&[gradle, "test", "--tests", &format!("{class}.{name}")])
            } else {
                let maven = if present("mvnw") { "./mvnw" } else { "mvn" };
                owned(&[maven, "test", &format!("-Dtest={class}#{name}")])
            }
        }
        _ => {
            let vitest = facts
                .texts
                .get("package.json")
                .is_some_and(|text| text.contains("vitest"));
            if vitest {
                owned(&["npx", "vitest", "run", &file, "-t", name])
            } else {
                owned(&["npx", "jest", &file, "-t", name])
            }
        }
    };
    Some(RunTarget {
        name: format!("Test: {name}"),
        command: RunCommand::Argv(argv),
        cwd: root.to_path_buf(),
        source: RunSource::CurrentFile,
    })
}

#[cfg(test)]
mod tests {
    use std::path::PathBuf;

    use super::*;

    fn at(file: &str, text: &str, line: usize) -> Option<TestAt> {
        test_at(Path::new(file), text, line)
    }

    fn name(file: &str, text: &str, line: usize) -> Option<String> {
        at(file, text, line).map(|t| t.name)
    }

    #[test]
    fn rust_needs_the_attribute_and_takes_the_function_after_it() {
        let text = "fn helper() {}\n\n#[test]\nfn first() {\n    x();\n}\n\n#[tokio::test]\nasync fn second() {\n}\n";
        assert_eq!(name("a.rs", text, 0), None, "above any test");
        assert_eq!(name("a.rs", text, 4).as_deref(), Some("first"));
        assert_eq!(
            name("a.rs", text, 2).as_deref(),
            Some("first"),
            "on the attribute"
        );
        assert_eq!(name("a.rs", text, 9).as_deref(), Some("second"));
    }

    #[test]
    fn each_language_finds_its_own_kind() {
        let go = "func helper() {}\nfunc TestIt(t *testing.T) {\n}\n";
        assert_eq!(name("a_test.go", go, 2).as_deref(), Some("TestIt"));
        assert_eq!(name("a_test.go", go, 0), None);
        let php = "class ATest extends TestCase {\n  public function testIt() {\n    x();\n  }\n  #[Test]\n  public function other() {}\n}\n";
        let found = at("a.php", php, 2).unwrap();
        assert_eq!(
            (found.name.as_str(), found.class.as_deref()),
            ("testIt", Some("ATest"))
        );
        let java = "class ATest {\n  @Test\n  void works() {\n  }\n}\n";
        let found = at("A.java", java, 3).unwrap();
        assert_eq!(
            (found.name.as_str(), found.class.as_deref()),
            ("works", Some("ATest"))
        );
        let js = "describe('x', () => {\n  it('does a thing', () => {\n    y();\n  });\n});\n";
        assert_eq!(name("a.test.ts", js, 2).as_deref(), Some("does a thing"));
        assert_eq!(name("a.txt", js, 2), None);
    }

    #[test]
    fn python_gives_a_class_only_to_a_method() {
        let text = "class TestA:\n    def test_m(self):\n        pass\n\ndef test_f():\n    pass\n";
        let method = at("t.py", text, 2).unwrap();
        assert_eq!(
            (method.name.as_str(), method.class.as_deref()),
            ("test_m", Some("TestA"))
        );
        let function = at("t.py", text, 5).unwrap();
        assert_eq!((function.name.as_str(), function.class), ("test_f", None));
    }

    fn command(file: &str, name: &str, class: Option<&str>, facts: &RunFacts) -> String {
        let test = TestAt {
            name: name.to_owned(),
            class: class.map(str::to_owned),
        };
        test_target(
            Path::new("/p"),
            &PathBuf::from("/p").join(file),
            &test,
            facts,
        )
        .unwrap()
        .command_line()
    }

    #[test]
    fn the_command_is_what_the_project_uses() {
        let none = RunFacts::default();
        assert_eq!(command("src/a.rs", "it", None, &none), "cargo test it");
        assert_eq!(
            command("x/y/a_test.go", "TestA", None, &none),
            "go test -run '^TestA$' ./x/y"
        );
        assert_eq!(
            command("a_test.go", "TestA", None, &none),
            "go test -run '^TestA$' ."
        );
        assert_eq!(
            command("t/a.py", "test_m", Some("TestA"), &none),
            "python3 -m pytest t/a.py::TestA::test_m"
        );
        assert_eq!(
            command("t/A.php", "testIt", None, &none),
            "phpunit --filter testIt t/A.php"
        );
        let mut facts = RunFacts::default();
        facts.present.insert("vendor/bin/phpunit".to_owned());
        facts.present.insert("gradlew".to_owned());
        assert_eq!(
            command("t/A.php", "testIt", None, &facts),
            "vendor/bin/phpunit --filter testIt t/A.php"
        );
        assert_eq!(
            command("A.java", "w", Some("ATest"), &facts),
            "./gradlew test --tests ATest.w"
        );
        assert_eq!(
            command("A.java", "w", Some("ATest"), &none),
            "mvn test '-Dtest=ATest#w'"
        );
        assert_eq!(
            command("a.test.ts", "does it", None, &none),
            "npx jest a.test.ts -t 'does it'"
        );
        facts
            .texts
            .insert("package.json".to_owned(), "\"vitest\"".to_owned());
        assert_eq!(
            command("a.test.ts", "x", None, &facts),
            "npx vitest run a.test.ts -t x"
        );
    }
}
