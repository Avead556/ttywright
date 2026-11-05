use std::path::PathBuf;

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Serialize, Deserialize)]
pub struct DuplicateFragment {
    pub path: PathBuf,
    pub first_line: u32,
    pub last_line: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DuplicateGroup {
    pub tokens: usize,
    pub fragments: Vec<DuplicateFragment>,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct DuplicatesReport {
    pub groups: Vec<DuplicateGroup>,
    pub files: usize,
    pub tokens: usize,
    pub truncated: bool,
    pub cancelled: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DuplicatesPanel {
    pub running: Option<u64>,
    pub report: DuplicatesReport,
    pub selected: usize,
    pub anonymize: bool,
    next_id: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DuplicatesEffect {
    Start {
        id: u64,
        root: PathBuf,
        anonymize: bool,
    },
    Stop,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum DuplicatesEvent {
    Finished { id: u64, report: DuplicatesReport },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DuplicateRow<'a> {
    Group(&'a DuplicateGroup),
    Fragment(&'a DuplicateFragment),
}

impl DuplicatesPanel {
    pub fn rows(&self) -> impl Iterator<Item = DuplicateRow<'_>> {
        self.report.groups.iter().flat_map(|group| {
            std::iter::once(DuplicateRow::Group(group))
                .chain(group.fragments.iter().map(DuplicateRow::Fragment))
        })
    }

    pub fn scan(&mut self, root: PathBuf) -> DuplicatesEffect {
        self.next_id += 1;
        self.running = Some(self.next_id);
        DuplicatesEffect::Start {
            id: self.next_id,
            root,
            anonymize: self.anonymize,
        }
    }

    pub fn stop(&mut self) -> Option<DuplicatesEffect> {
        self.running.take()?;
        self.report.cancelled = true;
        Some(DuplicatesEffect::Stop)
    }

    pub fn handle(&mut self, event: DuplicatesEvent) {
        let DuplicatesEvent::Finished { id, report } = event;
        if self.running != Some(id) {
            return;
        }
        self.running = None;
        self.report = report;
        self.selected = 0;
    }

    pub fn step(&mut self, delta: isize) {
        let last = self.row_count().saturating_sub(1);
        self.selected = self.selected.saturating_add_signed(delta).min(last);
    }

    pub fn select(&mut self, index: usize) -> bool {
        let there = index < self.row_count();
        if there {
            self.selected = index;
        }
        there
    }

    pub fn selected_place(&self) -> Option<DuplicateFragment> {
        self.rows().nth(self.selected).and_then(|row| match row {
            DuplicateRow::Fragment(fragment) => Some(fragment.clone()),
            DuplicateRow::Group(group) => group.fragments.first().cloned(),
        })
    }

    pub fn row_count(&self) -> usize {
        self.report
            .groups
            .iter()
            .map(|g| g.fragments.len() + 1)
            .sum()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn report() -> DuplicatesReport {
        let fragment = |line| DuplicateFragment {
            path: "a.rs".into(),
            first_line: line,
            last_line: line + 3,
        };
        DuplicatesReport {
            groups: vec![DuplicateGroup {
                tokens: 50,
                fragments: vec![fragment(10), fragment(40)],
            }],
            ..DuplicatesReport::default()
        }
    }

    #[test]
    fn only_the_latest_scan_is_shown_and_stop_marks_it_cancelled() {
        let mut panel = DuplicatesPanel::default();
        let DuplicatesEffect::Start { id: old, .. } = panel.scan("/p".into()) else {
            panic!("a scan");
        };
        let DuplicatesEffect::Start { id, .. } = panel.scan("/p".into()) else {
            panic!("a scan");
        };
        panel.handle(DuplicatesEvent::Finished {
            id: old,
            report: report(),
        });
        assert_eq!(panel.row_count(), 0, "a replaced scan is dropped");
        panel.handle(DuplicatesEvent::Finished {
            id,
            report: report(),
        });
        assert_eq!(panel.row_count(), 3);
        assert_eq!(panel.stop(), None);
        panel.scan("/p".into());
        assert_eq!(panel.stop(), Some(DuplicatesEffect::Stop));
        assert!(panel.report.cancelled);
    }

    #[test]
    fn a_group_row_points_at_its_first_fragment() {
        let mut panel = DuplicatesPanel::default();
        let DuplicatesEffect::Start { id, .. } = panel.scan("/p".into()) else {
            panic!("a scan");
        };
        panel.handle(DuplicatesEvent::Finished {
            id,
            report: report(),
        });
        assert_eq!(panel.selected_place().map(|f| f.first_line), Some(10));
        panel.step(10);
        assert_eq!(panel.selected, 2);
        assert_eq!(panel.selected_place().map(|f| f.first_line), Some(40));
        assert!(!panel.select(3));
        panel.step(-10);
        assert_eq!(panel.selected, 0);
    }
}
