use ropey::Rope;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum Operation {
    Retain(usize),
    Delete(usize),
    Insert(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Assoc {
    Before,
    After,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChangeSet {
    ops: Vec<Operation>,
    len_before: usize,
    len_after: usize,
}

impl ChangeSet {
    pub fn replace(len: usize, from: usize, to: usize, text: &str) -> Self {
        let mut cs = Self {
            ops: Vec::new(),
            len_before: len,
            len_after: len,
        };
        cs.retain(from);
        cs.delete(to - from);
        cs.insert(text);
        cs.retain(len - to);
        cs
    }

    pub fn ops(&self) -> &[Operation] {
        &self.ops
    }

    pub fn fits(&self, len: usize) -> bool {
        let consumed: usize = self
            .ops
            .iter()
            .map(|op| match op {
                Operation::Retain(n) | Operation::Delete(n) => *n,
                Operation::Insert(_) => 0,
            })
            .sum();
        consumed == len && self.len_before == len
    }

    pub fn is_empty(&self) -> bool {
        self.ops.iter().all(|op| matches!(op, Operation::Retain(_)))
    }

    fn retain(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        if let Some(Operation::Retain(last)) = self.ops.last_mut() {
            *last += n;
        } else {
            self.ops.push(Operation::Retain(n));
        }
    }

    fn delete(&mut self, n: usize) {
        if n == 0 {
            return;
        }
        self.len_after -= n;
        if let Some(Operation::Delete(last)) = self.ops.last_mut() {
            *last += n;
        } else {
            self.ops.push(Operation::Delete(n));
        }
    }

    fn insert(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        self.len_after += s.chars().count();
        if let Some(Operation::Insert(last)) = self.ops.last_mut() {
            last.push_str(s);
        } else {
            self.ops.push(Operation::Insert(s.to_owned()));
        }
    }

    pub fn apply(&self, text: &mut Rope) {
        debug_assert_eq!(text.len_chars(), self.len_before);
        let mut pos = 0;
        for op in &self.ops {
            match op {
                Operation::Retain(n) => pos += n,
                Operation::Delete(n) => text.remove(pos..pos + n),
                Operation::Insert(s) => {
                    text.insert(pos, s);
                    pos += s.chars().count();
                }
            }
        }
    }

    pub fn invert(&self, original: &Rope) -> Self {
        let mut inv = Self {
            ops: Vec::new(),
            len_before: self.len_after,
            len_after: self.len_after,
        };
        let mut pos = 0;
        for op in &self.ops {
            match op {
                Operation::Retain(n) => {
                    inv.retain(*n);
                    pos += n;
                }
                Operation::Delete(n) => {
                    inv.insert(&original.slice(pos..pos + n).to_string());
                    pos += n;
                }
                Operation::Insert(s) => inv.delete(s.chars().count()),
            }
        }
        inv
    }

    pub fn map_pos(&self, pos: usize, assoc: Assoc) -> usize {
        let mut old = 0;
        let mut new = 0;
        for op in &self.ops {
            match op {
                Operation::Retain(n) => {
                    if pos < old + n {
                        return new + (pos - old);
                    }
                    old += n;
                    new += n;
                }
                Operation::Delete(n) => {
                    if pos < old + n {
                        return new;
                    }
                    old += n;
                }
                Operation::Insert(s) => {
                    if pos == old && assoc == Assoc::Before {
                        return new;
                    }
                    new += s.chars().count();
                }
            }
        }
        new + pos.saturating_sub(old)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn applied(text: &str, cs: &ChangeSet) -> String {
        let mut r = Rope::from_str(text);
        cs.apply(&mut r);
        r.to_string()
    }

    #[test]
    fn replace_applies_and_inverts() {
        let original = Rope::from_str("hello world");
        let cs = ChangeSet::replace(11, 6, 11, "there");
        assert_eq!(applied("hello world", &cs), "hello there");
        let inv = cs.invert(&original);
        assert_eq!(applied("hello there", &inv), "hello world");
    }

    #[test]
    fn invert_restores_for_many_edits() {
        let text = "a\r\nбв界😀x";
        let len = text.chars().count();
        for from in 0..=len {
            for to in from..=len {
                for insert in ["", "Z", "\n", "é😀"] {
                    let original = Rope::from_str(text);
                    let cs = ChangeSet::replace(len, from, to, insert);
                    let after = applied(text, &cs);
                    assert_eq!(applied(&after, &cs.invert(&original)), text);
                }
            }
        }
    }

    #[test]
    fn map_pos_shifts_positions_after_the_edit() {
        let cs = ChangeSet::replace(10, 2, 4, "xyz");
        assert_eq!(cs.map_pos(1, Assoc::After), 1);
        assert_eq!(
            cs.map_pos(3, Assoc::After),
            2,
            "inside a deletion collapses to its start"
        );
        assert_eq!(cs.map_pos(4, Assoc::After), 5);
        assert_eq!(cs.map_pos(10, Assoc::After), 11);
    }

    #[test]
    fn map_pos_at_insertion_point_respects_assoc() {
        let cs = ChangeSet::replace(5, 2, 2, "ab");
        assert_eq!(cs.map_pos(2, Assoc::Before), 2);
        assert_eq!(cs.map_pos(2, Assoc::After), 4);
    }
}
