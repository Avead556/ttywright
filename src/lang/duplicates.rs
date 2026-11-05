use std::collections::HashMap;
use std::hash::{DefaultHasher, Hash, Hasher};
use std::ops::Range;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::{Duration, Instant};

use tree_sitter::{ParseOptions, ParseState, Parser};

use super::languages::Language;
use super::syntax::tree_language;
use crate::core::{DuplicateFragment, DuplicateGroup, DuplicatesReport};

pub const MIN_TOKENS: usize = 50;
const WINDOW: usize = 40;
const BASE: u64 = 0x0100_0000_01b3;
const MAX_TOKENS: usize = 8_000_000;
const MAX_FINGERPRINTS: usize = 2_000_000;
const MAX_CHAIN: usize = 8;
const MAX_GROUPS: usize = 500;
const MAX_FILE_BYTES: usize = 1024 * 1024;
const PARSE_BUDGET: Duration = Duration::from_secs(2);

pub struct Token {
    value: u64,
    line: u32,
}

fn hashed(value: impl Hash) -> u64 {
    let mut hasher = DefaultHasher::new();
    value.hash(&mut hasher);
    hasher.finish()
}

fn is_name_or_literal(kind: &str) -> bool {
    [
        "identifier",
        "name",
        "literal",
        "string",
        "number",
        "integer",
        "float",
        "boolean",
    ]
    .iter()
    .any(|word| kind.contains(word))
}

pub fn tokens(language: Language, text: &str, anonymize: bool) -> Vec<Token> {
    let mut found = Vec::new();
    if text.len() > MAX_FILE_BYTES {
        return found;
    }
    let Some(grammar) = tree_language(language) else {
        return found;
    };
    let mut parser = Parser::new();
    if parser.set_language(&grammar).is_err() {
        return found;
    }
    let started = Instant::now();
    let mut out_of_time = |_: &ParseState| started.elapsed() > PARSE_BUDGET;
    let options = ParseOptions::new().progress_callback(&mut out_of_time);
    let bytes = text.as_bytes();
    let mut read = |at: usize, _: tree_sitter::Point| -> &[u8] { bytes.get(at..).unwrap_or(&[]) };
    let Some(tree) = parser.parse_with_options(&mut read, None, Some(options)) else {
        return found;
    };
    let mut cursor = tree.walk();
    loop {
        let node = cursor.node();
        if node.is_extra() {
        } else if cursor.goto_first_child() {
            continue;
        } else {
            let value = if anonymize && is_name_or_literal(node.kind()) {
                hashed(node.kind())
            } else {
                hashed(text.get(node.byte_range()).unwrap_or(""))
            };
            found.push(Token {
                value,
                line: u32::try_from(node.start_position().row).unwrap_or(u32::MAX),
            });
        }
        while !cursor.goto_next_sibling() {
            if !cursor.goto_parent() {
                return found;
            }
        }
    }
}

struct FileSpan {
    path: PathBuf,
    tokens: Range<usize>,
}

#[derive(Default)]
pub struct Detector {
    values: Vec<u64>,
    lines: Vec<u32>,
    files: Vec<FileSpan>,
    truncated: bool,
}

fn window_power() -> u64 {
    (1..WINDOW).fold(1_u64, |power, _| power.wrapping_mul(BASE))
}

impl Detector {
    pub fn add(&mut self, path: PathBuf, tokens: Vec<Token>) -> bool {
        if self.values.len() + tokens.len() > MAX_TOKENS {
            self.truncated = true;
            return false;
        }
        if tokens.len() < MIN_TOKENS {
            return true;
        }
        let start = self.values.len();
        for token in tokens {
            self.values.push(token.value);
            self.lines.push(token.line);
        }
        self.files.push(FileSpan {
            path,
            tokens: start..self.values.len(),
        });
        true
    }

