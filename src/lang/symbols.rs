use std::collections::BTreeMap;
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use tree_sitter::{ParseOptions, ParseState, Parser, Query, QueryCursor, StreamingIterator};

use super::languages::Language;

pub const MAX_SYMBOL_BYTES: usize = 1024 * 1024;
const MAX_FILE_SYMBOLS: usize = 5000;
const MAX_NAME_CHARS: usize = 200;
const PARSE_BUDGET: Duration = Duration::from_millis(200);

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FoundSymbol {
    pub name: String,
    pub kind: &'static str,
    pub depth: usize,
    pub start: (u32, u32),
}

const KINDS: [&str; 15] = [
    "function",
    "method",
    "class",
    "struct",
    "enum",
    "interface",
    "trait",
    "module",
    "constant",
    "variable",
    "type",
    "macro",
    "member",
    "key",
    "heading",
];

pub fn kind_index(kind: &str) -> u8 {
    KINDS
        .iter()
        .position(|k| *k == kind)
        .and_then(|i| u8::try_from(i).ok())
        .unwrap_or(0)
}

pub fn kind_name(index: u8) -> Option<&'static str> {
    KINDS.get(usize::from(index)).copied()
}

const JS_NAMES: &str = r"
(function_declaration name: (identifier) @function)
(generator_function_declaration name: (identifier) @function)
(class_declaration name: (identifier) @class)
(method_definition name: (property_identifier) @method)
(program (lexical_declaration (variable_declarator name: (identifier) @variable)))
(program (variable_declaration (variable_declarator name: (identifier) @variable)))
(program (export_statement (lexical_declaration (variable_declarator name: (identifier) @variable))))
";

const TS_NAMES: &str = r"
(function_declaration name: (identifier) @function)
(generator_function_declaration name: (identifier) @function)
(function_signature name: (identifier) @function)
(class_declaration name: (type_identifier) @class)
(abstract_class_declaration name: (type_identifier) @class)
(interface_declaration name: (type_identifier) @interface)
(type_alias_declaration name: (type_identifier) @type)
(enum_declaration name: (identifier) @enum)
(internal_module name: (identifier) @module)
(method_definition name: (property_identifier) @method)
(abstract_method_signature name: (property_identifier) @method)
(program (lexical_declaration (variable_declarator name: (identifier) @variable)))
(program (export_statement declaration: (lexical_declaration (variable_declarator name: (identifier) @variable))))
";

fn names_query(language: Language) -> Option<&'static str> {
    Some(match language {
        Language::Rust => {
            r"
(function_item name: (identifier) @function)
(function_signature_item name: (identifier) @function)
(struct_item name: (type_identifier) @struct)
(union_item name: (type_identifier) @struct)
(enum_item name: (type_identifier) @enum)
(enum_variant name: (identifier) @member)
(trait_item name: (type_identifier) @trait)
(mod_item name: (identifier) @module)
(const_item name: (identifier) @constant)
(static_item name: (identifier) @constant)
(type_item name: (type_identifier) @type)
(macro_definition name: (identifier) @macro)
"
        }
        Language::Toml => {
            r"
(table (bare_key) @key)
(table (dotted_key) @key)
(table (quoted_key) @key)
(table_array_element (bare_key) @key)
(table_array_element (dotted_key) @key)
(table_array_element (quoted_key) @key)
"
        }
        Language::Markdown => {
            r"
(atx_heading (inline) @heading)
(setext_heading (paragraph (inline) @heading))
"
        }
        Language::Json => "(document (object (pair key: (string (string_content) @key))))",
        Language::Php => {
            r"
(function_definition name: (name) @function)
(class_declaration name: (name) @class)
(interface_declaration name: (name) @interface)
(trait_declaration name: (name) @trait)
(enum_declaration name: (name) @enum)
(method_declaration name: (name) @method)
(namespace_definition name: (namespace_name) @module)
(const_declaration (const_element (name) @constant))
"
        }
        Language::JavaScript | Language::Jsx => JS_NAMES,
        Language::TypeScript | Language::Tsx => TS_NAMES,
        Language::Java => {
            r"
(class_declaration name: (identifier) @class)
(interface_declaration name: (identifier) @interface)
(enum_declaration name: (identifier) @enum)
(record_declaration name: (identifier) @class)
(annotation_type_declaration name: (identifier) @interface)
(method_declaration name: (identifier) @method)
(constructor_declaration name: (identifier) @method)
(enum_constant name: (identifier) @member)
"
        }
        Language::Go => {
            r"
(function_declaration name: (identifier) @function)
(method_declaration name: (field_identifier) @method)
(type_spec name: (type_identifier) @type)
(source_file (const_declaration (const_spec name: (identifier) @constant)))
(source_file (var_declaration (var_spec name: (identifier) @variable)))
"
        }
        Language::Python => {
            r"
(function_definition name: (identifier) @function)
(class_definition name: (identifier) @class)
"
        }
        Language::Bash => "(function_definition name: (word) @function)",
        Language::PlainText => return None,
    })
}

