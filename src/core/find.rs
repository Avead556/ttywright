use super::trigram::TextFilter;
use std::ops::Range;
use std::path::PathBuf;

use regex::{Captures, Regex, RegexBuilder};
use ropey::Rope;
use serde::{Deserialize, Serialize};

use super::document::DiskState;
use std::collections::BTreeMap;
use std::path::Path;

use crate::lang::languages::Language;
use crate::lang::structural::{Compiled, Pattern, expand};

use super::input::LineInput;
use super::workspace::DocumentId;

pub enum Replacer {
    Text(Query, Template),
    Structural {
        pattern: Pattern,
        template: String,
        languages: BTreeMap<Language, Option<Compiled>>,
    },
}

impl Replacer {
    pub fn compile(query: &str, options: FindOptions, replacement: &str) -> Result<Self, String> {
        if options.structural {
            let pattern = Pattern::new(query)?;
            pattern.check_template(replacement)?;
            return Ok(Self::Structural {
                pattern,
                template: replacement.to_owned(),
                languages: BTreeMap::new(),
            });
        }
        let query = Query::new(query, options)?;
        let template = query.template(replacement)?;
        Ok(Self::Text(query, template))
    }

    pub fn edits(&mut self, path: &Path, text: &str) -> Vec<Edit> {
        match self {
            Self::Text(query, template) => query.plan(text, template),
            Self::Structural {
                pattern,
                template,
                languages,
            } => {
                let language = Language::from_path(path);
                let compiled = languages
                    .entry(language)
                    .or_insert_with(|| pattern.compile(language));
                compiled.as_ref().map_or_else(Vec::new, |compiled| {
                    compiled
                        .find(text)
                        .into_iter()
                        .map(|found| Edit {
                            text: expand(template, &found.holes),
                            bytes: found.bytes,
                        })
                        .collect()
                })
            }
        }
    }
}

