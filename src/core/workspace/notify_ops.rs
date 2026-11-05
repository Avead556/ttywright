use super::{Effect, ToolTab, Workspace};
use crate::core::action::Action;
use crate::core::notify::{Notifications, NotifyLevel};
use crate::core::picker::PickerKind;

impl Workspace {
    pub fn notifications(&self) -> &Notifications {
        &self.notifications
    }

    pub fn configure_notifications(&mut self, popups: bool, system: bool) {
        self.notifications.popups = popups;
        self.notifications.system = system;
    }

    pub(super) fn notify(
        &mut self,
        level: NotifyLevel,
        title: &str,
        text: &str,
        action: Option<Action>,
        urgent: bool,
    ) {
        self.notifications.push(level, title, text, action);
        if urgent && self.notifications.system && !self.notifications.terminal_in_front {
            let newest = &self.notifications.history()[0];
            self.notify_outbox.push(Effect::SystemNotify {
                title: newest.title.clone(),
                text: newest.text.clone(),
            });
        }
    }

    pub(super) fn notification_clicked(&mut self, id: u64, fx: &mut Vec<Effect>) {
        let asking = self.is_asking();
        if let Some(Some(action)) = self.notifications.close(id)
            && !asking
        {
            self.run_action(action, fx);
        }
    }

    pub(super) fn notifications_list(&mut self) {
        if self.notifications.history().is_empty() {
            return self.info("No notifications yet".to_owned());
        }
        self.open_picker(PickerKind::Notifications);
    }

    pub(super) fn notification_rows(&self) -> Vec<String> {
        self.notifications
            .history()
            .iter()
            .map(|n| format!("{}  {}", n.title, n.text))
            .collect()
    }

    pub(super) fn notification_chosen(&mut self, index: usize, fx: &mut Vec<Effect>) {
        let action = self
            .notifications
            .history()
            .get(index)
            .and_then(|n| n.action);
        if let Some(action) = action {
            self.run_action(action, fx);
        }
    }

    pub(super) fn notify_run_finished(&mut self, id: u64, name: &str, outcome: &str, ok: bool) {
        let in_sight = self.tool_window_visible
            && matches!(self.tool_tab, ToolTab::Run | ToolTab::Tests)
            && self.notifications.terminal_in_front;
        if in_sight {
            return;
        }
        use crate::core::test_results::TestState;
        let results = (self.tests.run_id == Some(id) && !self.tests.rows.is_empty()).then(|| {
            (
                self.tests.count(TestState::Failed),
                self.tests.count(TestState::Passed),
            )
        });
        let (text, action) = match results {
            Some((failed, passed)) => (
                format!("{failed} failed, {passed} passed"),
                Action::ShowTests,
            ),
            None => (outcome.to_owned(), Action::ShowRun),
        };
        let level = if ok {
            NotifyLevel::Success
        } else {
            NotifyLevel::Error
        };
        self.notify(level, name, &text, Some(action), true);
    }

    pub(super) fn notify_lsp_failed(&mut self, reason: &str) {
        self.notify(
            NotifyLevel::Error,
            "Language server stopped",
            reason,
            Some(Action::ShowLanguageServer),
            false,
        );
    }
}
