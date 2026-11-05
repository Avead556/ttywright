use std::collections::BTreeMap;
use std::ops::Range;
use std::time::{Duration, Instant};

use tree_sitter::{Node, ParseOptions, ParseState, Parser, Tree};

use super::languages::Language;
use super::syntax::tree_language;

const PLACEHOLDER: &str = "__idea_hole_";
const PARSE_BUDGET: Duration = Duration::from_secs(2);
const MAX_BYTES: usize = 5 * 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Pattern {
    source: String,
    holes: Vec<String>,
}

fn is_hole_name(name: &str) -> bool {
    !name.is_empty()
        && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        && !name.starts_with(|c: char| c.is_ascii_digit())
}

fn holes_in(text: &str) -> Vec<(Range<usize>, &str)> {
    let mut found = Vec::new();
    let mut from = 0;
    while let Some(open) = text[from..].find('$').map(|i| i + from) {
        let Some(close) = text[open + 1..].find('$').map(|i| i + open + 1) else {
            break;
        };
        let name = &text[open + 1..close];
        if is_hole_name(name) {
            found.push((open..close + 1, name));
            from = close + 1;
        } else {
            from = open + 1;
        }
    }
    found
}

impl Pattern {
    pub fn new(text: &str) -> Result<Self, String> {
        let text = text.trim().trim_end_matches(';').trim_end();
        if text.is_empty() {
            return Err("Type a pattern, such as foo($X$)".to_owned());
        }
        let mut holes: Vec<String> = Vec::new();
        let mut source = String::new();
        let mut copied = 0;
        let found = holes_in(text);
        for (range, name) in &found {
            source.push_str(&text[copied..range.start]);
            copied = range.end;
            let index = holes.iter().position(|h| h == name).unwrap_or_else(|| {
                holes.push((*name).to_owned());
                holes.len() - 1
            });
            source.push_str(&format!("{PLACEHOLDER}{index}__"));
        }
        source.push_str(&text[copied..]);
        if found.len() == 1 && found[0].0 == (0..text.len()) {
            return Err("A pattern of one hole would match everything".to_owned());
        }
        Ok(Self { source, holes })
    }

    pub fn check_template(&self, template: &str) -> Result<(), String> {
        for (_, name) in holes_in(template) {
            if !self.holes.iter().any(|h| h == name) {
                return Err(format!("${name}$ is not in the pattern"));
            }
        }
        Ok(())
    }

    pub fn compile(&self, language: Language) -> Option<Compiled> {
        wrappers(language).iter().find_map(|(before, after)| {
            let text = format!("{before}{}{after}", self.source);
            let tree = parse(language, &text)?;
            if tree.root_node().has_error() {
                return None;
            }
            let at = before.len()..before.len() + self.source.len();
            tree.root_node()
                .descendant_for_byte_range(at.start, at.end)?;
            Some(Compiled {
                language,
                text,
                at,
                tree,
                holes: self.holes.clone(),
            })
        })
    }
}

fn wrappers(language: Language) -> &'static [(&'static str, &'static str)] {
    match language {
        Language::Rust => &[("", ""), ("fn __w() { ", "; }"), ("fn __w() { ", " }")],
        Language::Go => &[("package p\n", "\n"), ("package p\nfunc __w() { ", " }")],
        Language::Java => &[
            ("", ""),
            ("class __W { ", " }"),
            ("class __W { void __w() { ", "; } }"),
            ("class __W { void __w() { ", " } }"),
        ],
        Language::Php => &[("<?php ", ";"), ("<?php ", "")],
        Language::JavaScript | Language::Jsx | Language::TypeScript | Language::Tsx => {
            &[("", ""), ("(", ")"), ("function __w() { ", " }")]
        }
        _ => &[("", "")],
    }
}

fn parse(language: Language, text: &str) -> Option<Tree> {
    let grammar = tree_language(language)?;
    let mut parser = Parser::new();
    parser.set_language(&grammar).ok()?;
    let started = Instant::now();
    let mut out_of_time = |_: &ParseState| started.elapsed() > PARSE_BUDGET;
    let options = ParseOptions::new().progress_callback(&mut out_of_time);
    let bytes = text.as_bytes();
    let mut read = |at: usize, _: tree_sitter::Point| -> &[u8] { bytes.get(at..).unwrap_or(&[]) };
    parser.parse_with_options(&mut read, None, Some(options))
}

