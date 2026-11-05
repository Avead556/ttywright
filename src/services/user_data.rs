use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

use crate::services::fs::write_atomic;

fn data_dir() -> Option<PathBuf> {
    Some(
        directories::ProjectDirs::from("", "", "ttywright")?
            .data_dir()
            .to_path_buf(),
    )
}

fn trust_file() -> Option<PathBuf> {
    Some(data_dir()?.join("trust.json"))
}

fn key(root: &Path) -> String {
    root.canonicalize()
        .unwrap_or_else(|_| root.to_path_buf())
        .display()
        .to_string()
}

fn read_trust(file: &Path) -> BTreeMap<String, bool> {
    std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn trust_in(file: &Path, root: &Path) -> Option<bool> {
    read_trust(file).get(&key(root)).copied()
}

pub fn set_trust_in(file: &Path, root: &Path, trusted: bool) -> Result<(), String> {
    let mut all = read_trust(file);
    all.insert(key(root), trusted);
    let bytes = serde_json::to_vec_pretty(&all).map_err(|e| e.to_string())?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    write_atomic(file, &bytes).map_err(|e| e.to_string())
}

pub fn trust(root: &Path) -> Option<bool> {
    trust_in(&trust_file()?, root)
}

pub fn save_trust(root: &Path, trusted: bool) -> Result<(), String> {
    let file = trust_file().ok_or("this system has no data folder")?;
    set_trust_in(&file, root, trusted)
}

fn run_history_file() -> Option<PathBuf> {
    Some(data_dir()?.join("run-anything.json"))
}

pub fn run_history_in(file: &Path) -> Vec<String> {
    std::fs::read(file)
        .ok()
        .and_then(|bytes| serde_json::from_slice(&bytes).ok())
        .unwrap_or_default()
}

pub fn save_run_history_in(file: &Path, commands: &[String]) -> Result<(), String> {
    use std::os::unix::fs::PermissionsExt;
    let bytes = serde_json::to_vec(commands).map_err(|e| e.to_string())?;
    if let Some(parent) = file.parent() {
        std::fs::create_dir_all(parent).map_err(|e| e.to_string())?;
    }
    write_atomic(file, &bytes).map_err(|e| e.to_string())?;
    std::fs::set_permissions(file, std::fs::Permissions::from_mode(0o600))
        .map_err(|e| e.to_string())
}

pub fn run_history() -> Vec<String> {
    run_history_file().map_or_else(Vec::new, |file| run_history_in(&file))
}

pub fn save_run_history(commands: &[String]) -> Result<(), String> {
    let file = run_history_file().ok_or("this system has no data folder")?;
    save_run_history_in(&file, commands)
}

pub fn scratch_dir() -> Option<PathBuf> {
    Some(data_dir()?.join("scratches"))
}

pub fn create_scratch(dir: &Path, extension: &str) -> Result<PathBuf, String> {
    std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    for n in 1..100_000 {
        let path = dir.join(format!("scratch-{n}.{extension}"));
        match std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&path)
        {
            Ok(_) => return Ok(path),
            Err(e) if e.kind() == std::io::ErrorKind::AlreadyExists => {}
            Err(e) => return Err(e.to_string()),
        }
    }
    Err("too many scratch files".to_owned())
}

pub fn list_scratches(dir: &Path) -> Vec<PathBuf> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut found: Vec<(std::time::SystemTime, PathBuf)> = entries
        .filter_map(Result::ok)
        .filter(|e| !e.file_name().to_string_lossy().starts_with('.'))
        .filter_map(|e| {
            let meta = e.metadata().ok()?;
            meta.is_file().then_some((meta.modified().ok()?, e.path()))
        })
        .collect();
    found.sort_by(|a, b| b.0.cmp(&a.0).then_with(|| a.1.cmp(&b.1)));
    found.into_iter().map(|(_, path)| path).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn scratch_files_get_their_own_names_and_list_newest_first() {
        let dir = tempfile::tempdir().unwrap();
        let scratches = dir.path().join("scratches");
        assert!(list_scratches(&scratches).is_empty());
        let first = create_scratch(&scratches, "rs").unwrap();
        let second = create_scratch(&scratches, "rs").unwrap();
        assert_ne!(first, second);
        assert_eq!(first.extension().unwrap(), "rs");
        std::fs::write(scratches.join(".scratch-1.rs.idea-tmp-1"), "").unwrap();
        let old = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::File::options()
            .write(true)
            .open(&first)
            .unwrap()
            .set_modified(old)
            .unwrap();
        assert_eq!(list_scratches(&scratches), [second, first]);
    }

    #[test]
    fn the_run_history_comes_back_and_only_its_owner_can_read_it() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("deep/history.json");
        assert!(run_history_in(&file).is_empty());
        let commands = vec!["a".to_owned(), "b c".to_owned()];
        save_run_history_in(&file, &commands).unwrap();
        assert_eq!(run_history_in(&file), commands);
        let mode = std::fs::metadata(&file).unwrap().permissions().mode();
        assert_eq!(mode & 0o777, 0o600);
    }

    #[test]
    fn an_answer_is_kept_per_folder_through_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("data/trust.json");
        let (a, b) = (dir.path().join("a"), dir.path().join("b"));
        std::fs::create_dir(&a).unwrap();
        std::fs::create_dir(&b).unwrap();
        std::os::unix::fs::symlink(&a, dir.path().join("link")).unwrap();
        assert_eq!(trust_in(&file, &a), None);
        set_trust_in(&file, &a, true).unwrap();
        set_trust_in(&file, &b, false).unwrap();
        assert_eq!(trust_in(&file, &a), Some(true));
        assert_eq!(trust_in(&file, &dir.path().join("link")), Some(true));
        assert_eq!(trust_in(&file, &b), Some(false));
        assert_eq!(trust_in(&file, &a.join("inner")), None, "not inherited");
    }

    #[test]
    fn a_damaged_file_means_asking_again() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("trust.json");
        std::fs::write(&file, "{not json").unwrap();
        assert_eq!(trust_in(&file, dir.path()), None);
    }
}
