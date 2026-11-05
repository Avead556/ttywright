use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::document::DiskState;
use super::layout::Axis;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Session {
    pub version: u32,
    pub layout: SavedLayout,
    pub project_visible: bool,
    pub tool_window_visible: bool,
    #[serde(default)]
    pub recent_files: Vec<PathBuf>,
    #[serde(default)]
    pub tool_height: Option<u16>,
    #[serde(default)]
    pub project_width: Option<u16>,
    #[serde(default)]
    pub bookmarks: Vec<SavedBookmark>,
    #[serde(default)]
    pub breakpoints: Vec<SavedBreakpoint>,
    #[serde(default)]
    pub watches: Vec<String>,
    #[serde(default)]
    pub changelists: std::collections::BTreeMap<String, std::collections::BTreeSet<PathBuf>>,
    #[serde(default)]
    pub markdown_preview: std::collections::BTreeMap<PathBuf, bool>,
}

impl Session {
    pub const VERSION: u32 = 1;

    pub fn paths(&self) -> Vec<PathBuf> {
        let mut paths = Vec::new();
        self.layout.collect_paths(&mut paths);
        paths.sort();
        paths.dedup();
        paths
    }

    pub fn map_paths(&mut self, mut change: impl FnMut(&Path) -> Option<PathBuf>) {
        self.layout.map_paths(&mut change);
        self.recent_files = self.recent_files.iter().filter_map(|p| change(p)).collect();
        self.markdown_preview = std::mem::take(&mut self.markdown_preview)
            .into_iter()
            .filter_map(|(path, shown)| Some((change(&path)?, shown)))
            .collect();
        self.bookmarks = std::mem::take(&mut self.bookmarks)
            .into_iter()
            .filter_map(|b| {
                let path = change(&b.path)?;
                Some(SavedBookmark { path, line: b.line })
            })
            .collect();
        self.breakpoints = std::mem::take(&mut self.breakpoints)
            .into_iter()
            .filter_map(|b| {
                let path = change(&b.path)?;
                Some(SavedBreakpoint { path, ..b })
            })
            .collect();
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedBreakpoint {
    pub path: PathBuf,
    pub line: usize,
    pub condition: Option<String>,
    pub log_message: Option<String>,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedBookmark {
    pub path: PathBuf,
    pub line: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum SavedLayout {
    Pane {
        views: Vec<SavedView>,
        selected: usize,
        active: bool,
    },
    Split {
        axis: Axis,
        children: Vec<SavedLayout>,
    },
}

impl SavedLayout {
    fn map_paths(&mut self, change: &mut impl FnMut(&Path) -> Option<PathBuf>) {
        match self {
            Self::Pane {
                views, selected, ..
            } => {
                views.retain_mut(|view| match change(&view.path) {
                    Some(path) => {
                        view.path = path;
                        true
                    }
                    None => false,
                });
                *selected = (*selected).min(views.len().saturating_sub(1));
            }
            Self::Split { children, .. } => {
                for child in children {
                    child.map_paths(change);
                }
            }
        }
    }

    fn collect_paths(&self, paths: &mut Vec<PathBuf>) {
        match self {
            Self::Pane { views, .. } => paths.extend(views.iter().map(|v| v.path.clone())),
            Self::Split { children, .. } => {
                for child in children {
                    child.collect_paths(paths);
                }
            }
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedView {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionFile {
    pub path: PathBuf,
    pub disk: Option<DiskState>,
    pub result: Result<Option<Vec<u8>>, String>,
}
