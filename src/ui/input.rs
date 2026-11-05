use std::time::{Duration, Instant};

use ratatui::crossterm::event::{
    Event, KeyCode, KeyEvent, KeyEventKind, KeyModifiers, ModifierKeyCode,
};

#[derive(Debug, Default)]
pub struct EscDecoder {
    pending: Vec<Event>,
}

fn plain_key(event: &Event) -> Option<KeyCode> {
    match event {
        Event::Key(k) if k.kind != KeyEventKind::Release && k.modifiers == KeyModifiers::NONE => {
            Some(k.code)
        }
        _ => None,
    }
}

impl EscDecoder {
    pub fn is_pending(&self) -> bool {
        !self.pending.is_empty()
    }

    pub fn push(&mut self, event: Event) -> Vec<Event> {
        let key = plain_key(&event);
        let completes = match &event {
            Event::Key(k)
                if self.pending.len() == 2
                    && k.modifiers - KeyModifiers::SHIFT == KeyModifiers::NONE =>
            {
                match k.code {
                    KeyCode::Char(c) => arrow(c),
                    _ => None,
                }
            }
            _ => None,
        };
        if let Some(code) = completes {
            self.pending.clear();
            return vec![Event::Key(KeyEvent::new(code, KeyModifiers::ALT))];
        }
        match (self.pending.len(), key) {
            (0, Some(KeyCode::Esc)) => {
                self.pending.push(event);
                Vec::new()
            }
            (0, _) => vec![event],
            (1, Some(KeyCode::Char('[' | 'O'))) => {
                self.pending.push(event);
                Vec::new()
            }
            _ => {
                let mut out = std::mem::take(&mut self.pending);
                out.extend(self.push(event));
                out
            }
        }
    }

    pub fn flush(&mut self) -> Vec<Event> {
        std::mem::take(&mut self.pending)
    }
}

const DOUBLE_SHIFT: Duration = Duration::from_millis(400);

#[derive(Debug, Default, Clone)]
pub struct DoubleShift {
    pressed: Option<Instant>,
}

impl DoubleShift {
    pub fn feed(&mut self, event: &Event, now: Instant) -> bool {
        let is_shift = match event {
            Event::Key(k) if k.kind != KeyEventKind::Press => return false,
            Event::Key(k) => matches!(
                k.code,
                KeyCode::Modifier(ModifierKeyCode::LeftShift | ModifierKeyCode::RightShift)
            ),
            Event::Mouse(_) | Event::Paste(_) => false,
            Event::FocusGained | Event::FocusLost | Event::Resize(..) => return false,
        };
        let first = self.pressed.take();
        if !is_shift {
            return false;
        }
        if first.is_some_and(|t| now.duration_since(t) <= DOUBLE_SHIFT) {
            return true;
        }
        self.pressed = Some(now);
        false
    }
}