pub struct Compiled {
    language: Language,
    text: String,
    at: Range<usize>,
    tree: Tree,
    holes: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Found {
    pub bytes: Range<usize>,
    pub holes: BTreeMap<String, String>,
}

impl Compiled {
    fn root(&self) -> Option<Node<'_>> {
        self.tree
            .root_node()
            .descendant_for_byte_range(self.at.start, self.at.end)
    }

    fn hole_of(&self, node: Node<'_>) -> Option<usize> {
        if node.child_count() > 0 {
            return None;
        }
        let text = self.text.get(node.byte_range())?;
        text.strip_prefix(PLACEHOLDER)?
            .strip_suffix("__")?
            .parse()
            .ok()
    }

    fn matches(
        &self,
        pattern: Node<'_>,
        node: Node<'_>,
        source: &str,
        taken: &mut Vec<Option<(usize, usize)>>,
    ) -> bool {
        if let Some(hole) = self.hole_of(pattern) {
            if !node.is_named() {
                return false;
            }
            let here = source.get(node.byte_range()).unwrap_or("");
            return match taken.get(hole).copied().flatten() {
                Some((from, to)) => source.get(from..to) == Some(here),
                None => {
                    if let Some(slot) = taken.get_mut(hole) {
                        *slot = Some((node.start_byte(), node.end_byte()));
                    }
                    true
                }
            };
        }
        if pattern.kind_id() != node.kind_id() {
            return false;
        }
        let parts = |n: Node<'_>| -> Vec<usize> {
            (0..n.child_count())
                .filter(|i| n.child(*i).is_some_and(|c| !c.is_extra()))
                .collect()
        };
        let (ours, theirs) = (parts(pattern), parts(node));
        if ours.len() != theirs.len() {
            return false;
        }
        if ours.is_empty() {
            return self.text.get(pattern.byte_range()) == source.get(node.byte_range());
        }
        ours.iter()
            .zip(&theirs)
            .all(|(a, b)| match (pattern.child(*a), node.child(*b)) {
                (Some(p), Some(n)) => self.matches(p, n, source, taken),
                _ => false,
            })
    }

    pub fn find(&self, source: &str) -> Vec<Found> {
        let mut found = Vec::new();
        if source.len() > MAX_BYTES {
            return found;
        }
        let (Some(pattern), Some(tree)) = (self.root(), parse(self.language, source)) else {
            return found;
        };
        let mut cursor = tree.walk();
        loop {
            let node = cursor.node();
            let mut taken = vec![None; self.holes.len()];
            let hit = !node.is_extra() && self.matches(pattern, node, source, &mut taken);
            if hit {
                found.push(Found {
                    bytes: node.byte_range(),
                    holes: self
                        .holes
                        .iter()
                        .zip(&taken)
                        .filter_map(|(name, range)| {
                            let (from, to) = (*range)?;
                            Some((name.clone(), source.get(from..to)?.to_owned()))
                        })
                        .collect(),
                });
            } else if cursor.goto_first_child() {
                continue;
            }
            while !cursor.goto_next_sibling() {
                if !cursor.goto_parent() {
                    return found;
                }
            }
        }
    }
}

