use std::path::{Path, PathBuf};
use std::sync::OnceLock;

fn lib_of(exe: &Path) -> Option<PathBuf> {
    Some(exe.canonicalize().ok()?.parent()?.join("lib"))
}

fn lib() -> Option<&'static Path> {
    static LIB: OnceLock<Option<PathBuf>> = OnceLock::new();
    LIB.get_or_init(|| lib_of(&std::env::current_exe().ok()?))
        .as_deref()
}

fn grammar_file(name: &str) -> String {
    format!("{name}.{}", std::env::consts::DLL_EXTENSION)
}

pub fn grammar(name: &str) -> Option<PathBuf> {
    Some(lib()?.join("grammars").join(grammar_file(name)))
}

pub fn plugin_host() -> Option<PathBuf> {
    Some(lib()?.join(format!("ttywright-plugins{}", std::env::consts::EXE_SUFFIX)))
}

pub fn rust_analyzer() -> Option<PathBuf> {
    let path = lib()?.join(format!("rust-analyzer{}", std::env::consts::EXE_SUFFIX));
    path.is_file().then_some(path)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_folder_is_found_from_the_real_program_not_a_link_to_it() {
        let dir = tempfile::tempdir().expect("temp dir");
        let root = dir.path().canonicalize().expect("real path");
        let (real, elsewhere) = (root.join("app"), root.join("bin"));
        std::fs::create_dir_all(&real).expect("makes");
        std::fs::create_dir_all(&elsewhere).expect("makes");
        let exe = real.join("ttyw");
        std::fs::write(&exe, "").expect("writes");
        assert_eq!(lib_of(&exe), Some(real.join("lib")));
        #[cfg(unix)]
        {
            let link = elsewhere.join("ttyw");
            std::os::unix::fs::symlink(&exe, &link).expect("links");
            assert_eq!(lib_of(&link), Some(real.join("lib")));
        }
    }

    #[test]
    fn a_program_that_is_gone_has_no_folder() {
        assert_eq!(lib_of(Path::new("/no/such/program")), None);
    }

    #[test]
    fn the_paths_handed_to_the_loader_are_absolute() {
        for path in [grammar("rust"), plugin_host()] {
            assert!(path.expect("this test program exists").is_absolute());
        }
    }
}