    fn grow(
        &self,
        a: usize,
        b: usize,
        a_file: &Range<usize>,
        b_file: &Range<usize>,
        floor: usize,
    ) -> (usize, usize, usize) {
        let (mut a_start, mut b_start) = (a, b);
        while a_start > a_file.start
            && b_start > b_file.start.max(floor)
            && self.values[a_start - 1] == self.values[b_start - 1]
        {
            a_start -= 1;
            b_start -= 1;
        }
        let mut len = 0;
        let same_file = a_file.start == b_file.start;
        while a_start + len < a_file.end
            && b_start + len < b_file.end
            && (!same_file || a_start + len < b_start)
            && self.values[a_start + len] == self.values[b_start + len]
        {
            len += 1;
        }
        (a_start, b_start, len)
    }

    fn fragment(&self, file: &FileSpan, at: Range<usize>) -> DuplicateFragment {
        DuplicateFragment {
            path: file.path.clone(),
            first_line: self.lines[at.start],
            last_line: self.lines[at.end - 1],
        }
    }

    pub fn finish(self, cancel: &AtomicBool) -> DuplicatesReport {
        let total = self.values.len();
        let sample = (total / MAX_FINGERPRINTS + 1) as u64;
        let power = window_power();
        let mut first: HashMap<u64, u32> = HashMap::new();
        let mut previous = vec![0_u32; total];
        let mut owner = vec![0_u32; if total == 0 { 0 } else { total }];
        let mut groups: HashMap<u64, DuplicateGroup> = HashMap::new();
        let mut cancelled = false;
        for (file_index, file) in self.files.iter().enumerate() {
            if cancel.load(Ordering::Relaxed) {
                cancelled = true;
                break;
            }
            for slot in &mut owner[file.tokens.clone()] {
                *slot = u32::try_from(file_index).unwrap_or(u32::MAX);
            }
            let Some(last) = file.tokens.end.checked_sub(WINDOW) else {
                continue;
            };
            let mut at = file.tokens.start;
            let mut floor = at;
            let mut hash = self.hash_at(at);
            let mut hash_at = at;
            while at <= last {
                while hash_at < at {
                    hash = hash
                        .wrapping_sub(self.values[hash_at].wrapping_mul(power))
                        .wrapping_mul(BASE)
                        .wrapping_add(self.values[hash_at + WINDOW]);
                    hash_at += 1;
                }
                if !hash.is_multiple_of(sample) {
                    at += 1;
                    continue;
                }
                let mut best: Option<(usize, usize, usize, usize)> = None;
                let mut candidate = first.get(&hash).copied();
                for _ in 0..MAX_CHAIN {
                    let Some(place) = candidate.map(|c| c as usize - 1) else {
                        break;
                    };
                    let other = owner[place] as usize;
                    let other_span = &self.files[other].tokens;
                    if self.values[place..place + WINDOW] == self.values[at..at + WINDOW] {
                        let (a, b, len) = self.grow(place, at, other_span, &file.tokens, floor);
                        if len >= MIN_TOKENS && best.is_none_or(|(.., l)| len > l) {
                            best = Some((other, a, b, len));
                        }
                    }
                    candidate = Some(previous[place]).filter(|p| *p > 0);
                }
                if let Some((other, a, b, len)) = best {
                    let key = hashed(&self.values[b..b + len]);
                    let group = groups.entry(key).or_insert_with(|| DuplicateGroup {
                        tokens: len,
                        fragments: Vec::new(),
                    });
                    for fragment in [
                        self.fragment(&self.files[other], a..a + len),
                        self.fragment(file, b..b + len),
                    ] {
                        if !group.fragments.contains(&fragment) {
                            group.fragments.push(fragment);
                        }
                    }
                    at = b + len;
                    floor = at;
                    continue;
                }
                let number = u32::try_from(at + 1).unwrap_or(u32::MAX);
                previous[at] = first.insert(hash, number).unwrap_or(0);
                at += 1;
            }
        }
        let mut groups: Vec<DuplicateGroup> = groups.into_values().collect();
        for group in &mut groups {
            group.fragments.sort();
        }
        groups.sort_by(|a, b| {
            b.tokens
                .cmp(&a.tokens)
                .then_with(|| a.fragments.cmp(&b.fragments))
        });
        let truncated = self.truncated || groups.len() > MAX_GROUPS;
        groups.truncate(MAX_GROUPS);
        DuplicatesReport {
            groups,
            files: self.files.len(),
            tokens: total,
            truncated,
            cancelled,
        }
    }