pub fn expand(template: &str, holes: &BTreeMap<String, String>) -> String {
    let mut out = String::new();
    let mut copied = 0;
    for (range, name) in holes_in(template) {
        out.push_str(&template[copied..range.start]);
        out.push_str(holes.get(name).map_or("", String::as_str));
        copied = range.end;
    }
    out.push_str(&template[copied..]);
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn found(language: Language, pattern: &str, source: &str) -> Vec<String> {
        let compiled = Pattern::new(pattern).unwrap().compile(language);
        let compiled = compiled.unwrap_or_else(|| panic!("{language:?}: {pattern}"));
        compiled
            .find(source)
            .into_iter()
            .map(|f| source[f.bytes].to_owned())
            .collect()
    }

    #[test]
    fn spacing_comments_and_line_breaks_do_not_matter() {
        let source = "fn a() { foo( 1 ,\n  /* why */ 2 ); bar(1, 2); foo(1); }";
        assert_eq!(
            found(Language::Rust, "foo($A$, $B$)", source),
            ["foo( 1 ,\n  /* why */ 2 )"]
        );
    }

    #[test]
    fn a_hole_takes_a_whole_expression_and_the_same_name_the_same_text() {
        let source = "fn a() { x(g(1) + g(1)); x(g(1) + g(2)); }";
        assert_eq!(found(Language::Rust, "$A$ + $A$", source), ["g(1) + g(1)"]);
        assert_eq!(found(Language::Rust, "$A$ + $B$", source).len(), 2);
    }

    #[test]
    fn each_language_takes_a_pattern_of_its_own_code() {
        let cases = [
            (
                Language::Rust,
                "$X$.unwrap()",
                "fn a() { b().unwrap(); }",
                "b().unwrap()",
            ),
            (
                Language::Python,
                "print($X$)",
                "def a():\n    print(1)\n",
                "print(1)",
            ),
            (
                Language::Go,
                "fmt.Println($X$)",
                "package a\nfunc b() { fmt.Println(1) }\n",
                "fmt.Println(1)",
            ),
            (
                Language::JavaScript,
                "console.log($X$)",
                "function a() { console.log(1); }",
                "console.log(1)",
            ),
            (
                Language::TypeScript,
                "$A$ === null",
                "const b = c === null;",
                "c === null",
            ),
            (
                Language::Java,
                "$X$.size()",
                "class A { int b() { return c.size(); } }",
                "c.size()",
            ),
            (
                Language::Php,
                "strlen($X$)",
                "<?php\n$a = strlen($b);\n",
                "strlen($b)",
            ),
            (
                Language::Bash,
                "echo $X$",
                "if true; then\n echo hi\nfi\n",
                "echo hi",
            ),
        ];
        for (language, pattern, source, expected) in cases {
            assert_eq!(found(language, pattern, source), [expected], "{language:?}");
        }
    }

    #[test]
    fn only_the_outermost_of_nested_matches_is_taken() {
        let source = "fn a() { f(f(1)); }";
        assert_eq!(found(Language::Rust, "f($X$)", source), ["f(f(1))"]);
    }

    #[test]
    fn a_word_in_a_string_or_comment_is_not_code() {
        let source = "fn a() { let s = \"foo(1)\"; /* foo(1) */ }";
        assert!(found(Language::Rust, "foo($X$)", source).is_empty());
    }

    #[test]
    fn what_cannot_be_a_pattern_is_refused() {
        assert!(Pattern::new("   ").is_err());
        assert!(Pattern::new("$A$").is_err());
        let not_rust = Pattern::new("fn (((").unwrap();
        assert!(not_rust.compile(Language::Rust).is_none());
    }

    #[test]
    fn a_language_without_a_grammar_finds_nothing() {
        assert!(
            Pattern::new("a($X$)")
                .unwrap()
                .compile(Language::PlainText)
                .is_none()
        );
    }

    #[test]
    fn the_replacement_uses_what_each_hole_took_and_only_known_holes() {
        let pattern = Pattern::new("foo($A$, $B$)").unwrap();
        assert!(pattern.check_template("bar($B$, $A$)").is_ok());
        assert!(pattern.check_template("bar($C$)").is_err());
        let compiled = pattern.compile(Language::Rust).unwrap();
        let hits = compiled.find("fn a() { foo(1 + 2, g()); }");
        assert_eq!(expand("bar($B$, $A$)", &hits[0].holes), "bar(g(), 1 + 2)");
    }

    #[test]
    fn deep_nesting_does_not_overflow_the_stack() {
        let source = format!(
            "fn a() {{ {}1{}; }}",
            "(".repeat(20_000),
            ")".repeat(20_000)
        );
        let _ = found(Language::Rust, "foo($X$)", &source);
    }
}
