pub const H_LINE: &str = "─";
pub const V_LINE: &str = "│";
pub const V_MARK: &str = "┃";
pub const COVERED: &str = "✓";
pub const NOT_COVERED: &str = "✗";
pub const DELETED_MARK: &str = "▁";
pub const TEE_DOWN: &str = "┬";
pub const TEE_UP: &str = "┴";
pub const TEE_RIGHT: &str = "├";
pub const TAB_UNDERLINE: &str = "▔";
pub const CLOSE: &str = "×";
pub const MODIFIED: &str = "●";
pub const BREADCRUMB: &str = "›";
pub const SUCCESS: &str = "✓";
pub const ERROR: &str = "✖";
pub const WARNING: &str = "▲";
pub const PROMPT: &str = "❯";
pub const SELECTED: &str = "▌";
pub const EXPANDED: &str = "▾";
pub const COLLAPSED: &str = "▸";
pub const SPINNER: &str = "⠋";
pub const FILE: &str = "▪";
pub const PROJECT: &str = "◫";
pub const FIND: &str = "⌕";
pub const GIT: &str = "⎇";
pub const CLAUDE: &str = "✳";
pub const DEBUG: &str = "◉";
pub const LANGUAGE_SERVER: &str = "λ";
pub const TODO: &str = "☑";
pub const TESTS: &str = "✓";
pub const DEPENDENCIES: &str = "⧉";
pub const BOOKMARKS: &str = "⚑";
pub const HTTP: &str = "⇄";
pub const DATABASE: &str = "⊟";
pub const DOCKER: &str = "▣";
pub const GITHUB: &str = "⑂";
pub const BREAKPOINT: &str = "●";
pub const BREAKPOINT_OFF: &str = "○";
pub const BREAKPOINT_CONDITION: &str = "◆";
pub const LOGPOINT: &str = "◇";
pub const EXECUTION: &str = "▶";
pub const STOP: &str = "■";

pub const NERD_FOLDER: &str = "\u{f07b}";
pub const NERD_FOLDER_OPEN: &str = "\u{f07c}";

pub fn nerd_file(path: &std::path::Path) -> &'static str {
    let name = path.file_name().and_then(|n| n.to_str()).unwrap_or("");
    match name {
        "Dockerfile" => return "\u{e7b0}",
        ".gitignore" | ".gitattributes" | ".gitmodules" => return "\u{e702}",
        "Cargo.lock" | "package-lock.json" | "composer.lock" | "yarn.lock" => return "\u{f023}",
        _ => {}
    }
    match path.extension().and_then(|e| e.to_str()).unwrap_or("") {
        "rs" => "\u{e7a8}",
        "php" | "phtml" => "\u{e73d}",
        "py" | "pyi" | "pyw" => "\u{e73c}",
        "js" | "mjs" | "cjs" | "jsx" => "\u{e74e}",
        "ts" | "mts" | "cts" | "tsx" => "\u{e628}",
        "java" => "\u{e738}",
        "go" => "\u{e627}",
        "rb" => "\u{e739}",
        "c" | "h" => "\u{e61e}",
        "cpp" | "cc" | "cxx" | "hpp" => "\u{e61d}",
        "html" | "htm" => "\u{e736}",
        "css" | "scss" => "\u{e749}",
        "vue" => "\u{e6a0}",
        "md" => "\u{e73e}",
        "json" => "\u{e60b}",
        "toml" | "yml" | "yaml" | "ini" => "\u{e615}",
        "sql" => "\u{f1c0}",
        "sh" | "zsh" | "bash" => "\u{f489}",
        _ => "\u{f15b}",
    }
}
