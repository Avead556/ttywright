use std::path::{Path, PathBuf};
use std::str;

use super::fs::{Written, disk_state, read_if_exists, write_guarded};
use crate::core::{
    ClosedPlan, Event, FilePlan, FileProblem, FindOptions, Replacer, SnapshotLabel, WriteGuard,
};

const BOM: &str = "\u{feff}";
const NOT_TEXT: &str = "not UTF-8 text";

fn split_bom(bytes: &[u8]) -> Result<(&str, &str), String> {
    let text = str::from_utf8(bytes).map_err(|_| NOT_TEXT.to_owned())?;
    Ok(match text.strip_prefix(BOM) {
        Some(rest) => (BOM, rest),
        None => ("", text),
    })
}

fn plan_file(path: &Path, replacer: &mut Replacer) -> Result<Option<ClosedPlan>, String> {
    let disk = disk_state(path).map_err(|e| e.to_string())?;
    let Some(bytes) = read_if_exists(path).map_err(|e| e.to_string())? else {
        return Err("no longer exists".to_owned());
    };
    let (_, text) = split_bom(&bytes)?;
    let edits = replacer.edits(path, text);
    if edits.is_empty() {
        return Ok(None);
    }
    Ok(Some(ClosedPlan {
        plan: FilePlan::new(path.to_path_buf(), text, &edits),
        disk,
    }))
}

pub fn plan(
    id: u64,
    files: &[PathBuf],
    query: &str,
    options: FindOptions,
    replacement: &str,
) -> Event {
    let mut plans = Vec::new();
    let mut failed: Vec<FileProblem> = Vec::new();
    match Replacer::compile(query, options, replacement) {
        Ok(mut replacer) => {
            for path in files {
                match plan_file(path, &mut replacer) {
                    Ok(Some(plan)) => plans.push(plan),
                    Ok(None) => {}
                    Err(why) => failed.push((path.clone(), why)),
                }
            }
        }
        Err(why) => failed.extend(files.iter().map(|p| (p.clone(), why.clone()))),
    }
    Event::ProjectReplacePlanned {
        id,
        files: plans,
        failed,
    }
}

enum Outcome {
    Changed,
    Skipped(String),
    Failed(String),
}

fn apply_file(
    path: &Path,
    expected: Option<crate::core::DiskState>,
    replacer: &mut Replacer,
    snapshot: &impl Fn(&Path, &[u8], SnapshotLabel) -> Result<(), String>,
) -> Outcome {
    let changed = || Outcome::Skipped("changed on disk since the preview".to_owned());
    match disk_state(path) {
        Ok(disk) if disk != expected => return changed(),
        Ok(_) => {}
        Err(e) => return Outcome::Failed(e.to_string()),
    }
    let bytes = match read_if_exists(path) {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return changed(),
        Err(e) => return Outcome::Failed(e.to_string()),
    };
    let (bom, text) = match split_bom(&bytes) {
        Ok(parts) => parts,
        Err(why) => return Outcome::Failed(why),
    };
    let edits = replacer.edits(path, text);
    let Some(first) = edits.first() else {
        return changed();
    };
    let mut new = String::with_capacity(bytes.len());
    new.push_str(bom);
    new.push_str(&text[..first.bytes.start]);
    let mut copied = first.bytes.start;
    for edit in &edits {
        new.push_str(&text[copied..edit.bytes.start]);
        new.push_str(&edit.text);
        copied = edit.bytes.end;
    }
    new.push_str(&text[copied..]);
    if let Err(why) = snapshot(path, &bytes, SnapshotLabel::BeforeReplaceInFiles) {
        return Outcome::Failed(format!("no Local History snapshot, left alone: {why}"));
    }
    match write_guarded(path, new.as_bytes(), WriteGuard::Unchanged(expected)) {
        Ok(Written::Done(_)) => {
            let _ = snapshot(path, new.as_bytes(), SnapshotLabel::Save);
            Outcome::Changed
        }
        Ok(Written::Refused(_)) => changed(),
        Err(e) => Outcome::Failed(e.to_string()),
    }
}

