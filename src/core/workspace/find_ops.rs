use crate::core::git_command::GitEffect;
use crate::core::{Replacer, check_query};

use super::{
    Action, ClosedPlan, Command, Cursor, Effect, FilePlan, FileProblem, FindOptions, Focus,
    LineInput, Motion, PathBuf, PlanSource, Query, ReplacePreview, ReplaceReport, ReplaceStage,
    Rope, USAGES_ARE_NOT_TEXT, Workspace, merge_plan, move_input,
};

impl Workspace {
    pub(super) fn open_find_bar(&mut self, replacing: bool) {
        let Some((view, d)) = self.active() else {
            return;
        };
        let selected = view
            .cursor
            .selection()
            .map(|span| d.text().slice(span).to_string())
            .filter(|s| !s.contains(['\n', '\r']));
        let mut bar = self.find_bar.take().unwrap_or_default();
        if let Some(query) = selected {
            bar.input = LineInput::default();
            bar.input.insert(&query);
        }
        bar.input.end();
        if replacing {
            bar.replacement.get_or_insert_with(LineInput::default).end();
        } else {
            bar.replacement = None;
        }
        bar.in_replacement = replacing && !bar.input.text().is_empty();
        self.find_bar = Some(bar);
        self.refresh_find();
        self.focus = Focus::FindBar;
    }

    pub(super) fn bar_plan(&mut self) -> Option<(String, Vec<crate::core::find::Edit>)> {
        let bar = self.find_bar.as_ref()?;
        let replacement = bar.replacement.as_ref()?.text().to_owned();
        let plan = Query::new(bar.input.text(), self.bar_options).and_then(|query| {
            let template = query.template(&replacement)?;
            let (_, doc) = self.active().ok_or("No file is open")?;
            let text = doc.text().to_string();
            let edits = query.plan(&text, &template);
            Ok((text, edits))
        });
        match plan {
            Ok(plan) => Some(plan),
            Err(message) => {
                self.error(message);
                None
            }
        }
    }

    pub(super) fn replace_current(&mut self) {
        let Some(bar) = self.find_bar.as_ref() else {
            return;
        };
        let Some((view, _)) = self.active() else {
            return;
        };
        let cursor = view.cursor.idx;
        let Some(found) = bar.current(cursor).map(|i| bar.matches[i].clone()) else {
            return self.find_step(true);
        };
        let Some((text, edits)) = self.bar_plan() else {
            return;
        };
        let at = |edit: &&crate::core::find::Edit| {
            text[..edit.bytes.start].chars().count() == found.start
        };
        let Some(replacement) = edits.iter().find(at).map(|e| e.text.clone()) else {
            return;
        };
        self.close_undo_group();
        self.edit(found.start, found.end, &replacement, false);
        self.close_undo_group();
        let Some((view, _)) = self.active() else {
            return;
        };
        let after = view.cursor.idx;
        let next = self.find_bar.as_ref().and_then(|bar| {
            bar.matches
                .iter()
                .find(|m| m.start >= after)
                .or(bar.matches.first())
                .cloned()
        });
        match next {
            Some(m) => self.set_active_cursor(Cursor::at(m.start)),
            None => self.info("No more matches".to_owned()),
        }
    }

    pub(super) fn replace_all(&mut self) {
        if self
            .find_bar
            .as_ref()
            .is_none_or(|b| b.replacement.is_none())
        {
            return;
        }
        let Some((text, edits)) = self.bar_plan() else {
            return;
        };
        let Some((from, to, merged)) = merge_plan(&text, &edits) else {
            return self.info("No matches".to_owned());
        };
        self.close_undo_group();
        self.edit(from, to, &merged, false);
        self.close_undo_group();
        let count = edits.len();
        let noun = if count == 1 {
            "occurrence"
        } else {
            "occurrences"
        };
        self.info(format!("Replaced {count} {noun}"));
    }

    pub(super) fn toggle_find_option(&mut self, flip: fn(&mut FindOptions), fx: &mut Vec<Effect>) {
        if self.focus == Focus::FindInFiles {
            flip(&mut self.search.options);
            if self.search.searched.is_some() && self.search.usages_of.is_none() {
                self.run_project_search(fx);
            }
        } else if self.find_bar.is_some() {
            flip(&mut self.bar_options);
            self.refresh_find();
        } else {
            self.info("Open Find or Find in Files first".to_owned());
        }
    }

