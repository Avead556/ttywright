use std::collections::{BTreeMap, BTreeSet};
use std::fmt;
use std::ops::Range;
use std::sync::{Mutex, PoisonError};
use std::time::{Duration, Instant, UNIX_EPOCH};

use ropey::Rope;
use tree_sitter::{
    InputEdit, Language as TsLanguage, ParseOptions, ParseState, Parser, Point, Query, QueryCursor,
    StreamingIterator, Tree,
};

use super::languages::{self, Language, Overlap};

pub const MAX_HIGHLIGHT_BYTES: usize = 5 * 1024 * 1024;
const REPARSE_BUDGET: Duration = Duration::from_millis(100);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Span {
    pub chars: Range<usize>,
    pub capture: &'static str,
}

struct Grammar {
    language: TsLanguage,
    query: Query,
    overlap: Overlap,
    names: Vec<&'static str>,
}

static GRAMMARS: Mutex<BTreeMap<Language, Result<&'static Grammar, String>>> =
    Mutex::new(BTreeMap::new());

#[cfg(not(test))]
static WANTED: Mutex<BTreeSet<Language>> = Mutex::new(BTreeSet::new());

fn remember(lang: Language, loaded: Result<Grammar, String>) -> Result<&'static Grammar, String> {
    let mut grammars = GRAMMARS.lock().unwrap_or_else(PoisonError::into_inner);
    grammars
        .entry(lang)
        .or_insert_with(|| loaded.map(|g| &*Box::leak(Box::new(g))))
        .clone()
}

fn known(lang: Language) -> Option<Result<&'static Grammar, String>> {
    let grammars = GRAMMARS.lock().unwrap_or_else(PoisonError::into_inner);
    grammars.get(&lang).cloned()
}

fn load(lang: Language) -> Option<Result<&'static Grammar, String>> {
    if let Some(known) = known(lang) {
        return Some(known);
    }
    Some(remember(lang, load_grammar(lang.spec().grammar.as_ref()?)))
}

fn grammar(lang: Language) -> Option<&'static Grammar> {
    load(lang)?.ok()
}

fn grammar_without_waiting(lang: Language) -> Option<&'static Grammar> {
    let spec = lang.spec().grammar.as_ref()?;
    if pending_in_test(lang) {
        note_wanted(lang);
        return None;
    }
    if spec.builtin.is_some() {
        return grammar(lang);
    }
    match known(lang) {
        Some(Ok(grammar)) => Some(grammar),
        Some(Err(_)) | None => {
            note_wanted(lang);
            None
        }
    }
}

#[cfg(not(test))]
fn note_wanted(lang: Language) {
    static ASKED: Mutex<BTreeSet<Language>> = Mutex::new(BTreeSet::new());
    if ASKED
        .lock()
        .unwrap_or_else(PoisonError::into_inner)
        .insert(lang)
    {
        WANTED
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .insert(lang);
    }
}

#[cfg(not(test))]
pub fn take_wanted() -> Vec<Language> {
    let mut wanted = WANTED.lock().unwrap_or_else(PoisonError::into_inner);
    std::mem::take(&mut *wanted).into_iter().collect()
}

pub fn load_module(lang: Language) -> Result<(), String> {
    match load(lang) {
        Some(Err(error)) => Err(error),
        _ => Ok(()),
    }
}

pub fn modules_fingerprint() -> String {
    let mut out = String::new();
    for row in languages::LANGUAGES {
        let Some(grammar) = row.grammar.as_ref().filter(|g| g.builtin.is_none()) else {
            continue;
        };
        let file = crate::modules::grammar(grammar.module).and_then(|p| p.metadata().ok());
        let stamp = file.as_ref().map(|m| {
            let at = m
                .modified()
                .ok()
                .and_then(|t| t.duration_since(UNIX_EPOCH).ok());
            (m.len(), at.map(|t| t.as_nanos()))
        });
        out.push_str(&format!("{}={stamp:?};", grammar.module));
    }
    out
}