struct Names {
    grammar: tree_sitter::Language,
    query: Query,
}

fn names_for(language: Language) -> Option<&'static Names> {
    static CACHE: OnceLock<Mutex<BTreeMap<Language, Option<&'static Names>>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(BTreeMap::new()));
    let mut cache = cache.lock().ok()?;
    *cache.entry(language).or_insert_with(|| {
        let source = names_query(language)?;
        let grammar = super::syntax::tree_language(language)?;
        let query = Query::new(&grammar, source).ok()?;
        Some(Box::leak(Box::new(Names { grammar, query })))
    })
}

pub fn extract(language: Language, text: &str) -> Vec<FoundSymbol> {
    if text.len() > MAX_SYMBOL_BYTES {
        return Vec::new();
    }
    let Some(names) = names_for(language) else {
        return Vec::new();
    };
    let mut parser = Parser::new();
    if parser.set_language(&names.grammar).is_err() {
        return Vec::new();
    }
    let started = Instant::now();
    let mut out_of_time = |_: &ParseState| started.elapsed() > PARSE_BUDGET;
    let options = ParseOptions::new().progress_callback(&mut out_of_time);
    let bytes = text.as_bytes();
    let mut read = |at: usize, _: tree_sitter::Point| -> &[u8] { bytes.get(at..).unwrap_or(&[]) };
    let Some(tree) = parser.parse_with_options(&mut read, None, Some(options)) else {
        return Vec::new();
    };
    let captures = names.query.capture_names();
    let mut cursor = QueryCursor::new();
    let mut matches = cursor.matches(&names.query, tree.root_node(), bytes);
    let mut found: Vec<(FoundSymbol, usize)> = Vec::new();
    while let Some(m) = matches.next() {
        for capture in m.captures {
            let node = capture.node;
            let Some(name) = clean_name(node.utf8_text(bytes).unwrap_or("")) else {
                continue;
            };
            let Some(kind) = KINDS
                .iter()
                .find(|k| **k == captures[capture.index as usize])
                .copied()
            else {
                continue;
            };
            let at = node.start_position();
            let end = node.parent().map_or(node.end_byte(), |p| p.end_byte());
            found.push((
                FoundSymbol {
                    name,
                    kind,
                    depth: 0,
                    start: (
                        u32::try_from(at.row).unwrap_or(u32::MAX),
                        u32::try_from(at.column).unwrap_or(u32::MAX),
                    ),
                },
                end,
            ));
        }
    }
    found.sort_by_key(|(s, _)| s.start);
    found.dedup_by(|a, b| a.0.start == b.0.start);
    found.truncate(MAX_FILE_SYMBOLS);
    with_depths(found, bytes)
}

fn with_depths(found: Vec<(FoundSymbol, usize)>, bytes: &[u8]) -> Vec<FoundSymbol> {
    let mut starts = vec![0];
    starts.extend(
        bytes
            .iter()
            .enumerate()
            .filter(|(_, b)| **b == b'\n')
            .map(|(i, _)| i + 1),
    );
    let mut open: Vec<usize> = Vec::new();
    found
        .into_iter()
        .map(|(mut symbol, end)| {
            let line = starts.get(symbol.start.0 as usize).copied().unwrap_or(0);
            let at = line + symbol.start.1 as usize;
            open.retain(|e| *e > at);
            symbol.depth = open.len();
            open.push(end);
            symbol
        })
        .collect()
}

