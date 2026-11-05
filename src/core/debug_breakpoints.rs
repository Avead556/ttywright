use std::path::{Path, PathBuf};

use ropey::Rope;

use super::debug::{Breakpoint, BreakpointState, ConfiguredDebug, DebugState, VarRow};
use super::debug_protocol::DebugReply;
use super::session::SavedBreakpoint;
use super::{Assoc, ChangeSet};

const INLINE_REACH: usize = 30;

fn line_start(text: &Rope, line: usize) -> usize {
    text.line_to_char(line.min(text.len_lines().saturating_sub(1)))
}

fn mentions(text: &str, name: &str) -> bool {
    if name.is_empty() {
        return false;
    }
    let is_word = |c: char| c.is_alphanumeric() || c == '_';
    text.match_indices(name).any(|(at, _)| {
        let before = text[..at].chars().next_back();
        let after = text[at + name.len()..].chars().next();
        !before.is_some_and(is_word) && !after.is_some_and(is_word)
    })
}

impl DebugState {
    pub fn configure(&mut self, entries: Vec<ConfiguredDebug>) {
        self.configured = entries;
    }

    pub fn var_rows(&self) -> Vec<VarRow> {
        self.session
            .as_ref()
            .map(|s| s.var_rows(&self.watches))
            .unwrap_or_default()
    }

    pub fn breakpoint_at(&self, path: &Path, line: usize) -> Option<&Breakpoint> {
        self.breakpoints
            .iter()
            .find(|b| b.line == line && b.path == path)
    }

    fn breakpoint_at_mut(&mut self, path: &Path, line: usize) -> Option<&mut Breakpoint> {
        self.breakpoints
            .iter_mut()
            .find(|b| b.line == line && b.path == path)
    }

    pub fn stopped_line(&self, path: &Path) -> Option<usize> {
        let session = self.session.as_ref().filter(|s| s.is_paused())?;
        let frame = session.frames.get(session.frame)?;
        (frame.path.as_deref() == Some(path)).then_some(frame.line)
    }

    pub fn inline_values(&self, path: &Path, line: usize, text: &str) -> Vec<String> {
        let Some(stop) = self.stopped_line(path).filter(|stop| line <= *stop) else {
            return Vec::new();
        };
        if stop - line > INLINE_REACH {
            return Vec::new();
        }
        let locals = self
            .session
            .as_ref()
            .and_then(|s| s.scopes.first())
            .and_then(|s| s.children.as_ref());
        locals.map_or_else(Vec::new, |locals| {
            locals
                .iter()
                .filter(|v| mentions(text, &v.name))
                .map(|v| format!("{}: {}", v.name, v.value))
                .collect()
        })
    }

    fn breakpoint_add(&mut self, path: PathBuf, line: usize, at: Option<usize>) {
        self.breakpoints.push(Breakpoint::new(path, line, at));
        self.breakpoints
            .sort_by(|a, b| (&a.path, a.line).cmp(&(&b.path, b.line)));
    }

    pub fn breakpoint_flip(
        &mut self,
        path: PathBuf,
        line: usize,
        at: Option<usize>,
        out: &mut Vec<DebugReply>,
    ) {
        match self
            .breakpoints
            .iter()
            .position(|b| b.line == line && b.path == path)
        {
            Some(index) => {
                self.breakpoints.remove(index);
            }
            None => self.breakpoint_add(path.clone(), line, at),
        }
        self.send_breakpoints(&path, out);
    }

    pub fn breakpoint_text(&mut self, path: &Path, line: usize, at: usize, log: bool) -> String {
        if self.breakpoint_at(path, line).is_none() {
            self.breakpoint_add(path.to_path_buf(), line, Some(at));
        }
        self.breakpoint_at(path, line)
            .and_then(|b| {
                if log {
                    b.log_message.clone()
                } else {
                    b.condition.clone()
                }
            })
            .unwrap_or_default()
    }

    pub fn breakpoint_set_text(
        &mut self,
        path: &Path,
        line: usize,
        log: bool,
        text: String,
        out: &mut Vec<DebugReply>,
    ) {
        let value = Some(text).filter(|t| !t.is_empty());
        if let Some(b) = self.breakpoint_at_mut(path, line) {
            if log {
                b.log_message = value;
            } else {
                b.condition = value;
            }
        }
        self.send_breakpoints(path, out);
    }