#[cfg(test)]
thread_local! {
    static PENDING_IN_TEST: std::cell::Cell<Option<Language>> =
        const { std::cell::Cell::new(None) };
}

#[cfg(test)]
thread_local! {
    static WANTED_IN_TEST: std::cell::RefCell<BTreeSet<Language>> =
        const { std::cell::RefCell::new(BTreeSet::new()) };
}

#[cfg(test)]
fn note_wanted(lang: Language) {
    WANTED_IN_TEST.with(|w| w.borrow_mut().insert(lang));
}

#[cfg(test)]
pub fn take_wanted() -> Vec<Language> {
    WANTED_IN_TEST.with(|w| std::mem::take(&mut *w.borrow_mut()).into_iter().collect())
}

#[cfg(test)]
pub(crate) fn pretend_pending(lang: Option<Language>) {
    PENDING_IN_TEST.with(|p| p.set(lang));
}

#[cfg(test)]
fn pending_in_test(lang: Language) -> bool {
    PENDING_IN_TEST.with(|p| p.get()) == Some(lang)
}

#[cfg(not(test))]
const fn pending_in_test(_lang: Language) -> bool {
    false
}

fn load_grammar(spec: &languages::Grammar) -> Result<Grammar, String> {
    let language = match spec.builtin {
        Some(load) => load(),
        None => {
            let path = crate::modules::grammar(spec.module)
                .ok_or("the program cannot find its own folder")?;
            ttywright_grammar_loader::load(&path)
                .map_err(|e| format!("lib/grammars/{}: {e}", spec.module))?
        }
    };
    let query = Query::new(&language, &spec.queries.concat())
        .map_err(|e| format!("lib/grammars/{}: {e}", spec.module))?;
    let names = query
        .capture_names()
        .iter()
        .map(|n| &*Box::leak((*n).to_owned().into_boxed_str()))
        .collect();
    Ok(Grammar {
        language,
        query,
        overlap: spec.overlap,
        names,
    })
}

#[derive(Clone, Copy)]
struct Claim {
    capture: u32,
    node_len: usize,
    pattern: usize,
}

impl Claim {
    fn beats(self, other: Self, overlap: Overlap) -> bool {
        self.node_len < other.node_len
            || (self.node_len == other.node_len
                && match overlap {
                    Overlap::FirstPatternWins => self.pattern < other.pattern,
                    Overlap::LastPatternWins => self.pattern > other.pattern,
                })
    }
}

#[cfg(test)]
pub(crate) fn capture_names(lang: Language) -> Vec<&'static str> {
    grammar(lang).map_or_else(Vec::new, |g| g.names.clone())
}

pub struct Syntax {
    grammar: &'static Grammar,
    parser: Parser,
    tree: Option<Tree>,
}

pub fn tree_language(lang: Language) -> Option<tree_sitter::Language> {
    grammar(lang).map(|g| g.language.clone())
}

fn is_declaration(kind: &str) -> bool {
    [
        "declaration",
        "definition",
        "item",
        "spec",
        "declarator",
        "variant",
        "signature",
    ]
    .iter()
    .any(|word| kind.contains(word))
}

fn wraps_declaration(kind: &str, node: &tree_sitter::Node<'_>) -> bool {
    match kind {
        "decorated_definition" | "export_statement" => true,
        "type_declaration"
        | "lexical_declaration"
        | "variable_declaration"
        | "field_declaration" => {
            let mut walk = node.walk();
            node.named_children(&mut walk)
                .filter(|c| !c.kind().contains("comment"))
                .filter(|c| c.child_by_field_name("name").is_some() || c.kind().ends_with("spec"))
                .count()
                == 1
        }
        _ => false,
    }
}

impl fmt::Debug for Syntax {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("Syntax")
            .field("parsed", &self.tree.is_some())
            .finish()
    }
}

