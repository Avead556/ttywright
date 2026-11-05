use std::fs::{self, File};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::process;
use std::sync::atomic::{AtomicU64, Ordering};

use ignore::WalkBuilder;

use crate::core::{DirEntry, DiskState, DocumentId, Event, WriteGuard, WriteOrigin};

pub fn write(
    doc: DocumentId,
    version: u64,
    path: &Path,
    bytes: &[u8],
    guard: WriteGuard,
    origin: WriteOrigin,
) -> Event {
    let failed = |e: io::Error| Event::WriteFailed {
        doc,
        version,
        message: e.to_string(),
        origin,
    };
    match write_guarded(path, bytes, guard) {
        Ok(Written::Done(disk)) => Event::FileWritten {
            doc,
            version,
            disk,
            origin,
        },
        Ok(Written::Refused(disk)) => Event::WriteConflict { doc, disk, origin },
        Err(e) => failed(e),
    }
}

#[derive(Debug, PartialEq, Eq)]
pub enum Written {
    Done(Option<DiskState>),
    Refused(Option<DiskState>),
}

pub fn write_guarded(path: &Path, bytes: &[u8], guard: WriteGuard) -> io::Result<Written> {
    if let WriteGuard::Unchanged(expected) = guard {
        let disk = disk_state(path)?;
        if disk != expected {
            return Ok(Written::Refused(disk));
        }
    }
    write_atomic(path, bytes)?;
    Ok(Written::Done(disk_state(path)?))
}

pub fn read(path: PathBuf, at: Option<(usize, usize)>) -> Event {
    let (disk, result) = read_with_state(&path);
    Event::FileRead {
        path,
        at,
        disk,
        result,
    }
}

fn picture_head(path: &Path) -> Option<Vec<u8>> {
    use crate::core::image_art::{HEAD_BYTES, has_image_extension, is_image};
    use std::io::Read;
    if !has_image_extension(path) {
        return None;
    }
    let mut head = Vec::new();
    fs::File::open(path)
        .ok()?
        .take(HEAD_BYTES)
        .read_to_end(&mut head)
        .ok()?;
    is_image(path, &head).then_some(head)
}

pub fn check(doc: DocumentId, path: &Path, known: Option<DiskState>) -> Option<Event> {
    if disk_state(path).is_ok_and(|disk| disk == known) {
        return None;
    }
    let (disk, result) = read_with_state(path);
    Some(Event::FileChanged { doc, disk, result })
}

pub fn reload(doc: DocumentId, path: &Path) -> Event {
    let (disk, result) = read_with_state(path);
    Event::FileReloaded { doc, disk, result }
}

fn read_with_state(path: &Path) -> (Option<DiskState>, Result<Option<Vec<u8>>, String>) {
    let disk = disk_state(path).unwrap_or(None);
    if let Some(head) = picture_head(path) {
        return (disk, Ok(Some(head)));
    }
    (disk, read_if_exists(path).map_err(|e| e.to_string()))
}

pub fn disk_state(path: &Path) -> io::Result<Option<DiskState>> {
    match fs::metadata(path) {
        Ok(meta) => Ok(Some(DiskState {
            modified: meta.modified().ok(),
            len: meta.len(),
        })),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn list_dir(path: PathBuf) -> Event {
    let result = walker(&path)
        .max_depth(Some(1))
        .build()
        .filter_map(Result::ok)
        .filter(|e| e.depth() == 1)
        .filter(|e| e.file_name() != ".ttywright")
        .map(|e| DirEntry {
            name: e.file_name().to_string_lossy().into_owned(),
            is_dir: e.file_type().is_some_and(|t| t.is_dir()),
            path: e.into_path(),
        })
        .collect();
    Event::DirListed {
        path,
        result: Ok(result),
    }
}

pub fn walker(root: &Path) -> WalkBuilder {
    let mut builder = WalkBuilder::new(root);
    builder
        .hidden(false)
        .require_git(false)
        .filter_entry(|e| e.file_name() != ".git" && !super::session::is_cache_folder(e.path()));
    builder
}

pub fn read_if_exists(path: &Path) -> io::Result<Option<Vec<u8>>> {
    match fs::read(path) {
        Ok(bytes) => Ok(Some(bytes)),
        Err(e) if e.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(e) => Err(e),
    }
}

pub fn write_atomic(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let target = resolve_symlink(path)?;
    let dir = match target.parent() {
        Some(d) if !d.as_os_str().is_empty() => d.to_path_buf(),
        _ => PathBuf::from("."),
    };
    let tmp = dir.join(temp_name(&target));

    let result = write_temp(&tmp, &target, bytes).and_then(|()| fs::rename(&tmp, &target));
    if result.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    result?;
    sync_dir(&dir)
}

fn write_temp(tmp: &Path, target: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create_new(tmp)?;
    file.write_all(bytes)?;
    file.sync_all()?;
    if let Ok(meta) = fs::metadata(target) {
        fs::set_permissions(tmp, meta.permissions())?;
    }
    Ok(())
}

fn resolve_symlink(path: &Path) -> io::Result<PathBuf> {
    match fs::symlink_metadata(path) {
        Ok(meta) if meta.file_type().is_symlink() => fs::canonicalize(path),
        _ => Ok(path.to_path_buf()),
    }
}

fn temp_name(target: &Path) -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);
    let name = target
        .file_name()
        .map(|n| n.to_string_lossy())
        .unwrap_or_default();
    let n = COUNTER.fetch_add(1, Ordering::Relaxed);
    format!(".{name}.idea-tmp-{}-{n}", process::id())
}