pub fn check_query(query: &str, options: FindOptions) -> Result<(), String> {
    if options.structural {
        Pattern::new(query).map(|_| ())
    } else {
        Query::new(query, options).map(|_| ())
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct FindOptions {
    pub regex: bool,
    pub case_sensitive: bool,
    pub whole_word: bool,
    pub structural: bool,
}

fn has_capital(query: &str, regex: bool) -> bool {
    let mut chars = query.chars();
    while let Some(c) = chars.next() {
        if regex && c == '\\' {
            chars.next();
        } else if c.is_uppercase() {
            return true;
        }
    }
    false
}

fn is_word_char(c: char) -> bool {
    c.is_alphanumeric() || c == '_'
}

#[derive(Debug, Clone)]
pub struct Query {
    re: Regex,
    regex: bool,
    needs: TextFilter,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Edit {
    pub bytes: Range<usize>,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LineHit {
    pub line: usize,
    pub col: usize,
    pub len: usize,
    pub text: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Piece {
    Text(String),
    Group(usize),
    Named(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Template {
    pieces: Vec<Piece>,
}

impl Template {
    fn expand(&self, caps: &Captures<'_>, out: &mut String) {
        for piece in &self.pieces {
            match piece {
                Piece::Text(text) => out.push_str(text),
                Piece::Group(n) => out.push_str(caps.get(*n).map_or("", |m| m.as_str())),
                Piece::Named(name) => out.push_str(caps.name(name).map_or("", |m| m.as_str())),
            }
        }
    }
}

impl Query {
    pub fn new(query: &str, options: FindOptions) -> Result<Self, String> {
        let body = if options.regex {
            query.to_owned()
        } else {
            regex::escape(query)
        };
        let edge = |c: Option<char>| {
            if options.regex || c.is_some_and(is_word_char) {
                r"\b"
            } else {
                ""
            }
        };
        let pattern = if options.whole_word {
            let (before, after) = (edge(query.chars().next()), edge(query.chars().last()));
            format!("{before}(?:{body}){after}")
        } else {
            body
        };
        let sensitive = options.case_sensitive || has_capital(query, options.regex);
        let re = RegexBuilder::new(&pattern)
            .case_insensitive(!sensitive)
            .multi_line(true)
            .crlf(true)
            .build()
            .map_err(|e| {
                let text = e.to_string();
                let last = text.lines().last().unwrap_or("invalid regex");
                last.trim_start_matches("error: ").to_owned()
            })?;
        let needs = regex_syntax::ParserBuilder::new()
            .case_insensitive(!sensitive)
            .multi_line(true)
            .crlf(true)
            .build()
            .parse(&pattern)
            .map_or(TextFilter::Anything, |hir| TextFilter::of_regex(&hir));
        Ok(Self {
            re,
            regex: options.regex,
            needs,
        })
    }

    pub fn needs(&self) -> &TextFilter {
        &self.needs
    }

    fn matches<'a>(&'a self, text: &'a str) -> impl Iterator<Item = Range<usize>> + 'a {
        self.re
            .find_iter(text)
            .filter(|m| !m.is_empty())
            .map(|m| m.range())
    }

    pub fn template(&self, replacement: &str) -> Result<Template, String> {
        if !self.regex {
            return Ok(Template {
                pieces: vec![Piece::Text(replacement.to_owned())],
            });
        }
        let groups = self.re.captures_len();
        let mut pieces = Vec::new();
        let mut text = String::new();
        let mut chars = replacement.chars().peekable();
        while let Some(c) = chars.next() {
            match (c, chars.peek().copied()) {
                ('\\', Some(next @ ('n' | 't' | '\\' | '$'))) => {
                    chars.next();
                    text.push(match next {
                        'n' => '\n',
                        't' => '\t',
                        other => other,
                    });
                }
                ('$', Some('$')) => {
                    chars.next();
                    text.push('$');
                }
                ('$', Some(d)) if d.is_ascii_digit() => {
                    let mut n = 0;
                    let mut any = false;
                    while let Some(v) = chars.peek().and_then(|d| d.to_digit(10)) {
                        let wider = n * 10 + v as usize;
                        if any && wider >= groups {
                            break;
                        }
                        n = wider;
                        any = true;
                        chars.next();
                    }
                    if n >= groups {
                        return Err(format!("no group {n} in the query"));
                    }
                    pieces.push(Piece::Text(std::mem::take(&mut text)));
                    pieces.push(Piece::Group(n));
                }
                ('$', Some('{')) => {
                    chars.next();
                    let name: String = chars.by_ref().take_while(|c| *c != '}').collect();
                    pieces.push(Piece::Text(std::mem::take(&mut text)));
                    match name.parse::<usize>() {
                        Ok(n) if n < groups => pieces.push(Piece::Group(n)),
                        Ok(n) => return Err(format!("no group {n} in the query")),
                        Err(_) if self.re.capture_names().flatten().any(|g| g == name) => {
                            pieces.push(Piece::Named(name));
                        }
                        Err(_) => return Err(format!("no group {name} in the query")),
                    }
                }
                _ => text.push(c),
            }
        }
        pieces.push(Piece::Text(text));
        Ok(Template { pieces })
    }

    pub fn plan(&self, text: &str, template: &Template) -> Vec<Edit> {
        self.re
            .captures_iter(text)
            .filter_map(|caps| {
                let whole = caps.get(0).filter(|m| !m.is_empty())?;
                let mut out = String::new();
                template.expand(&caps, &mut out);
                Some(Edit {
                    bytes: whole.range(),
                    text: out,
                })
            })
            .collect()
    }

    pub fn find_in(&self, text: &Rope) -> Vec<Range<usize>> {
        let s = text.to_string();
        char_ranges(&s, self.matches(&s))
    }

    pub fn line_hits(&self, text: &str) -> Vec<LineHit> {
        let mut hits: Vec<LineHit> = Vec::new();
        let (mut line, mut counted) = (0, 0);
        for m in self.matches(text) {
            line += text[counted..m.start]
                .bytes()
                .filter(|b| *b == b'\n')
                .count();
            counted = m.start;
            if hits.last().is_some_and(|h| h.line == line) {
                continue;
            }
            let start = text[..m.start].rfind('\n').map_or(0, |i| i + 1);
            let end = text[m.start..]
                .find('\n')
                .map_or(text.len(), |i| m.start + i);
            let content = text[start..end]
                .strip_suffix('\r')
                .unwrap_or(&text[start..end]);
            let on_line = m.end.min(start + content.len()).max(m.start);
            hits.push(LineHit {
                line,
                col: text[start..m.start].chars().count(),
                len: text[m.start..on_line].chars().count(),
                text: content.to_owned(),
            });
        }
        hits
    }
}

fn char_ranges(text: &str, ranges: impl Iterator<Item = Range<usize>>) -> Vec<Range<usize>> {
    let (mut bytes, mut chars) = (0, 0);
    let mut out = Vec::new();
    for r in ranges {
        chars += text[bytes..r.start].chars().count();
        let start = chars;
        chars += text[r.start..r.end].chars().count();
        bytes = r.end;
        out.push(start..chars);
    }
    out
}

pub fn merge_plan(text: &str, edits: &[Edit]) -> Option<(usize, usize, String)> {
    let (first, last) = (edits.first()?, edits.last()?);
    let mut merged = String::new();
    let mut copied = first.bytes.start;
    for edit in edits {
        merged.push_str(&text[copied..edit.bytes.start]);
        merged.push_str(&edit.text);
        copied = edit.bytes.end;
    }
    let from = text[..first.bytes.start].chars().count();
    let to = from + text[first.bytes.start..last.bytes.end].chars().count();
    Some((from, to, merged))
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LineChange {
    pub line: usize,
    pub old: String,
    pub new: String,
}

pub fn line_changes(text: &str, edits: &[Edit]) -> Vec<LineChange> {
    let shown = |s: &str| s.replace("\r\n", "⏎").replace('\n', "⏎");
    let mut changes = Vec::new();
    let (mut line, mut counted) = (0, 0);
    let mut i = 0;
    while let Some(first) = edits.get(i) {
        let start = text[..first.bytes.start].rfind('\n').map_or(0, |n| n + 1);
        line += text[counted..start].bytes().filter(|b| *b == b'\n').count();
        counted = start;
        let mut new = String::new();
        let mut copied = start;
        let mut end;
        loop {
            let edit = &edits[i];
            new.push_str(&text[copied..edit.bytes.start]);
            new.push_str(&edit.text);
            copied = edit.bytes.end;
            end = text[copied..].find('\n').map_or(text.len(), |n| copied + n);
            i += 1;
            if edits.get(i).is_none_or(|next| next.bytes.start > end) {
                break;
            }
        }
        new.push_str(&text[copied..end]);
        let old = &text[start..end];
        changes.push(LineChange {
            line,
            old: shown(old.strip_suffix('\r').unwrap_or(old)),
            new: shown(new.strip_suffix('\r').unwrap_or(&new)),
        });
    }
    changes
}

const MAX_PREVIEW_LINES: usize = 500;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FilePlan {
    pub path: PathBuf,
    pub occurrences: usize,
    pub changes: Vec<LineChange>,
    pub more_lines: usize,
}

impl FilePlan {
    pub fn new(path: PathBuf, text: &str, edits: &[Edit]) -> Self {
        let mut changes = line_changes(text, edits);
        let more_lines = changes.len().saturating_sub(MAX_PREVIEW_LINES);
        changes.truncate(MAX_PREVIEW_LINES);
        Self {
            path,
            occurrences: edits.len(),
            changes,
            more_lines,
        }
    }

    pub fn rows(&self) -> usize {
        1 + self.changes.len() * 2 + usize::from(self.more_lines > 0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ClosedPlan {
    pub plan: FilePlan,
    pub disk: Option<DiskState>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PlanSource {
    Open { doc: DocumentId, version: u64 },
    Closed { disk: Option<DiskState> },
}

pub type FileProblem = (PathBuf, String);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ReplacePreview {
    pub files: Vec<(FilePlan, PlanSource)>,
    pub failed: Vec<FileProblem>,
    pub query: String,
    pub options: FindOptions,
    pub replacement: String,
    pub truncated: bool,
    pub scroll: usize,
}

impl ReplacePreview {
    pub fn occurrences(&self) -> usize {
        self.files.iter().map(|(f, _)| f.occurrences).sum()
    }

    pub fn rows(&self) -> usize {
        self.files.iter().map(|(f, _)| f.rows()).sum::<usize>() + self.failed.len()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ReplaceReport {
    pub changed: Vec<PathBuf>,
    pub skipped: Vec<FileProblem>,
    pub failed: Vec<FileProblem>,
    pub scroll: usize,
}

impl ReplaceReport {
    pub fn rows(&self) -> usize {
        self.changed.len() + self.skipped.len() + self.failed.len()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReplaceStage {
    Planning {
        id: u64,
        open: Vec<(FilePlan, PlanSource)>,
        failed: Vec<FileProblem>,
        query: String,
        options: FindOptions,
        replacement: String,
        truncated: bool,
    },
    Preview(ReplacePreview),
    Applying {
        id: u64,
        report: ReplaceReport,
    },
    Report(ReplaceReport),
}

#[cfg(test)]
fn find_all(text: &Rope, query: &str, options: FindOptions) -> Vec<Range<usize>> {
    Query::new(query, options).map_or(Vec::new(), |q| q.find_in(text))
}

#[derive(Debug, Clone, Default)]
pub struct FindBar {
    pub input: LineInput,
    pub replacement: Option<LineInput>,
    pub in_replacement: bool,
    pub matches: Vec<Range<usize>>,
    pub error: Option<String>,
}

impl FindBar {
    pub fn refresh(&mut self, text: &Rope, options: FindOptions) {
        match Query::new(self.input.text(), options) {
            Ok(query) => {
                self.matches = query.find_in(text);
                self.error = None;
            }
            Err(message) => {
                self.matches.clear();
                self.error = Some(message);
            }
        }
    }

    pub fn next_from(&self, cursor: usize, forward: bool) -> Option<Range<usize>> {
        let found = if forward {
            self.matches
                .iter()
                .find(|m| m.start > cursor)
                .or(self.matches.first())
        } else {
            self.matches
                .iter()
                .rev()
                .find(|m| m.start < cursor)
                .or(self.matches.last())
        };
        found.cloned()
    }

    pub fn current(&self, cursor: usize) -> Option<usize> {
        self.matches.iter().position(|m| m.start == cursor)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SearchMatch {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub len: usize,
    pub text: String,
}

#[derive(Debug, Clone, Default)]
pub struct ProjectSearch {
    pub input: LineInput,
    pub results: Vec<SearchMatch>,
    pub truncated: bool,
    pub selected: Option<usize>,
    pub running: Option<u64>,
    pub searched: Option<String>,
    pub usages_of: Option<String>,
    pub usages_word: &'static str,
    pub options: FindOptions,
    pub error: Option<String>,
    pub replacement: Option<LineInput>,
    pub in_replacement: bool,
    pub replace: Option<ReplaceStage>,
}

#[cfg(test)]
mod tests {
    use super::*;

    const PLAIN: FindOptions = FindOptions {
        regex: false,
        case_sensitive: false,
        whole_word: false,
        structural: false,
    };
    const RE: FindOptions = FindOptions {
        regex: true,
        ..PLAIN
    };

    #[test]
    fn finds_all_smart_case_matches_in_char_units() {
        let t = Rope::from_str("Ёж ёж ЁЖ");
        assert_eq!(find_all(&t, "ёж", PLAIN), [0..2, 3..5, 6..8]);
        assert_eq!(
            find_all(&t, "Ёж", PLAIN),
            vec![0..2],
            "uppercase makes it case-sensitive"
        );
        assert!(find_all(&t, "", PLAIN).is_empty());
    }

    #[test]
    fn matches_do_not_overlap() {
        assert_eq!(find_all(&Rope::from_str("aaaa"), "aa", PLAIN), [0..2, 2..4]);
    }

    #[test]
    fn plain_text_is_not_a_pattern() {
        let t = Rope::from_str("a.c abc (x)");
        assert_eq!(find_all(&t, "a.c", PLAIN), vec![0..3]);
        assert_eq!(find_all(&t, "(x)", PLAIN), vec![8..11]);
    }

    #[test]
    fn case_sensitive_is_stricter_than_smart_case() {
        let t = Rope::from_str("abc ABC");
        assert_eq!(find_all(&t, "abc", PLAIN).len(), 2);
        let exact = FindOptions {
            case_sensitive: true,
            ..PLAIN
        };
        assert_eq!(find_all(&t, "abc", exact), vec![0..3]);
    }

    #[test]
    fn whole_word_respects_word_chars_on_both_sides() {
        let t = Rope::from_str("foo foobar barfoo foo_x foo");
        let word = FindOptions {
            whole_word: true,
            ..PLAIN
        };
        assert_eq!(find_all(&t, "foo", word), [0..3, 24..27]);
        let ru = Rope::from_str("кот котик кот");
        assert_eq!(find_all(&ru, "кот", word), [0..3, 10..13]);
    }

    #[test]
    fn whole_word_of_punctuation_does_not_demand_a_neighbouring_word() {
        let word = FindOptions {
            whole_word: true,
            ..PLAIN
        };
        assert_eq!(find_all(&Rope::from_str("a -> b"), "->", word), vec![2..4]);
        assert_eq!(find_all(&Rope::from_str("(x) y"), "(x", word), vec![0..2]);
    }

    #[test]
    fn regex_mode_understands_patterns_and_lines() {
        let t = Rope::from_str("fn a()\r\nfn b()\nlet c\n");
        assert_eq!(find_all(&t, r"^fn \w", RE), [0..4, 8..12]);
        assert_eq!(find_all(&t, r"\(\)$", RE), [4..6, 12..14]);
        assert_eq!(find_all(&t, r"\)\r?\nf", RE), vec![5..9], "across lines");
    }

    #[test]
    fn smart_case_ignores_escapes_of_a_regex_but_not_its_letters() {
        let t = Rope::from_str("ABC abc");
        assert_eq!(find_all(&t, r"a\S", RE).len(), 2, "\\S is not a capital");
        assert_eq!(find_all(&t, r"A\S", RE), vec![0..2]);
    }

    #[test]
    fn empty_regex_matches_are_ignored() {
        let t = Rope::from_str("abc\ndef");
        assert!(find_all(&t, "^", RE).is_empty());
        assert_eq!(find_all(&t, "b*", RE), vec![1..2]);
    }

    #[test]
    fn an_invalid_regex_is_a_message_on_one_line() {
        for bad in ["(", "[", "a{2,1}", r"\", "(?P<x", "*"] {
            let e = Query::new(bad, RE).expect_err(bad);
            assert!(!e.is_empty() && !e.contains('\n'), "{bad}: {e:?}");
        }
    }

    #[test]
    fn huge_regexes_are_refused_rather_than_exhausting_memory() {
        assert!(Query::new(r"(\w{1000}){1000}", RE).is_err());
    }

    fn replaced(query: &str, options: FindOptions, with: &str, text: &str) -> String {
        let q = Query::new(query, options).expect("query");
        let t = q.template(with).expect("template");
        let edits = q.plan(text, &t);
        let Some((from, to, merged)) = merge_plan(text, &edits) else {
            return text.to_owned();
        };
        let chars: Vec<char> = text.chars().collect();
        let mut out: String = chars[..from].iter().collect();
        out.push_str(&merged);
        out.extend(&chars[to..]);
        out
    }

    #[test]
    fn capture_groups_fill_the_replacement() {
        let r = |w| replaced(r"(\w+)=(\d+)", RE, w, "a=1 bb=22");
        assert_eq!(r("$2=$1"), "1=a 22=bb");
        assert_eq!(r("$1a"), "aa bba", "digits, then a letter");
        assert_eq!(r("${1}0"), "a0 bb0");
        assert_eq!(r("$0!"), "a=1! bb=22!");
        assert_eq!(r("$$1"), "$1 $1");
        assert_eq!(r(r"\$1"), "$1 $1");
        assert_eq!(r(r"$1\n"), "a\n bb\n");
        assert_eq!(r("$10"), "a0 bb0", "there is no group 10");
        assert_eq!(
            replaced(r"(?P<k>\w)=", RE, "${k}:", "x=1"),
            "x:1",
            "named group"
        );
    }

    #[test]
    fn a_group_that_does_not_exist_is_refused_not_deleted() {
        let q = Query::new(r"(a)", RE).expect("query");
        assert!(q.template("$2").is_err());
        assert!(q.template("${2}").is_err());
        assert!(q.template("${nope}").is_err());
        assert!(q.template("$1").is_ok());
    }

    #[test]
    fn plain_replacements_mean_themselves() {
        assert_eq!(replaced("a", PLAIN, r"$1\n$$", "a"), r"$1\n$$");
    }

    #[test]
    fn replacing_keeps_every_other_byte_including_line_endings() {
        assert_eq!(
            replaced("b", PLAIN, "ЖЖ", "ёb\r\nb\n\tb!"),
            "ёЖЖ\r\nЖЖ\n\tЖЖ!"
        );
    }

    #[test]
    fn line_hits_report_one_row_per_line_with_columns_in_chars() {
        let q = Query::new("x+", RE).expect("query");
        let hits = q.line_hits("ёxx x\r\nnone\n  xxx");
        assert_eq!(
            hits,
            [
                LineHit {
                    line: 0,
                    col: 1,
                    len: 2,
                    text: "ёxx x".to_owned()
                },
                LineHit {
                    line: 2,
                    col: 2,
                    len: 3,
                    text: "  xxx".to_owned()
                },
            ]
        );
    }

    #[test]
    fn a_match_across_lines_is_a_hit_on_its_first_line() {
        let q = Query::new(r"a\nb", RE).expect("query");
        let hits = q.line_hits("0\nza\nb\n");
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].line, hits[0].col, hits[0].len), (1, 1, 1));
        assert_eq!(hits[0].text, "za");
    }

    fn changes(query: &str, with: &str, text: &str) -> Vec<(usize, String, String)> {
        let q = Query::new(query, RE).expect("query");
        let edits = q.plan(text, &q.template(with).expect("template"));
        line_changes(text, &edits)
            .into_iter()
            .map(|c| (c.line, c.old, c.new))
            .collect()
    }

    fn row(line: usize, old: &str, new: &str) -> (usize, String, String) {
        (line, old.to_owned(), new.to_owned())
    }

    #[test]
    fn the_preview_has_one_row_per_changed_line_with_all_its_changes() {
        assert_eq!(
            changes("a", "X", "a a\nb\r\nca\r\nb\na"),
            [row(0, "a a", "X X"), row(2, "ca", "cX"), row(4, "a", "X")]
        );
    }

    #[test]
    fn a_replacement_across_lines_is_one_row_marked_as_such() {
        assert_eq!(
            changes(r"a\nb", "Z", "0\nxa\nby\nb a\n"),
            [row(1, "xa⏎by", "xZy")]
        );
        assert_eq!(
            changes(r"a\nb|c", "Z", "a\nb c\nd"),
            [row(0, "a⏎b c", "Z Z")],
            "a further edit on the line it ends on joins it"
        );
    }

    #[test]
    fn long_previews_are_capped_but_counted() {
        let text = "a\n".repeat(MAX_PREVIEW_LINES + 7);
        let q = Query::new("a", RE).expect("query");
        let edits = q.plan(&text, &q.template("b").expect("template"));
        let plan = FilePlan::new(PathBuf::from("x"), &text, &edits);
        assert_eq!(plan.occurrences, MAX_PREVIEW_LINES + 7);
        assert_eq!(plan.changes.len(), MAX_PREVIEW_LINES);
        assert_eq!(plan.more_lines, 7);
    }

    #[test]
    fn next_match_wraps_in_both_directions() {
        let mut bar = FindBar::default();
        bar.input.insert("x");
        bar.refresh(&Rope::from_str("x.x.x"), PLAIN);
        assert_eq!(bar.next_from(0, true), Some(2..3));
        assert_eq!(bar.next_from(4, true), Some(0..1));
        assert_eq!(bar.next_from(0, false), Some(4..5));
        assert_eq!(bar.current(2), Some(1));
    }

    #[test]
    fn a_bad_query_empties_the_bar_and_says_why_until_it_is_fixed() {
        let mut bar = FindBar::default();
        bar.input.insert("(");
        bar.refresh(&Rope::from_str("(a)"), RE);
        assert!(bar.matches.is_empty());
        assert!(bar.error.is_some());
        bar.input.insert("a)");
        bar.refresh(&Rope::from_str("(a)"), RE);
        assert_eq!(bar.matches, vec![1..2]);
        assert_eq!(bar.error, None);
    }
}

#[cfg(test)]
mod needs_tests {
    use super::*;
    use crate::core::trigram::TrigramFilter;

    struct Rng(u64);
    impl Rng {
        fn below(&mut self, n: usize) -> usize {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            usize::try_from(self.0 % n as u64).unwrap_or(0)
        }
        fn pick<'a>(&mut self, from: &[&'a str]) -> &'a str {
            from[self.below(from.len())]
        }
    }

    const PIECES: &[&str] = &[
        "a", "b", "k", "K", "\u{212A}", "s", "S", "\u{17F}", "я", "Я", "σ", "Σ", "ς", "ß", "ẞ",
        "i", "İ", "ı", "\n", "\r\n", " ", "_", "foo", "Bar", "kiss", "KISS", "\u{FFFD}", "é",
    ];
    const REGEX: &[&str] = &[
        ".",
        "\\w",
        "\\b",
        "^",
        "$",
        "a*",
        "b+",
        "(?:foo|bar)",
        "(k|s)",
        "[ab]",
        "[kK]",
        "(?i)",
        "(?-i)",
        "(?i:k)",
        "(?-i:ſ)",
        "x?",
        "(foo)+",
        "\\n",
        "\\s",
        "{2}",
        "|",
        "(?:)",
        "\\x41",
    ];

    fn text(rng: &mut Rng, pieces: usize) -> String {
        (0..pieces).map(|_| rng.pick(PIECES)).collect()
    }

    #[test]
    fn a_text_a_query_matches_is_never_ruled_out() {
        let mut rng = Rng(0x1234_5678_9ABC_DEF1);
        let mut matched = 0;
        let mut useful = 0;
        for _ in 0..15_000 {
            let length = 1 + rng.below(40);
            let body = text(&mut rng, length);
            let pieces = 1 + rng.below(5);
            let regex = rng.below(2) == 0;
            let raw: String = if regex {
                (0..pieces)
                    .map(|_| {
                        if rng.below(2) == 0 {
                            rng.pick(REGEX)
                        } else {
                            rng.pick(PIECES)
                        }
                    })
                    .collect()
            } else {
                let start = rng.below(body.chars().count().max(1));
                let taken: String = body.chars().skip(start).take(1 + rng.below(8)).collect();
                match rng.below(3) {
                    0 => taken.to_uppercase(),
                    1 => taken.to_lowercase(),
                    _ => taken,
                }
            };
            let options = FindOptions {
                regex,
                case_sensitive: rng.below(2) == 0,
                whole_word: rng.below(3) == 0,
                structural: false,
            };
            let Ok(query) = Query::new(&raw, options) else {
                continue;
            };
            let filter = TrigramFilter::of_text(&body);
            if *query.needs() != TextFilter::Anything {
                useful += 1;
            }
            if query.re.is_match(&body) {
                matched += 1;
                assert!(
                    query.needs().may_match(&filter),
                    "{raw:?} {options:?} matches {body:?}, needs {:?}",
                    query.needs()
                );
            }
        }
        assert!(matched > 1_000 && useful > 1_000, "{matched} {useful}");
    }

    #[test]
    fn a_plain_word_rules_out_a_text_without_it() {
        let filter = TrigramFilter::of_text("nothing of the kind here\n");
        for query in ["zzunique", "ZZUNIQUE"] {
            let q = Query::new(query, FindOptions::default()).unwrap();
            assert!(!q.needs().may_match(&filter));
        }
        let word = Query::new("kind", FindOptions::default()).unwrap();
        assert!(word.needs().may_match(&filter));
    }

    #[test]
    fn a_regex_with_a_word_in_it_needs_that_word_and_alternatives_need_one() {
        let regex = FindOptions {
            regex: true,
            ..FindOptions::default()
        };
        let has_foo = TrigramFilter::of_text("xx foo yy");
        let neither = TrigramFilter::of_text("xx yy zz qq");
        for query in [
            r"fooo+|barbar",
            r"\bfoo\w*",
            r"(foo)\s",
            r"(?:foo|bazbaz)x?",
        ] {
            let q = Query::new(query, regex).unwrap();
            assert!(!q.needs().may_match(&neither), "{query}");
        }
        let q = Query::new(r"\bfoo\w*", regex).unwrap();
        assert!(q.needs().may_match(&has_foo));
    }

    #[test]
    fn a_regex_with_nothing_to_hold_on_to_reads_everything() {
        let regex = FindOptions {
            regex: true,
            ..FindOptions::default()
        };
        for query in [r"\d+", r".*", r"^$", r"a?b?c?", r"ab", r"[a-z]{4}"] {
            let q = Query::new(query, regex).unwrap();
            assert_eq!(*q.needs(), TextFilter::Anything, "{query}");
        }
    }

    #[test]
    fn a_huge_text_keeps_a_bounded_summary_that_still_never_says_no_wrongly() {
        let mut rng = Rng(99);
        let body: String = (0..400_000)
            .map(|_| (b'a' + rng.below(26) as u8) as char)
            .collect();
        let filter = TrigramFilter::of_text(&body);
        assert!(filter.as_bytes().len() <= 2 * 1024);
        let q = Query::new(&body[1000..1040], FindOptions::default()).unwrap();
        assert!(q.needs().may_match(&filter));
    }
}

#[cfg(test)]
mod crlf_tests {
    use super::*;

    #[test]
    fn a_match_starting_at_the_carriage_return_does_not_panic() {
        let regex = FindOptions {
            regex: true,
            ..FindOptions::default()
        };
        let hits = Query::new(r"\r\nb", regex).unwrap().line_hits("a\r\nb\n");
        assert_eq!(hits.len(), 1);
        assert_eq!((hits[0].line, hits[0].col, hits[0].len), (0, 1, 0));
    }
}