impl Syntax {
    pub fn new(lang: Language, text: &Rope) -> Option<Self> {
        if text.len_bytes() > MAX_HIGHLIGHT_BYTES {
            return None;
        }
        let grammar = grammar_without_waiting(lang)?;
        let mut parser = Parser::new();
        parser.set_language(&grammar.language).ok()?;
        let mut syntax = Self {
            grammar,
            parser,
            tree: None,
        };
        syntax.reparse(text);
        Some(syntax)
    }

    pub fn declaration_at(&self, text: &Rope, char_idx: usize) -> Option<Range<usize>> {
        let byte = text.char_to_byte(char_idx);
        let name = self
            .tree
            .as_ref()?
            .root_node()
            .descendant_for_byte_range(byte, byte)?;
        let mut node = name.parent()?;
        if node.child_by_field_name("name")?.id() != name.id() || !is_declaration(node.kind()) {
            return None;
        }
        while let Some(parent) = node.parent().filter(|p| wraps_declaration(p.kind(), p)) {
            node = parent;
        }
        let mut first = node;
        while let Some(above) = first
            .prev_named_sibling()
            .filter(|s| s.kind().contains("comment") || s.kind().contains("attribute"))
            .filter(|s| s.end_position().row + 1 >= first.start_position().row)
        {
            first = above;
        }
        let from = text.byte_to_char(first.start_byte());
        let to = text.byte_to_char(node.end_byte());
        let line_start = text.line_to_char(text.char_to_line(from));
        let before_is_blank = text
            .slice(line_start..from)
            .chars()
            .all(char::is_whitespace);
        let last_line = text.char_to_line(to);
        let line_end = text.line_to_char(last_line) + text.line(last_line).len_chars();
        let after_is_blank = text.slice(to..line_end).chars().all(char::is_whitespace);
        Some(if before_is_blank && after_is_blank {
            line_start..line_end
        } else {
            from..to
        })
    }

    pub fn edit(&mut self, edit: &InputEdit) {
        if let Some(tree) = &mut self.tree {
            tree.edit(edit);
        }
    }

    pub fn reparse(&mut self, text: &Rope) {
        self.reparse_within(text, REPARSE_BUDGET);
    }

    fn reparse_within(&mut self, text: &Rope, budget: Duration) -> bool {
        let mut read = |byte: usize, _: Point| -> &[u8] {
            if byte >= text.len_bytes() {
                return &[];
            }
            let (chunk, start, _, _) = text.chunk_at_byte(byte);
            &chunk.as_bytes()[byte - start..]
        };
        let started = Instant::now();
        let mut out_of_time = |_: &ParseState| started.elapsed() > budget;
        let options = self
            .tree
            .is_some()
            .then(|| ParseOptions::new().progress_callback(&mut out_of_time));
        match self
            .parser
            .parse_with_options(&mut read, self.tree.as_ref(), options)
        {
            Some(tree) => {
                self.tree = Some(tree);
                true
            }
            None => {
                self.parser.reset();
                false
            }
        }
    }

    pub fn highlight(&self, text: &Rope, lines: Range<usize>) -> Vec<Vec<Span>> {
        let lines = lines.start..lines.end.min(text.len_lines());
        let mut out = vec![Vec::new(); lines.len()];
        let Some(tree) = &self.tree else {
            return out;
        };
        let start_byte = text.line_to_byte(lines.start);
        let end_byte = text.line_to_byte(lines.end);
        let mut assigned: Vec<Option<Claim>> = vec![None; end_byte - start_byte];
        let mut cursor = QueryCursor::new();
        cursor.set_byte_range(start_byte..end_byte);
        let provider = |node: tree_sitter::Node<'_>| {
            text.get_byte_slice(node.byte_range())
                .into_iter()
                .flat_map(|slice| slice.chunks())
                .map(str::as_bytes)
        };
        let mut captures = cursor.captures(&self.grammar.query, tree.root_node(), provider);
        while let Some((m, i)) = captures.next() {
            let capture = m.captures[*i];
            let range = capture.node.byte_range();
            let from = range.start.max(start_byte) - start_byte;
            let to = range.end.min(end_byte).saturating_sub(start_byte);
            let claim = Claim {
                capture: capture.index,
                node_len: range.len(),
                pattern: m.pattern_index,
            };
            for slot in assigned.iter_mut().take(to).skip(from) {
                if slot.is_none_or(|held| claim.beats(held, self.grammar.overlap)) {
                    *slot = Some(claim);
                }
            }
        }
        for (row, line) in lines.clone().enumerate() {
            let line_start = text.line_to_byte(line) - start_byte;
            let slice = text.line(line);
            let mut spans: Vec<Span> = Vec::new();
            let mut byte = line_start;
            for (col, ch) in slice.chars().enumerate() {
                if let Some(Some(claim)) = assigned.get(byte) {
                    let capture = self.grammar.names[claim.capture as usize];
                    match spans.last_mut() {
                        Some(last) if last.chars.end == col && last.capture == capture => {
                            last.chars.end = col + 1;
                        }
                        _ => spans.push(Span {
                            chars: col..col + 1,
                            capture,
                        }),
                    }
                }
                byte += ch.len_utf8();
            }
            out[row] = spans;
        }
        out
    }
}

