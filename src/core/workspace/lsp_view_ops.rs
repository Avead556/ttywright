use super::{
    Command, Cursor, Diagnostic, Effect, HOVER_PAGE, Hover, HoverLine, LspLocation, MAX_JUMPS_BACK,
    Motion, PathBuf, PositionEncoding, RawDiagnostic, ServerKey, Workspace, char_at, hover_lines,
};

impl Workspace {
    pub(super) fn show_hover(&mut self, markdown: Option<&str>) {
        let Some((view, d)) = self.active() else {
            return;
        };
        let idx = view.cursor.idx;
        let mut lines: Vec<HoverLine> = self
            .diagnostics(d.path())
            .iter()
            .filter(|p| {
                p.chars
                    .as_ref()
                    .is_some_and(|c| c.start <= idx && idx <= c.end)
            })
            .flat_map(|p| p.raw.message.lines())
            .map(|text| HoverLine {
                text: text.to_owned(),
                code: false,
            })
            .collect();
        let docs = hover_lines(markdown.unwrap_or_default());
        if !lines.is_empty() && !docs.is_empty() {
            lines.push(HoverLine {
                text: String::new(),
                code: false,
            });
        }
        lines.extend(docs);
        if lines.is_empty() {
            return self.info("No documentation found".to_owned());
        }
        self.hover = Some(Hover { lines, scroll: 0 });
    }

    pub(super) fn hover_input(&mut self, command: &Command) -> bool {
        let Some(hover) = self.hover.as_mut() else {
            return false;
        };
        let last = hover.lines.len() - 1;
        match command {
            Command::Move(Motion::Down) => hover.scroll = (hover.scroll + 1).min(last),
            Command::Move(Motion::Up) => hover.scroll = hover.scroll.saturating_sub(1),
            Command::Move(Motion::PageDown { rows }) => {
                hover.scroll = (hover.scroll + (*rows).min(HOVER_PAGE)).min(last);
            }
            Command::Move(Motion::PageUp { rows }) => {
                hover.scroll = hover.scroll.saturating_sub((*rows).min(HOVER_PAGE));
            }
            Command::Escape => self.hover = None,
            _ => {
                self.hover = None;
                return false;
            }
        }
        true
    }

    pub(super) fn jump_to(&mut self, location: LspLocation, fx: &mut Vec<Effect>) {
        if let Some((view, d)) = self.active() {
            let t = d.text();
            let line = t.char_to_line(view.cursor.idx);
            let col = view.cursor.idx - t.line_to_char(line);
            let start = (d.path().to_path_buf(), line, col);
            self.jumps_back.push(start);
            if self.jumps_back.len() > MAX_JUMPS_BACK {
                self.jumps_back.remove(0);
            }
        }
        self.open_file(location.path.clone(), None, fx);
        let shown = self
            .active()
            .is_some_and(|(_, d)| d.path() == location.path);
        if shown {
            self.place_cursor(&location);
        } else {
            self.pending_jump = Some(location);
        }
    }

    pub(super) fn place_cursor(&mut self, location: &LspLocation) {
        if let Some((_, d)) = self.active().filter(|(_, d)| d.path() == location.path) {
            let idx = char_at(d.text(), location.start, self.answer_encoding);
            self.set_active_cursor(Cursor::at(idx));
        }
    }

    pub(super) fn set_diagnostics(
        &mut self,
        server: &ServerKey,
        path: PathBuf,
        items: Vec<RawDiagnostic>,
    ) {
        let encoding = self
            .lsp
            .servers
            .get(server)
            .map_or(PositionEncoding::Utf16, |s| s.encoding);
        if items.is_empty() {
            self.diagnostics.remove(&path);
        } else {
            let items = items
                .into_iter()
                .map(|raw| Diagnostic {
                    raw,
                    server: server.clone(),
                    encoding,
                    chars: None,
                })
                .collect();
            self.diagnostics.insert(path, items);
        }
        let count = self.problems().len();
        self.problem_selected = self
            .problem_selected
            .filter(|_| count > 0)
            .map(|i| i.min(count - 1));
    }

    pub(super) fn problems_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        let problems = self.problems();
        if problems.is_empty() {
            self.problem_selected = None;
            return;
        }
        let last = problems.len() - 1;
        match input {
            Command::Move(Motion::Down) => {
                self.problem_selected =
                    Some(self.problem_selected.map_or(0, |i| (i + 1).min(last)));
            }
            Command::Move(Motion::Up) => {
                self.problem_selected =
                    Some(self.problem_selected.map_or(0, |i| i.saturating_sub(1)));
            }
            Command::Move(Motion::PageDown { rows }) => {
                self.problem_selected =
                    Some(self.problem_selected.map_or(0, |i| (i + rows).min(last)));
            }
            Command::Move(Motion::PageUp { rows }) => {
                self.problem_selected =
                    Some(self.problem_selected.map_or(0, |i| i.saturating_sub(rows)));
            }
            Command::Newline => {
                let index = self.problem_selected.unwrap_or(0).min(last);
                let problem = &problems[index];
                self.open_file(problem.path.clone(), Some((problem.line, problem.col)), fx);
            }
            _ => {}
        }
    }
}
