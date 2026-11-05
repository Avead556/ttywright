use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tree_sitter::Language as TsLanguage;

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub enum Language {
    Rust,
    Toml,
    Markdown,
    Json,
    Php,
    JavaScript,
    Jsx,
    TypeScript,
    Tsx,
    Java,
    Go,
    Python,
    Bash,
    PlainText,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Overlap {
    FirstPatternWins,
    LastPatternWins,
}

#[cfg(feature = "static-modules")]
macro_rules! builtin {
    ($language:expr) => {
        Some(|| $language.into())
    };
}
#[cfg(not(feature = "static-modules"))]
macro_rules! builtin {
    ($language:expr) => {
        None
    };
}

pub struct Grammar {
    pub module: &'static str,
    pub builtin: Option<fn() -> TsLanguage>,
    pub queries: &'static [&'static str],
    pub overlap: Overlap,
}

pub struct Candidate {
    pub command: &'static str,
    pub args: &'static [&'static str],
    pub initialization: Option<&'static str>,
    pub install: &'static str,
}

pub struct WorkspaceMarker {
    pub file: &'static str,
    pub line_start: &'static str,
}

pub struct ServerSpec {
    pub id: Language,
    pub candidates: &'static [Candidate],
    pub root_markers: &'static [&'static str],
    pub workspace: Option<WorkspaceMarker>,
    pub provisioned: bool,
}

pub struct LanguageServer {
    pub language_id: &'static str,
    pub server: &'static ServerSpec,
}

pub struct LanguageSpec {
    pub language: Language,
    pub name: &'static str,
    pub config_name: &'static str,
    pub extensions: &'static [&'static str],
    pub filenames: &'static [&'static str],
    pub interpreters: &'static [&'static str],
    pub lsp: Option<LanguageServer>,
    pub grammar: Option<Grammar>,
}

pub static RUST_SERVER: ServerSpec = ServerSpec {
    id: Language::Rust,
    candidates: &[Candidate {
        command: "rust-analyzer",
        args: &[],
        initialization: None,
        install: "rustup component add rust-analyzer rust-src",
    }],
    root_markers: &["Cargo.toml"],
    workspace: Some(WorkspaceMarker {
        file: "Cargo.toml",
        line_start: "[workspace",
    }),
    provisioned: true,
};

