use super::{Focus, ToolTab, Workspace};
use crate::core::action::Action;
use crate::core::command::{Command, Effect, Motion};
use crate::core::db::{
    DbConnection, DbEffect, DbPanel, DbPurpose, Safety, classify, normalized, statement_at,
};

fn is_sql_file(path: &std::path::Path) -> bool {
    path.extension()
        .is_some_and(|e| e.eq_ignore_ascii_case("sql"))
}

impl Workspace {
    pub const fn db_panel(&self) -> &DbPanel {
        self.db.panel()
    }

    pub fn configure_databases(&mut self, connections: Vec<DbConnection>) {
        self.db.configure(connections);
    }

    pub(super) fn db_action(&mut self, action: Action, fx: &mut Vec<Effect>) {
        match action {
            Action::DbRun => self.db_run_statement(fx),
            Action::DbStop => match self.db.stop() {
                Some(effect) => fx.push(Effect::Db(effect)),
                None => self.info("No query is running".to_owned()),
            },
            _ => self.db_show(),
        }
    }

    fn db_show(&mut self) {
        self.tool_window_visible = true;
        self.tool_tab = ToolTab::Database;
        self.focus = Focus::Database;
    }

    fn db_sent(&mut self, sent: Vec<DbEffect>, fx: &mut Vec<Effect>) {
        if sent.iter().any(|e| matches!(e, DbEffect::Query { .. })) {
            self.tool_window_visible = true;
            self.tool_tab = ToolTab::Database;
        }
        fx.extend(sent.into_iter().map(Effect::Db));
    }

    fn db_run_statement(&mut self, fx: &mut Vec<Effect>) {
        if self.window {
            return self.info("Queries are run from the main window".to_owned());
        }
        let Some((view, d)) = self.active() else {
            return self.info("Open a .sql file to run a statement".to_owned());
        };
        if !is_sql_file(d.path()) {
            return self.info("Open a .sql file to run a statement".to_owned());
        }
        let (text, cursor, selection) = (
            d.text().to_string(),
            view.cursor.idx,
            view.cursor.selection(),
        );
        let Some(index) = self.db.connection() else {
            self.db_show();
            return self.info(if self.db.panel().connections.is_empty() {
                "No connections: add [[database]] to your config.toml".to_owned()
            } else {
                "Choose a connection first".to_owned()
            });
        };
        let kind = self.db.panel().connections[index].kind();
        let range = selection.or_else(|| statement_at(kind, &text, cursor));
        let chosen = range.map(|r| text.chars().skip(r.start).take(r.len()).collect::<String>());
        let Some(statement) = chosen.and_then(|s| normalized(kind, &s)) else {
            return self.info("No statement under the cursor".to_owned());
        };
        match classify(kind, &statement) {
            Safety::ReadOnly => {
                let sent = self.db.send(index, statement, true, DbPurpose::Grid);
                self.db_sent(sent, fx);
            }
            Safety::Write => self.db.ask(index, statement),
            Safety::Refused(why) => self.error(why),
        }
    }

    pub(super) fn db_confirm_input(&mut self, input: Command, fx: &mut Vec<Effect>) {
        if !self.db.asking() {
            return;
        }
        match input {
            Command::Insert(text) if text == "y" || text == "Y" => {
                let sent = self.db.confirm();
                self.db_sent(sent, fx);
            }
            Command::Move(Motion::Up) => self.db.scroll_confirm(-1),
            Command::Move(Motion::Down) => self.db.scroll_confirm(1),
            _ => {
                self.db.decline();
                self.info("Not run".to_owned());
            }
        }
    }

    pub(super) fn db_pick(&mut self, table: bool, index: usize, fx: &mut Vec<Effect>) {
        let sent = self.db.pick(table, index);
        self.db_sent(sent, fx);
    }

    pub(super) fn db_input(&mut self, input: &Command, fx: &mut Vec<Effect>) {
        let rows = |rows: usize| isize::try_from(rows).unwrap_or(isize::MAX);
        match input {
            Command::Insert(letter) if letter.eq_ignore_ascii_case("s") => {
                self.db_action(Action::DbStop, fx);
            }
            Command::Indent => self.db.next_section(),
            Command::Newline => {
                let sent = self.db.enter();
                self.db_sent(sent, fx);
            }
            Command::Move(Motion::Up) => self.db.step(-1),
            Command::Move(Motion::Down) => self.db.step(1),
            Command::Move(Motion::PageUp { rows: n }) => self.db.step(-rows(*n)),
            Command::Move(Motion::PageDown { rows: n }) => self.db.step(rows(*n)),
            Command::Move(Motion::Left) => self.db.step_column(-1),
            Command::Move(Motion::Right) => self.db.step_column(1),
            _ => {}
        }
    }
}
