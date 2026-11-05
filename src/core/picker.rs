use nucleo_matcher::pattern::{CaseMatching, Normalization, Pattern};
use nucleo_matcher::{Config, Matcher, Utf32Str};

use super::input::LineInput;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PickerKind {
    Actions,
    Files,
    RecentFiles,
    Everywhere,
    Locations,
    Notifications,
    Servers,
    CodeActions,
    Refactorings,
    CompareBranches,
    Changelists,
    Branches,
    Run,
    Symbols,
    ProjectSymbols,
    Breakpoints,
    DebugTargets,
    DebugExecutables,
    DebugThreads,
    FileTemplates,
    ProjectTemplates,
    ScratchKinds,
    Scratches,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PickerMatch {
    pub index: usize,
    pub indices: Vec<u32>,
}

#[derive(Debug, Clone)]
pub struct Picker {
    pub kind: PickerKind,
    pub input: LineInput,
    pub matches: Vec<PickerMatch>,
    pub selected: usize,
}

impl Picker {
    pub fn new(kind: PickerKind, haystack: &[String]) -> Self {
        let mut picker = Self {
            kind,
            input: LineInput::default(),
            matches: Vec::new(),
            selected: 0,
        };
        picker.refilter(haystack);
        picker
    }

    pub fn refilter(&mut self, haystack: &[String]) {
        self.matches = fuzzy_filter(
            self.input.text(),
            haystack,
            matches!(
                self.kind,
                PickerKind::Files
                    | PickerKind::RecentFiles
                    | PickerKind::Everywhere
                    | PickerKind::Locations
            ),
        );
        self.selected = 0;
    }

    pub fn select_next(&mut self, by: usize) {
        if !self.matches.is_empty() {
            self.selected = (self.selected + by).min(self.matches.len() - 1);
        }
    }

    pub fn select_previous(&mut self, by: usize) {
        self.selected = self.selected.saturating_sub(by);
    }

    pub fn selected_index(&self) -> Option<usize> {
        self.matches.get(self.selected).map(|m| m.index)
    }
}

pub fn fuzzy_filter(query: &str, haystack: &[String], paths: bool) -> Vec<PickerMatch> {
    if query.trim().is_empty() {
        return (0..haystack.len())
            .map(|index| PickerMatch {
                index,
                indices: Vec::new(),
            })
            .collect();
    }
    let config = if paths {
        Config::DEFAULT.match_paths()
    } else {
        Config::DEFAULT
    };
    let mut matcher = Matcher::new(config);
    let pattern = Pattern::parse(query, CaseMatching::Smart, Normalization::Smart);
    let mut buf = Vec::new();
    let mut scored: Vec<(u32, PickerMatch)> = haystack
        .iter()
        .enumerate()
        .filter_map(|(index, item)| {
            let mut indices = Vec::new();
            let score =
                pattern.indices(Utf32Str::new(item, &mut buf), &mut matcher, &mut indices)?;
            indices.sort_unstable();
            indices.dedup();
            Some((score, PickerMatch { index, indices }))
        })
        .collect();
    scored.sort_by(|a, b| b.0.cmp(&a.0).then(a.1.index.cmp(&b.1.index)));
    scored.into_iter().map(|(_, m)| m).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn items(xs: &[&str]) -> Vec<String> {
        xs.iter().map(|s| (*s).to_owned()).collect()
    }

    #[test]
    fn empty_query_keeps_everything_in_order() {
        let list = items(&["b", "a"]);
        let idx: Vec<_> = fuzzy_filter("", &list, false)
            .iter()
            .map(|m| m.index)
            .collect();
        assert_eq!(idx, [0, 1]);
    }

    #[test]
    fn filters_and_ranks_with_match_positions() {
        let list = items(&["Split Vertically", "Save", "Show Local History"]);
        let found = fuzzy_filter("spv", &list, false);
        assert_eq!(found[0].index, 0);
        assert_eq!(found[0].indices, [0, 1, 6]);
        assert!(found.iter().all(|m| m.index != 1), "Save has no v");
    }

    #[test]
    fn path_matching_prefers_file_names() {
        let list = items(&["src/core/mod.rs", "src/main.rs"]);
        let found = fuzzy_filter("main", &list, true);
        assert_eq!(found.len(), 1);
        assert_eq!(found[0].index, 1);
    }

    #[test]
    fn selection_stays_in_bounds() {
        let list = items(&["a", "b"]);
        let mut p = Picker::new(PickerKind::Actions, &list);
        p.select_next(10);
        assert_eq!(p.selected_index(), Some(1));
        p.select_previous(10);
        assert_eq!(p.selected_index(), Some(0));
    }
}
