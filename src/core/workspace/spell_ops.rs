use super::{Effect, Focus, ToolTab, Workspace};
use crate::core::SearchMatch;

impl Workspace {
    pub(super) fn spell_check(&mut self, fx: &mut Vec<Effect>) {
        let Some((_, d)) = self.active() else {
            return self.info("Open a file to check it".to_owned());
        };
        fx.push(Effect::SpellCheck {
            path: d.path().to_path_buf(),
            text: d.text().to_string(),
            language: d.language(),
        });
    }

    pub(super) fn spell_add_word(&mut self, fx: &mut Vec<Effect>) {
        let word = self.word_at_cursor();
        if word.is_empty() {
            return self.info("Put the cursor on the word".to_owned());
        }
        fx.push(Effect::SpellAddWord { word });
    }

    pub(super) fn spell_checked(&mut self, result: Result<Vec<SearchMatch>, String>) {
        let results = match result {
            Ok(results) if results.is_empty() => {
                return self.info("No spelling problems found".to_owned());
            }
            Ok(results) => results,
            Err(reason) => return self.error(format!("Cannot check spelling: {reason}")),
        };
        let name = self
            .active()
            .map_or_else(String::new, |(_, d)| d.file_name());
        let s = &mut self.search;
        s.results = results;
        s.truncated = false;
        s.running = None;
        s.selected = Some(0);
        s.usages_of = Some(name);
        s.usages_word = "unknown word";
        self.tool_window_visible = true;
        self.tool_tab = ToolTab::Find;
        self.focus = Focus::FindInFiles;
    }
}