#[cfg(unix)]
fn sync_dir(dir: &Path) -> io::Result<()> {
    File::open(dir)?.sync_all()
}

#[cfg(not(unix))]
fn sync_dir(_dir: &Path) -> io::Result<()> {
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn entries(dir: &Path) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    }

    #[test]
    fn creates_and_replaces_without_leftovers() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        write_atomic(&path, b"one").unwrap();
        write_atomic(&path, b"two").unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"two");
        assert_eq!(entries(dir.path()), ["a.txt"]);
    }

    #[cfg(unix)]
    #[test]
    fn keeps_file_permissions() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("run.sh");
        fs::write(&path, "old").unwrap();
        fs::set_permissions(&path, fs::Permissions::from_mode(0o750)).unwrap();
        write_atomic(&path, b"new").unwrap();
        assert_eq!(
            fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o750
        );
    }

    #[cfg(unix)]
    #[test]
    fn writes_through_symlinks() {
        let dir = tempfile::tempdir().unwrap();
        let real = dir.path().join("real.txt");
        let link = dir.path().join("link.txt");
        fs::write(&real, "old").unwrap();
        std::os::unix::fs::symlink(&real, &link).unwrap();
        write_atomic(&link, b"new").unwrap();
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read(&real).unwrap(), b"new");
    }

    #[test]
    fn failed_write_reports_and_leaves_no_temp_file() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("missing").join("a.txt");
        let event = write(
            DocumentId(1),
            3,
            &path,
            b"x",
            WriteGuard::Overwrite,
            WriteOrigin::User,
        );
        assert!(matches!(event, Event::WriteFailed { version: 3, .. }));
        assert!(entries(dir.path()).is_empty());
    }

    #[test]
    fn list_dir_hides_git_and_ignored_entries() {
        let dir = tempfile::tempdir().unwrap();
        for d in [".git", "target", "src", ".ttywright"] {
            fs::create_dir(dir.path().join(d)).unwrap();
        }
        fs::write(dir.path().join(".gitignore"), "target/\n").unwrap();
        fs::write(dir.path().join("src").join("lib.rs"), "").unwrap();
        let Event::DirListed {
            result: Ok(entries),
            ..
        } = list_dir(dir.path().to_path_buf())
        else {
            panic!("listing failed");
        };
        let mut names: Vec<_> = entries.iter().map(|e| e.name.as_str()).collect();
        names.sort_unstable();
        assert_eq!(names, [".gitignore", "src"]);
    }

    fn guarded(path: &Path, bytes: &[u8], guard: WriteGuard) -> Event {
        write(DocumentId(1), 1, path, bytes, guard, WriteOrigin::Auto)
    }

    #[test]
    fn guarded_write_goes_ahead_when_the_file_is_as_expected() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        let event = guarded(&path, b"new", WriteGuard::Unchanged(None));
        let Event::FileWritten { disk, .. } = event else {
            panic!("{event:?}");
        };
        assert_eq!(disk, disk_state(&path).unwrap());
        assert!(matches!(
            guarded(&path, b"newer", WriteGuard::Unchanged(disk)),
            Event::FileWritten { .. }
        ));
        assert_eq!(fs::read(&path).unwrap(), b"newer");
    }

    #[test]
    fn guarded_write_refuses_a_file_changed_or_deleted_by_someone_else() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "old").unwrap();
        let seen = disk_state(&path).unwrap();
        fs::write(&path, "theirs!").unwrap();

        let event = guarded(&path, b"mine", WriteGuard::Unchanged(seen));
        assert!(matches!(event, Event::WriteConflict { disk: Some(_), .. }));
        assert_eq!(fs::read(&path).unwrap(), b"theirs!");

        fs::remove_file(&path).unwrap();
        let event = guarded(&path, b"mine", WriteGuard::Unchanged(seen));
        assert!(matches!(event, Event::WriteConflict { disk: None, .. }));
        assert!(!path.exists());
    }

    #[test]
    fn overwrite_ignores_what_is_on_disk() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "theirs").unwrap();
        assert!(matches!(
            guarded(&path, b"mine", WriteGuard::Overwrite),
            Event::FileWritten { .. }
        ));
        assert_eq!(fs::read(&path).unwrap(), b"mine");
    }

    #[test]
    fn check_stays_silent_until_the_file_differs() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.txt");
        fs::write(&path, "one").unwrap();
        let known = disk_state(&path).unwrap();
        assert!(check(DocumentId(1), &path, known).is_none());

        fs::write(&path, "three").unwrap();
        let Some(Event::FileChanged { result, disk, .. }) = check(DocumentId(1), &path, known)
        else {
            panic!("expected a change");
        };
        assert_eq!(result, Ok(Some(b"three".to_vec())));
        assert_eq!(disk, disk_state(&path).unwrap());

        fs::remove_file(&path).unwrap();
        let Some(Event::FileChanged { result, .. }) = check(DocumentId(1), &path, known) else {
            panic!("expected a change");
        };
        assert_eq!(result, Ok(None));
    }

    #[test]
    fn read_if_exists_distinguishes_missing_files() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(read_if_exists(&dir.path().join("nope")).unwrap(), None);
    }
}
