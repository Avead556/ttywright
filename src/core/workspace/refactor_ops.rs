use std::ops::Range;

use super::{Document, DocumentId, Effect, Focus, LspQuery, LspUsage, Workspace};
use super::{PositionEncoding, char_at, position_of};
use crate::core::RefactorOffer;
use crate::core::file_op::FilePromptKind;
use crate::core::picker::PickerKind;

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) enum RefactorRow {
    Rename,
    SafeDelete,
    Offer(RefactorOffer),
    NoIntroduceVariable,
}

impl RefactorRow {
    pub(super) fn label(&self) -> String {
        match self {
            Self::Rename => "Rename Symbol".to_owned(),
            Self::SafeDelete => "Safe Delete".to_owned(),
            Self::Offer(offer) => offer.action.title.clone(),
            Self::NoIntroduceVariable => "Introduce Variable (not offered here)".to_owned(),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(super) struct PendingSafeDelete {
    doc: DocumentId,
    version: u64,
    range: Range<usize>,
}

fn introduces_variable(title: &str) -> bool {
    let title = title.to_lowercase();
    ["variable", "constant", "local"]
        .iter()
        .any(|w| title.contains(w))
        && ["extract", "introduce"].iter().any(|w| title.contains(w))
}

impl Workspace {
    pub(super) fn refactor_ask(&mut self, fx: &mut Vec<Effect>) {
        self.lsp_ask(
            |text, cursor, encoding| {
                let range = cursor.selection().unwrap_or(cursor.idx..cursor.idx);
                LspQuery::Refactorings {
                    at: position_of(text, range.start, encoding),
                    to: position_of(text, range.end, encoding),
                }
            },
            fx,
        );
    }

    pub(super) fn refactor_show(&mut self, offers: Vec<RefactorOffer>) {
        let has_variable = offers.iter().any(|o| introduces_variable(&o.action.title));
        let mut rows = vec![RefactorRow::Rename, RefactorRow::SafeDelete];
        rows.extend(offers.into_iter().map(RefactorRow::Offer));
        if !has_variable {
            rows.push(RefactorRow::NoIntroduceVariable);
        }
        self.refactor_rows = rows;
        self.code_actions_stamp = self.edit_stamp();
        self.open_picker(PickerKind::Refactorings);
    }

    pub(super) fn refactor_chosen(&mut self, index: usize, fx: &mut Vec<Effect>) {
        match self.refactor_rows.get(index).cloned() {
            Some(RefactorRow::Rename) => self.rename_ask(),
            Some(RefactorRow::SafeDelete) => self.safe_delete_ask(fx),
            Some(RefactorRow::NoIntroduceVariable) => self.info(
                "The language server does not offer Introduce Variable here; select an expression"
                    .to_owned(),
            ),
            Some(RefactorRow::Offer(_)) if self.edit_stamp() != self.code_actions_stamp => {
                self.info("The text changed; ask for refactorings again".to_owned());
            }
            Some(RefactorRow::Offer(offer)) => {
                self.rename_preview(Some(offer.action.changes), offer.places);
            }
            None => {}
        }
    }

    pub(super) fn safe_delete_ask(&mut self, fx: &mut Vec<Effect>) {
        if self.focus != Focus::Editor {
            return;
        }
        let Some((doc, range, version)) = self.active().and_then(|(view, d)| {
            let word = super::word_start(d.text(), view.cursor.idx);
            let range = d.syntax()?.declaration_at(d.text(), word)?;
            Some((view.doc, range, d.version()))
        }) else {
            return self.info("Put the cursor on the name in a declaration".to_owned());
        };
        self.pending_safe_delete = Some(PendingSafeDelete {
            doc,
            version,
            range,
        });
        self.lsp_query(|at| LspQuery::SafeDeleteUsages { at }, fx);
    }

    pub(super) fn safe_delete_answered(&mut self, usages: Vec<LspUsage>) {
        let Some(pending) = self.pending_safe_delete.clone() else {
            return;
        };
        let Some(d) = self.documents.get(&pending.doc) else {
            self.pending_safe_delete = None;
            return;
        };
        let others = other_usages(usages, d, &pending.range, self.answer_encoding);
        if others.is_empty() {
            return self.safe_delete_confirmed();
        }
        let summary = format!(
            "{} uses remain, listed in Find. Delete anyway?",
            others.len()
        );
        self.show_usages(others);
        self.file_prompt_open(FilePromptKind::SafeDeleteConfirm { summary }, "");
    }

    pub(super) fn safe_delete_declined(&mut self) {
        self.pending_safe_delete = None;
        self.focus = Focus::Editor;
    }

    pub(super) fn safe_delete_confirmed(&mut self) {
        self.focus = Focus::Editor;
        let Some(pending) = self.pending_safe_delete.take() else {
            return;
        };
        let view = self.active_view().filter(|v| {
            self.views
                .get(v)
                .is_some_and(|view| view.doc == pending.doc)
        });
        let Some(d) = self.documents.get_mut(&pending.doc) else {
            return;
        };
        if d.version() != pending.version {
            return self.info("The text changed; nothing was deleted".to_owned());
        }
        d.close_undo_group();
        let applied = d.edit(pending.range.start, pending.range.end, "", false);
        d.close_undo_group();
        self.after_change(view, pending.doc, &applied);
        if view.is_some() {
            self.set_active_cursor(super::Cursor::at(pending.range.start));
        }
        self.info("Deleted; Ctrl+Z brings it back".to_owned());
    }
}

fn other_usages(
    usages: Vec<LspUsage>,
    d: &Document,
    range: &Range<usize>,
    encoding: PositionEncoding,
) -> Vec<LspUsage> {
    usages
        .into_iter()
        .filter(|u| {
            u.location.path != d.path()
                || !range.contains(&char_at(d.text(), u.location.start, encoding))
        })
        .collect()
}
