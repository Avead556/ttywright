use std::path::{Path, PathBuf};

use ropey::Rope;

use crate::core::spell::{Dictionary, check_line};
use crate::core::{Event, Language, SearchMatch};
use crate::lang::syntax::Syntax;

const SYSTEM_LISTS: [&str; 2] = ["/usr/share/dict/words", "/usr/share/dict/american-english"];
const MAX_RESULTS: usize = 2000;

fn own_words_file() -> Option<PathBuf> {
    Some(
        directories::ProjectDirs::from("", "", "ttywright")?
            .data_dir()
            .join("dictionary.txt"),
    )
}

fn dictionary() -> Result<Dictionary, String> {
    let system: Vec<String> = SYSTEM_LISTS
        .iter()
        .filter_map(|p| std::fs::read_to_string(p).ok())
        .collect();
    if system.is_empty() {
        return Err("no word list on this system: /usr/share/dict/words".to_owned());
    }
    let own = own_words_file()
        .and_then(|f| std::fs::read_to_string(f).ok())
        .unwrap_or_default();
    Ok(Dictionary::from_lists(
        system.iter().map(String::as_str).chain([own.as_str()]),
    ))
}

fn regions(text: &str, language: Language) -> Vec<(usize, String, usize)> {
    let rope = Rope::from_str(text);
    let prose = matches!(language, Language::Markdown | Language::PlainText);
    if prose {
        let mut in_code = false;
        return text
            .lines()
            .enumerate()
            .filter_map(|(n, line)| {
                if line.trim_start().starts_with("```") {
                    in_code = !in_code;
                    return None;
                }
                (!in_code).then(|| (n, line.to_owned(), 0))
            })
            .collect();
    }
    let Some(syntax) = Syntax::new(language, &rope) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    for (n, spans) in syntax
        .highlight(&rope, 0..rope.len_lines())
        .iter()
        .enumerate()
    {
        let line: Vec<char> = rope.line(n).chars().collect();
        for span in spans {
            if span.capture.starts_with("comment") || span.capture.starts_with("string") {
                let end = span.chars.end.min(line.len());
                let start = span.chars.start.min(end);
                out.push((n, line[start..end].iter().collect(), start));
            }
        }
    }
    out
}

pub fn check(path: &Path, text: &str, language: Language, words: &Dictionary) -> Vec<SearchMatch> {
    let lines: Vec<&str> = text.lines().collect();
    let mut out = Vec::new();
    for (n, region, offset) in regions(text, language) {
        for wrong in check_line(&region, words) {
            if out.len() == MAX_RESULTS {
                return out;
            }
            out.push(SearchMatch {
                path: path.to_path_buf(),
                line: n,
                col: offset + wrong.col,
                len: wrong.word.chars().count(),
                text: lines.get(n).copied().unwrap_or_default().to_owned(),
            });
        }
    }
    out
}

pub fn check_event(path: PathBuf, text: &str, language: Language) -> Event {
    Event::SpellChecked {
        result: dictionary().map(|words| check(&path, text, language, &words)),
    }
}

pub fn add_word(word: &str) -> Result<(), String> {
    use std::io::Write;
    if word.is_empty() || !word.chars().all(|c| c.is_ascii_alphabetic()) {
        return Err("only a plain word can be added".to_owned());
    }
    let file = own_words_file().ok_or("no data folder")?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    let mut out = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&file)
        .map_err(|e| e.to_string())?;
    writeln!(out, "{}", word.to_ascii_lowercase()).map_err(|e| e.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn words() -> Dictionary {
        Dictionary::from_lists(["this\nfile\nword\nmain\nhello"])
    }

    fn found(text: &str, language: Language) -> Vec<(usize, usize, usize)> {
        check(Path::new("/a"), text, language, &words())
            .into_iter()
            .map(|m| (m.line, m.col, m.len))
            .collect()
    }

    #[test]
    fn in_code_only_comments_and_strings_count_and_columns_are_of_the_line() {
        let text = "fn qwzxy() {\n    // this wrod\n    let a = \"hello wrld\";\n}\n";
        assert_eq!(found(text, Language::Rust), [(1, 12, 4), (2, 19, 4)]);
    }

    #[test]
    fn prose_is_checked_and_its_code_blocks_are_not() {
        let text = "this wrod\n```\nqwzxy\n```\nword\n";
        assert_eq!(found(text, Language::Markdown), [(0, 5, 4)]);
        assert_eq!(found("wrod\n", Language::PlainText), [(0, 0, 4)]);
    }

    #[test]
    fn only_a_plain_word_can_go_in_the_users_list() {
        for bad in ["", "a b", "a\nb", "../x", "слово"] {
            assert!(add_word(bad).is_err(), "{bad:?}");
        }
    }
}
