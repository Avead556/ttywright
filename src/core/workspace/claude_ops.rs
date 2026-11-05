use serde_json::{Value, json};

use super::{Focus, ToolTab, Workspace};
use crate::core::claude::{ClaudePanel, ClaudeSend, ClaudeState};
use crate::core::command::Effect;

const MAX_SELECTION_CHARS: usize = 100_000;

fn file_url(path: &std::path::Path) -> String {
    format!("file://{}", path.display())
}

impl Workspace {
    pub fn claude(&self) -> Option<&ClaudePanel> {
        self.claude.as_ref()
    }

    pub fn overlay_open(&self) -> bool {
        self.picker.is_some()
            || self.menu.is_some()
            || self.file_prompt.is_some()
            || self.mcp_review.is_some()
            || self.merge.is_some()
            || self.local_history.is_some()
    }

    fn claude_show(&mut self) {
        self.tool_window_visible = true;
        self.tool_tab = ToolTab::Claude;
        self.focus = Focus::Claude;
    }

    pub(super) fn claude_toggle(&mut self, fx: &mut Vec<Effect>) {
        if self.window {
            return self.info("Claude Code opens in the main window".to_owned());
        }
        if self.focus == Focus::Claude {
            self.focus = Focus::Editor;
            return;
        }
        self.claude_show();
        self.claude_ensure_started(fx);
    }

    fn claude_ensure_started(&mut self, fx: &mut Vec<Effect>) {
        match self.claude.as_mut() {
            Some(panel) if matches!(panel.state, ClaudeState::Running | ClaudeState::Waiting) => {}
            Some(panel) => {
                let size = panel.size;
                let pending = std::mem::take(&mut panel.pending);
                *panel = ClaudePanel::new();
                panel.pending = pending;
                if let Some((rows, columns)) = size {
                    panel.resize(rows, columns);
                    panel.state = ClaudeState::Running;
                    fx.push(Effect::ClaudeStart {
                        root: self.root.clone(),
                        rows,
                        columns,
                    });
                }
            }
            None => self.claude = Some(ClaudePanel::new()),
        }
    }

    pub(super) fn claude_sized(&mut self, rows: u16, columns: u16, fx: &mut Vec<Effect>) {
        let Some(panel) = self.claude.as_mut() else {
            return;
        };
        panel.resize(rows, columns);
        match panel.state {
            ClaudeState::Waiting => {
                panel.state = ClaudeState::Running;
                fx.push(Effect::ClaudeStart {
                    root: self.root.clone(),
                    rows,
                    columns,
                });
            }
            ClaudeState::Running => fx.push(Effect::ClaudeResize { rows, columns }),
            _ => {}
        }
    }

    pub(super) fn claude_typed(&mut self, bytes: Vec<u8>, fx: &mut Vec<Effect>) {
        let Some(panel) = self.claude.as_mut() else {
            return;
        };
        if panel.is_running() {
            panel.screen.screen_mut().set_scrollback(0);
            fx.push(Effect::ClaudeWrite { bytes });
        } else if bytes == b"\r" {
            self.claude_ensure_started(fx);
        }
    }

    pub(super) fn claude_output(&mut self, bytes: &[u8], fx: &mut Vec<Effect>) {
        if let Some(panel) = self.claude.as_mut() {
            let answer = panel.feed(bytes);
            if !answer.is_empty() {
                fx.push(Effect::ClaudeWrite { bytes: answer });
            }
        }
    }

    pub(super) fn claude_ended(&mut self, state: ClaudeState, fx: &mut Vec<Effect>) {
        if let Some(panel) = self.claude.as_mut() {
            panel.state = state;
            panel.connected = false;
        }
        self.ide_disconnected(fx);
    }

    pub(super) fn ide_connected(&mut self, fx: &mut Vec<Effect>) {
        self.ide_clients += 1;
        self.ide_sent = None;
        if let Some(panel) = self.claude.as_mut() {
            panel.connected = true;
            for send in std::mem::take(&mut panel.pending) {
                match send {
                    ClaudeSend::Notify { method, params } => {
                        fx.push(Effect::IdeNotify { method, params });
                    }
                    ClaudeSend::Type(text) => fx.push(Effect::ClaudeWrite {
                        bytes: text.into_bytes(),
                    }),
                }
            }
        }
    }

