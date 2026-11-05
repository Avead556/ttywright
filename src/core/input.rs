use super::text::word_motion_len;

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct LineInput {
    text: String,
    cursor: usize,
}

impl LineInput {
    pub fn motion(&mut self, motion: super::command::Motion) {
        use super::command::Motion;
        match motion {
            Motion::Left => self.left(),
            Motion::Right => self.right(),
            Motion::WordLeft => self.word_left(),
            Motion::WordRight => self.word_right(),
            Motion::LineStart | Motion::DocStart => self.home(),
            Motion::LineEnd | Motion::DocEnd => self.end(),
            Motion::Up | Motion::Down | Motion::PageUp { .. } | Motion::PageDown { .. } => {}
        }
    }

    pub fn text(&self) -> &str {
        &self.text
    }

    pub fn cursor(&self) -> usize {
        self.cursor
    }

    pub fn insert(&mut self, s: &str) {
        let clean: String = s.chars().filter(|c| *c != '\n' && *c != '\r').collect();
        let at = self.byte_at(self.cursor);
        self.text.insert_str(at, &clean);
        self.cursor += clean.chars().count();
    }

    pub fn insert_lines(&mut self, s: &str) {
        let clean = s.replace("\r\n", "\n").replace('\r', "\n");
        let at = self.byte_at(self.cursor);
        self.text.insert_str(at, &clean);
        self.cursor += clean.chars().count();
    }

    pub fn delete_backward(&mut self) {
        if self.cursor == 0 {
            return;
        }
        self.cursor -= 1;
        let at = self.byte_at(self.cursor);
        self.text.remove(at);
    }

    pub fn delete_forward(&mut self) {
        if self.cursor < self.text.chars().count() {
            let at = self.byte_at(self.cursor);
            self.text.remove(at);
        }
    }

    pub fn left(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    pub fn right(&mut self) {
        self.cursor = (self.cursor + 1).min(self.text.chars().count());
    }

    pub fn word_left(&mut self) {
        let behind = self.text.chars().count() - self.cursor;
        self.cursor -= word_motion_len(self.text.chars().rev().skip(behind));
    }

    pub fn word_right(&mut self) {
        self.cursor += word_motion_len(self.text.chars().skip(self.cursor));
    }

    pub fn home(&mut self) {
        self.cursor = 0;
    }

    pub fn end(&mut self) {
        self.cursor = self.text.chars().count();
    }

    fn byte_at(&self, char_idx: usize) -> usize {
        self.text
            .char_indices()
            .nth(char_idx)
            .map_or(self.text.len(), |(i, _)| i)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn edits_at_the_cursor_with_multibyte_text() {
        let mut input = LineInput::default();
        input.insert("пр\nиf");
        assert_eq!(input.text(), "приf");
        input.left();
        input.delete_backward();
        assert_eq!((input.text(), input.cursor()), ("прf", 2));
        input.home();
        input.delete_forward();
        input.end();
        input.insert("!");
        assert_eq!(input.text(), "рf!");
    }
}
