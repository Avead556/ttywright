use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

use super::claude::{feed_screen, new_screen, scroll_screen};

pub struct TerminalTab {
    pub id: u32,
    pub title: String,
    pub screen: vt100::Parser,
    started: bool,
    command: Vec<String>,
    project_command: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalEffect {
    Start {
        id: u32,
        root: PathBuf,
        rows: u16,
        columns: u16,
        command: Vec<String>,
        project_command: bool,
    },
    Write {
        id: u32,
        bytes: Vec<u8>,
    },
    Resize {
        id: u32,
        rows: u16,
        columns: u16,
    },
    Close {
        id: u32,
    },
}

impl TerminalEffect {
    pub const fn needs_trust(&self) -> bool {
        match self {
            Self::Start {
                project_command, ..
            } => *project_command,
            Self::Write { .. } | Self::Resize { .. } | Self::Close { .. } => false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum TerminalEvent {
    Output { id: u32, bytes: Vec<u8> },
    Ended { id: u32, failure: Option<String> },
}

pub struct TerminalState {
    tabs: Vec<TerminalTab>,
    active: usize,
    opened: usize,
    size: Option<(u16, u16)>,
    next_id: u32,
}

impl Default for TerminalState {
    fn default() -> Self {
        Self {
            tabs: Vec::new(),
            active: 0,
            opened: 0,
            size: None,
            next_id: 1,
        }
    }
}

impl TerminalState {
    pub fn tabs(&self) -> &[TerminalTab] {
        &self.tabs
    }

    pub const fn active(&self) -> usize {
        self.active
    }

    pub fn is_empty(&self) -> bool {
        self.tabs.is_empty()
    }

    pub fn screen(&self) -> Option<&vt100::Screen> {
        self.tabs.get(self.active).map(|tab| tab.screen.screen())
    }

    pub fn shell_title(&mut self) -> String {
        self.opened += 1;
        match self.opened {
            1 => "Local".to_owned(),
            n => format!("Local ({n})"),
        }
    }

    pub fn open(
        &mut self,
        title: String,
        command: Vec<String>,
        project_command: bool,
        root: &Path,
    ) -> Option<TerminalEffect> {
        let id = self.next_id;
        self.next_id += 1;
        let mut tab = TerminalTab {
            id,
            title,
            screen: new_screen(),
            started: false,
            command,
            project_command,
        };
        let start = self
            .size
            .map(|(rows, columns)| start(&mut tab, rows, columns, root));
        self.tabs.push(tab);
        self.active = self.tabs.len() - 1;
        start
    }

    pub fn sized(&mut self, rows: u16, columns: u16, root: &Path) -> Vec<TerminalEffect> {
        self.size = Some((rows, columns));
        self.tabs
            .iter_mut()
            .map(|tab| {
                if tab.started {
                    tab.screen.screen_mut().set_size(rows, columns);
                    TerminalEffect::Resize {
                        id: tab.id,
                        rows,
                        columns,
                    }
                } else {
                    start(tab, rows, columns, root)
                }
            })
            .collect()
    }

    pub fn typed(&mut self, bytes: Vec<u8>) -> Option<TerminalEffect> {
        let tab = self.tabs.get_mut(self.active)?;
        tab.screen.screen_mut().set_scrollback(0);
        Some(TerminalEffect::Write { id: tab.id, bytes })
    }

    pub fn scrolled(&mut self, delta: isize) {
        if let Some(tab) = self.tabs.get_mut(self.active) {
            scroll_screen(&mut tab.screen, delta);
        }
    }

    pub fn handle(&mut self, event: TerminalEvent) -> TerminalReply {
        match event {
            TerminalEvent::Output { id, bytes } => {
                let Some(tab) = self.tabs.iter_mut().find(|tab| tab.id == id) else {
                    return TerminalReply::default();
                };
                let answer = feed_screen(&mut tab.screen, &bytes);
                TerminalReply {
                    effect: (!answer.is_empty())
                        .then_some(TerminalEffect::Write { id, bytes: answer }),
                    ..TerminalReply::default()
                }
            }
            TerminalEvent::Ended { id, failure } => {
                let index = self.tabs.iter().position(|tab| tab.id == id);
                TerminalReply {
                    effect: index.and_then(|index| self.close(index)),
                    failure,
                }
            }
        }
    }

    pub fn select(&mut self, index: usize) -> bool {
        let found = index < self.tabs.len();
        if found {
            self.active = index;
        }
        found
    }

    pub fn step(&mut self, forward: bool) -> bool {
        let count = self.tabs.len();
        count > 0 && {
            let step = if forward { 1 } else { count - 1 };
            self.select((self.active + step) % count)
        }
    }

    pub fn close(&mut self, index: usize) -> Option<TerminalEffect> {
        if index >= self.tabs.len() {
            return None;
        }
        let tab = self.tabs.remove(index);
        if index < self.active || self.active >= self.tabs.len() {
            self.active = self.active.saturating_sub(1);
        }
        if self.tabs.is_empty() {
            self.opened = 0;
        }
        Some(TerminalEffect::Close { id: tab.id })
    }
}

#[derive(Debug, Default, PartialEq, Eq)]
pub struct TerminalReply {
    pub effect: Option<TerminalEffect>,
    pub failure: Option<String>,
}

fn start(tab: &mut TerminalTab, rows: u16, columns: u16, root: &Path) -> TerminalEffect {
    tab.screen.screen_mut().set_size(rows, columns);
    tab.started = true;
    TerminalEffect::Start {
        id: tab.id,
        root: root.to_path_buf(),
        rows,
        columns,
        command: tab.command.clone(),
        project_command: tab.project_command,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn root() -> PathBuf {
        PathBuf::from("/p")
    }

    #[test]
    fn nothing_starts_before_the_panel_is_measured_and_then_everything_does() {
        let mut state = TerminalState::default();
        let title = state.shell_title();
        assert_eq!(state.open(title, Vec::new(), false, &root()), None);
        let title = state.shell_title();
        assert_eq!(title, "Local (2)");
        state.open(title, Vec::new(), false, &root());
        let fx = state.sized(10, 40, &root());
        assert!(
            fx.iter()
                .all(|e| matches!(e, TerminalEffect::Start { rows: 10, .. })),
            "{fx:?}"
        );
        let fx = state.sized(12, 40, &root());
        assert!(
            fx.iter()
                .all(|e| matches!(e, TerminalEffect::Resize { rows: 12, .. })),
            "{fx:?}"
        );
        let started = state.open("x".to_owned(), vec!["ls".to_owned()], true, &root());
        assert!(started.is_some_and(|e| e.needs_trust()));
    }

    #[test]
    fn a_tab_that_ends_closes_and_the_last_one_resets_the_titles() {
        let mut state = TerminalState::default();
        state.sized(10, 40, &root());
        for _ in 0..2 {
            let title = state.shell_title();
            state.open(title, Vec::new(), false, &root());
        }
        let id = state.tabs()[1].id;
        let reply = state.handle(TerminalEvent::Ended {
            id,
            failure: Some("gone".to_owned()),
        });
        assert_eq!(reply.effect, Some(TerminalEffect::Close { id }));
        assert_eq!(reply.failure.as_deref(), Some("gone"));
        assert_eq!(state.active(), 0);
        assert!(state.close(0).is_some());
        assert!(state.is_empty());
        assert_eq!(state.shell_title(), "Local");
        assert_eq!(state.close(5), None);
        assert!(!state.step(true));
    }
}