pub static PHP_SERVER: ServerSpec = ServerSpec {
    id: Language::Php,
    candidates: &[Candidate {
        command: "intelephense",
        args: &["--stdio"],
        initialization: Some(r#"{"storagePath":"{storage}","globalStoragePath":"{storage}"}"#),
        install: "npm install -g intelephense",
    }],
    root_markers: &["composer.json"],
    workspace: None,
    provisioned: false,
};

pub static TYPESCRIPT_SERVER: ServerSpec = ServerSpec {
    id: Language::TypeScript,
    candidates: &[Candidate {
        command: "typescript-language-server",
        args: &["--stdio"],
        initialization: Some(r#"{"hostInfo":"ttywright"}"#),
        install: "npm install -g typescript-language-server typescript",
    }],
    root_markers: &["package.json", "tsconfig.json"],
    workspace: None,
    provisioned: false,
};

pub static JAVA_SERVER: ServerSpec = ServerSpec {
    id: Language::Java,
    candidates: &[Candidate {
        command: "jdtls",
        args: &["-data", "{storage}"],
        initialization: None,
        install: "brew install jdtls",
    }],
    root_markers: &["pom.xml", "build.gradle", "build.gradle.kts"],
    workspace: None,
    provisioned: false,
};

pub static GO_SERVER: ServerSpec = ServerSpec {
    id: Language::Go,
    candidates: &[Candidate {
        command: "gopls",
        args: &[],
        initialization: None,
        install: "go install golang.org/x/tools/gopls@latest",
    }],
    root_markers: &["go.mod"],
    workspace: None,
    provisioned: false,
};

pub static PYTHON_SERVER: ServerSpec = ServerSpec {
    id: Language::Python,
    candidates: &[
        Candidate {
            command: "pyright-langserver",
            args: &["--stdio"],
            initialization: None,
            install: "npm install -g pyright",
        },
        Candidate {
            command: "pylsp",
            args: &[],
            initialization: None,
            install: "pip install python-lsp-server",
        },
    ],
    root_markers: &["pyproject.toml", "setup.py", "requirements.txt"],
    workspace: None,
    provisioned: false,
};

pub static BASH_SERVER: ServerSpec = ServerSpec {
    id: Language::Bash,
    candidates: &[Candidate {
        command: "bash-language-server",
        args: &["start"],
        initialization: None,
        install: "npm install -g bash-language-server",
    }],
    root_markers: &[],
    workspace: None,
    provisioned: false,
};

const fn served(language_id: &'static str, server: &'static ServerSpec) -> Option<LanguageServer> {
    Some(LanguageServer {
        language_id,
        server,
    })
}

static PLAIN_TEXT: LanguageSpec = LanguageSpec {
    language: Language::PlainText,
    name: "Plain Text",
    config_name: "plaintext",
    extensions: &[],
    filenames: &[],
    interpreters: &[],
    lsp: None,
    grammar: None,
};

pub static LANGUAGES: &[LanguageSpec] = &[
    LanguageSpec {
        language: Language::Rust,
        name: "Rust",
        config_name: "rust",
        extensions: &["rs"],
        filenames: &[],
        interpreters: &[],
        lsp: served("rust", &RUST_SERVER),
        grammar: Some(Grammar {
            module: "rust",
            builtin: builtin!(tree_sitter_rust::LANGUAGE),
            queries: &[tree_sitter_rust::HIGHLIGHTS_QUERY],
            overlap: Overlap::FirstPatternWins,
        }),
    },
    LanguageSpec {
        language: Language::Toml,
        name: "TOML",
        config_name: "toml",
        extensions: &["toml"],
        filenames: &["Cargo.lock", "Pipfile"],
        interpreters: &[],
        lsp: None,
        grammar: Some(Grammar {
            module: "toml",
            builtin: builtin!(tree_sitter_toml_ng::LANGUAGE),
            queries: &[tree_sitter_toml_ng::HIGHLIGHTS_QUERY],
            overlap: Overlap::FirstPatternWins,
        }),
    },
    LanguageSpec {
        language: Language::Markdown,
        name: "Markdown",
        config_name: "markdown",
        extensions: &["md", "markdown"],
        filenames: &[],
        interpreters: &[],
        lsp: None,
        grammar: Some(Grammar {
            module: "markdown",
            builtin: builtin!(tree_sitter_md::LANGUAGE),
            queries: &[tree_sitter_md::HIGHLIGHT_QUERY_BLOCK],
            overlap: Overlap::FirstPatternWins,
        }),
    },
    LanguageSpec {
        language: Language::Json,
        name: "JSON",
        config_name: "json",
        extensions: &["json"],
        filenames: &[],
        interpreters: &[],
        lsp: None,
        grammar: Some(Grammar {
            module: "json",
            builtin: builtin!(tree_sitter_json::LANGUAGE),
            queries: &[tree_sitter_json::HIGHLIGHTS_QUERY],
            overlap: Overlap::FirstPatternWins,
        }),
    },
    LanguageSpec {
        language: Language::Php,
        name: "PHP",
        config_name: "php",
        extensions: &["php", "phtml"],
        filenames: &[],
        interpreters: &["php"],
        lsp: served("php", &PHP_SERVER),
        grammar: Some(Grammar {
            module: "php",
            builtin: builtin!(tree_sitter_php::LANGUAGE_PHP),
            queries: &[tree_sitter_php::HIGHLIGHTS_QUERY],
            overlap: Overlap::LastPatternWins,
        }),
    },
    LanguageSpec {
        language: Language::JavaScript,
        name: "JavaScript",
        config_name: "javascript",
        extensions: &["js", "mjs", "cjs"],
        filenames: &[],
        interpreters: &["node"],
        lsp: served("javascript", &TYPESCRIPT_SERVER),
        grammar: Some(Grammar {
            module: "javascript",
            builtin: builtin!(tree_sitter_javascript::LANGUAGE),
            queries: &[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
            ],
            overlap: Overlap::LastPatternWins,
        }),
    },
    LanguageSpec {
        language: Language::Jsx,
        name: "JSX",
        config_name: "jsx",
        extensions: &["jsx"],
        filenames: &[],
        interpreters: &[],
        lsp: served("javascriptreact", &TYPESCRIPT_SERVER),
        grammar: Some(Grammar {
            module: "javascript",
            builtin: builtin!(tree_sitter_javascript::LANGUAGE),
            queries: &[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
            ],
            overlap: Overlap::LastPatternWins,
        }),
    },
    LanguageSpec {
        language: Language::TypeScript,
        name: "TypeScript",
        config_name: "typescript",
        extensions: &["ts", "mts", "cts"],
        filenames: &[],
        interpreters: &[],
        lsp: served("typescript", &TYPESCRIPT_SERVER),
        grammar: Some(Grammar {
            module: "typescript",
            builtin: builtin!(tree_sitter_typescript::LANGUAGE_TYPESCRIPT),
            queries: &[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
            ],
            overlap: Overlap::LastPatternWins,
        }),
    },
    LanguageSpec {
        language: Language::Tsx,
        name: "TSX",
        config_name: "tsx",
        extensions: &["tsx"],
        filenames: &[],
        interpreters: &[],
        lsp: served("typescriptreact", &TYPESCRIPT_SERVER),
        grammar: Some(Grammar {
            module: "tsx",
            builtin: builtin!(tree_sitter_typescript::LANGUAGE_TSX),
            queries: &[
                tree_sitter_javascript::HIGHLIGHT_QUERY,
                tree_sitter_javascript::JSX_HIGHLIGHT_QUERY,
                tree_sitter_typescript::HIGHLIGHTS_QUERY,
            ],
            overlap: Overlap::LastPatternWins,
        }),
    },
    LanguageSpec {
        language: Language::Java,
        name: "Java",
        config_name: "java",
        extensions: &["java"],
        filenames: &[],
        interpreters: &[],
        lsp: served("java", &JAVA_SERVER),
        grammar: Some(Grammar {
            module: "java",
            builtin: builtin!(tree_sitter_java::LANGUAGE),
            queries: &[tree_sitter_java::HIGHLIGHTS_QUERY],
            overlap: Overlap::LastPatternWins,
        }),
    },
    LanguageSpec {
        language: Language::Go,
        name: "Go",
        config_name: "go",
        extensions: &["go"],
        filenames: &[],
        interpreters: &[],
        lsp: served("go", &GO_SERVER),
        grammar: Some(Grammar {
            module: "go",
            builtin: builtin!(tree_sitter_go::LANGUAGE),
            queries: &[tree_sitter_go::HIGHLIGHTS_QUERY],
            overlap: Overlap::FirstPatternWins,
        }),
    },
    LanguageSpec {
        language: Language::Python,
        name: "Python",
        config_name: "python",
        extensions: &["py", "pyi", "pyw"],
        filenames: &[],
        interpreters: &["python", "pypy"],
        lsp: served("python", &PYTHON_SERVER),
        grammar: Some(Grammar {
            module: "python",
            builtin: builtin!(tree_sitter_python::LANGUAGE),
            queries: &[tree_sitter_python::HIGHLIGHTS_QUERY],
            overlap: Overlap::LastPatternWins,
        }),
    },
    LanguageSpec {
        language: Language::Bash,
        name: "Bash",
        config_name: "bash",
        extensions: &["sh", "bash", "zsh"],
        filenames: &[
            ".bashrc",
            ".bash_profile",
            ".bash_logout",
            ".profile",
            ".zshrc",
            ".zprofile",
        ],
        interpreters: &["bash", "sh", "dash", "ksh", "zsh"],
        lsp: served("shellscript", &BASH_SERVER),
        grammar: Some(Grammar {
            module: "bash",
            builtin: builtin!(tree_sitter_bash::LANGUAGE),
            queries: &[tree_sitter_bash::HIGHLIGHT_QUERY],
            overlap: Overlap::LastPatternWins,
        }),
    },
];

fn shebang_program(first_line: &str) -> Option<&str> {
    let line = first_line.strip_prefix("#!")?;
    let mut words = line.split_whitespace();
    let mut program = words.next()?.rsplit('/').next()?;
    if program == "env" {
        program = words.find(|w| !w.starts_with('-') && !w.contains('='))?;
        program = program.rsplit('/').next()?;
    }
    Some(program.trim_end_matches(|c: char| c.is_ascii_digit() || c == '.'))
}

impl Language {
    pub fn spec(self) -> &'static LanguageSpec {
        LANGUAGES
            .iter()
            .find(|row| row.language == self)
            .unwrap_or(&PLAIN_TEXT)
    }

    pub fn from_fence(info: &str) -> Self {
        let Some(word) = info.split_whitespace().next() else {
            return Self::PlainText;
        };
        let word = word.split(',').next().unwrap_or(word);
        LANGUAGES
            .iter()
            .find(|row| {
                row.config_name.eq_ignore_ascii_case(word)
                    || row.name.eq_ignore_ascii_case(word)
                    || row.extensions.iter().any(|x| x.eq_ignore_ascii_case(word))
                    || row
                        .interpreters
                        .iter()
                        .any(|x| x.eq_ignore_ascii_case(word))
            })
            .map_or(Self::PlainText, |row| row.language)
    }

    pub fn from_path(path: &Path) -> Self {
        let name = path.file_name().and_then(|n| n.to_str());
        let extension = path.extension().and_then(|e| e.to_str());
        LANGUAGES
            .iter()
            .find(|row| {
                name.is_some_and(|n| row.filenames.contains(&n))
                    || extension
                        .is_some_and(|e| row.extensions.iter().any(|x| x.eq_ignore_ascii_case(e)))
            })
            .map_or(Self::PlainText, |row| row.language)
    }

    pub fn detect(path: &Path, first_line: &str) -> Self {
        match Self::from_path(path) {
            Self::PlainText => shebang_program(first_line)
                .and_then(|program| {
                    LANGUAGES
                        .iter()
                        .find(|row| row.interpreters.contains(&program))
                })
                .map_or(Self::PlainText, |row| row.language),
            known => known,
        }
    }

    pub fn name(self) -> &'static str {
        self.spec().name
    }

    pub fn server(self) -> Option<&'static ServerSpec> {
        self.spec().lsp.as_ref().map(|lsp| lsp.server)
    }

    pub fn language_id(self) -> Option<&'static str> {
        self.spec().lsp.as_ref().map(|lsp| lsp.language_id)
    }
}

