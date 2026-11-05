use std::collections::{BTreeMap, BTreeSet};
use std::path::{Path, PathBuf};

use super::git::{GitBranch, GitChange, GitFile, GitOperation};

#[derive(Debug, Default)]
pub struct GitRepo {
    branch: Option<String>,
    top: Option<PathBuf>,
    changes: BTreeMap<PathBuf, GitChange>,
    changed_dirs: BTreeSet<PathBuf>,
    bases: BTreeMap<PathBuf, Option<String>>,
    base_stamp: u64,
    branches: Vec<GitBranch>,
    operation: Option<GitOperation>,
}

impl GitRepo {
    pub fn branch(&self) -> Option<&str> {
        self.branch.as_deref()
    }

    pub fn is_current(&self, branch: &GitBranch) -> bool {
        self.branch() == Some(branch.name.as_str())
    }

    pub fn top_or(&self, root: &Path) -> PathBuf {
        self.top.clone().unwrap_or_else(|| root.to_path_buf())
    }

    pub fn change(&self, path: &Path) -> Option<GitChange> {
        self.changes.get(path).copied()
    }

    pub fn dir_changed(&self, path: &Path) -> bool {
        self.changed_dirs.contains(path)
    }

    pub fn base(&self, path: &Path) -> (Option<&str>, u64) {
        let text = self.bases.get(path).and_then(Option::as_deref);
        (text, self.base_stamp)
    }

    pub fn set_base(&mut self, path: PathBuf, text: Option<String>) {
        self.bases.insert(path, text);
        self.base_stamp += 1;
    }

    pub fn branches(&self) -> &[GitBranch] {
        &self.branches
    }

    pub fn set_branches(&mut self, branches: Vec<GitBranch>) {
        self.branches = branches;
    }

    pub const fn operation(&self) -> Option<GitOperation> {
        self.operation
    }

    pub fn status_read(
        &mut self,
        branch: Option<String>,
        top: Option<PathBuf>,
        files: Result<&[GitFile], &str>,
        operation: Option<GitOperation>,
    ) {
        self.operation = operation;
        self.branch = branch;
        self.changes.clear();
        self.changed_dirs.clear();
        if let (Some(top), Ok(files)) = (&top, files) {
            for file in files {
                let path = top.join(&file.path);
                let mut dir = path.parent();
                while let Some(folder) = dir.filter(|d| d.starts_with(top)) {
                    self.changed_dirs.insert(folder.to_path_buf());
                    dir = folder.parent();
                }
                self.changes.insert(path, file.change);
            }
        }
        self.top = top;
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::git::GitCode;

    #[test]
    fn a_change_marks_its_folders_up_to_the_repository_and_no_further() {
        let mut repo = GitRepo::default();
        assert_eq!(repo.top_or(Path::new("/p")), PathBuf::from("/p"));
        let file = GitFile {
            path: PathBuf::from("src/core/a.rs"),
            old_path: None,
            change: GitChange::Modified,
            code: GitCode::new(' ', 'M'),
        };
        repo.status_read(
            Some("main".to_owned()),
            Some(PathBuf::from("/repo")),
            Ok(std::slice::from_ref(&file)),
            None,
        );
        assert_eq!(
            repo.change(Path::new("/repo/src/core/a.rs")),
            Some(GitChange::Modified)
        );
        assert!(repo.dir_changed(Path::new("/repo/src/core")));
        assert!(repo.dir_changed(Path::new("/repo")));
        assert!(!repo.dir_changed(Path::new("/")));
        assert_eq!(repo.top_or(Path::new("/p")), PathBuf::from("/repo"));
        assert!(repo.is_current(&GitBranch {
            name: "main".to_owned(),
            remote: false,
        }));
        repo.status_read(None, None, Err("not a repository"), None);
        assert_eq!(repo.change(Path::new("/repo/src/core/a.rs")), None);
    }

    #[test]
    fn every_base_read_moves_the_stamp() {
        let mut repo = GitRepo::default();
        let path = PathBuf::from("/repo/a.rs");
        let (_, before) = repo.base(&path);
        repo.set_base(path.clone(), Some("old".to_owned()));
        assert_eq!(repo.base(&path), (Some("old"), before + 1));
    }
}