    fn hash_at(&self, at: usize) -> u64 {
        self.values.get(at..at + WINDOW).map_or(0, |window| {
            window.iter().fold(0_u64, |hash, value| {
                hash.wrapping_mul(BASE).wrapping_add(*value)
            })
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const BODY: &str = "fn NAME(a: i32, b: i32) -> i32 {
    let total = a + b * 2;
    if total > 10 {
        println!(\"large {}\", total);
        return total - 1;
    }
    for i in 0..total {
        println!(\"{}\", i + a);
    }
    total + b
}
";

    fn run(files: &[(&str, String)], anonymize: bool) -> DuplicatesReport {
        let mut detector = Detector::default();
        for (name, text) in files {
            let found = tokens(Language::Rust, text, anonymize);
            assert!(detector.add(PathBuf::from(name), found));
        }
        detector.finish(&AtomicBool::new(false))
    }

    #[test]
    fn the_same_code_in_two_files_is_one_group_with_both_places() {
        let a = BODY.replace("NAME", "f");
        let b = format!("// first\n\n{}", BODY.replace("NAME", "f"));
        let report = run(&[("a.rs", a), ("b.rs", b)], false);
        assert_eq!(report.groups.len(), 1, "{report:#?}");
        let places = &report.groups[0].fragments;
        assert_eq!(places.len(), 2);
        assert_eq!(places[0].path, PathBuf::from("a.rs"));
        assert_eq!((places[0].first_line, places[0].last_line), (0, 10));
        assert_eq!((places[1].first_line, places[1].last_line), (2, 12));
    }

    #[test]
    fn comments_and_spacing_do_not_hide_a_copy() {
        let a = BODY.replace("NAME", "f");
        let b = a.replace("let total", "// note\n   let   total");
        assert_eq!(run(&[("a.rs", a), ("b.rs", b)], false).groups.len(), 1);
    }

    #[test]
    fn other_names_are_a_copy_only_when_anonymized() {
        let a = BODY.replace("NAME", "f").replace("total", "sum");
        let b = BODY.replace("NAME", "g").replace("total", "count");
        let pair = [("a.rs", a), ("b.rs", b)];
        assert!(run(&pair, false).groups.is_empty());
        assert_eq!(run(&pair, true).groups.len(), 1);
    }

    #[test]
    fn a_copy_within_one_file_does_not_overlap_itself() {
        let text = format!("{}{}", BODY.replace("NAME", "f"), BODY.replace("NAME", "f"));
        let report = run(&[("a.rs", text)], false);
        assert_eq!(report.groups.len(), 1, "{report:#?}");
        let places = &report.groups[0].fragments;
        assert!(places[0].last_line < places[1].first_line, "{places:?}");
    }

    #[test]
    fn different_code_and_short_copies_are_not_reported() {
        let short = "fn a() { let x = 1; }\n".to_owned();
        let report = run(
            &[
                ("a.rs", short.clone()),
                ("b.rs", short),
                ("c.rs", BODY.replace("NAME", "f")),
            ],
            false,
        );
        assert!(report.groups.is_empty());
    }

    #[test]
    fn a_file_of_one_repeated_line_ends_quickly() {
        let text = "fn a() { x(); }\n".repeat(5000);
        let started = Instant::now();
        let report = run(&[("a.rs", text)], false);
        assert!(started.elapsed() < Duration::from_secs(10));
        assert!(!report.groups.is_empty());
    }

    #[test]
    fn a_cancel_stops_it_and_says_so() {
        let mut detector = Detector::default();
        let text = BODY.replace("NAME", "f");
        detector.add(PathBuf::from("a"), tokens(Language::Rust, &text, false));
        let report = detector.finish(&AtomicBool::new(true));
        assert!(report.cancelled);
    }

    #[test]
    fn over_the_limit_a_file_is_refused_and_the_report_says_so() {
        let mut detector = Detector::default();
        let many = (0..MAX_TOKENS + 1)
            .map(|i| Token {
                value: i as u64,
                line: 0,
            })
            .collect();
        assert!(!detector.add(PathBuf::from("a"), many));
        assert!(detector.finish(&AtomicBool::new(false)).truncated);
    }
}
