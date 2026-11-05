use std::fs;
use std::io;
use std::path::{Component, Path, PathBuf};

use super::fs::{disk_state, read_if_exists, write_atomic};
use crate::core::{Event, Session, SessionFile};

const PROJECT_FOLDER: &str = ".ttywright";
const SESSION_FILE: &str = "workspace.json";
const FOLDER_GITIGNORE: &str = "workspace.json\n.gitignore\nswap/\nhistory/\nindex/\n";
const OLD_FOLDER_GITIGNORE: &str = "workspace.json\n.gitignore\n";
const CACHE_FOLDERS: [&str; 3] = ["swap", "history", "index"];

pub fn is_own_file(path: &Path) -> bool {
    let inside = path
        .parent()
        .and_then(Path::file_name)
        .is_some_and(|dir| dir == PROJECT_FOLDER);
    inside
        && path
            .file_name()
            .is_some_and(|n| n == SESSION_FILE || n == ".gitignore")
}

pub fn is_cache_folder(path: &Path) -> bool {
    path.parent()
        .and_then(Path::file_name)
        .is_some_and(|dir| dir == PROJECT_FOLDER)
        && path
            .file_name()
            .is_some_and(|n| CACHE_FOLDERS.iter().any(|c| n == *c))
}

pub fn is_in_cache_folder(path: &Path) -> bool {
    path.ancestors().any(is_cache_folder)
}

fn write_folder_gitignore(dir: &Path) -> io::Result<()> {
    let ignore = dir.join(".gitignore");
    let current = fs::read_to_string(&ignore).ok();
    if current.is_none() || current.as_deref() == Some(OLD_FOLDER_GITIGNORE) {
        write_atomic(&ignore, FOLDER_GITIGNORE.as_bytes())?;
    }
    Ok(())
}

