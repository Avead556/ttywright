use std::collections::{HashMap, HashSet};
use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DirEntry {
    pub name: String,
    pub path: PathBuf,
    pub is_dir: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TreeRow {
    pub path: PathBuf,
    pub name: String,
    pub depth: usize,
    pub is_dir: bool,
    pub expanded: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeRequest {
    None,
    List(PathBuf),
    Open(PathBuf),
}

#[derive(Debug, Clone)]
pub struct ProjectTree {
    root: PathBuf,
    children: HashMap<PathBuf, Vec<DirEntry>>,
    expanded: HashSet<PathBuf>,
    selected: PathBuf,
}

impl ProjectTree {
    pub fn new(root: PathBuf) -> Self {
        Self {
            expanded: HashSet::from([root.clone()]),
            selected: root.clone(),
            children: HashMap::new(),
            root,
        }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn selected(&self) -> &Path {
        &self.selected
    }

    pub fn set_children(&mut self, dir: PathBuf, mut entries: Vec<DirEntry>) {
        entries.sort_by(|a, b| {
            b.is_dir
                .cmp(&a.is_dir)
                .then_with(|| a.name.to_lowercase().cmp(&b.name.to_lowercase()))
        });
        self.children.insert(dir, entries);
    }

    pub fn is_listed(&self, dir: &Path) -> bool {
        self.children.contains_key(dir)
    }

    pub fn rows(&self) -> Vec<TreeRow> {
        let mut rows = vec![TreeRow {
            path: self.root.clone(),
            name: self.root.file_name().map_or_else(
                || self.root.display().to_string(),
                |n| n.to_string_lossy().into_owned(),
            ),
            depth: 0,
            is_dir: true,
            expanded: self.expanded.contains(&self.root),
        }];
        if self.expanded.contains(&self.root) {
            self.push_children(&self.root, 1, &mut rows);
        }
        rows
    }

    fn push_children(&self, dir: &Path, depth: usize, rows: &mut Vec<TreeRow>) {
        for entry in self.children.get(dir).into_iter().flatten() {
            let expanded = entry.is_dir && self.expanded.contains(&entry.path);
            rows.push(TreeRow {
                path: entry.path.clone(),
                name: entry.name.clone(),
                depth,
                is_dir: entry.is_dir,
                expanded,
            });
            if expanded {
                self.push_children(&entry.path, depth + 1, rows);
            }
        }
    }

    fn selected_pos(&self, rows: &[TreeRow]) -> usize {
        rows.iter()
            .position(|r| r.path == self.selected)
            .unwrap_or(0)
    }

    pub fn selected_index(&self) -> usize {
        self.selected_pos(&self.rows())
    }

    pub fn select(&mut self, path: &Path) -> bool {
        let visible = self.rows().iter().any(|r| r.path == path);
        if visible {
            self.selected = path.to_path_buf();
        }
        visible
    }

    pub fn move_selection(&mut self, delta: isize) {
        let rows = self.rows();
        let pos = self
            .selected_pos(&rows)
            .saturating_add_signed(delta)
            .min(rows.len() - 1);
        self.selected = rows[pos].path.clone();
    }

    pub fn expand(&mut self) -> TreeRequest {
        let rows = self.rows();
        let pos = self.selected_pos(&rows);
        let row = &rows[pos];
        if !row.is_dir {
            return TreeRequest::None;
        }
        if !row.expanded {
            return self.toggle(row.path.clone());
        }
        if let Some(child) = rows.get(pos + 1).filter(|r| r.depth > row.depth) {
            self.selected = child.path.clone();
        }
        TreeRequest::None
    }

    pub fn collapse(&mut self) {
        let rows = self.rows();
        let row = &rows[self.selected_pos(&rows)];
        if row.is_dir && row.expanded {
            self.expanded.remove(&row.path);
        } else if let Some(parent) = row.path.parent().filter(|_| row.path != self.root) {
            self.selected = parent.to_path_buf();
        }
    }

    pub fn activate(&mut self) -> TreeRequest {
        let rows = self.rows();
        let row = &rows[self.selected_pos(&rows)];
        if row.is_dir {
            self.toggle(row.path.clone())
        } else {
            TreeRequest::Open(row.path.clone())
        }
    }

    fn toggle(&mut self, dir: PathBuf) -> TreeRequest {
        if self.expanded.remove(&dir) {
            return TreeRequest::None;
        }
        self.expanded.insert(dir.clone());
        if self.children.contains_key(&dir) {
            TreeRequest::None
        } else {
            TreeRequest::List(dir)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entry(path: &str, is_dir: bool) -> DirEntry {
        let path = PathBuf::from(path);
        DirEntry {
            name: path.file_name().unwrap().to_string_lossy().into_owned(),
            path,
            is_dir,
        }
    }

    fn tree() -> ProjectTree {
        let mut t = ProjectTree::new(PathBuf::from("/p"));
        t.set_children(
            PathBuf::from("/p"),
            vec![
                entry("/p/main.rs", false),
                entry("/p/src", true),
                entry("/p/Cargo.toml", false),
            ],
        );
        t
    }

    fn names(t: &ProjectTree) -> Vec<String> {
        t.rows()
            .iter()
            .map(|r| format!("{}{}", "  ".repeat(r.depth), r.name))
            .collect()
    }

    #[test]
    fn lists_directories_first_then_files_case_insensitively() {
        assert_eq!(names(&tree()), ["p", "  src", "  Cargo.toml", "  main.rs"]);
    }

    #[test]
    fn expanding_an_unlisted_directory_requests_a_listing() {
        let mut t = tree();
        t.move_selection(1);
        assert_eq!(t.expand(), TreeRequest::List(PathBuf::from("/p/src")));
        t.set_children(PathBuf::from("/p/src"), vec![entry("/p/src/lib.rs", false)]);
        assert_eq!(names(&t)[2], "    lib.rs");
        assert_eq!(t.expand(), TreeRequest::None);
        assert_eq!(t.selected(), Path::new("/p/src/lib.rs"));
    }

    #[test]
    fn collapse_goes_to_parent_then_folds() {
        let mut t = tree();
        t.move_selection(3);
        t.collapse();
        assert_eq!(t.selected(), Path::new("/p"));
        t.collapse();
        assert_eq!(names(&t), ["p"]);
    }

    #[test]
    fn enter_on_a_file_opens_it() {
        let mut t = tree();
        t.move_selection(99);
        assert_eq!(t.activate(), TreeRequest::Open(PathBuf::from("/p/main.rs")));
    }
}
