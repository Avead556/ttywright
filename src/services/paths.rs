use std::path::{Path, PathBuf};

use super::session::SessionStore;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProjectData {
    pub swap_dir: PathBuf,
    pub session: SessionStore,
    pub history_dir: PathBuf,
    pub server_choice_file: PathBuf,
    pub index_file: PathBuf,
}

impl ProjectData {
    pub fn for_root(root: &Path) -> Option<Self> {
        let dirs = directories::ProjectDirs::from("", "", "ttywright")?;
        let data_dir = dirs.data_dir();
        let Some(folder) = super::session::project_folder(root) else {
            return Some(Self::under(data_dir, data_dir, root));
        };
        Some(Self::moved_from(data_dir, &folder, root))
    }

    fn moved_from(data_dir: &Path, folder: &Path, root: &Path) -> Self {
        let old = Self::under(data_dir, data_dir, root);
        let mut new = Self::under(data_dir, folder, root);
        new.swap_dir = bring(&old.swap_dir, new.swap_dir);
        new.history_dir = bring(&old.history_dir, new.history_dir);
        new.index_file = bring(&old.index_file, new.index_file);
        new
    }

    fn under(data_dir: &Path, base: &Path, root: &Path) -> Self {
        let hash = path_hash(root);
        Self {
            swap_dir: base.join("swap").join(&hash),
            session: SessionStore::new(
                root,
                data_dir.join("sessions").join(format!("{hash}.json")),
            ),
            history_dir: base.join("history").join(&hash),
            server_choice_file: data_dir.join("lsp/choices").join(format!("{hash}.json")),
            index_file: base.join("index").join(format!("{hash}.bin")),
        }
    }
}

fn bring(old: &Path, new: PathBuf) -> PathBuf {
    if !old.exists() || new.exists() {
        return new;
    }
    let moved = new
        .parent()
        .is_some_and(|parent| std::fs::create_dir_all(parent).is_ok())
        && std::fs::rename(old, &new).is_ok();
    if moved { new } else { old.to_path_buf() }
}

pub fn servers_dirs() -> Option<(PathBuf, PathBuf)> {
    let dirs = directories::ProjectDirs::from("", "", "ttywright")?;
    let lsp = dirs.data_dir().join("lsp");
    Some((lsp.join("bundled"), lsp.join("downloaded")))
}

pub fn server_storage(server: &str, root: &Path) -> PathBuf {
    let base = directories::ProjectDirs::from("", "", "ttywright")
        .map_or_else(std::env::temp_dir, |d| d.data_dir().to_path_buf());
    base.join("lsp/storage").join(server).join(path_hash(root))
}

pub fn path_hash(path: &Path) -> String {
    blake3::hash(path.as_os_str().as_encoded_bytes())
        .to_hex()
        .to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn projects_do_not_share_data() {
        let data = Path::new("/data");
        let a = ProjectData::under(data, data, Path::new("/p/a"));
        let b = ProjectData::under(data, data, Path::new("/p/b"));
        assert_ne!(a.swap_dir, b.swap_dir);
        assert_ne!(a.session, b.session);
        assert_eq!(a, ProjectData::under(data, data, Path::new("/p/a")));
    }

    #[test]
    fn the_project_keeps_its_own_files_and_the_choice_of_a_server_stays_with_the_user() {
        let root = Path::new("/p/a");
        let d = ProjectData::under(Path::new("/data"), Path::new("/p/a/.ttywright"), root);
        for own in [&d.swap_dir, &d.history_dir, &d.index_file] {
            assert!(own.starts_with("/p/a/.ttywright"), "{own:?}");
        }
        assert!(
            d.server_choice_file.starts_with("/data"),
            "a repository must not pick a program"
        );
    }

    #[test]
    fn unsaved_text_and_history_move_into_the_project_and_are_not_left_behind() {
        let dir = tempfile::tempdir().unwrap();
        let data = dir.path().join("data");
        let root = dir.path().join("p");
        let folder = root.join(".ttywright");
        std::fs::create_dir_all(&folder).unwrap();
        let old = ProjectData::under(&data, &data, &root);
        std::fs::create_dir_all(&old.swap_dir).unwrap();
        std::fs::write(old.swap_dir.join("a.swap"), "unsaved").unwrap();
        let new = ProjectData::moved_from(&data, &folder, &root);
        assert!(new.swap_dir.starts_with(&folder));
        assert_eq!(
            std::fs::read_to_string(new.swap_dir.join("a.swap")).unwrap(),
            "unsaved"
        );
        assert!(!old.swap_dir.exists());
    }

    #[test]
    fn what_cannot_be_moved_stays_in_use_where_it_is() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("old");
        std::fs::write(&old, "x").unwrap();
        std::fs::write(dir.path().join("blocked"), "").unwrap();
        let wanted = dir.path().join("blocked/new");
        assert_eq!(bring(&old, wanted), old);
    }
}
