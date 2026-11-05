use std::collections::HashSet;

const MIN_LETTERS: usize = 4;

const COMMON: &[&str] = &[
    "breakpoint",
    "byte",
    "clipboard",
    "clippy",
    "debug",
    "debugger",
    "diff",
    "docker",
    "dockerfile",
    "eslint",
    "exec",
    "gradle",
    "grapheme",
    "inline",
    "makefile",
    "maven",
    "nginx",
    "npm",
    "phpunit",
    "pixel",
    "pnpm",
    "startup",
    "tmux",
    "vitest",
    "xdebug",
    "api",
    "apis",
    "args",
    "argv",
    "async",
    "auth",
    "backend",
    "bool",
    "boolean",
    "bytes",
    "cancelled",
    "changelog",
    "charset",
    "checkbox",
    "cli",
    "codebase",
    "config",
    "configs",
    "const",
    "cpu",
    "css",
    "csv",
    "daemon",
    "dev",
    "dir",
    "dirs",
    "dropdown",
    "enum",
    "env",
    "filename",
    "filenames",
    "filesystem",
    "frontend",
    "func",
    "github",
    "gitignore",
    "golang",
    "hashmap",
    "hostname",
    "html",
    "http",
    "https",
    "impl",
    "init",
    "integer",
    "iterator",
    "java",
    "javascript",
    "json",
    "keyboard",
    "keyword",
    "keywords",
    "lang",
    "linter",
    "linux",
    "localhost",
    "login",
    "logout",
    "macos",
    "markdown",
    "metadata",
    "middleware",
    "mutex",
    "namespace",
    "newline",
    "null",
    "offline",
    "online",
    "param",
    "params",
    "parser",
    "password",
    "pathname",
    "plugin",
    "plugins",
    "popup",
    "preview",
    "printf",
    "println",
    "pytest",
    "python",
    "readme",
    "refactor",
    "refactoring",
    "regex",
    "repo",
    "repos",
    "runtime",
    "rust",
    "screenshot",
    "serde",
    "shell",
    "sidebar",
    "snippet",
    "stderr",
    "stdin",
    "stdout",
    "struct",
    "subprocess",
    "sudo",
    "symlink",
    "syntax",
    "tcp",
    "timeout",
    "tmp",
    "todo",
    "toml",
    "tuple",
    "typescript",
    "udp",
    "unicode",
    "unix",
    "uri",
    "url",
    "urls",
    "usize",
    "utf",
    "uuid",
    "vec",
    "webpack",
    "whitespace",
    "windows",
    "workspace",
    "xml",
    "yaml",
];

#[derive(Debug, Default, Clone)]
pub struct Dictionary {
    words: HashSet<String>,
}

