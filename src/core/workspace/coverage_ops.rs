use std::path::PathBuf;

use super::{Document, Effect, Workspace};
use crate::core::coverage::{CoverageEffect, CoverageEvent, CoverageState};
use crate::core::file_op::FilePromptKind;

impl Workspace {
    pub(super) fn coverage_load_ask(&mut self) {
        self.file_prompt_open(FilePromptKind::CoverageReport, "");
    }

    pub(super) fn coverage_toggle(&mut self) {
        if !self.coverage.toggle() {
            self.coverage_load_ask();
        }
    }

    pub(super) fn coverage_named(&mut self, typed: &str, fx: &mut Vec<Effect>) {
        let typed = typed.trim();
        fx.push(Effect::Coverage(CoverageEffect::Read {
            root: self.root.clone(),
            file: (!typed.is_empty()).then(|| PathBuf::from(typed)),
        }));
    }

    pub(super) fn coverage_read(&mut self, event: CoverageEvent) {
        match self.coverage.handle(event) {
            Ok(text) => self.info(text),
            Err(text) => self.error(text),
        }
    }

    pub fn coverage_state(&self, d: &Document) -> Option<CoverageState> {
        self.coverage.state(d)
    }

    pub fn coverage_mark(&self, d: &Document, line: usize) -> Option<bool> {
        self.coverage.mark(d, line)
    }
}