impl ServerSpec {
    pub fn find_root(
        &self,
        file: &Path,
        project: &Path,
        exists: &dyn Fn(&Path) -> bool,
        has_line_starting: &dyn Fn(&Path, &str) -> bool,
    ) -> Option<PathBuf> {
        file.strip_prefix(project).ok()?;
        let walk = || {
            file.ancestors()
                .skip(1)
                .take_while(|dir| dir.starts_with(project))
        };
        let nearest = walk().find(|dir| self.root_markers.iter().any(|m| exists(&dir.join(m))));
        let Some(nearest) = nearest else {
            return Some(project.to_path_buf());
        };
        let workspace = self.workspace.as_ref().and_then(|w| {
            walk()
                .filter(|dir| has_line_starting(&dir.join(w.file), w.line_start))
                .last()
        });
        Some(workspace.unwrap_or(nearest).to_path_buf())
    }

    pub fn name(&self) -> &'static str {
        self.candidates.first().map_or("", |c| c.command)
    }

    pub fn config_names(&self) -> Vec<&'static str> {
        LANGUAGES
            .iter()
            .filter(|row| {
                row.lsp
                    .as_ref()
                    .is_some_and(|l| std::ptr::eq(l.server, self))
            })
            .map(|row| row.config_name)
            .collect()
    }
}

