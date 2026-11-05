use std::collections::BTreeMap;
use std::fs;
use std::path::Path;

use crate::lang::languages::Language;
use crate::lang::structural::{Compiled, Pattern};

use crate::core::{Event, FindOptions, Query, SearchMatch};
use crate::services::fs::walker;
use crate::services::index::IndexReader;

pub const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
const MAX_MATCHES: usize = 2000;

pub fn index_files(root: &Path) -> Event {
    let files = walker(root)
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_some_and(|t| t.is_file()))
        .map(ignore::DirEntry::into_path)
        .collect();
    Event::FilesIndexed { files }
}

pub fn search(id: u64, root: &Path, query: &str, options: FindOptions) -> Event {
    search_with(id, root, query, options, None)
}

pub fn search_with(
    id: u64,
    root: &Path,
    query: &str,
    options: FindOptions,
    index: Option<&IndexReader>,
) -> Event {
    if options.structural {
        return search_structural(id, root, query);
    }
    let mut matches = Vec::new();
    let mut truncated = false;
    let Ok(query) = Query::new(query, options) else {
        return Event::SearchFinished {
            id,
            matches,
            truncated,
        };
    };
    'files: for entry in walker(root).build().filter_map(Result::ok) {
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let Ok(metadata) = entry.metadata() else {
            continue;
        };
        if metadata.len() > MAX_FILE_BYTES
            || index.is_some_and(|i| i.can_skip(entry.path(), &metadata, query.needs()))
        {
            continue;
        }
        let Ok(bytes) = fs::read(entry.path()) else {
            continue;
        };
        if bytes.contains(&0) {
            continue;
        }
        let text = String::from_utf8_lossy(&bytes);
        for hit in query.line_hits(&text) {
            if matches.len() == MAX_MATCHES {
                truncated = true;
                break 'files;
            }
            matches.push(SearchMatch {
                path: entry.path().to_path_buf(),
                line: hit.line,
                col: hit.col,
                len: hit.len,
                text: hit.text,
            });
        }
    }
    matches.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    Event::SearchFinished {
        id,
        matches,
        truncated,
    }
}

fn search_structural(id: u64, root: &Path, query: &str) -> Event {
    let mut matches = Vec::new();
    let mut truncated = false;
    let mut languages: BTreeMap<Language, Option<Compiled>> = BTreeMap::new();
    if let Ok(pattern) = Pattern::new(query) {
        'files: for entry in walker(root).build().filter_map(Result::ok) {
            let small = entry.file_type().is_some_and(|t| t.is_file())
                && entry.metadata().is_ok_and(|m| m.len() <= MAX_FILE_BYTES);
            if !small {
                continue;
            }
            let language = Language::from_path(entry.path());
            let compiled = languages
                .entry(language)
                .or_insert_with(|| pattern.compile(language));
            let Some(compiled) = compiled else {
                continue;
            };
            let Ok(text) = fs::read_to_string(entry.path()) else {
                continue;
            };
            for found in compiled.find(&text) {
                if matches.len() == MAX_MATCHES {
                    truncated = true;
                    break 'files;
                }
                let start = found.bytes.start;
                let line_start = text[..start].rfind('\n').map_or(0, |i| i + 1);
                let line_end = text[start..].find('\n').map_or(text.len(), |i| i + start);
                matches.push(SearchMatch {
                    path: entry.path().to_path_buf(),
                    line: text[..start].matches('\n').count(),
                    col: text[line_start..start].chars().count(),
                    len: text[start..found.bytes.end.min(line_end)].chars().count(),
                    text: text[line_start..line_end].trim_end_matches('\r').to_owned(),
                });
            }
        }
    }
    matches.sort_by(|a, b| a.path.cmp(&b.path).then(a.line.cmp(&b.line)));
    Event::SearchFinished {
        id,
        matches,
        truncated,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn finds_lines_and_skips_binaries_and_ignored_files() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "skip.txt\n").unwrap();
        fs::write(dir.path().join("a.rs"), "one\nlet Needle = 1;\n").unwrap();
        fs::write(dir.path().join("skip.txt"), "needle").unwrap();
        fs::write(dir.path().join("bin.dat"), b"needle\0").unwrap();
        let Event::SearchFinished {
            matches, truncated, ..
        } = search(7, dir.path(), "needle", FindOptions::default())
        else {
            panic!("wrong event");
        };
        assert!(!truncated);
        assert_eq!(matches.len(), 1);
        assert_eq!((matches[0].line, matches[0].col), (1, 4));
        assert!(matches[0].path.ends_with("a.rs"));
    }

    fn found(dir: &Path, query: &str, options: FindOptions) -> Vec<(usize, usize, usize)> {
        let Event::SearchFinished { matches, .. } = search(1, dir, query, options) else {
            panic!("wrong event");
        };
        matches.iter().map(|m| (m.line, m.col, m.len)).collect()
    }

    #[test]
    fn options_reach_the_search() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join("a.txt"), "Foo foobar\nfoo\n").unwrap();
        let p = dir.path();
        let plain = FindOptions::default();
        assert_eq!(found(p, "foo", plain), [(0, 0, 3), (1, 0, 3)]);
        let exact = FindOptions {
            case_sensitive: true,
            ..plain
        };
        assert_eq!(found(p, "foo", exact), [(0, 4, 3), (1, 0, 3)]);
        let word = FindOptions {
            whole_word: true,
            case_sensitive: true,
            ..plain
        };
        assert_eq!(found(p, "foo", word), [(1, 0, 3)]);
        let re = FindOptions {
            regex: true,
            ..plain
        };
        assert_eq!(found(p, "fo+b.r", re), [(0, 4, 6)]);
        assert!(found(p, "(", re).is_empty(), "no panic on a bad regex");
    }

    #[test]
    fn index_lists_files_recursively() {
        let dir = tempfile::tempdir().unwrap();
        fs::create_dir(dir.path().join("src")).unwrap();
        fs::write(dir.path().join("src").join("main.rs"), "").unwrap();
        let Event::FilesIndexed { files } = index_files(dir.path()) else {
            panic!("wrong event");
        };
        assert!(files.iter().any(|f| f.ends_with("src/main.rs")));
    }

    #[test]
    fn structural_search_finds_code_by_shape_and_keeps_ignore_rules() {
        let dir = tempfile::tempdir().unwrap();
        fs::write(dir.path().join(".gitignore"), "skip.rs\n").unwrap();
        fs::write(
            dir.path().join("a.rs"),
            "fn a() {\n    foo( 1,\n 2);\n    // foo(3, 4)\n}\n",
        )
        .unwrap();
        fs::write(dir.path().join("skip.rs"), "fn b() { foo(1, 2); }\n").unwrap();
        fs::write(dir.path().join("n.txt"), "foo(1, 2)\n").unwrap();
        let options = FindOptions {
            structural: true,
            ..FindOptions::default()
        };
        let Event::SearchFinished { matches, .. } = search(1, dir.path(), "foo($A$, $B$)", options)
        else {
            panic!("wrong event");
        };
        assert_eq!(matches.len(), 1);
        assert_eq!((matches[0].line, matches[0].col), (1, 4));
        assert_eq!(matches[0].len, "foo( 1,".len());
        assert_eq!(matches[0].text, "    foo( 1,");
    }
}
