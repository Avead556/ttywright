use std::collections::VecDeque;

use super::action::Action;
use super::run::visible;

const MAX_SHOWN: usize = 4;
const MAX_HISTORY: usize = 100;
const MAX_TEXT: usize = 200;
const INFO_TICKS: u8 = 6;
const WARNING_TICKS: u8 = 12;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum NotifyLevel {
    Success,
    Info,
    Warning,
    Error,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Notification {
    pub id: u64,
    pub level: NotifyLevel,
    pub title: String,
    pub text: String,
    pub action: Option<Action>,
    ticks_left: Option<u8>,
}

#[derive(Debug)]
pub struct Notifications {
    shown: Vec<Notification>,
    history: VecDeque<Notification>,
    next_id: u64,
    pub popups: bool,
    pub system: bool,
    pub terminal_in_front: bool,
}

impl Default for Notifications {
    fn default() -> Self {
        Self {
            shown: Vec::new(),
            history: VecDeque::new(),
            next_id: 1,
            popups: true,
            system: true,
            terminal_in_front: true,
        }
    }
}

fn clean(text: &str) -> String {
    visible(text).chars().take(MAX_TEXT).collect()
}

impl Notifications {
    pub fn shown(&self) -> &[Notification] {
        &self.shown
    }

    pub fn history(&self) -> &VecDeque<Notification> {
        &self.history
    }

    pub fn is_ticking(&self) -> bool {
        self.shown.iter().any(|n| n.ticks_left.is_some())
    }

    pub fn push(&mut self, level: NotifyLevel, title: &str, text: &str, action: Option<Action>) {
        let (title, text) = (clean(title), clean(text));
        let ticks_left = match level {
            NotifyLevel::Success | NotifyLevel::Info => Some(INFO_TICKS),
            NotifyLevel::Warning => Some(WARNING_TICKS),
            NotifyLevel::Error => None,
        };
        let notification = Notification {
            id: self.next_id,
            level,
            title,
            text,
            action,
            ticks_left,
        };
        self.next_id += 1;
        let same = |n: &Notification| n.title == notification.title && n.text == notification.text;
        self.history.retain(|n| !same(n));
        self.history.push_front(notification.clone());
        self.history.truncate(MAX_HISTORY);
        if !self.popups {
            return;
        }
        self.shown.retain(|n| !same(n));
        self.shown.push(notification);
        while self.shown.len() > MAX_SHOWN {
            let at = self.shown.iter().position(|n| n.ticks_left.is_some());
            self.shown.remove(at.unwrap_or(0));
        }
    }

    pub fn tick(&mut self) {
        for n in &mut self.shown {
            if let Some(left) = n.ticks_left.as_mut() {
                *left = left.saturating_sub(1);
            }
        }
        self.shown.retain(|n| n.ticks_left != Some(0));
    }

    pub fn close(&mut self, id: u64) -> Option<Option<Action>> {
        let at = self.shown.iter().position(|n| n.id == id)?;
        Some(self.shown.remove(at).action)
    }

    pub fn clear(&mut self) {
        self.shown.clear();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_oldest_fading_one_makes_room_and_errors_stay() {
        let mut n = Notifications::default();
        n.push(NotifyLevel::Error, "e", "1", None);
        for i in 0..5 {
            n.push(NotifyLevel::Info, "i", &i.to_string(), None);
        }
        assert_eq!(n.shown().len(), MAX_SHOWN);
        assert_eq!(n.shown()[0].level, NotifyLevel::Error);
        assert_eq!(n.history().len(), 6);
        assert_eq!(n.history()[0].text, "4", "newest first");
    }

    #[test]
    fn a_repeat_replaces_and_starts_its_time_again() {
        let mut n = Notifications::default();
        n.push(NotifyLevel::Info, "t", "x", None);
        for _ in 0..INFO_TICKS - 1 {
            n.tick();
        }
        n.push(NotifyLevel::Info, "t", "x", None);
        n.tick();
        assert_eq!(n.shown().len(), 1);
        assert_eq!(n.history().len(), 1);
    }

    #[test]
    fn a_hostile_name_cannot_reach_the_screen_as_codes() {
        let mut n = Notifications::default();
        n.push(
            NotifyLevel::Info,
            "\u{1b}]0;x\u{7}",
            &"y".repeat(1000),
            None,
        );
        assert!(!n.shown()[0].title.contains('\u{1b}'));
        assert!(n.shown()[0].text.chars().count() <= MAX_TEXT);
    }

    #[test]
    fn with_popups_off_it_still_goes_in_the_list() {
        let mut n = Notifications {
            popups: false,
            ..Notifications::default()
        };
        n.push(NotifyLevel::Info, "t", "x", None);
        assert!(n.shown().is_empty());
        assert_eq!(n.history().len(), 1);
    }
}