pub fn server_for_config_name(name: &str) -> Option<&'static ServerSpec> {
    LANGUAGES
        .iter()
        .find(|row| row.config_name == name)
        .and_then(|row| row.language.server())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_variant_has_its_own_row() {
        let all = |l: Language| match l {
            Language::Rust
            | Language::Toml
            | Language::Markdown
            | Language::Json
            | Language::Php
            | Language::JavaScript
            | Language::Jsx
            | Language::TypeScript
            | Language::Tsx
            | Language::Java
            | Language::Go
            | Language::Python
            | Language::Bash
            | Language::PlainText => l,
        };
        for row in LANGUAGES {
            assert_eq!(all(row.language).spec().name, row.name);
            assert!(row.grammar.is_some(), "{}", row.name);
        }
        assert_eq!(LANGUAGES.len(), 13, "every variant but plain text");
        let mut names: Vec<_> = LANGUAGES.iter().map(|r| r.name).collect();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), LANGUAGES.len());
    }

    #[test]
    fn files_are_known_by_extension_and_name() {
        let cases = [
            ("a.rs", Language::Rust),
            ("Cargo.lock", Language::Toml),
            ("a.toml", Language::Toml),
            ("README.md", Language::Markdown),
            ("a.json", Language::Json),
            ("a.php", Language::Php),
            ("a.js", Language::JavaScript),
            ("a.mjs", Language::JavaScript),
            ("a.jsx", Language::Jsx),
            ("a.ts", Language::TypeScript),
            ("a.d.ts", Language::TypeScript),
            ("a.tsx", Language::Tsx),
            ("A.java", Language::Java),
            ("a.go", Language::Go),
            ("a.py", Language::Python),
            ("a.pyi", Language::Python),
            ("a.sh", Language::Bash),
            ("A.SH", Language::Bash),
            (".bashrc", Language::Bash),
            ("a.txt", Language::PlainText),
            ("Makefile", Language::PlainText),
        ];
        for (name, language) in cases {
            assert_eq!(Language::from_path(Path::new(name)), language, "{name}");
        }
    }

    #[test]
    fn a_shebang_decides_only_when_the_name_says_nothing() {
        let detect = |name: &str, line: &str| Language::detect(Path::new(name), line);
        assert_eq!(detect("run", "#!/bin/bash"), Language::Bash);
        assert_eq!(detect("run", "#!/bin/sh -e"), Language::Bash);
        assert_eq!(detect("run", "#! /usr/bin/env bash\r"), Language::Bash);
        assert_eq!(detect("run", "#!/usr/bin/env python3"), Language::Python);
        assert_eq!(detect("run", "#!/usr/bin/python3.12 -u"), Language::Python);
        assert_eq!(
            detect("run", "#!/usr/bin/env -S FOO=1 python3 -u"),
            Language::Python
        );
        assert_eq!(detect("run", "#!/usr/bin/env node"), Language::JavaScript);
        assert_eq!(detect("a.md", "#!/bin/bash"), Language::Markdown);
        assert_eq!(detect("run", "#!/usr/bin/ruby"), Language::PlainText);
        assert_eq!(detect("run", "#!"), Language::PlainText);
        assert_eq!(detect("run", "#!/usr/bin/env"), Language::PlainText);
        assert_eq!(detect("run", "echo #!/bin/bash"), Language::PlainText);
        assert_eq!(detect("run", ""), Language::PlainText);
    }

    fn root_of(server: &ServerSpec, file: &str, files: &[(&str, &str)]) -> Option<PathBuf> {
        let exists = |p: &Path| files.iter().any(|(name, _)| Path::new(name) == p);
        let has = |p: &Path, start: &str| {
            files
                .iter()
                .any(|(name, text)| Path::new(name) == p && text.starts_with(start))
        };
        server.find_root(Path::new(file), Path::new("/p"), &exists, &has)
    }

    #[test]
    fn the_root_is_the_nearest_marker_and_never_above_the_project() {
        let at = |s: &str| Some(PathBuf::from(s));
        let go = &GO_SERVER;
        let files = [("/p/a/go.mod", ""), ("/p/go.mod", ""), ("/go.mod", "")];
        assert_eq!(root_of(go, "/p/a/b/x.go", &files), at("/p/a"));
        assert_eq!(root_of(go, "/p/c/x.go", &files), at("/p"));
        let above_only = [("/go.mod", "")];
        assert_eq!(root_of(go, "/p/c/x.go", &above_only), at("/p"), "no marker");
        assert_eq!(root_of(go, "/elsewhere/x.go", &files), None);
    }

    #[test]
    fn each_servers_markers_count() {
        let cases: [(&ServerSpec, &str); 11] = [
            (&PHP_SERVER, "composer.json"),
            (&TYPESCRIPT_SERVER, "package.json"),
            (&TYPESCRIPT_SERVER, "tsconfig.json"),
            (&JAVA_SERVER, "pom.xml"),
            (&JAVA_SERVER, "build.gradle"),
            (&JAVA_SERVER, "build.gradle.kts"),
            (&GO_SERVER, "go.mod"),
            (&PYTHON_SERVER, "pyproject.toml"),
            (&PYTHON_SERVER, "setup.py"),
            (&PYTHON_SERVER, "requirements.txt"),
            (&RUST_SERVER, "Cargo.toml"),
        ];
        for (server, marker) in cases {
            let manifest = format!("/p/sub/{marker}");
            let files = [(manifest.as_str(), "")];
            assert_eq!(
                root_of(server, "/p/sub/deep/file", &files),
                Some(PathBuf::from("/p/sub")),
                "{marker}"
            );
        }
    }

    #[test]
    fn a_cargo_workspace_is_one_root_but_a_lone_nested_crate_its_own() {
        let at = |s: &str| Some(PathBuf::from(s));
        let rust = &RUST_SERVER;
        let workspace = [
            ("/p/Cargo.toml", "[workspace]\nmembers = [\"a\"]"),
            ("/p/a/Cargo.toml", "[package]"),
        ];
        assert_eq!(root_of(rust, "/p/a/src/x.rs", &workspace), at("/p"));
        let separate = [
            ("/p/Cargo.toml", "[package]"),
            ("/p/a/Cargo.toml", "[package]"),
        ];
        assert_eq!(root_of(rust, "/p/a/src/x.rs", &separate), at("/p/a"));
    }

    #[test]
    fn javascript_and_typescript_share_a_server_and_differ_in_language_id() {
        for language in [
            Language::JavaScript,
            Language::Jsx,
            Language::TypeScript,
            Language::Tsx,
        ] {
            assert_eq!(language.server().map(|s| s.id), Some(Language::TypeScript));
        }
        assert_eq!(Language::Jsx.language_id(), Some("javascriptreact"));
        assert_eq!(Language::Tsx.language_id(), Some("typescriptreact"));
        assert_eq!(Language::Bash.language_id(), Some("shellscript"));
        assert!(Language::Toml.server().is_none());
        assert_eq!(
            TYPESCRIPT_SERVER.config_names(),
            ["javascript", "jsx", "typescript", "tsx"]
        );
    }

    #[test]
    fn every_servers_initialization_is_json() {
        for row in LANGUAGES {
            let Some(server) = row.language.server() else {
                continue;
            };
            assert!(!server.candidates.is_empty(), "{}", row.name);
            for candidate in server.candidates {
                assert!(!candidate.install.is_empty(), "{}", candidate.command);
                if let Some(json) = candidate.initialization {
                    let parsed: Result<serde_json::Value, _> = serde_json::from_str(json);
                    assert!(parsed.is_ok(), "{}", candidate.command);
                }
            }
        }
    }
}