fn clean_name(raw: &str) -> Option<String> {
    let raw = raw.trim();
    if raw.is_empty() || raw.chars().any(char::is_control) {
        return None;
    }
    Some(raw.chars().take(MAX_NAME_CHARS).collect())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn names(language: Language, text: &str) -> Vec<(String, &'static str)> {
        extract(language, text)
            .into_iter()
            .map(|s| (s.name, s.kind))
            .collect()
    }

    fn has(language: Language, text: &str, wanted: &[(&str, &str)]) {
        let got = names(language, text);
        for (name, kind) in wanted {
            assert!(
                got.iter().any(|(n, k)| n == name && k == kind),
                "{language:?} lacks {name} {kind}: {got:?}"
            );
        }
    }

    #[test]
    fn every_language_with_a_grammar_has_a_query_that_compiles() {
        for language in [
            Language::Rust,
            Language::Toml,
            Language::Markdown,
            Language::Json,
            Language::Php,
            Language::JavaScript,
            Language::Jsx,
            Language::TypeScript,
            Language::Tsx,
            Language::Java,
            Language::Go,
            Language::Python,
            Language::Bash,
        ] {
            assert!(names_for(language).is_some(), "{language:?}");
        }
    }

    #[test]
    fn each_language_names_what_it_declares() {
        has(
            Language::Rust,
            "mod m { pub struct S; enum E { V } trait T { fn t(); } const C: i32 = 1;\n fn f() {} macro_rules! mm { () => {} } type A = i32; }",
            &[
                ("m", "module"),
                ("S", "struct"),
                ("E", "enum"),
                ("V", "member"),
                ("T", "trait"),
                ("t", "function"),
                ("C", "constant"),
                ("f", "function"),
                ("mm", "macro"),
                ("A", "type"),
            ],
        );
        has(
            Language::Toml,
            "[package]\na = 1\n[[bin]]\n",
            &[("package", "key"), ("bin", "key")],
        );
        has(
            Language::Markdown,
            "# Title\n\ntext\n\nOther\n=====\n",
            &[("Title", "heading"), ("Other", "heading")],
        );
        has(
            Language::Json,
            r#"{"name": {"inner": 1}}"#,
            &[("name", "key")],
        );
        has(
            Language::Php,
            "<?php\nnamespace N;\nfunction f() {}\nclass C { function m() {} }\ninterface I {}\ntrait T {}\nenum E {}\nconst K = 1;\n",
            &[
                ("N", "module"),
                ("f", "function"),
                ("C", "class"),
                ("m", "method"),
                ("I", "interface"),
                ("T", "trait"),
                ("E", "enum"),
                ("K", "constant"),
            ],
        );
        for js in [Language::JavaScript, Language::Jsx] {
            has(
                js,
                "function f() {}\nclass C { m() {} }\nconst v = 1;\nexport const w = 2;\n",
                &[
                    ("f", "function"),
                    ("C", "class"),
                    ("m", "method"),
                    ("v", "variable"),
                    ("w", "variable"),
                ],
            );
        }
        for ts in [Language::TypeScript, Language::Tsx] {
            has(
                ts,
                "function f() {}\nclass C { m() {} }\ninterface I {}\ntype A = number;\nenum E {}\nconst v = 1;\nexport const w = 2;\n",
                &[
                    ("f", "function"),
                    ("C", "class"),
                    ("m", "method"),
                    ("I", "interface"),
                    ("A", "type"),
                    ("E", "enum"),
                    ("v", "variable"),
                    ("w", "variable"),
                ],
            );
        }
        has(
            Language::Java,
            "class C { C() {} void m() {} }\ninterface I {}\nenum E { V }\nrecord R(int a) {}\n",
            &[
                ("C", "class"),
                ("C", "method"),
                ("m", "method"),
                ("I", "interface"),
                ("E", "enum"),
                ("V", "member"),
                ("R", "class"),
            ],
        );
        has(
            Language::Go,
            "package p\nfunc f() {}\nfunc (s S) m() {}\ntype S struct{}\nconst K = 1\nvar V = 2\n",
            &[
                ("f", "function"),
                ("m", "method"),
                ("S", "type"),
                ("K", "constant"),
                ("V", "variable"),
            ],
        );
        has(
            Language::Python,
            "class C:\n    def m(self):\n        pass\ndef f():\n    pass\n",
            &[("C", "class"), ("m", "function"), ("f", "function")],
        );
        has(
            Language::Bash,
            "f() { :; }\nfunction g { :; }\n",
            &[("f", "function"), ("g", "function")],
        );
    }

    #[test]
    fn plain_text_has_none_and_locals_are_not_declarations() {
        assert!(extract(Language::PlainText, "fn f() {}").is_empty());
        let got = names(Language::JavaScript, "function f() { const local = 1; }\n");
        assert_eq!(got, vec![("f".to_owned(), "function")]);
    }

    #[test]
    fn places_are_lines_and_byte_columns_and_nesting_shows_as_depth() {
        let text = "class Я:\n    def m(self):\n        pass\ndef f():\n    pass\n";
        let got = extract(Language::Python, text);
        let row: Vec<_> = got
            .iter()
            .map(|s| (s.name.as_str(), s.depth, s.start))
            .collect();
        assert_eq!(
            row,
            vec![("Я", 0, (0, 6)), ("m", 1, (1, 8)), ("f", 0, (3, 4))]
        );
    }

    #[test]
    fn broken_code_still_gives_what_it_can() {
        let got = names(Language::Rust, "fn good() {}\nfn broken( {\n");
        assert!(got.iter().any(|(n, _)| n == "good"));
    }

    #[test]
    fn a_name_cannot_put_control_characters_on_the_screen() {
        let got = names(Language::Markdown, "# a\u{1b}[31mb\n");
        assert!(got.iter().all(|(n, _)| !n.contains('\u{1b}')), "{got:?}");
    }

    #[test]
    fn a_big_file_gives_nothing_rather_than_taking_long() {
        let big = "fn f() {}\n".repeat(MAX_SYMBOL_BYTES / 9);
        assert!(extract(Language::Rust, &big).is_empty());
    }
}
