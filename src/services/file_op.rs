use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};

use super::Event;
use crate::core::file_op::FileOp;

pub fn run(op: FileOp) -> Event {
    let op = resolved(op);
    let result = apply(&op).map_err(|e| e.to_string());
    Event::FileOpDone { op, result }
}

fn resolved(op: FileOp) -> FileOp {
    match op {
        FileOp::CreateProject { dir, files } => FileOp::CreateProject {
            dir: at_home(&dir).unwrap_or(dir),
            files,
        },
        other => other,
    }
}

fn at_home(dir: &Path) -> Option<PathBuf> {
    let home = std::env::var_os("HOME")?;
    let mut parts = dir.components();
    parts.by_ref().find(|c| c.as_os_str() == "~")?;
    Some(Path::new(&home).join(parts.as_path()))
}

fn create_project(dir: &Path, files: &[(String, String)]) -> io::Result<()> {
    let occupied = fs::read_dir(dir).is_ok_and(|mut entries| entries.next().is_some());
    if occupied || (dir.exists() && !dir.is_dir()) {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} is not empty", dir.display()),
        ));
    }
    fs::create_dir_all(dir)?;
    for (name, contents) in files {
        let path = dir.join(name);
        make_parent(&path)?;
        fs::write(&path, contents)?;
        if name.ends_with(".sh") {
            make_executable(&path)?;
        }
    }
    let _ = std::process::Command::new("git")
        .args(["init", "-q"])
        .current_dir(dir)
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .status();
    Ok(())
}

fn make_executable(path: &Path) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    let mode = fs::metadata(path)?.permissions().mode();
    fs::set_permissions(path, fs::Permissions::from_mode(mode | 0o111))
}

fn apply(op: &FileOp) -> io::Result<()> {
    match op {
        FileOp::Create {
            path,
            contents,
            executable,
        } => {
            make_parent(path)?;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?
                .write_all(contents.as_bytes())?;
            if *executable {
                make_executable(path)?;
            }
            Ok(())
        }
        FileOp::CreateProject { dir, files } => create_project(dir, files),
        FileOp::NewFile(path) => {
            make_parent(path)?;
            fs::OpenOptions::new()
                .write(true)
                .create_new(true)
                .open(path)?;
            Ok(())
        }
        FileOp::NewFolder(path) => {
            make_parent(path)?;
            fs::create_dir(path)
        }
        FileOp::Rename { from, to } => rename(from, to),
        FileOp::Trash(path) => to_trash(path),
    }
}

fn make_parent(path: &Path) -> io::Result<()> {
    path.parent().map_or(Ok(()), fs::create_dir_all)
}

fn rename(from: &Path, to: &Path) -> io::Result<()> {
    let same_name = from.to_string_lossy().to_lowercase() == to.to_string_lossy().to_lowercase();
    if !same_name && to.symlink_metadata().is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            format!("{} already exists", to.display()),
        ));
    }
    make_parent(to)?;
    fs::rename(from, to)
}

