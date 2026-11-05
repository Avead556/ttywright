use std::path::Path;

use libloading::Library;
use tree_sitter::{LANGUAGE_VERSION, Language, MIN_COMPATIBLE_LANGUAGE_VERSION};
use tree_sitter_language::LanguageFn;

pub const EXPORT: &[u8] = b"ttywright_grammar";

type GrammarFn = unsafe extern "C" fn() -> *const ();

pub fn load(path: &Path) -> Result<Language, String> {
    if !path.is_absolute() {
        return Err("not an absolute path".to_owned());
    }
    let library = unsafe { Library::new(path) }.map_err(|e| e.to_string())?;
    let found = unsafe { library.get::<GrammarFn>(EXPORT) }
        .map(|symbol| *symbol)
        .map_err(|e| e.to_string())?;
    std::mem::forget(library);
    if unsafe { found() }.is_null() {
        return Err("it has no grammar in it".to_owned());
    }
    let language = Language::new(unsafe { LanguageFn::from_raw(found) });
    let abi = language.abi_version();
    if !(MIN_COMPATIBLE_LANGUAGE_VERSION..=LANGUAGE_VERSION).contains(&abi) {
        return Err(format!(
            "made for grammar version {abi}, this program reads \
             {MIN_COMPATIBLE_LANGUAGE_VERSION} to {LANGUAGE_VERSION}"
        ));
    }
    Ok(language)
}

#[cfg(test)]
mod tests {
    use std::process::Command;

    use super::*;

    fn built(dir: &Path, name: &str, source: &str) -> std::path::PathBuf {
        let (c, out) = (dir.join(format!("{name}.c")), dir.join(name));
        std::fs::write(&c, source).unwrap_or_else(|e| panic!("{e}"));
        let status = Command::new("cc")
            .arg("-shared")
            .arg("-fPIC")
            .arg(&c)
            .arg("-o")
            .arg(&out)
            .status()
            .unwrap_or_else(|e| panic!("{e}"));
        assert!(status.success());
        out
    }

    fn claiming_version(version: u32) -> String {
        format!(
            "static const unsigned language[64] = {{ {version} }};\n\
             const void *ttywright_grammar(void) {{ return language; }}\n"
        )
    }

    fn dir() -> tempfile::TempDir {
        tempfile::tempdir().unwrap_or_else(|e| panic!("{e}"))
    }

    #[test]
    fn a_library_of_a_version_this_program_reads_loads() {
        let dir = dir();
        let good = u32::try_from(LANGUAGE_VERSION).unwrap_or_else(|e| panic!("{e}"));
        let path = built(dir.path(), "good", &claiming_version(good));
        assert_eq!(load(&path).map(|l| l.abi_version()), Ok(LANGUAGE_VERSION));
    }

    #[test]
    fn a_stale_or_future_version_says_so_and_does_not_crash() {
        let dir = dir();
        for version in [1, 9999] {
            let name = format!("v{version}");
            let path = built(dir.path(), &name, &claiming_version(version));
            let error = load(&path).err().unwrap_or_default();
            assert!(error.contains(&format!("version {version}")), "{error}");
        }
    }

    #[test]
    fn a_library_without_the_grammar_is_a_message() {
        let dir = dir();
        let other = built(
            dir.path(),
            "other",
            "int something_else(void) { return 1; }\n",
        );
        assert!(load(&other).is_err());
        let empty = built(
            dir.path(),
            "empty",
            "const void *ttywright_grammar(void) { return 0; }\n",
        );
        assert_eq!(
            load(&empty).err().as_deref(),
            Some("it has no grammar in it")
        );
    }

    #[test]
    fn garbage_a_cut_file_and_no_file_are_messages() {
        let dir = dir();
        let garbage = dir.path().join("garbage");
        std::fs::write(&garbage, b"this is no library at all").unwrap_or_else(|e| panic!("{e}"));
        assert!(load(&garbage).is_err());

        let whole = built(dir.path(), "whole", &claiming_version(14));
        let bytes = std::fs::read(&whole).unwrap_or_else(|e| panic!("{e}"));
        let cut = dir.path().join("cut");
        std::fs::write(&cut, &bytes[..bytes.len() / 8]).unwrap_or_else(|e| panic!("{e}"));
        assert!(load(&cut).is_err());

        assert!(load(&dir.path().join("nothing")).is_err());
    }

    #[test]
    fn a_bare_name_is_refused_before_anything_searches_for_it() {
        assert!(load(Path::new("rust.dylib")).is_err());
        assert!(load(Path::new("lib/rust.dylib")).is_err());
    }
}
