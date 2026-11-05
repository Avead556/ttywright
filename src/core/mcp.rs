use std::path::{Component, Path, PathBuf};

use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::local_history::{DiffRow, diff_rows};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ProposedEdit {
    pub path: PathBuf,
    pub old_text: String,
    pub new_text: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum McpCall {
    OpenFiles,
    Selection,
    ReadBuffer {
        path: PathBuf,
        from: Option<usize>,
        to: Option<usize>,
    },
    Diagnostics {
        path: Option<PathBuf>,
    },
    OpenFile {
        path: PathBuf,
        line: Option<usize>,
        column: Option<usize>,
    },
    ProposeEdit {
        summary: String,
        edits: Vec<ProposedEdit>,
    },
    RunOutput {
        last: usize,
    },
    OpenDiff {
        path: PathBuf,
        contents: String,
        tab: String,
    },
    CloseDiffs {
        tab: Option<String>,
    },
    IdeDiagnostics,
    WorkspaceFolders,
}

const DEFAULT_RUN_LINES: usize = 200;

fn text_arg(args: &Value, name: &str) -> Result<String, String> {
    args.get(name)
        .and_then(Value::as_str)
        .map(str::to_owned)
        .ok_or_else(|| format!("`{name}` is required"))
}

fn number_arg(args: &Value, name: &str) -> Option<usize> {
    args.get(name)
        .and_then(Value::as_u64)
        .and_then(|n| usize::try_from(n).ok())
}

fn project_path(root: &Path, text: &str) -> Result<PathBuf, String> {
    let path = Path::new(text);
    if path.components().any(|c| c == Component::ParentDir) {
        return Err(format!("`..` is not accepted in a path: {text}"));
    }
    let path = root.join(path);
    if path.starts_with(root) {
        Ok(path)
    } else {
        Err(format!("Outside this project: {text}"))
    }
}

impl McpCall {
    pub fn parse(tool: &str, args: &Value, root: &Path) -> Result<Self, String> {
        let path = || project_path(root, &text_arg(args, "path")?);
        Ok(match tool {
            "get_open_files" => Self::OpenFiles,
            "get_selection" => Self::Selection,
            "read_buffer" => Self::ReadBuffer {
                path: path()?,
                from: number_arg(args, "from_line"),
                to: number_arg(args, "to_line"),
            },
            "get_diagnostics" => Self::Diagnostics {
                path: args.get("path").is_some().then(path).transpose()?,
            },
            "open_file" => Self::OpenFile {
                path: path()?,
                line: number_arg(args, "line"),
                column: number_arg(args, "column"),
            },
            "propose_edit" => {
                let list = args
                    .get("edits")
                    .and_then(Value::as_array)
                    .filter(|l| !l.is_empty())
                    .ok_or("`edits` must be a list with at least one edit")?;
                let edits = list
                    .iter()
                    .map(|e| {
                        Ok(ProposedEdit {
                            path: project_path(root, &text_arg(e, "path")?)?,
                            old_text: text_arg(e, "old_text")?,
                            new_text: text_arg(e, "new_text")?,
                        })
                    })
                    .collect::<Result<_, String>>()?;
                Self::ProposeEdit {
                    summary: text_arg(args, "summary").unwrap_or_default(),
                    edits,
                }
            }
            "get_run_output" => Self::RunOutput {
                last: number_arg(args, "last_lines").unwrap_or(DEFAULT_RUN_LINES),
            },
            "openDiff" => Self::OpenDiff {
                path: project_path(root, &text_arg(args, "new_file_path")?)?,
                contents: text_arg(args, "new_file_contents")?,
                tab: text_arg(args, "tab_name")?,
            },
            "close_tab" => Self::CloseDiffs {
                tab: Some(text_arg(args, "tab_name")?),
            },
            "closeAllDiffTabs" => Self::CloseDiffs { tab: None },
            "openFile" => Self::OpenFile {
                path: project_path(root, &text_arg(args, "filePath")?)?,
                line: None,
                column: None,
            },
            "getDiagnostics" => Self::IdeDiagnostics,
            "getCurrentSelection" => Self::Selection,
            "getOpenEditors" => Self::OpenFiles,
            "getWorkspaceFolders" => Self::WorkspaceFolders,
            other => return Err(format!("Unknown tool: {other}")),
        })
    }

    pub fn paths(&self) -> Vec<PathBuf> {
        match self {
            Self::ReadBuffer { path, .. } | Self::OpenDiff { path, .. } => vec![path.clone()],
            Self::ProposeEdit { edits, .. } => {
                let mut paths: Vec<PathBuf> = edits.iter().map(|e| e.path.clone()).collect();
                paths.sort();
                paths.dedup();
                paths
            }
            _ => Vec::new(),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Decision {
    Accepted,
    Rejected,
    Stale,
    Applying,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProposedFile {
    pub path: PathBuf,
    pub original: String,
    pub proposed: String,
    pub rows: Vec<DiffRow>,
    pub decision: Option<Decision>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct McpProposal {
    pub id: u64,
    pub tab: Option<String>,
    pub summary: String,
    pub files: Vec<ProposedFile>,
}

pub fn propose_file(
    path: &Path,
    original: &str,
    edits: &[&ProposedEdit],
) -> Result<ProposedFile, String> {
    let name = path.display();
    let mut text = original.to_owned();
    for edit in edits {
        if edit.old_text.is_empty() {
            return Err(format!("`old_text` is empty for {name}"));
        }
        match text.matches(&edit.old_text).count() {
            0 => return Err(format!("`old_text` is not in {name}: read it again")),
            1 => text = text.replacen(&edit.old_text, &edit.new_text, 1),
            n => {
                return Err(format!(
                    "`old_text` is in {name} {n} times: include more of the lines around it"
                ));
            }
        }
    }
    if text == original {
        return Err(format!("These edits change nothing in {name}"));
    }
    Ok(ProposedFile {
        path: path.to_path_buf(),
        rows: diff_rows(original, &text),
        original: original.to_owned(),
        proposed: text,
        decision: None,
    })
}

pub fn diff_file(path: &Path, original: &str, proposed: String) -> ProposedFile {
    ProposedFile {
        path: path.to_path_buf(),
        rows: diff_rows(original, &proposed),
        original: original.to_owned(),
        proposed,
        decision: None,
    }
}

pub fn smallest_change(old: &str, new: &str) -> (usize, usize, String) {
    let old: Vec<char> = old.chars().collect();
    let new: Vec<char> = new.chars().collect();
    let start = old.iter().zip(&new).take_while(|(a, b)| a == b).count();
    let room = old.len().min(new.len()) - start;
    let tail = old
        .iter()
        .rev()
        .zip(new.iter().rev())
        .take(room)
        .take_while(|(a, b)| a == b)
        .count();
    let text = new[start..new.len() - tail].iter().collect();
    (start, old.len() - tail, text)
}

pub fn proposal_answer(proposal: &McpProposal) -> String {
    if let (Some(tab), Some(file)) = (&proposal.tab, proposal.files.first()) {
        let items: Vec<&str> = if file.decision == Some(Decision::Accepted) {
            vec!["FILE_SAVED", &file.proposed]
        } else {
            vec!["DIFF_REJECTED", tab]
        };
        return serde_json::to_string(&items).unwrap_or_default();
    }
    let names = |decision: Decision| {
        let list: Vec<String> = proposal
            .files
            .iter()
            .filter(|f| f.decision == Some(decision))
            .map(|f| f.path.display().to_string())
            .collect();
        list
    };
    serde_json::json!({
        "accepted": names(Decision::Accepted),
        "rejected": names(Decision::Rejected),
        "changed_before_it_could_be_applied": names(Decision::Stale),
    })
    .to_string()
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    const ROOT: &str = "/p";

    fn edit(old: &str, new: &str) -> ProposedEdit {
        ProposedEdit {
            path: "/p/a".into(),
            old_text: old.into(),
            new_text: new.into(),
        }
    }

    #[test]
    fn paths_are_the_project_s_and_stay_inside_it() {
        let root = Path::new(ROOT);
        let parse = |p: &str| McpCall::parse("read_buffer", &json!({ "path": p }), root);
        assert!(
            matches!(parse("a/b"), Ok(McpCall::ReadBuffer { path, .. }) if path == Path::new("/p/a/b"))
        );
        assert!(
            matches!(parse("/p/a"), Ok(McpCall::ReadBuffer { path, .. }) if path == Path::new("/p/a"))
        );
        assert!(parse("/etc/passwd").is_err());
        assert!(parse("../secret").is_err());
        assert!(parse("/p/../etc").is_err());
    }

    #[test]
    fn a_bad_call_says_what_is_missing() {
        let root = Path::new(ROOT);
        assert!(
            McpCall::parse("nope", &json!({}), root)
                .unwrap_err()
                .contains("nope")
        );
        assert!(
            McpCall::parse("read_buffer", &json!({}), root)
                .unwrap_err()
                .contains("path")
        );
        assert!(McpCall::parse("propose_edit", &json!({ "edits": [] }), root).is_err());
        let one = json!({ "edits": [{ "path": "a", "old_text": "x" }] });
        assert!(
            McpCall::parse("propose_edit", &one, root)
                .unwrap_err()
                .contains("new_text")
        );
    }

    #[test]
    fn an_edit_applies_only_where_it_is_unmistakable() {
        let path = Path::new("/p/a");
        let file = propose_file(path, "one\ntwo\n", &[&edit("two", "2")]).unwrap();
        assert_eq!(file.proposed, "one\n2\n");
        assert!(
            propose_file(path, "x x", &[&edit("x", "y")])
                .unwrap_err()
                .contains("2 times")
        );
        assert!(
            propose_file(path, "x", &[&edit("z", "y")])
                .unwrap_err()
                .contains("not in")
        );
        assert!(propose_file(path, "x", &[&edit("", "y")]).is_err());
        assert!(propose_file(path, "x", &[&edit("x", "x")]).is_err());
    }

    #[test]
    fn later_edits_see_what_earlier_ones_did() {
        let file = propose_file(
            Path::new("/p/a"),
            "a b",
            &[&edit("a", "c"), &edit("c b", "done")],
        )
        .unwrap();
        assert_eq!(file.proposed, "done");
    }

    #[test]
    fn the_change_is_as_small_as_it_can_be_in_chars() {
        assert_eq!(
            smallest_change("héllo wörld", "héllo wörlds"),
            (11, 11, "s".to_owned())
        );
        assert_eq!(smallest_change("abc", "axc"), (1, 2, "x".to_owned()));
        assert_eq!(smallest_change("aaa", "aa"), (2, 3, String::new()));
        assert_eq!(smallest_change("aa", "aaa"), (2, 2, "a".to_owned()));
        assert_eq!(smallest_change("", "x"), (0, 0, "x".to_owned()));
    }
}
