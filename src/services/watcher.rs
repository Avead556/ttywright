use std::path::{Path, PathBuf};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use ignore::gitignore::Gitignore;
use notify::{RecommendedWatcher, RecursiveMode, Watcher};

use crate::core::Event;

const QUIET: Duration = Duration::from_millis(150);

pub fn watch(
    root: &Path,
    emit: impl Fn(Event) + Send + 'static,
) -> notify::Result<RecommendedWatcher> {
    let (tx, rx) = mpsc::channel();
    let mut watcher = notify::recommended_watcher(tx)?;
    watcher.watch(root, RecursiveMode::Recursive)?;
    let (ignored, _) = Gitignore::new(root.join(".gitignore"));
    thread::spawn(move || {
        while let Ok(first) = rx.recv() {
            let mut batch = vec![first];
            while let Ok(next) = rx.recv_timeout(QUIET) {
                batch.push(next);
            }
            let mut paths: Vec<PathBuf> = batch
                .into_iter()
                .filter_map(Result::ok)
                .filter(|e: &notify::Event| !e.kind.is_access())
                .flat_map(|e| e.paths)
                .filter(|p| is_relevant(p, &ignored))
                .collect();
            paths.sort();
            paths.dedup();
            if !paths.is_empty() {
                emit(Event::PathsChanged { paths });
            }
        }
    });
    Ok(watcher)
}

fn is_relevant(path: &Path, ignored: &Gitignore) -> bool {
    let is_own_temp = path
        .file_name()
        .is_some_and(|n| n.to_string_lossy().contains(".idea-tmp-"));
    !is_own_temp
        && !super::session::is_own_file(path)
        && !super::session::is_in_cache_folder(path)
        && !path.components().any(|c| c.as_os_str() == ".git")
        && !ignored
            .matched_path_or_any_parents(path, path.is_dir())
            .is_ignore()
}

#[cfg(test)]
mod tests {
    use std::sync::mpsc::channel;
    use std::time::Instant;

    use super::*;

    #[test]
    fn reports_changes_but_not_ignored_git_or_temp_files() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path().canonicalize().unwrap();
        std::fs::write(root.join(".gitignore"), "/target\n").unwrap();
        std::fs::create_dir(root.join("target")).unwrap();
        std::fs::create_dir(root.join(".git")).unwrap();

        let (tx, rx) = channel();
        let _watcher = watch(&root, move |e| {
            let _ = tx.send(e);
        })
        .unwrap();
        thread::sleep(Duration::from_millis(300));
        std::fs::write(root.join("target/out"), "x").unwrap();
        std::fs::write(root.join(".git/index"), "x").unwrap();
        std::fs::write(root.join(".a.rs.idea-tmp-1-1"), "x").unwrap();
        std::fs::write(root.join("a.rs"), "x").unwrap();

        let deadline = Instant::now() + Duration::from_secs(5);
        let mut seen = Vec::new();
        while Instant::now() < deadline && !seen.contains(&root.join("a.rs")) {
            if let Ok(Event::PathsChanged { paths }) = rx.recv_timeout(Duration::from_millis(100)) {
                seen.extend(paths);
            }
        }
        assert!(seen.contains(&root.join("a.rs")), "saw {seen:?}");
        assert!(seen.iter().all(|p| !p.starts_with(root.join("target"))));
        assert!(seen.iter().all(|p| !p.starts_with(root.join(".git"))));
        assert!(!seen.contains(&root.join(".a.rs.idea-tmp-1-1")));
    }
}