pub fn apply(
    id: u64,
    files: &[(PathBuf, Option<crate::core::DiskState>)],
    query: &str,
    options: FindOptions,
    replacement: &str,
    snapshot: impl Fn(&Path, &[u8], SnapshotLabel) -> Result<(), String>,
) -> Event {
    let mut changed = Vec::new();
    let mut skipped = Vec::new();
    let mut failed = Vec::new();
    match Replacer::compile(query, options, replacement) {
        Ok(mut replacer) => {
            for (path, expected) in files {
                match apply_file(path, *expected, &mut replacer, &snapshot) {
                    Outcome::Changed => changed.push(path.clone()),
                    Outcome::Skipped(why) => skipped.push((path.clone(), why)),
                    Outcome::Failed(why) => failed.push((path.clone(), why)),
                }
            }
        }
        Err(why) => failed.extend(files.iter().map(|(p, _)| (p.clone(), why.clone()))),
    }
    Event::ProjectReplaceApplied {
        id,
        changed,
        skipped,
        failed,
    }
}

#[cfg(test)]
mod tests {
    use std::cell::RefCell;
    use std::fs;
    use std::os::unix::fs::PermissionsExt;

    use super::*;

    type Snapshots = RefCell<Vec<(PathBuf, Vec<u8>, SnapshotLabel)>>;

    struct Run {
        changed: Vec<PathBuf>,
        skipped: Vec<FileProblem>,
        failed: Vec<FileProblem>,
    }

    fn state(path: &Path) -> Option<crate::core::DiskState> {
        disk_state(path).unwrap()
    }

    fn run(files: &[(PathBuf, Option<crate::core::DiskState>)], snaps: &Snapshots) -> Run {
        let event = apply(1, files, "foo", FindOptions::default(), "bar", |p, b, l| {
            snaps.borrow_mut().push((p.to_path_buf(), b.to_vec(), l));
            Ok(())
        });
        let Event::ProjectReplaceApplied {
            changed,
            skipped,
            failed,
            ..
        } = event
        else {
            panic!("wrong event");
        };
        Run {
            changed,
            skipped,
            failed,
        }
    }

