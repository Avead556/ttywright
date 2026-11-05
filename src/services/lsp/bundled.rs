use std::io;
use std::path::{Path, PathBuf};

const PREFIX: &str = "rust-analyzer-";

#[cfg(bundled_lsp)]
static COMPRESSED: &[u8] = include_bytes!(concat!(env!("OUT_DIR"), "/rust-analyzer.zst"));

pub fn embedded() -> Option<&'static [u8]> {
    #[cfg(bundled_lsp)]
    return Some(COMPRESSED);
    #[cfg(not(bundled_lsp))]
    None
}

pub fn extract(compressed: &[u8], dir: &Path) -> io::Result<PathBuf> {
    let hash = blake3::hash(compressed).to_hex();
    let name = format!("{PREFIX}{}", &hash.as_str()[..16]);
    let path = dir.join(&name);
    if !path.exists() {
        std::fs::create_dir_all(dir)?;
        let temporary = dir.join(format!(".{name}.{}", std::process::id()));
        std::fs::write(&temporary, zstd::decode_all(compressed)?)?;
        make_executable(&temporary)?;
        std::fs::rename(&temporary, &path)?;
    }
    remove_other_versions(dir, &name);
    Ok(path)
}

#[cfg(unix)]
fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    std::fs::set_permissions(path, std::fs::Permissions::from_mode(0o755))
}

#[cfg(not(unix))]
fn make_executable(_path: &Path) -> io::Result<()> {
    Ok(())
}

fn remove_other_versions(dir: &Path, keep: &str) {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let name = name.to_string_lossy();
        if name.starts_with(PREFIX) && name != keep {
            let _ = std::fs::remove_file(entry.path());
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn packed(text: &str) -> Vec<u8> {
        zstd::encode_all(text.as_bytes(), 1).expect("compresses")
    }

    #[test]
    fn unpacks_once_as_an_executable_file() {
        let dir = tempfile::tempdir().expect("temp dir");
        let bytes = packed("server");
        let path = extract(&bytes, dir.path()).expect("extracts");
        assert_eq!(std::fs::read_to_string(&path).expect("reads"), "server");
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&path).expect("stat").permissions().mode();
            assert_eq!(mode & 0o111, 0o111);
        }
        std::fs::write(&path, "in use").expect("writes");
        assert_eq!(extract(&bytes, dir.path()).expect("extracts"), path);
        assert_eq!(std::fs::read_to_string(&path).expect("reads"), "in use");
    }

    #[test]
    fn a_new_version_replaces_the_old_and_leaves_other_files() {
        let dir = tempfile::tempdir().expect("temp dir");
        let old = extract(&packed("old"), dir.path()).expect("extracts");
        let other = dir.path().join("notes");
        std::fs::write(&other, "").expect("writes");
        let new = extract(&packed("new"), dir.path()).expect("extracts");
        assert_ne!(old, new);
        assert!(!old.exists());
        assert!(new.exists());
        assert!(other.exists());
    }

    #[test]
    fn damaged_data_leaves_nothing_behind() {
        let dir = tempfile::tempdir().expect("temp dir");
        assert!(extract(b"not zstd", dir.path()).is_err());
        assert_eq!(std::fs::read_dir(dir.path()).expect("lists").count(), 0);
    }
}