    pub(super) fn ide_disconnected(&mut self, fx: &mut Vec<Effect>) {
        self.ide_clients = self.ide_clients.saturating_sub(1);
        if self.ide_clients == 0 {
            if let Some(panel) = self.claude.as_mut() {
                panel.connected = false;
            }
            self.mcp_close_diffs(None, fx);
        }
    }

    fn claude_send(&mut self, send: ClaudeSend, fx: &mut Vec<Effect>) {
        self.claude_show();
        self.claude_ensure_started(fx);
        let Some(panel) = self.claude.as_mut() else {
            return;
        };
        if !panel.connected {
            panel.pending.push(send);
            return;
        }
        match send {
            ClaudeSend::Notify { method, params } => fx.push(Effect::IdeNotify { method, params }),
            ClaudeSend::Type(text) => fx.push(Effect::ClaudeWrite {
                bytes: text.into_bytes(),
            }),
        }
    }

    pub(super) fn claude_mention(&mut self, fx: &mut Vec<Effect>) {
        let Some((view, d)) = self.active() else {
            return self.info("Open a file first".to_owned());
        };
        let text = d.text();
        let mut params = json!({ "filePath": d.path() });
        if let Some(span) = view.cursor.selection() {
            let last = span.end.saturating_sub(1).max(span.start);
            params["lineStart"] = json!(text.char_to_line(span.start));
            params["lineEnd"] = json!(text.char_to_line(last));
        }
        let method = "at_mentioned".to_owned();
        self.claude_send(ClaudeSend::Notify { method, params }, fx);
    }

    pub(super) fn claude_fix(&mut self, fx: &mut Vec<Effect>) {
        let Some((view, d)) = self.active() else {
            return self.info("Open a file first".to_owned());
        };
        let line = u32::try_from(d.text().char_to_line(view.cursor.idx)).unwrap_or(u32::MAX);
        let items = self.diagnostics(d.path());
        let Some(item) = items
            .iter()
            .find(|i| i.raw.start.0 == line)
            .or_else(|| items.first())
        else {
            return self.info("No problems in this file".to_owned());
        };
        let name = self.display_path(d.path()).display().to_string();
        let message = item
            .raw
            .message
            .split_whitespace()
            .collect::<Vec<_>>()
            .join(" ");
        let text = format!(
            "Fix this problem in {name}, line {}: {message}",
            item.raw.start.0 + 1
        );
        self.claude_send(ClaudeSend::Type(text), fx);
    }

    pub(super) fn ide_sync(&mut self, fx: &mut Vec<Effect>) {
        if self.ide_clients == 0 {
            return;
        }
        let Some((view, d)) = self.active() else {
            return;
        };
        let span = view.cursor.selection();
        let key = (
            d.path().to_path_buf(),
            view.cursor.idx,
            span.clone(),
            d.version(),
        );
        if self.ide_sent.as_ref() == Some(&key) {
            return;
        }
        let text = d.text();
        let place = |at: usize| {
            let line = text.char_to_line(at);
            json!({ "line": line, "character": at - text.line_to_char(line) })
        };
        let (start, end, selected): (Value, Value, String) = match &span {
            Some(s) => (
                place(s.start),
                place(s.end),
                text.slice(s.start..s.end.min(s.start + MAX_SELECTION_CHARS))
                    .to_string(),
            ),
            None => (
                place(view.cursor.idx),
                place(view.cursor.idx),
                String::new(),
            ),
        };
        fx.push(Effect::IdeNotify {
            method: "selection_changed".to_owned(),
            params: json!({
                "text": selected,
                "filePath": d.path(),
                "fileUrl": file_url(d.path()),
                "selection": {
                    "start": start,
                    "end": end,
                    "isEmpty": span.is_none(),
                },
            }),
        });
        self.ide_sent = Some(key);
    }
}
