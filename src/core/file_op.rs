use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::input::LineInput;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum FileOp {
    NewFile(PathBuf),
    NewFolder(PathBuf),
    Create {
        path: PathBuf,
        contents: String,
        executable: bool,
    },
    CreateProject {
        dir: PathBuf,
        files: Vec<(String, String)>,
    },
    Rename {
        from: PathBuf,
        to: PathBuf,
    },
    Trash(PathBuf),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FilePromptKind {
    NewFile { dir: PathBuf },
    NewFolder { dir: PathBuf },
    Rename { path: PathBuf },
    Delete { path: PathBuf },
    FromTemplate { dir: PathBuf, index: usize },
    NewProject { index: usize },
    BreakpointCondition { path: PathBuf, line: usize },
    BreakpointLog { path: PathBuf, line: usize },
    DebugEvaluate,
    DebugWatch,
    TrustProject { why: Option<String> },
    RunAnything { back: usize },
    RenameSymbol { old: String },
    CoverageReport,
    SafeDeleteConfirm { summary: String },
    RenameConfirm { summary: String },
}

#[derive(Debug, Clone)]
pub struct FilePrompt {
    pub kind: FilePromptKind,
    pub input: LineInput,
    pub error: Option<String>,
    pub busy: bool,
}

pub fn name_under(dir: &Path, name: &str) -> Result<PathBuf, String> {
    let name = name.trim();
    if name.is_empty() {
        return Err("Type a name".to_owned());
    }
    let relative = Path::new(name);
    if !relative
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
    {
        return Err("The name must stay inside this folder: no `..` and no leading `/`".to_owned());
    }
    Ok(dir.join(relative))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_name_may_go_deeper_but_never_out() {
        let dir = Path::new("/p/src");
        assert_eq!(name_under(dir, " a.rs "), Ok(PathBuf::from("/p/src/a.rs")));
        assert_eq!(
            name_under(dir, "x/y/a.rs"),
            Ok(PathBuf::from("/p/src/x/y/a.rs"))
        );
        for bad in ["", "   ", "../a", "x/../../a", "/etc/a", "."] {
            assert!(name_under(dir, bad).is_err(), "{bad:?}");
        }
    }
}