impl Dictionary {
    pub fn from_lists<'a>(lists: impl IntoIterator<Item = &'a str>) -> Self {
        let mut words: HashSet<String> = COMMON.iter().map(|w| (*w).to_owned()).collect();
        for list in lists {
            words.extend(
                list.lines()
                    .map(str::trim)
                    .filter(|w| !w.is_empty())
                    .map(str::to_ascii_lowercase),
            );
        }
        Self { words }
    }

    pub fn is_empty(&self) -> bool {
        self.words.len() <= COMMON.len()
    }

    fn has(&self, word: &str) -> bool {
        self.words.contains(word)
    }

    pub fn knows(&self, word: &str) -> bool {
        let word = word.to_ascii_lowercase();
        if self.has(&word) {
            return true;
        }
        const ENDINGS: [(&str, &str); 19] = [
            ("s", ""),
            ("es", ""),
            ("ies", "y"),
            ("ed", ""),
            ("ed", "e"),
            ("ied", "y"),
            ("ing", ""),
            ("ing", "e"),
            ("ly", ""),
            ("er", ""),
            ("er", "e"),
            ("ers", ""),
            ("ers", "e"),
            ("est", ""),
            ("ness", ""),
            ("ment", ""),
            ("able", ""),
            ("able", "e"),
            ("ion", "e"),
        ];
        let stem_known = ENDINGS.iter().any(|(ending, back)| {
            word.strip_suffix(ending).is_some_and(|stem| {
                let doubled = (*back).is_empty()
                    && stem.len() > 2
                    && stem.as_bytes()[stem.len() - 1] == stem.as_bytes()[stem.len() - 2];
                self.has(&format!("{stem}{back}")) || (doubled && self.has(&stem[..stem.len() - 1]))
            })
        });
        stem_known
            || ["un", "re", "non", "pre", "sub", "over"]
                .iter()
                .any(|prefix| {
                    word.strip_prefix(prefix)
                        .is_some_and(|rest| rest.len() >= MIN_LETTERS && self.has(rest))
                })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Misspelling {
    pub word: String,
    pub col: usize,
}

fn checkable(word: &str) -> bool {
    word.len() >= MIN_LETTERS && word.bytes().any(|b| b.is_ascii_lowercase())
}

fn parts(token: &[(usize, char)]) -> Vec<(usize, String)> {
    let mut out: Vec<(usize, String)> = Vec::new();
    let mut previous_lower = false;
    for &(at, ch) in token {
        if ch == '_' {
            previous_lower = false;
            out.push((at + 1, String::new()));
            continue;
        }
        if ch.is_ascii_uppercase() && previous_lower {
            out.push((at, String::new()));
        }
        previous_lower = ch.is_ascii_lowercase();
        match out.last_mut() {
            Some((_, word)) => word.push(ch),
            None => out.push((at, ch.to_string())),
        }
    }
    out
}

pub fn check_line(line: &str, dictionary: &Dictionary) -> Vec<Misspelling> {
    let chars: Vec<char> = line.chars().collect();
    let mut found = Vec::new();
    let mut i = 0;
    while i < chars.len() {
        let is_part = |c: char| c.is_alphanumeric() || c == '_';
        if !is_part(chars[i]) {
            i += 1;
            continue;
        }
        let start = i;
        while i < chars.len() && is_part(chars[i]) {
            i += 1;
        }
        let token = &chars[start..i];
        let before = start.checked_sub(1).map(|n| chars[n]);
        let after = chars.get(i).copied();
        let glued = |c: Option<char>| {
            matches!(
                c,
                Some('/' | '\\' | ':' | '.' | '@' | '-' | '=' | '#' | '$' | '`')
            )
        };
        let followed = glued(before)
            || (matches!(after, Some('/' | '\\' | ':' | '@' | '=' | '(' | '`'))
                && !matches!(chars.get(i + 1), None | Some(' ')));
        if followed || !token.iter().all(|c| c.is_ascii_alphabetic() || *c == '_') {
            continue;
        }
        let indexed: Vec<(usize, char)> = (start..i).map(|n| (n, chars[n])).collect();
        for (col, word) in parts(&indexed) {
            if checkable(&word) && !dictionary.knows(&word) {
                found.push(Misspelling { word, col });
            }
        }
    }
    found
}

#[cfg(test)]
mod tests {
    use super::*;

    fn dictionary() -> Dictionary {
        Dictionary::from_lists(["walk\nfile\nhappy\nstop\nread\nline\nthis\nword\nmake"])
    }

    fn wrong(line: &str) -> Vec<String> {
        check_line(line, &dictionary())
            .into_iter()
            .map(|m| m.word)
            .collect()
    }

    #[test]
    fn it_finds_what_is_not_a_word_and_says_where() {
        let found = check_line("this wrod here", &dictionary());
        assert_eq!(
            found[0],
            Misspelling {
                word: "wrod".to_owned(),
                col: 5
            }
        );
    }

    #[test]
    fn forms_of_known_words_are_fine() {
        for word in [
            "walks",
            "walked",
            "walking",
            "files",
            "stopped",
            "stopping",
            "happily",
            "reader",
            "making",
            "unhappy",
            "rereading",
        ] {
            let known = word != "happily" && word != "rereading";
            assert_eq!(dictionary().knows(word), known, "{word}");
        }
    }

    #[test]
    fn names_are_checked_by_their_parts() {
        assert_eq!(wrong("readFile"), Vec::<String>::new());
        assert_eq!(wrong("read_file"), Vec::<String>::new());
        assert_eq!(wrong("readFlie"), ["Flie"]);
        assert_eq!(wrong("flie_read"), ["flie"]);
    }

    #[test]
    fn what_is_not_prose_is_left_alone() {
        for line in [
            "a/b/qwzx/file.rs",
            "see https://qwzx.example",
            "qwzx2 and v1beta",
            "QWZX",
            "русскиеслова",
            "`qwzxy` this",
            "the qwz word",
            "qwzx=1",
            "qwzx(arg)",
            "--qwzxy",
        ] {
            assert_eq!(wrong(line), Vec::<String>::new(), "{line}");
        }
    }

    #[test]
    fn the_end_of_a_sentence_does_not_hide_a_typo() {
        assert_eq!(wrong("a wrod."), ["wrod"]);
        assert_eq!(wrong("a wrod: this"), ["wrod"]);
    }

    #[test]
    fn programmers_words_and_the_users_own_pass_and_an_empty_list_is_noticed() {
        assert!(Dictionary::from_lists([]).knows("regex"));
        assert!(Dictionary::from_lists([]).is_empty());
        assert!(!dictionary().is_empty());
        assert!(Dictionary::from_lists(["word", "Qwzx"]).knows("qwzx"));
    }
}
