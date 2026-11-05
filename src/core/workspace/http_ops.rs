use super::{Focus, ToolTab, Workspace};
use crate::core::action::Action;
use crate::core::command::{Command, Effect, Motion};
use crate::core::http::{HttpPanel, request_at};

fn is_http_file(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("http") || e.eq_ignore_ascii_case("rest"))
}

impl Workspace {
    pub const fn http_panel(&self) -> &HttpPanel {
        self.http.panel()
    }

    pub(super) fn http_action(&mut self, action: Action, fx: &mut Vec<Effect>) {
        match action {
            Action::HttpRun => self.http_send(fx),
            Action::HttpStop => match self.http.stop() {
                Some(effect) => fx.push(Effect::Http(effect)),
                None => self.info("No request is running".to_owned()),
            },
            _ => {
                self.tool_window_visible = true;
                self.tool_tab = ToolTab::Http;
                self.focus = Focus::Http;
            }
        }
    }

    fn http_send(&mut self, fx: &mut Vec<Effect>) {
        if self.window {
            return self.info("Requests are sent from the main window".to_owned());
        }
        let Some((view, d)) = self.active() else {
            return self.info("Open a .http file to send a request".to_owned());
        };
        if !is_http_file(d.path()) {
            return self.info("Open a .http file to send a request".to_owned());
        }
        let line = d.text().char_to_line(view.cursor.idx);
        let path = d.path().to_path_buf();
        let template = match request_at(&d.text().to_string(), line) {
            Ok(Some(template)) => template,
            Ok(None) => return self.info("No request under the cursor".to_owned()),
            Err(message) => return self.error(message),
        };
        let sent = self.http.send(path, self.root.clone(), template);
        fx.extend(sent.into_iter().map(Effect::Http));
        self.tool_window_visible = true;
        self.tool_tab = ToolTab::Http;
    }

    pub(super) fn http_input(&mut self, input: &Command, fx: &mut Vec<Effect>) {
        match input {
            Command::Insert(letter) if letter.eq_ignore_ascii_case("s") => {
                self.http_action(Action::HttpStop, fx);
            }
            Command::Move(Motion::Up) => self.http.scroll(-1),
            Command::Move(Motion::Down) => self.http.scroll(1),
            Command::Move(Motion::PageUp { rows }) => {
                self.http
                    .scroll(-isize::try_from(*rows).unwrap_or(isize::MAX));
            }
            Command::Move(Motion::PageDown { rows }) => {
                self.http
                    .scroll(isize::try_from(*rows).unwrap_or(isize::MAX));
            }
            _ => {}
        }
    }
}