fn arrow(c: char) -> Option<KeyCode> {
    Some(match c {
        'A' => KeyCode::Up,
        'B' => KeyCode::Down,
        'C' => KeyCode::Right,
        'D' => KeyCode::Left,
        'H' => KeyCode::Home,
        'F' => KeyCode::End,
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn shift_press() -> Event {
        Event::Key(KeyEvent::new(
            KeyCode::Modifier(ModifierKeyCode::LeftShift),
            KeyModifiers::SHIFT,
        ))
    }

    #[test]
    fn two_quick_shifts_make_a_pair_and_a_third_starts_over() {
        let mut d = DoubleShift::default();
        let t = Instant::now();
        assert!(!d.feed(&shift_press(), t));
        assert!(d.feed(&shift_press(), t + Duration::from_millis(100)));
        assert!(!d.feed(&shift_press(), t + Duration::from_millis(200)));
    }

    #[test]
    fn slow_shifts_or_a_key_in_between_are_no_pair() {
        let t = Instant::now();
        let mut d = DoubleShift::default();
        assert!(!d.feed(&shift_press(), t));
        assert!(!d.feed(&shift_press(), t + Duration::from_secs(1)));
        let mut d = DoubleShift::default();
        let letter = Event::Key(KeyEvent::new(KeyCode::Char('A'), KeyModifiers::SHIFT));
        assert!(!d.feed(&shift_press(), t));
        assert!(!d.feed(&letter, t));
        assert!(!d.feed(&shift_press(), t));
    }

    #[test]
    fn releases_do_not_break_a_pair() {
        let t = Instant::now();
        let mut d = DoubleShift::default();
        let mut release = KeyEvent::new(
            KeyCode::Modifier(ModifierKeyCode::LeftShift),
            KeyModifiers::NONE,
        );
        release.kind = KeyEventKind::Release;
        assert!(!d.feed(&shift_press(), t));
        assert!(!d.feed(&Event::Key(release), t));
        assert!(d.feed(&shift_press(), t));
    }

    fn k(code: KeyCode) -> Event {
        Event::Key(KeyEvent::new(code, KeyModifiers::NONE))
    }

    fn feed(d: &mut EscDecoder, events: &[Event]) -> Vec<Event> {
        events.iter().flat_map(|e| d.push(e.clone())).collect()
    }

    fn upper(c: char) -> Event {
        Event::Key(KeyEvent::new(KeyCode::Char(c), KeyModifiers::SHIFT))
    }

    #[test]
    fn meta_prefixed_arrow_as_crossterm_reports_it_becomes_alt_arrow() {
        let mut d = EscDecoder::default();
        for (letter, code) in [
            ('A', KeyCode::Up),
            ('B', KeyCode::Down),
            ('C', KeyCode::Right),
            ('D', KeyCode::Left),
        ] {
            let out = feed(
                &mut d,
                &[k(KeyCode::Esc), k(KeyCode::Char('[')), upper(letter)],
            );
            assert_eq!(out, [Event::Key(KeyEvent::new(code, KeyModifiers::ALT))]);
        }
    }

    #[test]
    fn meta_prefixed_arrow_becomes_alt_arrow() {
        let mut d = EscDecoder::default();
        let out = feed(
            &mut d,
            &[
                k(KeyCode::Esc),
                k(KeyCode::Char('[')),
                k(KeyCode::Char('B')),
            ],
        );
        assert_eq!(
            out,
            [Event::Key(KeyEvent::new(KeyCode::Down, KeyModifiers::ALT))]
        );
        assert!(!d.is_pending());
    }

    #[test]
    fn lone_escape_is_released_on_timeout() {
        let mut d = EscDecoder::default();
        assert!(d.push(k(KeyCode::Esc)).is_empty());
        assert!(d.is_pending());
        assert_eq!(d.flush(), [k(KeyCode::Esc)]);
    }

    #[test]
    fn escape_followed_by_typing_keeps_both() {
        let mut d = EscDecoder::default();
        let out = feed(&mut d, &[k(KeyCode::Esc), k(KeyCode::Char('x'))]);
        assert_eq!(out, [k(KeyCode::Esc), k(KeyCode::Char('x'))]);
    }

    #[test]
    fn non_sequence_after_bracket_is_passed_through() {
        let mut d = EscDecoder::default();
        let out = feed(
            &mut d,
            &[
                k(KeyCode::Esc),
                k(KeyCode::Char('[')),
                k(KeyCode::Char('z')),
            ],
        );
        assert_eq!(
            out,
            [
                k(KeyCode::Esc),
                k(KeyCode::Char('[')),
                k(KeyCode::Char('z'))
            ]
        );
    }

    #[test]
    fn double_escape_holds_the_second_one() {
        let mut d = EscDecoder::default();
        assert_eq!(
            feed(&mut d, &[k(KeyCode::Esc), k(KeyCode::Esc)]),
            [k(KeyCode::Esc)]
        );
        assert!(d.is_pending());
    }

    #[test]
    fn ordinary_keys_pass_straight_through() {
        let mut d = EscDecoder::default();
        assert_eq!(d.push(k(KeyCode::Char('['))), [k(KeyCode::Char('['))]);
    }
}
