use std::fs;
use std::path::Path;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

use crate::core::{DuplicatesEffect, DuplicatesEvent, DuplicatesReport, Event};
use crate::lang::duplicates::{Detector, tokens};
use crate::lang::languages::Language;
use crate::services::fs::walker;

pub fn scan(root: &Path, anonymize: bool, cancel: &AtomicBool) -> DuplicatesReport {
    let mut detector = Detector::default();
    for entry in walker(root).build().filter_map(Result::ok) {
        if cancel.load(Ordering::Relaxed) {
            return DuplicatesReport {
                cancelled: true,
                ..DuplicatesReport::default()
            };
        }
        if !entry.file_type().is_some_and(|t| t.is_file()) {
            continue;
        }
        let language = Language::from_path(entry.path());
        let Ok(text) = fs::read_to_string(entry.path()) else {
            continue;
        };
        let found = tokens(language, &text, anonymize);
        if !detector.add(entry.path().to_path_buf(), found) {
            break;
        }
    }
    detector.finish(cancel)
}

#[derive(Default)]
pub struct DuplicatesHost {
    cancel: Option<Arc<AtomicBool>>,
}

impl DuplicatesHost {
    pub fn start(
        &mut self,
        id: u64,
        root: std::path::PathBuf,
        anonymize: bool,
        send: impl FnOnce(Event) + Send + 'static,
    ) {
        self.stop();
        let cancel = Arc::new(AtomicBool::new(false));
        self.cancel = Some(Arc::clone(&cancel));
        std::thread::spawn(move || {
            let report = scan(&root, anonymize, &cancel);
            send(Event::Duplicates(DuplicatesEvent::Finished { id, report }));
        });
    }

    pub fn run(&mut self, effect: DuplicatesEffect, send: impl FnOnce(Event) + Send + 'static) {
        match effect {
            DuplicatesEffect::Start {
                id,
                root,
                anonymize,
            } => self.start(id, root, anonymize, send),
            DuplicatesEffect::Stop => self.stop(),
        }
    }

    pub fn stop(&mut self) {
        if let Some(cancel) = self.cancel.take() {
            cancel.store(true, Ordering::Relaxed);
        }
    }
}

impl Drop for DuplicatesHost {
    fn drop(&mut self) {
        self.stop();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ignored_files_do_not_count_and_a_cancel_returns_at_once() {
        let dir = tempfile::tempdir().unwrap();
        let body = "fn f(a: i32) -> i32 {\n let t = a + 1;\n if t > 2 { println!(\"{}\", t); return t - 1; }\n for i in 0..t { println!(\"{}\", i + a); }\n t * a + 3\n}\n";
        fs::write(dir.path().join(".gitignore"), "skip.rs\n").unwrap();
        fs::write(dir.path().join("a.rs"), body).unwrap();
        fs::write(dir.path().join("skip.rs"), body).unwrap();
        let none = scan(dir.path(), false, &AtomicBool::new(false));
        assert!(none.groups.is_empty(), "{none:#?}");
        fs::write(dir.path().join("b.rs"), body).unwrap();
        let found = scan(dir.path(), false, &AtomicBool::new(false));
        assert_eq!(found.groups.len(), 1, "{found:#?}");
        assert_eq!(found.groups[0].fragments.len(), 2);
        assert!(scan(dir.path(), false, &AtomicBool::new(true)).cancelled);
    }
}