fn to_trash(path: &Path) -> io::Result<()> {
    #[cfg_attr(not(target_os = "macos"), allow(unused_mut))]
    let mut trash = trash::TrashContext::default();
    #[cfg(target_os = "macos")]
    {
        use trash::macos::{DeleteMethod, TrashContextExtMacos};
        trash.set_delete_method(DeleteMethod::NsFileManager);
    }
    trash.delete(path).map_err(io::Error::other)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct Dir(tempfile::TempDir);

    impl Dir {
        fn new() -> Self {
            Self(tempfile::tempdir().unwrap())
        }
        fn path(&self, name: &str) -> std::path::PathBuf {
            self.0.path().join(name)
        }
    }

    #[test]
    fn a_new_file_makes_its_folders_and_never_replaces_anything() {
        let d = Dir::new();
        let path = d.path("a/b/c.txt");
        apply(&FileOp::NewFile(path.clone())).unwrap();
        assert_eq!(fs::read(&path).unwrap(), b"");
        fs::write(&path, "kept").unwrap();
        assert!(apply(&FileOp::NewFile(path.clone())).is_err());
        assert_eq!(fs::read(&path).unwrap(), b"kept");
    }

    #[test]
    fn a_new_folder_that_exists_is_an_error() {
        let d = Dir::new();
        apply(&FileOp::NewFolder(d.path("x/y"))).unwrap();
        assert!(d.path("x/y").is_dir());
        assert!(apply(&FileOp::NewFolder(d.path("x/y"))).is_err());
    }

    #[test]
    fn a_rename_moves_and_refuses_to_replace() {
        let d = Dir::new();
        let (from, to) = (d.path("a"), d.path("deep/b"));
        fs::write(&from, "one").unwrap();
        let op = FileOp::Rename {
            from: from.clone(),
            to: to.clone(),
        };
        apply(&op).unwrap();
        assert_eq!(fs::read(&to).unwrap(), b"one");
        assert!(!from.exists());
        fs::write(&from, "two").unwrap();
        let err = apply(&op).unwrap_err();
        assert!(err.to_string().contains("already exists"), "{err}");
        assert_eq!(fs::read(&to).unwrap(), b"one");
        assert_eq!(fs::read(&from).unwrap(), b"two");
    }

    #[test]
    fn a_rename_of_only_the_case_is_allowed() {
        let d = Dir::new();
        fs::write(d.path("readme"), "x").unwrap();
        let op = FileOp::Rename {
            from: d.path("readme"),
            to: d.path("README"),
        };
        apply(&op).unwrap();
        assert_eq!(fs::read(d.path("README")).unwrap(), b"x");
    }

    #[test]
    fn a_missing_file_reports_instead_of_pretending() {
        let d = Dir::new();
        let op = FileOp::Rename {
            from: d.path("none"),
            to: d.path("b"),
        };
        assert!(matches!(run(op), Event::FileOpDone { result: Err(_), .. }));
    }

    #[test]
    fn a_template_file_never_replaces_anything_and_scripts_can_run() {
        use std::os::unix::fs::PermissionsExt;
        let d = Dir::new();
        let path = d.path("deep/run.sh");
        let op = FileOp::Create {
            path: path.clone(),
            contents: "#!/bin/sh\n".into(),
            executable: true,
        };
        apply(&op).unwrap();
        assert_eq!(fs::read_to_string(&path).unwrap(), "#!/bin/sh\n");
        assert_ne!(fs::metadata(&path).unwrap().permissions().mode() & 0o100, 0);
        fs::write(&path, "mine").unwrap();
        assert!(apply(&op).is_err());
        assert_eq!(fs::read_to_string(&path).unwrap(), "mine");
    }

    #[test]
    fn a_project_makes_its_files_and_repository_but_never_in_a_folder_in_use() {
        let d = Dir::new();
        let dir = d.path("new/app");
        let files = vec![("src/a.txt".to_owned(), "x".to_owned())];
        let op = FileOp::CreateProject {
            dir: dir.clone(),
            files,
        };
        apply(&op).unwrap();
        assert_eq!(fs::read_to_string(dir.join("src/a.txt")).unwrap(), "x");
        assert!(dir.join(".git").exists());
        assert!(apply(&op).unwrap_err().to_string().contains("not empty"));
        let empty = d.path("empty");
        fs::create_dir(&empty).unwrap();
        apply(&FileOp::CreateProject {
            dir: empty.clone(),
            files: Vec::new(),
        })
        .unwrap();
    }

    #[test]
    fn a_tilde_means_home_wherever_the_core_joined_it() {
        let home = std::env::var("HOME").unwrap();
        assert_eq!(
            at_home(Path::new("/work/~/projects/x")),
            Some(Path::new(&home).join("projects/x"))
        );
        assert_eq!(at_home(Path::new("/work/x")), None);
    }
}
