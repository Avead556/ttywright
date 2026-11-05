use super::*;
use crate::core::command::Effect;
use crate::core::git::GitBlame;
use crate::core::git_command::GitEffect;

#[derive(Debug, Clone, Default)]
pub(super) struct BlameState {
    known: Option<(u64, GitBlame)>,
    asked: Option<u64>,
    now: u64,
}

pub struct BlameView<'a> {
    pub blame: Option<&'a GitBlame>,
    pub now: u64,
}

impl Workspace {
    pub fn blame(&self, doc: DocumentId) -> Option<BlameView<'_>> {
        let state = self.git_blames.get(&doc)?;
        let version = self.documents.get(&doc)?.version();
        Some(BlameView {
            blame: state
                .known
                .as_ref()
                .filter(|(known, _)| *known == version)
                .map(|(_, blame)| blame),
            now: state.now,
        })
    }

    pub(super) fn git_toggle_blame(&mut self, fx: &mut Vec<Effect>) {
        let Some(doc) = self.active().map(|(view, _)| view.doc) else {
            return self.info("Open a file to annotate it".to_owned());
        };
        if self.git_blames.remove(&doc).is_some() {
            return;
        }
        self.git_blames.insert(doc, BlameState::default());
        self.autosave(fx);
    }

    pub(super) fn git_blame_catch_up(&mut self, fx: &mut Vec<Effect>) {
        if self.git_blames.is_empty() {
            return;
        }
        let documents = &self.documents;
        self.git_blames.retain(|doc, _| documents.contains_key(doc));
        let root = self.root.clone();
        for (doc, state) in &mut self.git_blames {
            let Some(d) = self.documents.get(doc) else {
                continue;
            };
            let queued = fx
                .iter()
                .any(|e| matches!(e, Effect::WriteFile { doc: d, .. } if d == doc));
            if !d.is_dirty() && !queued && state.asked != Some(d.version()) {
                state.asked = Some(d.version());
                fx.push(Effect::Git(GitEffect::Blame {
                    root: root.clone(),
                    doc: *doc,
                    path: d.path().to_path_buf(),
                    version: d.version(),
                }));
            }
        }
    }

    pub(super) fn git_blame_expire(&mut self) {
        for state in self.git_blames.values_mut() {
            state.asked = None;
        }
    }

    pub(super) fn git_blame_read(
        &mut self,
        doc: DocumentId,
        version: u64,
        result: Result<GitBlame, String>,
        now: u64,
    ) {
        let Some(state) = self.git_blames.get_mut(&doc) else {
            return;
        };
        match result {
            Ok(blame) => {
                let lines = self.documents.get(&doc).map_or(0, |d| d.text().len_lines());
                if lines == blame.lines.len() || lines == blame.lines.len() + 1 {
                    state.known = Some((version, blame));
                } else {
                    state.known = None;
                }
                state.now = now;
            }
            Err(text) => {
                self.git_blames.remove(&doc);
                self.error(format!("Blame: {text}"));
            }
        }
    }
}