    pub(super) fn run_project_search(&mut self, fx: &mut Vec<Effect>) {
        let query = self.search.input.text().to_owned();
        if query.is_empty() {
            return;
        }
        let options = self.search.options;
        if let Err(message) = check_query(&query, options) {
            let s = &mut self.search;
            s.error = Some(message);
            s.results.clear();
            s.selected = None;
            s.running = None;
            s.searched = None;
            return;
        }
        let id = u64::from(self.alloc_id());
        let s = &mut self.search;
        s.error = None;
        s.running = Some(id);
        s.searched = Some(query.clone());
        s.usages_of = None;
        fx.push(Effect::SearchProject {
            id,
            root: self.root.clone(),
            query,
            options,
        });
    }

    pub(super) fn find_bar_input(&mut self, input: Command) {
        let Some(bar) = self.find_bar.as_mut() else {
            return;
        };
        match input {
            Command::Indent | Command::Outdent => {
                if bar.replacement.is_some() {
                    bar.in_replacement = !bar.in_replacement;
                }
                return;
            }
            Command::Newline if bar.in_replacement => return self.replace_current(),
            Command::Newline | Command::Move(Motion::Down) => return self.find_step(true),
            Command::Move(Motion::Up) => return self.find_step(false),
            _ => {}
        }
        if bar.in_replacement {
            let Some(field) = bar.replacement.as_mut() else {
                return;
            };
            match input {
                Command::Insert(s) => field.insert(&s),
                Command::DeleteBackward => field.delete_backward(),
                Command::DeleteForward => field.delete_forward(),
                Command::Move(m) => move_input(field, m),
                _ => {}
            }
            return;
        }
        match input {
            Command::Insert(s) => bar.input.insert(&s),
            Command::DeleteBackward => bar.input.delete_backward(),
            Command::DeleteForward => bar.input.delete_forward(),
            Command::Move(m) => return move_input(&mut bar.input, m),
            _ => return,
        }
        self.refresh_find();
        let Some((view, _)) = self.active() else {
            return;
        };
        let cursor = view.cursor.idx;
        let target = self.find_bar.as_ref().and_then(|bar| {
            bar.matches
                .iter()
                .find(|m| m.start >= cursor)
                .or(bar.matches.first())
                .cloned()
        });
        if let Some(m) = target {
            self.set_active_cursor(Cursor::at(m.start));
        }
    }

    pub(super) fn find_step(&mut self, forward: bool) {
        if self
            .find_bar
            .as_ref()
            .is_none_or(|b| b.input.text().is_empty())
        {
            return self.run_action(Action::Find, &mut Vec::new());
        }
        let Some((view, _)) = self.active() else {
            return;
        };
        let cursor = view.cursor.idx;
        match self
            .find_bar
            .as_ref()
            .and_then(|b| b.next_from(cursor, forward))
        {
            Some(m) => self.set_active_cursor(Cursor::at(m.start)),
            None => match self.find_bar.as_ref().and_then(|b| b.error.clone()) {
                Some(message) => self.error(message),
                None => self.info("No matches".to_owned()),
            },
        }
    }

    pub(super) fn refresh_find(&mut self) {
        let text: Option<Rope> = self.active().map(|(_, d)| d.text().clone());
        if let (Some(bar), Some(text)) = (self.find_bar.as_mut(), text) {
            bar.refresh(&text, self.bar_options);
        }
    }

