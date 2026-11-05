use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};
use std::time::SystemTime;

use serde::{Deserialize, Serialize};

pub const MAX_REPORT_BYTES: u64 = 32 * 1024 * 1024;
const MAX_LINES: usize = 4_000_000;

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct CoverageReport {
    pub files: BTreeMap<PathBuf, BTreeMap<u32, u64>>,
    pub dropped: usize,
    pub truncated: bool,
    pub source: PathBuf,
    pub made: Option<SystemTime>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CoverageSummary {
    pub covered: usize,
    pub total: usize,
}

impl CoverageSummary {
    pub fn percent(self) -> usize {
        (self.covered * 100).checked_div(self.total).unwrap_or(0)
    }
}

impl CoverageReport {
    pub fn summary(&self) -> CoverageSummary {
        let mut all = CoverageSummary {
            covered: 0,
            total: 0,
        };
        for lines in self.files.values() {
            all.total += lines.len();
            all.covered += lines.values().filter(|hits| **hits > 0).count();
        }
        all
    }

    pub fn file_summary(&self, path: &Path) -> Option<CoverageSummary> {
        let lines = self.files.get(path)?;
        Some(CoverageSummary {
            covered: lines.values().filter(|hits| **hits > 0).count(),
            total: lines.len(),
        })
    }

    fn line_count(&self) -> usize {
        self.files.values().map(BTreeMap::len).sum()
    }

    fn add(&mut self, file: Option<&PathBuf>, line: u32, hits: u64) {
        let Some(file) = file else {
            return;
        };
        if line == 0 {
            return;
        }
        if self.line_count_is_full() {
            self.truncated = true;
            return;
        }
        let entry = self
            .files
            .entry(file.clone())
            .or_default()
            .entry(line)
            .or_insert(0);
        *entry = entry.saturating_add(hits);
    }

    fn line_count_is_full(&self) -> bool {
        self.files.len() > 200_000 || self.line_count() >= MAX_LINES
    }
}

fn in_project(root: &Path, base: &Path, name: &str) -> Option<PathBuf> {
    let name = Path::new(name);
    let full = if name.is_absolute() {
        name.to_path_buf()
    } else {
        base.join(name)
    };
    let plain = full
        .components()
        .all(|c| !matches!(c, Component::ParentDir));
    let full: PathBuf = full
        .components()
        .filter(|c| !matches!(c, Component::CurDir))
        .collect();
    (plain && full.starts_with(root)).then_some(full)
}

fn parse_lcov(text: &str, root: &Path) -> CoverageReport {
    let mut report = CoverageReport::default();
    let mut file = None;
    for line in text.lines() {
        if let Some(name) = line.strip_prefix("SF:") {
            file = in_project(root, root, name.trim());
            report.dropped += usize::from(file.is_none());
        } else if let Some(data) = line.strip_prefix("DA:") {
            let mut parts = data.split(',');
            let number = parts.next().and_then(|n| n.trim().parse().ok());
            let hits = parts.next().and_then(|n| n.trim().parse().ok());
            if let (Some(number), Some(hits)) = (number, hits) {
                report.add(file.as_ref(), number, hits);
                if report.truncated {
                    break;
                }
            }
        } else if line.trim() == "end_of_record" {
            file = None;
        }
    }
    report
}

fn unescape(value: &str) -> String {
    value
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&quot;", "\"")
        .replace("&apos;", "'")
        .replace("&amp;", "&")
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    let mut rest = tag;
    loop {
        let at = rest.find(name)?;
        let before_ok = rest[..at].ends_with(char::is_whitespace);
        let after = rest[at + name.len()..].trim_start();
        if before_ok && let Some(value) = after.strip_prefix('=') {
            let value = value.trim_start();
            let quote = value.chars().next().filter(|c| *c == '"' || *c == '\'')?;
            let value = &value[1..];
            let end = value.find(quote)?;
            return Some(unescape(&value[..end]));
        }
        rest = &rest[at + name.len()..];
    }
}