fn point(text: &Rope, byte: usize) -> Point {
    let row = text.byte_to_line(byte);
    Point {
        row,
        column: byte - text.line_to_byte(row),
    }
}

pub struct PendingEdit {
    start_byte: usize,
    old_end_byte: usize,
    start_position: Point,
    old_end_position: Point,
}

pub fn begin_edit(text: &Rope, from: usize, to: usize) -> PendingEdit {
    let start_byte = text.char_to_byte(from);
    let old_end_byte = text.char_to_byte(to);
    PendingEdit {
        start_byte,
        old_end_byte,
        start_position: point(text, start_byte),
        old_end_position: point(text, old_end_byte),
    }
}

pub fn finish_edit(pending: PendingEdit, text: &Rope, new_end: usize) -> InputEdit {
    let new_end_byte = text.char_to_byte(new_end);
    InputEdit {
        start_byte: pending.start_byte,
        old_end_byte: pending.old_end_byte,
        new_end_byte,
        start_position: pending.start_position,
        old_end_position: pending.old_end_position,
        new_end_position: point(text, new_end_byte),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::lang::languages::LANGUAGES;

    fn captures_on(syntax: &Syntax, text: &Rope, line: usize) -> Vec<(String, &'static str)> {
        let line_text = text.line(line).to_string();
        syntax.highlight(text, line..line + 1)[0]
            .iter()
            .map(|s| {
                (
                    line_text
                        .chars()
                        .skip(s.chars.start)
                        .take(s.chars.len())
                        .collect(),
                    s.capture,
                )
            })
            .collect()
    }

    #[test]
    fn highlights_rust_keywords_strings_and_functions() {
        let text = Rope::from_str("fn main() {\n    let s = \"hi\";\n}\n");
        let syntax = Syntax::new(Language::Rust, &text).unwrap();
        let line0 = captures_on(&syntax, &text, 0);
        assert!(line0.contains(&("fn".into(), "keyword")), "{line0:?}");
        assert!(
            line0
                .iter()
                .any(|(t, c)| t == "main" && c.starts_with("function")),
            "{line0:?}"
        );
        let line1 = captures_on(&syntax, &text, 1);
        assert!(line1.contains(&("\"hi\"".into(), "string")), "{line1:?}");
    }

    #[test]
    fn incremental_edit_matches_a_fresh_parse() {
        let mut text = Rope::from_str("fn a() {}\n");
        let mut syntax = Syntax::new(Language::Rust, &text).unwrap();
        let at = 8;
        let pending = begin_edit(&text, at, at);
        text.insert(at, "\"ж\"");
        syntax.edit(&finish_edit(pending, &text, at + 3));
        syntax.reparse(&text);
        let fresh = Syntax::new(Language::Rust, &text).unwrap();
        assert_eq!(syntax.highlight(&text, 0..1), fresh.highlight(&text, 0..1));
        assert!(captures_on(&syntax, &text, 0).contains(&("\"ж\"".into(), "string")));
    }

    #[test]
    fn plain_text_and_huge_files_have_no_syntax() {
        assert!(Syntax::new(Language::PlainText, &Rope::from_str("x")).is_none());
        let big = Rope::from_str(&"a".repeat(MAX_HIGHLIGHT_BYTES + 1));
        assert!(Syntax::new(Language::Rust, &big).is_none());
    }

    #[test]
    fn every_bundled_grammar_and_query_loads() {
        for row in LANGUAGES {
            let spec = row.grammar.as_ref().unwrap();
            let language = spec.builtin.expect("these tests run with them")();
            let source = spec.queries.concat();
            if let Err(e) = Query::new(&language, &source) {
                panic!("{}: {e}", row.name);
            }
            assert!(grammar(row.language).is_some(), "{}", row.name);
            let mut parser = Parser::new();
            parser.set_language(&language).unwrap();
        }
    }

    #[test]
    fn every_grammar_has_a_module_that_the_release_folder_builds() {
        let features = include_str!("../../crates/grammar-module/Cargo.toml");
        let script = include_str!("../../scripts/build-release.sh");
        for row in LANGUAGES {
            let module = row.grammar.as_ref().unwrap().module;
            assert!(features.contains(&format!("\n{module} = [")), "{module}");
            assert!(
                script.contains(&format!(" {module} ")) || script.contains(&format!(" {module};")),
                "{module}"
            );
        }
    }

    #[test]
    fn the_fingerprint_of_built_in_grammars_never_changes() {
        assert_eq!(modules_fingerprint(), "");
    }

    #[test]
    fn a_query_that_does_not_compile_means_no_highlighting_not_a_panic() {
        let broken = languages::Grammar {
            module: "json",
            builtin: Some(|| tree_sitter_json::LANGUAGE.into()),
            queries: &["(no_such_node) @x"],
            overlap: Overlap::LastPatternWins,
        };
        assert!(load_grammar(&broken).is_err());
        let bad_syntax = languages::Grammar {
            module: "json",
            builtin: Some(|| tree_sitter_json::LANGUAGE.into()),
            queries: &["(("],
            overlap: Overlap::LastPatternWins,
        };
        assert!(load_grammar(&bad_syntax).is_err());
    }

    fn capture_of(lang: Language, source: &str, word: &str) -> String {
        let text = Rope::from_str(source);
        let syntax = Syntax::new(lang, &text).unwrap();
        let (line, column) = source
            .lines()
            .enumerate()
            .find_map(|(i, l)| l.find(word).map(|at| (i, l[..at].chars().count())))
            .unwrap_or_else(|| panic!("{word} is not in the source"));
        syntax.highlight(&text, line..line + 1)[0]
            .iter()
            .find(|s| s.chars.contains(&column))
            .map_or("none".to_owned(), |s| s.capture.to_owned())
    }

    #[test]
    fn each_language_colors_its_own_code() {
        let php = "<?php\nclass Foo {\n  public function bar(int $x): string { return \"s\"; }\n  function baz(Foo $a) {}\n}\necho strlen($y); // note\n";
        let js = "// note\nconst MAX_ITEMS = 1;\nfunction load(arg) { return `a${arg}b`; }\nclass Item {}\nload(2);\nconst el = <Box size=\"1\">t</Box>;\n";
        let ts = "// note\ninterface Shape { area: number }\nfunction draw(shape: Shape): void { return; }\nconst s = \"x\";\n";
        let tsx = "function app(p: Props) { return <Box size=\"1\"><div /></Box>; }\n";
        let java =
            "// note\nclass Foo {\n  @Override\n  public int bar(String s) { return baz(1); }\n}\n";
        let go = "package main\n// note\nfunc run(x int) string {\n\treturn helper(\"s\\n\")\n}\n";
        let python = "# note\nclass Foo:\n    def bar(self):\n        return helper(\"s\", 1)\n";
        let bash = "#!/bin/bash\n# note\nfor x in 1; do\n  echo \"s\" $x\ndone\nfn() { ls; }\n";
        let cases: &[(Language, &str, &str, &str)] = &[
            (Language::Php, php, "class", "keyword"),
            (Language::Php, php, "Foo $a", "type"),
            (Language::Php, php, "bar", "function.method"),
            (Language::Php, php, "int", "type.builtin"),
            (Language::Php, php, "x)", "variable"),
            (Language::Php, php, "<?php", "tag"),
            (Language::Php, php, "\"s\"", "string"),
            (Language::Php, php, "strlen", "function"),
            (Language::Php, php, "// note", "comment"),
            (Language::JavaScript, js, "// note", "comment"),
            (Language::JavaScript, js, "const", "keyword"),
            (Language::JavaScript, js, "MAX_ITEMS", "constant"),
            (Language::JavaScript, js, "load", "function"),
            (Language::JavaScript, js, "arg", "variable"),
            (Language::JavaScript, js, "Item", "constructor"),
            (Language::JavaScript, js, "${", "punctuation.special"),
            (Language::JavaScript, js, "`a", "string"),
            (Language::JavaScript, js, "2", "number"),
            (Language::Jsx, js, "<Box", "punctuation.bracket"),
            (Language::Jsx, js, "Box", "constructor"),
            (Language::TypeScript, ts, "interface", "keyword"),
            (Language::TypeScript, ts, "Shape", "type"),
            (Language::TypeScript, ts, "number", "type.builtin"),
            (Language::TypeScript, ts, "draw", "function"),
            (Language::TypeScript, ts, "shape:", "variable.parameter"),
            (Language::TypeScript, ts, "\"x\"", "string"),
            (Language::Tsx, tsx, "Props", "type"),
            (Language::Tsx, tsx, "app", "function"),
            (Language::Tsx, tsx, "Box", "type"),
            (Language::Tsx, tsx, "div", "tag"),
            (Language::Java, java, "class", "keyword"),
            (Language::Java, java, "Foo", "type"),
            (Language::Java, java, "Override", "attribute"),
            (Language::Java, java, "bar", "function.method"),
            (Language::Java, java, "baz", "function.method"),
            (Language::Java, java, "int", "type.builtin"),
            (Language::Java, java, "// note", "comment"),
            (Language::Go, go, "func", "keyword"),
            (Language::Go, go, "run", "function"),
            (Language::Go, go, "helper", "function"),
            (Language::Go, go, "x int", "variable"),
            (Language::Go, go, "int", "type"),
            (Language::Go, go, "\"s", "string"),
            (Language::Go, go, "\\n", "escape"),
            (Language::Go, go, "// note", "comment"),
            (Language::Python, python, "class", "keyword"),
            (Language::Python, python, "Foo", "constructor"),
            (Language::Python, python, "bar", "function"),
            (Language::Python, python, "helper", "function"),
            (Language::Python, python, "self", "variable"),
            (Language::Python, python, "\"s\"", "string"),
            (Language::Python, python, "# note", "comment"),
            (Language::Bash, bash, "for", "keyword"),
            (Language::Bash, bash, "echo", "function"),
            (Language::Bash, bash, "\"s\"", "string"),
            (Language::Bash, bash, "x in", "property"),
            (Language::Bash, bash, "# note", "comment"),
        ];
        let wrong: Vec<String> = cases
            .iter()
            .filter_map(|(language, source, word, expected)| {
                let got = capture_of(*language, source, word);
                (got != *expected).then(|| format!("{language:?} {word}: {got}, not {expected}"))
            })
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
    }

    #[test]
    fn a_parse_out_of_time_keeps_the_old_colors_and_catches_up_later() {
        let source: String = (0..20_000)
            .map(|i| format!("class C{i} {{ int m() {{ return {i}; }} }}\n"))
            .collect();
        let mut text = Rope::from_str(&source);
        let mut syntax = Syntax::new(Language::Java, &text).unwrap();
        let line = 10_000;
        let at = text.line_to_char(line) + 2;
        let pending = begin_edit(&text, at, at);
        text.insert(at, "\"ж");
        syntax.edit(&finish_edit(pending, &text, at + 2));
        assert!(
            !syntax.reparse_within(&text, Duration::ZERO),
            "too big to parse in no time"
        );
        let stale = syntax.highlight(&text, line..line + 1);
        assert!(!stale[0].is_empty(), "still colored");
        let at = text.line_to_char(line + 1);
        let pending = begin_edit(&text, at, at);
        text.insert(at, "class X {}\n");
        syntax.edit(&finish_edit(pending, &text, at + 11));
        assert!(syntax.reparse_within(&text, Duration::from_secs(600)));
        let fresh = Syntax::new(Language::Java, &text).unwrap();
        assert_eq!(
            syntax.highlight(&text, line..line + 2),
            fresh.highlight(&text, line..line + 2)
        );
    }

    #[test]
    fn editing_every_language_stays_equal_to_a_fresh_parse() {
        for row in LANGUAGES {
            let mut text = Rope::from_str("x = 1\n\"s\" // ж\n");
            let mut syntax = Syntax::new(row.language, &text).unwrap();
            let pending = begin_edit(&text, 4, 4);
            text.insert(4, "ж 2 ");
            syntax.edit(&finish_edit(pending, &text, 8));
            syntax.reparse(&text);
            let fresh = Syntax::new(row.language, &text).unwrap();
            assert_eq!(
                syntax.highlight(&text, 0..2),
                fresh.highlight(&text, 0..2),
                "{}",
                row.name
            );
        }
    }
}

#[cfg(test)]
mod declaration_tests {
    use super::*;

    fn deleted(lang: Language, source: &str, name: &str) -> Option<String> {
        let text = Rope::from_str(source);
        let at = text.byte_to_char(source.find(name).expect("in the source"));
        let range = Syntax::new(lang, &text)?.declaration_at(&text, at)?;
        let mut after = text.clone();
        after.remove(range);
        Some(after.to_string())
    }

    #[test]
    fn each_language_finds_the_whole_declaration_from_its_name() {
        let cases = [
            (
                Language::Rust,
                "// c\nfn gone() {}\nfn keep() {}\n",
                "gone",
                "fn keep() {}\n",
            ),
            (
                Language::Python,
                "@d\ndef gone():\n    pass\nx = 1\n",
                "gone",
                "x = 1\n",
            ),
            (
                Language::Go,
                "package a\ntype Gone struct{}\nvar x = 1\n",
                "Gone",
                "package a\nvar x = 1\n",
            ),
            (
                Language::JavaScript,
                "export function gone() {}\nlet x;\n",
                "gone",
                "let x;\n",
            ),
            (
                Language::TypeScript,
                "const gone = 1;\nlet x;\n",
                "gone",
                "let x;\n",
            ),
            (
                Language::Java,
                "class A {\n  void gone() {}\n  int x;\n}\n",
                "gone",
                "class A {\n  int x;\n}\n",
            ),
            (
                Language::Php,
                "<?php\nfunction gone() {}\n$x = 1;\n",
                "gone",
                "<?php\n$x = 1;\n",
            ),
            (Language::Bash, "gone() { :; }\nx=1\n", "gone", "x=1\n"),
        ];
        for (lang, source, name, expected) in cases {
            assert_eq!(
                deleted(lang, source, name).as_deref(),
                Some(expected),
                "{lang:?}"
            );
        }
    }

    #[test]
    fn a_use_is_not_a_declaration() {
        assert_eq!(
            deleted(Language::Rust, "fn a() { gone(); }\n", "gone"),
            None
        );
    }

    #[test]
    fn one_of_several_declared_together_goes_alone() {
        assert_eq!(
            deleted(Language::JavaScript, "let a = 1, gone = 2;\n", "gone").as_deref(),
            Some("let a = 1, ;\n")
        );
    }
}
