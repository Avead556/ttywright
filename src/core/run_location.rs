use std::path::{Component, Path, PathBuf};
use std::sync::LazyLock;

use regex::Regex;

const PATH: &str = r#"[^\s:'"()\[\]<>,=]+"#;
const MAX_PATH_BYTES: usize = 4096;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Place,
    JvmFrame,
}

const FORMATS: &[(&str, Shape)] = &[
    (r"^\s*(?:-->|:::)\s*(.+?):(\d+):(\d+)\s*$", Shape::Place),
    (r#"\bFile "([^"]+)", line (\d+)()"#, Shape::Place),
    (r"\bin (\S.*?) on line (\d+)()", Shape::Place),
    (r"^#\d+\s+(.+?)\((\d+)\)()", Shape::Place),
    (
        r"\bat\s+([\w.$/<>]+)\(([\w$]+\.(?:java|kt|scala|groovy)):(\d+)\)",
        Shape::JvmFrame,
    ),
    (
        r"\bat\s+(?:[^()]*\()?(?:file://)?([^()]+?):(\d+):(\d+)\)?\s*$",
        Shape::Place,
    ),
    (r"^([^:]+): line (\d+)()(?::|$)", Shape::Place),
    (r"^([^:\s]+): (\d+)(): ", Shape::Place),
];

fn generic_formats() -> [String; 2] {
    let before = r#"(?:^|[\s'"(\[<=,])"#;
    [
        format!(r"{before}({PATH})\((\d+),(\d+)\)"),
        format!(r"{before}(?:file://)?({PATH}):(\d+)(?::(\d+))?"),
    ]
}

static COMPILED: LazyLock<Vec<(Regex, Shape)>> = LazyLock::new(|| {
    FORMATS
        .iter()
        .map(|(pattern, shape)| ((*pattern).to_owned(), *shape))
        .chain(generic_formats().map(|p| (p, Shape::Place)))
        .filter_map(|(pattern, shape)| Regex::new(&pattern).ok().map(|r| (r, shape)))
        .collect()
});

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Named {
    Path(String),
    JvmFile { qualified: String, file: String },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Candidate {
    pub named: Named,
    pub line: usize,
    pub col: Option<usize>,
    pub span: (usize, usize),
}

fn char_range(text: &str, start: usize, end: usize) -> (usize, usize) {
    let first = text[..start].chars().count();
    (first, first + text[start..end].chars().count())
}

fn positive(text: Option<&str>) -> Option<usize> {
    text?.parse().ok().filter(|n| *n > 0)
}

pub fn candidates(text: &str) -> Vec<Candidate> {
    if !text.bytes().any(|b| b.is_ascii_digit()) {
        return Vec::new();
    }
    let mut found = Vec::new();
    for (regex, shape) in COMPILED.iter() {
        for captures in regex.captures_iter(text) {
            let (Some(first), Some(whole)) = (captures.get(1), captures.get(0)) else {
                continue;
            };
            let candidate = match shape {
                Shape::Place => positive(captures.get(2).map(|m| m.as_str())).map(|line| {
                    let end = captures.get(3).filter(|c| !c.is_empty()).map_or_else(
                        || captures.get(2).map_or(whole.end(), |m| m.end()),
                        |c| c.end(),
                    );
                    Candidate {
                        named: Named::Path(first.as_str().to_owned()),
                        line,
                        col: positive(captures.get(3).map(|m| m.as_str())),
                        span: char_range(text, first.start(), end),
                    }
                }),
                Shape::JvmFrame => positive(captures.get(3).map(|m| m.as_str())).map(|line| {
                    let file = captures.get(2).map_or("", |m| m.as_str()).to_owned();
                    let end = captures.get(3).map_or(whole.end(), |m| m.end());
                    Candidate {
                        named: Named::JvmFile {
                            qualified: first.as_str().to_owned(),
                            file,
                        },
                        line,
                        col: None,
                        span: char_range(text, first.start(), end),
                    }
                }),
            };
            found.extend(candidate.filter(|c| match &c.named {
                Named::Path(p) => p.len() <= MAX_PATH_BYTES,
                Named::JvmFile { .. } => true,
            }));
        }
    }
    found
}

pub fn tidy_path(path: &Path) -> PathBuf {
    let mut out = PathBuf::new();
    for part in path.components() {
        match part {
            Component::CurDir => {}
            Component::ParentDir => {
                if !out.pop() {
                    out.push("..");
                }
            }
            other => out.push(other.as_os_str()),
        }
    }
    out
}

pub fn jvm_file<'a>(qualified: &str, file: &str, among: &'a [PathBuf]) -> Option<&'a PathBuf> {
    let name = qualified.rsplit('/').next().unwrap_or(qualified);
    let mut parts: Vec<&str> = name.split('.').collect();
    parts.truncate(parts.len().saturating_sub(2));
    if parts.is_empty() {
        return match among {
            [only] => Some(only),
            _ => None,
        };
    }
    let wanted: PathBuf = parts.iter().collect::<PathBuf>().join(file);
    among.iter().find(|path| path.ends_with(&wanted))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn places(text: &str) -> Vec<(String, usize, Option<usize>)> {
        candidates(text)
            .into_iter()
            .filter_map(|c| match c.named {
                Named::Path(path) => Some((path, c.line, c.col)),
                Named::JvmFile { .. } => None,
            })
            .collect()
    }

    fn first(text: &str) -> (String, usize, Option<usize>) {
        places(text)
            .into_iter()
            .next()
            .unwrap_or_else(|| panic!("nothing found in {text:?}"))
    }

    #[test]
    fn every_format_compiles() {
        assert_eq!(COMPILED.len(), FORMATS.len() + generic_formats().len());
    }

    #[test]
    fn sample_lines_of_each_tool() {
        for (line, path, at, col) in [
            ("  --> src/main.rs:2:5", "src/main.rs", 2, Some(5)),
            ("   ::: src/lib.rs:10:1", "src/lib.rs", 10, Some(1)),
            ("  --> my dir/a.rs:2:5", "my dir/a.rs", 2, Some(5)),
            ("./main.go:3:5: undefined: x", "./main.go", 3, Some(5)),
            ("src/a.ts:3:5 - error TS2322", "src/a.ts", 3, Some(5)),
            ("src/a.ts(3,5): error TS2322", "src/a.ts", 3, Some(5)),
            ("a.c:12:9: error: expected", "a.c", 12, Some(9)),
            ("/abs/a.js:1:2: Missing semicolon", "/abs/a.js", 1, Some(2)),
            (
                "  File \"my dir/a.py\", line 3, in <module>",
                "my dir/a.py",
                3,
                None,
            ),
            (
                "PHP Parse error:  syntax error in /x/my a.php on line 7",
                "/x/my a.php",
                7,
                None,
            ),
            ("#0 /x/a.php(12): foo()", "/x/a.php", 12, None),
            ("/x/a.php:12", "/x/a.php", 12, None),
            ("    at fn (/x/my a.js:3:5)", "/x/my a.js", 3, Some(5)),
            ("    at /x/a.js:3:5", "/x/a.js", 3, Some(5)),
            ("    at file:///x/a.mjs:3:5", "/x/a.mjs", 3, Some(5)),
            (
                "    at async Module.fn (file:///x/a.mjs:3:5)",
                "/x/a.mjs",
                3,
                Some(5),
            ),
            ("./a.sh: line 3: x: command not found", "./a.sh", 3, None),
            (
                "./my a.sh: line 3: x: command not found",
                "./my a.sh",
                3,
                None,
            ),
            ("./a.sh: 3: x: not found", "./a.sh", 3, None),
            ("main.go:3", "main.go", 3, None),
            ("see (a.rb:4) above", "a.rb", 4, None),
        ] {
            assert_eq!(first(line), (path.to_owned(), at, col), "in {line:?}");
        }
    }

    #[test]
    fn a_jvm_frame_names_a_file_to_search_for() {
        let found = candidates("\tat com.acme.Main.run(Main.java:12)");
        assert_eq!(found[0].line, 12);
        assert_eq!(
            found[0].named,
            Named::JvmFile {
                qualified: "com.acme.Main.run".to_owned(),
                file: "Main.java".to_owned(),
            }
        );
        assert!(places("\tat java.base/jdk.Native.run(Native Method)").is_empty());
    }

    #[test]
    fn jvm_files_are_matched_by_package() {
        let among = [
            PathBuf::from("/p/src/main/java/com/acme/Main.java"),
            PathBuf::from("/p/src/main/java/org/other/Main.java"),
        ];
        assert_eq!(
            jvm_file("com.acme.Main.run", "Main.java", &among),
            Some(&among[0])
        );
        assert_eq!(
            jvm_file("org.other.Main$Inner.run", "Main.java", &among),
            Some(&among[1])
        );
        assert_eq!(jvm_file("java.lang.Main.run", "Main.java", &among), None);
        assert_eq!(
            jvm_file("java.base/java.lang.Main.run", "Main.java", &among),
            None
        );
        assert_eq!(jvm_file("Main.main", "Main.java", &among), None);
        assert_eq!(
            jvm_file("Main.main", "Main.java", &among[..1]),
            Some(&among[0])
        );
    }

    #[test]
    fn other_things_with_colons_only_propose_what_the_disk_will_refuse() {
        for (line, wrong) in [
            ("https://example.com:8080/x", "https"),
            ("http://localhost:3000", "localhost"),
            ("Listening on 127.0.0.1:8080", "never"),
        ] {
            assert!(
                places(line).iter().all(|(p, _, _)| !p.starts_with(wrong)),
                "{line:?}: {:?}",
                places(line)
            );
        }
        for line in [
            "no numbers here: at all",
            "",
            "::::",
            ":1:1",
            "plain 12 text",
        ] {
            assert!(places(line).is_empty(), "{line:?}: {:?}", places(line));
        }
    }

    #[test]
    fn a_line_number_must_be_a_real_one() {
        assert!(places("a.c:0:1").is_empty());
        assert!(places("a.c:99999999999999999999999:1").is_empty());
        assert_eq!(first("a.c:3:0"), ("a.c".to_owned(), 3, None));
    }

    #[test]
    fn the_span_covers_the_place_in_chars() {
        let text = "ошибка a.c:3:5: x";
        let found = candidates(text);
        let (start, end) = found[0].span;
        let shown: String = text.chars().skip(start).take(end - start).collect();
        assert_eq!(shown, "a.c:3:5");
    }

    #[test]
    fn hostile_text_neither_panics_nor_proposes_a_giant_path() {
        let long = format!("{}:1:1", "a".repeat(100_000));
        assert!(places(&long).is_empty());
        for text in [
            "\u{fffd}:1",
            ":\u{fffd}:",
            "at (:1:1)",
            "#0 (1)",
            "in  on line 1",
        ] {
            let _ = candidates(text);
        }
    }

    #[test]
    fn paths_are_tidied_without_the_disk() {
        for (from, to) in [
            ("/r/./src/a.rs", "/r/src/a.rs"),
            ("/r/x/../a.rs", "/r/a.rs"),
            ("a/../../b", "../b"),
        ] {
            assert_eq!(tidy_path(Path::new(from)), PathBuf::from(to));
        }
    }
}