    pub fn breakpoint_enable(&mut self, path: &Path, line: usize, out: &mut Vec<DebugReply>) {
        let Some(b) = self.breakpoint_at_mut(path, line) else {
            return out.push(DebugReply::Info("No breakpoint on this line".to_owned()));
        };
        b.enabled = !b.enabled;
        if !b.enabled {
            b.state = BreakpointState::Unknown;
        }
        self.send_breakpoints(path, out);
    }

    pub fn breakpoints_clear(&mut self, out: &mut Vec<DebugReply>) {
        let mut paths: Vec<PathBuf> = self.breakpoints.drain(..).map(|b| b.path).collect();
        paths.dedup();
        for path in paths {
            self.send_breakpoints(&path, out);
        }
        out.push(DebugReply::Info("All breakpoints removed".to_owned()));
    }

    pub fn breakpoints_changed(&mut self, path: &Path, text: &Rope, changes: &[ChangeSet]) {
        for b in self.breakpoints.iter_mut().filter(|b| b.path == path) {
            let mut at = b.at.unwrap_or_else(|| line_start(text, b.line));
            for change in changes {
                at = change.map_pos(at, Assoc::After);
            }
            let at = at.min(text.len_chars());
            b.line = text.char_to_line(at);
            b.at = Some(at);
        }
        self.breakpoints
            .dedup_by(|a, b| a.path == b.path && a.line == b.line);
    }

    pub fn breakpoints_sync<'a>(&mut self, open: impl Fn(&Path) -> Option<&'a Rope>) {
        for b in &mut self.breakpoints {
            match (open(&b.path), b.at) {
                (Some(text), None) => b.at = Some(line_start(text, b.line)),
                (None, Some(_)) => b.at = None,
                _ => {}
            }
        }
    }

    pub fn saved_breakpoints(&self) -> Vec<SavedBreakpoint> {
        self.breakpoints
            .iter()
            .map(|b| SavedBreakpoint {
                path: b.path.clone(),
                line: b.line,
                condition: b.condition.clone(),
                log_message: b.log_message.clone(),
                enabled: b.enabled,
            })
            .collect()
    }

    pub fn restore_breakpoints(&mut self, saved: Vec<SavedBreakpoint>) {
        self.breakpoints = saved
            .into_iter()
            .map(|s| Breakpoint {
                condition: s.condition,
                log_message: s.log_message,
                enabled: s.enabled,
                ..Breakpoint::new(s.path, s.line, None)
            })
            .collect();
    }

    pub fn file_saved(&mut self, path: &Path, out: &mut Vec<DebugReply>) {
        if self.breakpoints.iter().any(|b| b.path == path) {
            self.send_breakpoints(path, out);
        }
    }

    pub fn add_watch(&mut self, expression: String, out: &mut Vec<DebugReply>) {
        self.watches.push(expression);
        self.refresh_watches(out);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_breakpoint_flips_and_its_condition_can_be_taken_away() {
        let mut state = DebugState::default();
        let path = PathBuf::from("/p/a.py");
        let mut out = Vec::new();
        state.breakpoint_flip(path.clone(), 3, None, &mut out);
        assert!(state.breakpoint_at(&path, 3).is_some());
        assert!(out.is_empty(), "no session: nothing to tell");
        assert_eq!(state.breakpoint_text(&path, 3, 0, false), "");
        state.breakpoint_set_text(&path, 3, false, "x > 1".to_owned(), &mut out);
        assert_eq!(state.breakpoint_text(&path, 3, 0, false), "x > 1");
        state.breakpoint_set_text(&path, 3, false, String::new(), &mut out);
        assert_eq!(
            state
                .breakpoint_at(&path, 3)
                .and_then(|b| b.condition.clone()),
            None
        );
        state.breakpoint_flip(path.clone(), 3, None, &mut out);
        assert!(state.breakpoints.is_empty());
        state.breakpoint_enable(&path, 3, &mut out);
        assert_eq!(
            out,
            [DebugReply::Info("No breakpoint on this line".to_owned())]
        );
    }

    #[test]
    fn whole_names_only() {
        assert!(mentions("x = total + 1", "total"));
        assert!(!mentions("subtotal = 1", "total"));
        assert!(mentions("$a = 1", "$a"));
        assert!(!mentions("anything", ""));
    }
}