    #[test]
    fn a_closed_file_is_snapshotted_then_replaced_keeping_every_other_byte() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        let original = "\u{feff}foo\r\nкот foo\n\tlast foo".as_bytes();
        fs::write(&p, original).unwrap();
        fs::set_permissions(&p, fs::Permissions::from_mode(0o640)).unwrap();
        let snaps = Snapshots::default();
        let r = run(&[(p.clone(), state(&p))], &snaps);
        assert_eq!(r.changed, std::slice::from_ref(&p));
        assert!(r.skipped.is_empty() && r.failed.is_empty());
        assert_eq!(
            fs::read(&p).unwrap(),
            "\u{feff}bar\r\nкот bar\n\tlast bar".as_bytes()
        );
        assert_eq!(
            fs::metadata(&p).unwrap().permissions().mode() & 0o777,
            0o640
        );
        let snaps = snaps.borrow();
        assert_eq!(snaps[0].1, original, "the old text comes first");
        assert_eq!(snaps[0].2, SnapshotLabel::BeforeReplaceInFiles);
        let leftovers = fs::read_dir(dir.path()).unwrap().count();
        assert_eq!(leftovers, 1, "no temp files");
    }

    #[test]
    fn a_file_changed_since_the_preview_is_skipped_and_kept() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        fs::write(&p, "foo").unwrap();
        let seen = state(&p);
        fs::write(&p, "foo and someone else's work").unwrap();
        let snaps = Snapshots::default();
        let r = run(&[(p.clone(), seen)], &snaps);
        assert!(r.changed.is_empty());
        assert_eq!(r.skipped.len(), 1);
        assert_eq!(
            fs::read_to_string(&p).unwrap(),
            "foo and someone else's work"
        );
        assert!(snaps.borrow().is_empty());
    }

    #[test]
    fn a_change_slipped_in_after_the_read_is_refused_by_the_write_guard() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        fs::write(&p, "foo").unwrap();
        let event = apply(
            1,
            &[(p.clone(), state(&p))],
            "foo",
            FindOptions::default(),
            "bar",
            |path, _, _| fs::write(path, "foo, edited just now").map_err(|e| e.to_string()),
        );
        let Event::ProjectReplaceApplied {
            changed, skipped, ..
        } = event
        else {
            panic!("wrong event");
        };
        assert!(changed.is_empty());
        assert_eq!(skipped.len(), 1);
        assert_eq!(fs::read_to_string(&p).unwrap(), "foo, edited just now");
    }

    #[test]
    fn a_deleted_file_is_skipped_and_not_recreated() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        fs::write(&p, "foo").unwrap();
        let seen = state(&p);
        fs::remove_file(&p).unwrap();
        let r = run(&[(p.clone(), seen)], &Snapshots::default());
        assert_eq!(r.skipped.len(), 1);
        assert!(!p.exists());
    }

    #[test]
    fn no_snapshot_means_no_write() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        fs::write(&p, "foo").unwrap();
        let event = apply(
            1,
            &[(p.clone(), state(&p))],
            "foo",
            FindOptions::default(),
            "bar",
            |_, _, _| Err("disk full".to_owned()),
        );
        let Event::ProjectReplaceApplied {
            failed, changed, ..
        } = event
        else {
            panic!("wrong event");
        };
        assert!(changed.is_empty());
        assert!(failed[0].1.contains("disk full"), "{failed:?}");
        assert_eq!(fs::read_to_string(&p).unwrap(), "foo");
    }

    #[test]
    fn one_bad_file_does_not_stop_the_others() {
        let dir = tempfile::tempdir().unwrap();
        let bad = dir.path().join("bad.txt");
        let good = dir.path().join("good.txt");
        fs::write(&bad, b"foo \xFF\xFE").unwrap();
        fs::write(&good, "foo").unwrap();
        let r = run(
            &[(bad.clone(), state(&bad)), (good.clone(), state(&good))],
            &Snapshots::default(),
        );
        assert_eq!(r.changed, std::slice::from_ref(&good));
        assert_eq!(r.failed[0].0, bad);
        assert_eq!(r.failed[0].1, NOT_TEXT);
        assert_eq!(fs::read(&bad).unwrap(), b"foo \xFF\xFE");
        assert_eq!(fs::read_to_string(&good).unwrap(), "bar");
    }

    #[test]
    fn a_read_only_folder_is_a_failure_in_the_report_and_not_a_panic() {
        let dir = tempfile::tempdir().unwrap();
        let sub = dir.path().join("ro");
        fs::create_dir(&sub).unwrap();
        let p = sub.join("a.txt");
        fs::write(&p, "foo").unwrap();
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o555)).unwrap();
        let r = run(&[(p.clone(), state(&p))], &Snapshots::default());
        fs::set_permissions(&sub, fs::Permissions::from_mode(0o755)).unwrap();
        assert_eq!(r.failed.len(), 1, "{:?}", r.changed);
        assert_eq!(fs::read_to_string(&p).unwrap(), "foo");
    }

    #[test]
    fn a_symlink_is_written_through_and_stays_a_symlink() {
        let dir = tempfile::tempdir().unwrap();
        let target = dir.path().join("target.txt");
        let link = dir.path().join("link.txt");
        fs::write(&target, "foo").unwrap();
        std::os::unix::fs::symlink(&target, &link).unwrap();
        let r = run(&[(link.clone(), state(&link))], &Snapshots::default());
        assert_eq!(r.changed.len(), 1);
        assert!(
            fs::symlink_metadata(&link)
                .unwrap()
                .file_type()
                .is_symlink()
        );
        assert_eq!(fs::read_to_string(&target).unwrap(), "bar");
    }

    #[test]
    fn planning_reports_what_will_change_and_writes_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let a = dir.path().join("a.txt");
        let none = dir.path().join("none.txt");
        let bad = dir.path().join("bad.txt");
        fs::write(&a, "foo\nfoo foo\nx\n").unwrap();
        fs::write(&none, "nothing").unwrap();
        fs::write(&bad, b"foo\xFF").unwrap();
        let event = plan(
            3,
            &[a.clone(), none, bad.clone(), dir.path().join("gone.txt")],
            "foo",
            FindOptions::default(),
            "bar",
        );
        let Event::ProjectReplacePlanned { id, files, failed } = event else {
            panic!("wrong event");
        };
        assert_eq!(id, 3);
        assert_eq!(files.len(), 1);
        assert_eq!(files[0].disk, state(&a));
        assert_eq!(files[0].plan.occurrences, 3);
        assert_eq!(files[0].plan.changes[1].new, "bar bar");
        assert_eq!(failed.len(), 2, "{failed:?}");
        assert_eq!(fs::read_to_string(&a).unwrap(), "foo\nfoo foo\nx\n");
    }

    #[test]
    fn capture_groups_work_on_closed_files_too() {
        let dir = tempfile::tempdir().unwrap();
        let p = dir.path().join("a.txt");
        fs::write(&p, "a=1\nb=2\n").unwrap();
        let options = FindOptions {
            regex: true,
            ..FindOptions::default()
        };
        apply(
            1,
            &[(p.clone(), state(&p))],
            r"^(\w)=(\d)$",
            options,
            "$2=$1",
            |_, _, _| Ok(()),
        );
        assert_eq!(fs::read_to_string(&p).unwrap(), "1=a\n2=b\n");
    }

    #[test]
    fn a_structural_plan_and_apply_change_only_the_code_keeping_every_other_byte() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("a.rs");
        let text = "fn a() {\r\n  foo(1, 2);\r\n  // foo(3, 4)\r\n}\r\n";
        fs::write(&path, format!("\u{feff}{text}")).unwrap();
        let options = FindOptions {
            structural: true,
            ..FindOptions::default()
        };
        let Event::ProjectReplacePlanned { files, failed, .. } = plan(
            1,
            std::slice::from_ref(&path),
            "foo($A$, $B$)",
            options,
            "bar($B$, $A$)",
        ) else {
            panic!("wrong event");
        };
        assert!(failed.is_empty(), "{failed:?}");
        assert_eq!(files[0].plan.occurrences, 1);
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            format!("\u{feff}{text}")
        );
        let event = apply(
            2,
            &[(path.clone(), files[0].disk)],
            "foo($A$, $B$)",
            options,
            "bar($B$, $A$)",
            |_, _, _| Ok(()),
        );
        assert!(
            matches!(event, Event::ProjectReplaceApplied { ref changed, .. } if changed.len() == 1)
        );
        assert_eq!(
            fs::read_to_string(&path).unwrap(),
            "\u{feff}fn a() {\r\n  bar(2, 1);\r\n  // foo(3, 4)\r\n}\r\n"
        );
    }

    #[test]
    fn a_replacement_naming_an_unknown_hole_changes_nothing() {
        let options = FindOptions {
            structural: true,
            ..FindOptions::default()
        };
        let path = PathBuf::from("/nowhere/a.rs");
        let Event::ProjectReplacePlanned { failed, .. } =
            plan(1, &[path], "foo($A$)", options, "bar($B$)")
        else {
            panic!("wrong event");
        };
        assert!(failed[0].1.contains("$B$"), "{failed:?}");
    }
}