fn parse_cobertura(text: &str, root: &Path) -> CoverageReport {
    let mut report = CoverageReport::default();
    let mut sources: Vec<PathBuf> = Vec::new();
    let mut file: Option<PathBuf> = None;
    let mut rest = text;
    while let Some(open) = rest.find('<') {
        rest = &rest[open..];
        if let Some(comment) = rest.strip_prefix("<!--") {
            rest = comment.find("-->").map_or("", |end| &comment[end + 3..]);
            continue;
        }
        let Some(close) = rest.find('>') else {
            break;
        };
        let tag = &rest[1..close];
        rest = &rest[close + 1..];
        let name = tag.split_whitespace().next().unwrap_or("");
        match name {
            "source" => {
                if let Some(end) = rest.find('<') {
                    sources.push(PathBuf::from(unescape(rest[..end].trim())));
                }
            }
            "class" => {
                file = attribute(tag, "filename").and_then(|name| {
                    let found = cobertura_file(root, &sources, &name);
                    report.dropped += usize::from(found.is_none());
                    found
                });
            }
            "line" => {
                let number = attribute(tag, "number").and_then(|n| n.parse().ok());
                let hits = attribute(tag, "hits").and_then(|n| n.parse().ok());
                if let (Some(number), Some(hits)) = (number, hits) {
                    report.add(file.as_ref(), number, hits);
                    if report.truncated {
                        break;
                    }
                }
            }
            _ => {}
        }
    }
    report
}

fn cobertura_file(root: &Path, sources: &[PathBuf], name: &str) -> Option<PathBuf> {
    sources
        .iter()
        .map(|source| root.join(source))
        .find_map(|base| in_project(root, &base, name))
        .or_else(|| in_project(root, root, name))
}