pub fn project_folder(root: &Path) -> Option<PathBuf> {
    let dir = root.join(PROJECT_FOLDER);
    fs::create_dir_all(&dir).ok()?;
    write_folder_gitignore(&dir).ok()?;
    let probe = dir.join(format!(".idea-tmp-probe-{}", std::process::id()));
    fs::write(&probe, b"").ok()?;
    fs::remove_file(&probe).ok()?;
    Some(dir)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SessionStore {
    root: PathBuf,
    file: PathBuf,
    fallback: PathBuf,
}

impl SessionStore {
    pub fn new(root: &Path, fallback: PathBuf) -> Self {
        Self {
            root: root.to_path_buf(),
            file: root.join(PROJECT_FOLDER).join(SESSION_FILE),
            fallback,
        }
    }

    pub fn load(&self) -> Option<Event> {
        match read(&self.file) {
            Some(session) => Some(with_files(self.limited_to_project(session))),
            None => load(&self.fallback),
        }
    }

    fn limited_to_project(&self, mut session: Session) -> Session {
        let root = fs::canonicalize(&self.root).unwrap_or_else(|_| self.root.clone());
        session.map_paths(|saved| {
            let path = self.root.join(saved);
            let inside = fs::canonicalize(&path).is_ok_and(|real| real.starts_with(&root))
                && !saved.components().any(|c| c == Component::ParentDir);
            inside.then_some(path)
        });
        session
    }

    pub fn save(&self, session: &Session) -> io::Result<()> {
        let mut relative = session.clone();
        relative
            .map_paths(|path| Some(path.strip_prefix(&self.root).unwrap_or(path).to_path_buf()));
        if self.save_in_project(&relative).is_err() {
            return save(&self.fallback, session);
        }
        match fs::remove_file(&self.fallback) {
            Err(e) if e.kind() != io::ErrorKind::NotFound => Err(e),
            _ => Ok(()),
        }
    }

    fn save_in_project(&self, session: &Session) -> io::Result<()> {
        let dir = self.root.join(PROJECT_FOLDER);
        fs::create_dir_all(&dir)?;
        write_folder_gitignore(&dir)?;
        save(&self.file, session)
    }
}

fn read(file: &Path) -> Option<Session> {
    let session: Session = serde_json::from_slice(&fs::read(file).ok()?).ok()?;
    (session.version == Session::VERSION).then_some(session)
}

pub fn load(file: &Path) -> Option<Event> {
    read(file).map(with_files)
}

fn with_files(mut session: Session) -> Event {
    session.recent_files.retain(|path| path.exists());
    let files = session
        .paths()
        .into_iter()
        .map(|path| SessionFile {
            disk: disk_state(&path).unwrap_or(None),
            result: read_if_exists(&path).map_err(|e| e.to_string()),
            path,
        })
        .collect();
    Event::SessionLoaded { session, files }
}

pub fn save(file: &Path, session: &Session) -> io::Result<()> {
    if let Some(dir) = file.parent() {
        fs::create_dir_all(dir)?;
    }
    write_atomic(file, &serde_json::to_vec(session)?)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::core::{SavedLayout, SavedView};

    fn session(path: &Path) -> Session {
        Session {
            version: Session::VERSION,
            layout: SavedLayout::Pane {
                views: vec![SavedView {
                    path: path.to_path_buf(),
                    line: 1,
                    col: 2,
                }],
                selected: 0,
                active: true,
            },
            project_visible: false,
            tool_window_visible: true,
            changelists: std::collections::BTreeMap::new(),
            markdown_preview: std::collections::BTreeMap::new(),
            recent_files: Vec::new(),
            project_width: None,
            bookmarks: Vec::new(),
            breakpoints: Vec::new(),
            watches: Vec::new(),
            tool_height: Some(12),
        }
    }

    #[test]
    fn a_session_of_an_older_build_loads_with_nothing_lost() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("a.txt");
        fs::write(&text, "x").unwrap();
        let file = dir.path().join("s.json");
        let old = serde_json::json!({
            "version": Session::VERSION,
            "layout": {"Pane": {
                "views": [{"path": text, "line": 1, "col": 2}],
                "selected": 0,
                "active": true,
            }},
            "project_visible": false,
            "tool_window_visible": true,
        });
        fs::write(&file, old.to_string()).unwrap();
        let Some(Event::SessionLoaded { session, files }) = load(&file) else {
            panic!("the old session was dropped");
        };
        assert_eq!(files.len(), 1);
        assert!(!session.project_visible && session.tool_window_visible);
        assert!(session.recent_files.is_empty());
        assert_eq!(session.tool_height, None);
    }

    #[test]
    fn recent_files_that_are_gone_are_forgotten_on_load() {
        let dir = tempfile::tempdir().unwrap();
        let kept = dir.path().join("kept.txt");
        fs::write(&kept, "x").unwrap();
        let file = dir.path().join("s.json");
        let mut saved = session(&kept);
        saved.recent_files = vec![dir.path().join("gone.txt"), kept.clone()];
        save(&file, &saved).unwrap();
        let Some(Event::SessionLoaded { session, files }) = load(&file) else {
            panic!("no session");
        };
        assert_eq!(session.recent_files, [kept]);
        assert_eq!(files.len(), 1, "recent files are not read at startup");
    }

    #[test]
    fn saved_session_loads_back_with_its_files() {
        let dir = tempfile::tempdir().unwrap();
        let text = dir.path().join("a.txt");
        fs::write(&text, "x").unwrap();
        let file = dir.path().join("sessions/s.json");
        let saved = session(&text);
        save(&file, &saved).unwrap();

        let Some(Event::SessionLoaded { session, files }) = load(&file) else {
            panic!("no session");
        };
        assert_eq!(session, saved);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].result, Ok(Some(b"x".to_vec())));
        assert_eq!(files[0].disk, disk_state(&text).unwrap());
    }

    #[test]
    fn missing_damaged_or_foreign_sessions_are_ignored() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("s.json");
        assert!(load(&file).is_none());
        fs::write(&file, "{not json").unwrap();
        assert!(load(&file).is_none());
        let mut other = session(&file);
        other.version = Session::VERSION + 1;
        save(&file, &other).unwrap();
        assert!(load(&file).is_none());
    }

    fn store(dir: &Path) -> (PathBuf, SessionStore) {
        let root = fs::canonicalize(dir).unwrap().join("proj");
        fs::create_dir_all(&root).unwrap();
        let store = SessionStore::new(&root, dir.join("data/s.json"));
        (root, store)
    }

    fn loaded(store: &SessionStore) -> (Session, Vec<PathBuf>) {
        let Some(Event::SessionLoaded { session, files }) = store.load() else {
            panic!("no session");
        };
        (session, files.into_iter().map(|f| f.path).collect())
    }

    #[test]
    fn it_is_kept_in_the_project_with_relative_paths_and_out_of_git() {
        let dir = tempfile::tempdir().unwrap();
        let (root, store) = store(dir.path());
        fs::write(root.join("a.txt"), "x").unwrap();
        store.save(&session(&root.join("a.txt"))).unwrap();
        let text = fs::read_to_string(root.join(".ttywright/workspace.json")).unwrap();
        assert!(
            text.contains("\"a.txt\"") && !text.contains(root.to_str().unwrap()),
            "{text}"
        );
        assert_eq!(
            fs::read_to_string(root.join(".ttywright/.gitignore")).unwrap(),
            FOLDER_GITIGNORE
        );
        let (session, files) = loaded(&store);
        assert_eq!(files, [root.join("a.txt")]);
        assert_eq!(session.tool_height, Some(12));
    }

    #[test]
    fn a_project_that_moved_keeps_its_session() {
        let dir = tempfile::tempdir().unwrap();
        let (root, store) = store(dir.path());
        fs::write(root.join("a.txt"), "x").unwrap();
        store.save(&session(&root.join("a.txt"))).unwrap();
        let moved = root.with_file_name("moved");
        fs::rename(&root, &moved).unwrap();
        let store = SessionStore::new(&moved, dir.path().join("data/s.json"));
        assert_eq!(loaded(&store).1, [moved.join("a.txt")]);
    }

    #[test]
    fn a_session_from_before_moves_into_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let (root, store) = store(dir.path());
        fs::write(root.join("a.txt"), "x").unwrap();
        let old = dir.path().join("data/s.json");
        save(&old, &session(&root.join("a.txt"))).unwrap();
        let (session, files) = loaded(&store);
        assert_eq!(files, [root.join("a.txt")]);
        store.save(&session).unwrap();
        assert!(!old.exists());
        assert_eq!(loaded(&store).1, [root.join("a.txt")]);
    }

    #[test]
    fn a_shipped_file_opens_nothing_outside_the_project() {
        let dir = tempfile::tempdir().unwrap();
        let (root, store) = store(dir.path());
        let secret = dir.path().join("secret");
        fs::write(&secret, "s").unwrap();
        fs::write(root.join("a.txt"), "x").unwrap();
        std::os::unix::fs::symlink(&secret, root.join("link")).unwrap();
        let mut shipped = session(&secret);
        if let SavedLayout::Pane { views, .. } = &mut shipped.layout {
            for path in ["../secret", "link", "a.txt"] {
                views.push(SavedView {
                    path: PathBuf::from(path),
                    line: 0,
                    col: 0,
                });
            }
        }
        shipped.recent_files = vec![secret.clone()];
        let folder = root.join(".ttywright");
        fs::create_dir_all(&folder).unwrap();
        fs::write(
            folder.join("workspace.json"),
            serde_json::to_vec(&shipped).unwrap(),
        )
        .unwrap();
        let (session, files) = loaded(&store);
        assert_eq!(files, [root.join("a.txt")]);
        assert!(session.recent_files.is_empty());
    }

    #[test]
    fn a_read_only_project_keeps_its_session_in_the_data_folder() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let (root, store) = store(dir.path());
        fs::write(root.join("a.txt"), "x").unwrap();
        fs::set_permissions(&root, fs::Permissions::from_mode(0o555)).unwrap();
        let saved = store.save(&session(&root.join("a.txt")));
        let files = loaded(&store).1;
        fs::set_permissions(&root, fs::Permissions::from_mode(0o755)).unwrap();
        saved.unwrap();
        assert_eq!(files, [root.join("a.txt")]);
        assert!(dir.path().join("data/s.json").exists());
    }

    #[test]
    fn only_our_own_writes_are_ignored_by_the_watcher() {
        assert!(is_own_file(Path::new("/p/.ttywright/workspace.json")));
        assert!(is_own_file(Path::new("/p/.ttywright/.gitignore")));
        assert!(!is_own_file(Path::new("/p/.ttywright/config.toml")));
        assert!(!is_own_file(Path::new("/p/workspace.json")));
    }
}

