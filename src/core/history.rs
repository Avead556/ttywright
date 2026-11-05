use super::change::ChangeSet;

#[derive(Debug, Clone)]
struct Step {
    forward: ChangeSet,
    inverse: ChangeSet,
}

#[derive(Debug, Clone)]
struct Revision {
    parent: usize,
    last_child: Option<usize>,
    steps: Vec<Step>,
    cursor_before: usize,
    cursor_after: usize,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Travel {
    pub changes: Vec<ChangeSet>,
    pub cursor: usize,
}

#[derive(Debug, Clone)]
pub struct History {
    revisions: Vec<Revision>,
    current: usize,
    open_group: bool,
}

impl Default for History {
    fn default() -> Self {
        let root = Revision {
            parent: 0,
            last_child: None,
            steps: Vec::new(),
            cursor_before: 0,
            cursor_after: 0,
        };
        Self {
            revisions: vec![root],
            current: 0,
            open_group: false,
        }
    }
}

impl History {
    pub fn record(
        &mut self,
        forward: ChangeSet,
        inverse: ChangeSet,
        cursors: (usize, usize),
        mergeable: bool,
    ) {
        let step = Step { forward, inverse };
        if mergeable && self.open_group && self.current != 0 {
            let rev = &mut self.revisions[self.current];
            rev.steps.push(step);
            rev.cursor_after = cursors.1;
            return;
        }
        let id = self.revisions.len();
        self.revisions.push(Revision {
            parent: self.current,
            last_child: None,
            steps: vec![step],
            cursor_before: cursors.0,
            cursor_after: cursors.1,
        });
        self.revisions[self.current].last_child = Some(id);
        self.current = id;
        self.open_group = mergeable;
    }

    pub fn close_group(&mut self) {
        self.open_group = false;
    }

    pub fn revision(&self) -> usize {
        self.current
    }

    pub fn undo(&mut self) -> Option<Travel> {
        if self.current == 0 {
            return None;
        }
        self.open_group = false;
        let rev = &self.revisions[self.current];
        let travel = Travel {
            changes: rev.steps.iter().rev().map(|s| s.inverse.clone()).collect(),
            cursor: rev.cursor_before,
        };
        self.current = rev.parent;
        Some(travel)
    }

    pub fn redo(&mut self) -> Option<Travel> {
        let next = self.revisions[self.current].last_child?;
        self.open_group = false;
        self.current = next;
        let rev = &self.revisions[next];
        Some(Travel {
            changes: rev.steps.iter().map(|s| s.forward.clone()).collect(),
            cursor: rev.cursor_after,
        })
    }
}

#[cfg(test)]
mod tests {
    use ropey::Rope;

    use super::*;

    struct Doc {
        text: Rope,
        history: History,
    }

    impl Doc {
        fn new(s: &str) -> Self {
            Self {
                text: Rope::from_str(s),
                history: History::default(),
            }
        }

        fn edit(&mut self, from: usize, to: usize, s: &str, mergeable: bool) {
            let cs = ChangeSet::replace(self.text.len_chars(), from, to, s);
            let inv = cs.invert(&self.text);
            cs.apply(&mut self.text);
            self.history
                .record(cs, inv, (from, from + s.chars().count()), mergeable);
        }

        fn travel(&mut self, t: Option<Travel>) -> Option<usize> {
            let t = t?;
            for cs in &t.changes {
                cs.apply(&mut self.text);
            }
            Some(t.cursor)
        }

        fn undo(&mut self) -> Option<usize> {
            let t = self.history.undo();
            self.travel(t)
        }

        fn redo(&mut self) -> Option<usize> {
            let t = self.history.redo();
            self.travel(t)
        }

        fn s(&self) -> String {
            self.text.to_string()
        }
    }

    #[test]
    fn typing_is_undone_as_one_group() {
        let mut d = Doc::new("");
        d.edit(0, 0, "a", true);
        d.edit(1, 1, "b", true);
        d.edit(2, 2, "c", true);
        assert_eq!(d.undo(), Some(0));
        assert_eq!(d.s(), "");
        assert_eq!(d.redo(), Some(3));
        assert_eq!(d.s(), "abc");
    }

    #[test]
    fn closing_the_group_splits_undo_steps() {
        let mut d = Doc::new("");
        d.edit(0, 0, "a", true);
        d.history.close_group();
        d.edit(1, 1, "b", true);
        d.undo();
        assert_eq!(d.s(), "a");
    }

    #[test]
    fn non_mergeable_edits_are_separate() {
        let mut d = Doc::new("x");
        d.edit(0, 1, "", false);
        d.edit(0, 0, "y", false);
        d.undo();
        assert_eq!(d.s(), "");
        d.undo();
        assert_eq!(d.s(), "x");
        assert_eq!(d.undo(), None);
    }

    #[test]
    fn new_edit_after_undo_branches_and_redo_follows_the_new_branch() {
        let mut d = Doc::new("");
        d.edit(0, 0, "old", false);
        d.undo();
        d.edit(0, 0, "new", false);
        d.undo();
        assert_eq!(d.s(), "");
        d.redo();
        assert_eq!(d.s(), "new");
        assert_eq!(d.redo(), None);
    }
}