    pub(super) fn search_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        if self.search.replace.is_some() {
            return self.replace_stage_input(input, fx);
        }
        let s = &mut self.search;
        if s.replacement.is_some() {
            match input {
                Command::Indent | Command::Outdent => {
                    s.in_replacement = !s.in_replacement;
                    s.selected = None;
                    return;
                }
                Command::Newline if s.in_replacement => return self.start_replace(fx),
                _ => {}
            }
            if let Some(field) = s.replacement.as_mut().filter(|_| s.in_replacement) {
                match input {
                    Command::Insert(text) => return field.insert(&text),
                    Command::DeleteBackward => return field.delete_backward(),
                    Command::DeleteForward => return field.delete_forward(),
                    Command::Move(m @ (Motion::Up | Motion::Down)) => {
                        s.in_replacement = false;
                        return self.search_input(Command::Move(m), fx);
                    }
                    Command::Move(m) => return move_input(field, m),
                    _ => return,
                }
            }
        }
        match input {
            Command::Insert(text) => {
                s.input.insert(&text);
                s.selected = None;
            }
            Command::DeleteBackward => {
                s.input.delete_backward();
                s.selected = None;
            }
            Command::DeleteForward => s.input.delete_forward(),
            Command::Move(Motion::Down) if !s.results.is_empty() => {
                s.selected = Some(s.selected.map_or(0, |i| (i + 1).min(s.results.len() - 1)));
            }
            Command::Move(Motion::Up) => s.selected = s.selected.and_then(|i| i.checked_sub(1)),
            Command::Move(m) if s.selected.is_none() => move_input(&mut s.input, m),
            Command::Newline => match s.selected {
                Some(i) => {
                    if let Some(m) = s.results.get(i).cloned() {
                        self.open_file(m.path, Some((m.line, m.col)), fx);
                    }
                }
                None => self.run_project_search(fx),
            },
            _ => {}
        }
    }

    pub(super) fn leave_replace_stage(&mut self) {
        if !matches!(self.search.replace, Some(ReplaceStage::Applying { .. })) {
            self.search.replace = None;
        }
    }

    pub(super) fn replace_stage_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let (scroll, rows) = match self.search.replace.as_mut() {
            Some(ReplaceStage::Preview(p)) => {
                let rows = p.rows();
                (&mut p.scroll, rows)
            }
            Some(ReplaceStage::Report(r)) => {
                let rows = r.rows();
                (&mut r.scroll, rows)
            }
            _ => return,
        };
        let last = rows.saturating_sub(1);
        match input {
            Command::Move(Motion::Up) => *scroll = scroll.saturating_sub(1),
            Command::Move(Motion::Down) => *scroll = (*scroll + 1).min(last),
            Command::Move(Motion::PageUp { rows }) => *scroll = scroll.saturating_sub(rows),
            Command::Move(Motion::PageDown { rows }) => *scroll = (*scroll + rows).min(last),
            Command::Insert(key) if matches!(key.as_str(), "y" | "Y" | "н" | "Н") => {
                self.apply_replace(fx);
            }
            _ => {}
        }
    }

    pub(super) fn start_replace(&mut self, fx: &mut Vec<Effect>) {
        let s = &self.search;
        if s.usages_of.is_some() {
            return self.info(USAGES_ARE_NOT_TEXT.to_owned());
        }
        if s.running.is_some() {
            return self.info("Still searching".to_owned());
        }
        let query = s.input.text().to_owned();
        if s.searched.as_deref() != Some(query.as_str()) {
            return self.info("Press Enter in the search field to search first".to_owned());
        }
        if s.results.is_empty() {
            return self.info("No matches to replace".to_owned());
        }
        let options = s.options;
        let truncated = s.truncated;
        let replacement = s
            .replacement
            .as_ref()
            .map_or(String::new(), |r| r.text().to_owned());
        let mut replacer = match Replacer::compile(&query, options, &replacement) {
            Ok(replacer) => replacer,
            Err(message) => return self.error(message),
        };
        let mut paths: Vec<PathBuf> = s.results.iter().map(|m| m.path.clone()).collect();
        paths.sort();
        paths.dedup();
        let mut open = Vec::new();
        let mut failed = Vec::new();
        let mut closed = Vec::new();
        for path in paths {
            let doc = self.documents.iter().find(|(_, d)| d.path() == path);
            let Some((id, d)) = doc else {
                closed.push(path);
                continue;
            };
            if d.conflict().is_some() {
                let why = "differs from the file on disk: resolve that first".to_owned();
                failed.push((path, why));
                continue;
            }
            let text = d.text().to_string();
            let edits = replacer.edits(&path, &text);
            if !edits.is_empty() {
                let source = PlanSource::Open {
                    doc: *id,
                    version: d.version(),
                };
                open.push((FilePlan::new(path, &text, &edits), source));
            }
        }
        let id = u64::from(self.alloc_id());
        self.search.replace = Some(ReplaceStage::Planning {
            id,
            open,
            failed,
            query: query.clone(),
            options,
            replacement: replacement.clone(),
            truncated,
        });
        if closed.is_empty() {
            self.replace_planned(id, Vec::new(), Vec::new());
        } else {
            fx.push(Effect::PlanProjectReplace {
                id,
                files: closed,
                query,
                options,
                replacement,
            });
        }
    }

    pub(super) fn replace_planned(
        &mut self,
        id: u64,
        files: Vec<ClosedPlan>,
        more_failed: Vec<FileProblem>,
    ) {
        let Some(ReplaceStage::Planning {
            id: waiting,
            open,
            failed,
            query,
            options,
            replacement,
            truncated,
        }) = self.search.replace.take()
        else {
            return;
        };
        if waiting != id {
            return;
        }
        let mut plans = open;
        plans.extend(files.into_iter().map(|c| {
            let source = PlanSource::Closed { disk: c.disk };
            (c.plan, source)
        }));
        plans.sort_by(|a, b| a.0.path.cmp(&b.0.path));
        let mut failed = failed;
        failed.extend(more_failed);
        if plans.is_empty() && failed.is_empty() {
            return self.info("Nothing to replace".to_owned());
        }
        self.search.replace = Some(ReplaceStage::Preview(ReplacePreview {
            files: plans,
            failed,
            query,
            options,
            replacement,
            truncated,
            scroll: 0,
        }));
    }

    pub(super) fn apply_replace(&mut self, fx: &mut Vec<Effect>) {
        let Some(ReplaceStage::Preview(preview)) = self.search.replace.take() else {
            return;
        };
        let mut report = ReplaceReport {
            failed: preview.failed.clone(),
            ..ReplaceReport::default()
        };
        let compiled = Replacer::compile(&preview.query, preview.options, &preview.replacement);
        let mut replacer = match compiled {
            Ok(replacer) => replacer,
            Err(message) => return self.error(message),
        };
        let mut closed = Vec::new();
        for (plan, source) in preview.files {
            let path = plan.path;
            let open = self
                .documents
                .iter()
                .find(|(_, d)| d.path() == path)
                .map(|(id, _)| *id);
            match (source, open) {
                (PlanSource::Closed { disk }, None) => closed.push((path, disk)),
                (PlanSource::Closed { .. }, Some(_)) => {
                    report
                        .skipped
                        .push((path, "opened since the preview".to_owned()));
                }
                (PlanSource::Open { doc, version }, Some(found)) if found == doc => {
                    let Some(d) = self.documents.get_mut(&doc) else {
                        continue;
                    };
                    if d.version() != version || d.conflict().is_some() {
                        let why = "changed since the preview".to_owned();
                        report.skipped.push((path, why));
                        continue;
                    }
                    let text = d.text().to_string();
                    let edits = replacer.edits(&path, &text);
                    let Some((from, to, merged)) = merge_plan(&text, &edits) else {
                        continue;
                    };
                    d.close_undo_group();
                    let applied = d.edit(from, to, &merged, false);
                    d.close_undo_group();
                    self.after_change(None, doc, &applied);
                    report.changed.push(path);
                }
                (PlanSource::Open { .. }, _) => {
                    report
                        .skipped
                        .push((path, "closed since the preview".to_owned()));
                }
            }
        }
        if closed.is_empty() {
            self.finish_replace(report, fx);
        } else {
            let id = u64::from(self.alloc_id());
            self.search.replace = Some(ReplaceStage::Applying { id, report });
            fx.push(Effect::ApplyProjectReplace {
                id,
                files: closed,
                query: preview.query,
                options: preview.options,
                replacement: preview.replacement,
            });
        }
    }

    pub(super) fn replace_applied(
        &mut self,
        id: u64,
        changed: Vec<PathBuf>,
        skipped: Vec<FileProblem>,
        failed: Vec<FileProblem>,
        fx: &mut Vec<Effect>,
    ) {
        let Some(ReplaceStage::Applying { id: waiting, .. }) = self.search.replace else {
            return;
        };
        if waiting != id {
            return;
        }
        let Some(ReplaceStage::Applying { mut report, .. }) = self.search.replace.take() else {
            return;
        };
        report.changed.extend(changed);
        report.skipped.extend(skipped);
        report.failed.extend(failed);
        self.finish_replace(report, fx);
    }

    pub(super) fn finish_replace(&mut self, mut report: ReplaceReport, fx: &mut Vec<Effect>) {
        report.changed.sort();
        let (changed, skipped, failed) = (
            report.changed.len(),
            report.skipped.len(),
            report.failed.len(),
        );
        let noun = if changed == 1 { "file" } else { "files" };
        let text = format!("Replaced in {changed} {noun}, {skipped} skipped, {failed} failed");
        if skipped + failed > 0 {
            self.error(text);
        } else {
            self.info(text);
        }
        let s = &mut self.search;
        s.results.clear();
        s.selected = None;
        s.searched = None;
        s.truncated = false;
        s.replace = Some(ReplaceStage::Report(report));
        fx.push(Effect::Git(GitEffect::Status {
            root: self.root.clone(),
        }));
    }
}