pub fn parse_report(text: &str, root: &Path) -> Result<CoverageReport, String> {
    let body = text.trim_start_matches('\u{feff}').trim_start();
    let report = if body.starts_with('<') {
        if !body.contains("<coverage") {
            return Err("This XML is not a Cobertura report".to_owned());
        }
        parse_cobertura(body, root)
    } else if body.lines().any(|l| l.starts_with("SF:")) {
        parse_lcov(body, root)
    } else {
        return Err("Not an lcov or Cobertura report".to_owned());
    };
    Ok(report)
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoverageEffect {
    Read {
        root: PathBuf,
        file: Option<PathBuf>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum CoverageEvent {
    Read {
        result: Result<CoverageReport, String>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CoverageState {
    Current(CoverageSummary),
    Stale,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct CoverageView {
    report: Option<CoverageReport>,
    shown: bool,
}

impl CoverageView {
    pub fn toggle(&mut self) -> bool {
        if self.report.is_none() {
            return false;
        }
        self.shown = !self.shown;
        true
    }

    pub fn handle(&mut self, event: CoverageEvent) -> Result<String, String> {
        let CoverageEvent::Read { result } = event;
        let report = match result? {
            report if report.files.is_empty() => {
                return Err(format!(
                    "The report has no lines for this project; {} files outside it",
                    report.dropped
                ));
            }
            report => report,
        };
        let all = report.summary();
        let mut text = format!(
            "Coverage {}%: {} of {} lines, {} files",
            all.percent(),
            all.covered,
            all.total,
            report.files.len()
        );
        if report.dropped > 0 {
            text.push_str(&format!(
                "; {} outside the project left out",
                report.dropped
            ));
        }
        if report.truncated {
            text.push_str("; too large, the rest left out");
        }
        self.report = Some(report);
        self.shown = true;
        Ok(text)
    }

    fn shown(&self) -> Option<&CoverageReport> {
        self.report.as_ref().filter(|_| self.shown)
    }

    fn is_stale(report: &CoverageReport, d: &super::Document) -> bool {
        let newer = d
            .disk()
            .and_then(|disk| disk.modified)
            .zip(report.made)
            .is_some_and(|(file, made)| file > made);
        d.is_dirty() || newer
    }

    pub fn state(&self, d: &super::Document) -> Option<CoverageState> {
        let report = self.shown()?;
        let summary = report.file_summary(d.path())?;
        Some(if Self::is_stale(report, d) {
            CoverageState::Stale
        } else {
            CoverageState::Current(summary)
        })
    }

    pub fn mark(&self, d: &super::Document, line: usize) -> Option<bool> {
        let report = self.shown()?;
        if Self::is_stale(report, d) {
            return None;
        }
        let number = u32::try_from(line + 1).ok()?;
        report
            .files
            .get(d.path())?
            .get(&number)
            .map(|hits| *hits > 0)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/ROOT")
    }

    #[test]
    fn a_real_report_of_llvm_cov_is_read_line_by_line() {
        let text = include_str!("../../tests/fixtures/coverage/clang-llvm-cov.lcov");
        let report = parse_report(text, &root()).unwrap();
        let lines = &report.files[Path::new("/ROOT/a.c")];
        assert_eq!(lines.len(), 9);
        assert_eq!(lines[&3], 1);
        assert_eq!(lines[&5], 0, "the branch the test never took");
        let summary = report.file_summary(Path::new("/ROOT/a.c")).unwrap();
        assert_eq!(
            (summary.covered, summary.total, summary.percent()),
            (8, 9, 88)
        );
    }

    #[test]
    fn relative_lcov_names_are_in_the_project_and_repeats_add_up() {
        let text = "SF:./src/a.rs\nDA:1,2\nDA:1,3\nend_of_record\nDA:9,9\n";
        let report = parse_report(text, &root()).unwrap();
        assert_eq!(report.files[Path::new("/ROOT/src/a.rs")][&1], 5);
        assert_eq!(
            report.line_count(),
            1,
            "data outside a record belongs to nothing"
        );
    }

    #[test]
    fn places_outside_the_project_are_dropped_and_counted() {
        for name in [
            "/etc/passwd",
            "../outside.rs",
            "a/../../outside.rs",
            "/ROOTED/a.rs",
        ] {
            let text = format!("SF:{name}\nDA:1,1\nend_of_record\n");
            let report = parse_report(&text, &root()).unwrap();
            assert!(report.files.is_empty(), "{name}");
            assert_eq!(report.dropped, 1, "{name}");
        }
    }

    #[test]
    fn nonsense_numbers_do_not_count_or_panic() {
        let text = "SF:a.rs\nDA:x,1\nDA:0,1\nDA:-1,1\nDA:1\nDA:99999999999,1\nDA:2,99999999999999999999\nDA:3,1,checksum\n";
        let report = parse_report(text, &root()).unwrap();
        let lines = &report.files[Path::new("/ROOT/a.rs")];
        assert_eq!(lines.keys().copied().collect::<Vec<_>>(), [3]);
    }

    const COBERTURA: &str = r#"<?xml version="1.0"?>
<!DOCTYPE coverage SYSTEM "http://evil.example/x.dtd">
<!-- <class filename="commented.py"> -->
<coverage line-rate="0.5">
  <sources><source>/ROOT/src</source></sources>
  <packages><package name="p"><classes>
    <class name="a" filename="a&amp;b.py" line-rate="0.5">
      <methods><method name="f"><lines><line number="1" hits="1"/></lines></method></methods>
      <lines>
        <line number="1" hits="1"/>
        <line hits='0' number='2' branch="false"/>
        <line number="bad" hits="1"/>
      </lines>
    </class>
    <class name="o" filename="/etc/passwd"><lines><line number="1" hits="1"/></lines></class>
  </classes></package></packages>
</coverage>"#;

    #[test]
    fn cobertura_is_read_without_its_doctype_or_comments_mattering() {
        let report = parse_report(COBERTURA, &root()).unwrap();
        let lines = &report.files[Path::new("/ROOT/src/a&b.py")];
        assert_eq!(lines[&2], 0);
        assert!(lines[&1] > 0);
        assert_eq!(report.files.len(), 1);
        assert_eq!(report.dropped, 1);
    }

    #[test]
    fn what_is_neither_format_is_said_to_be_so() {
        for text in ["", "hello\n", "<html></html>", "\u{feff}plain"] {
            assert!(parse_report(text, &root()).is_err(), "{text:?}");
        }
    }

    #[test]
    fn broken_xml_ends_quietly() {
        for text in [
            "<coverage",
            "<coverage><class filename=\"a.rs",
            "<coverage><!-- never closed",
            "<coverage><line number=\"1\" hits=\"1\"",
            "<coverage><class filename='a.rs'><line number=>",
        ] {
            assert!(parse_report(text, &root()).is_ok(), "{text:?}");
        }
    }
}