#[cfg(test)]
mod folder_tests {
    use super::*;

    #[test]
    fn the_folder_is_hidden_from_git_and_an_old_list_is_brought_up_to_date() {
        let dir = tempfile::tempdir().unwrap();
        let folder = project_folder(dir.path()).unwrap();
        let ignore = folder.join(".gitignore");
        for name in ["swap/", "history/", "index/"] {
            assert!(fs::read_to_string(&ignore).unwrap().contains(name));
        }
        fs::write(&ignore, OLD_FOLDER_GITIGNORE).unwrap();
        project_folder(dir.path()).unwrap();
        assert_eq!(fs::read_to_string(&ignore).unwrap(), FOLDER_GITIGNORE);
        fs::write(&ignore, "my own rules\n").unwrap();
        project_folder(dir.path()).unwrap();
        assert_eq!(
            fs::read_to_string(&ignore).unwrap(),
            "my own rules\n",
            "not ours to change"
        );
    }

    #[test]
    fn only_our_folders_count_as_ours() {
        assert!(is_in_cache_folder(Path::new("/p/.ttywright/index/x.bin")));
        assert!(is_in_cache_folder(Path::new("/p/.ttywright/swap")));
        assert!(!is_in_cache_folder(Path::new("/p/.ttywright/config.toml")));
        assert!(!is_in_cache_folder(Path::new("/p/src/index/x.rs")));
    }
}
