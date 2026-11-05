use std::path::{Path, PathBuf};

use crate::core::Event;
use crate::core::coverage::{CoverageEvent, CoverageReport, MAX_REPORT_BYTES, parse_report};

const CONVENTIONAL: [&str; 9] = [
    "lcov.info",
    "coverage/lcov.info",
    "target/llvm-cov/lcov.info",
    "target/lcov.info",
    "coverage.lcov",
    "coverage.xml",
    "cobertura.xml",
    "coverage/cobertura.xml",
    "coverage/cobertura-coverage.xml",
];

fn read(root: &Path, file: &Path) -> Result<CoverageReport, String> {
    let name = file.display();
    let meta = std::fs::metadata(file).map_err(|e| format!("{name}: {e}"))?;
    if !meta.is_file() {
        return Err(format!("{name} is not a file"));
    }
    if meta.len() > MAX_REPORT_BYTES {
        return Err(format!("{name} is too large for a coverage report"));
    }
    let bytes = std::fs::read(file).map_err(|e| format!("{name}: {e}"))?;
    let mut report =
        parse_report(&String::from_utf8_lossy(&bytes), root).map_err(|e| format!("{name}: {e}"))?;
    report.source = file.to_path_buf();
    report.made = meta.modified().ok();
    Ok(report)
}

pub fn read_event(root: &Path, file: Option<&Path>) -> Event {
    let result = match file {
        Some(file) => read(root, &root.join(file)),
        None => CONVENTIONAL
            .iter()
            .map(|name| root.join(name))
            .find(|path: &PathBuf| path.is_file())
            .map_or_else(
                || {
                    Err(format!(
                        "No coverage report found; looked for {}",
                        CONVENTIONAL.join(", ")
                    ))
                },
                |path| read(root, &path),
            ),
    };
    Event::Coverage(CoverageEvent::Read { result })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn answer(root: &Path, file: Option<&Path>) -> Result<CoverageReport, String> {
        let Event::Coverage(CoverageEvent::Read { result }) = read_event(root, file) else {
            panic!("a coverage answer");
        };
        result
    }

    #[test]
    fn a_report_is_found_where_tools_write_it_or_where_it_is_named() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        assert!(answer(root, None).unwrap_err().contains("lcov.info"));
        std::fs::create_dir(root.join("coverage")).unwrap();
        std::fs::write(
            root.join("coverage/lcov.info"),
            "SF:a.rs\nDA:1,1\nend_of_record\n",
        )
        .unwrap();
        let found = answer(root, None).unwrap();
        assert!(found.files.contains_key(&root.join("a.rs")));
        assert_eq!(found.source, root.join("coverage/lcov.info"));
        assert!(found.made.is_some());
        std::fs::write(root.join("other"), "SF:b.rs\nDA:1,1\nend_of_record\n").unwrap();
        let named = answer(root, Some(Path::new("other"))).unwrap();
        assert!(named.files.contains_key(&root.join("b.rs")));
    }

    #[test]
    fn a_huge_a_binary_or_a_missing_report_is_refused_not_read() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let big = std::fs::File::create(root.join("big")).unwrap();
        big.set_len(MAX_REPORT_BYTES + 1).unwrap();
        assert!(
            answer(root, Some(Path::new("big")))
                .unwrap_err()
                .contains("too large")
        );
        std::fs::write(root.join("bin"), [0_u8, 159, 146, 150]).unwrap();
        assert!(answer(root, Some(Path::new("bin"))).is_err());
        assert!(answer(root, Some(Path::new("none"))).is_err());
        assert!(answer(root, Some(root)).unwrap_err().contains("not a file"));
    }
}
