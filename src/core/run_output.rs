use std::collections::VecDeque;
use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::run::{RunTarget, is_unprintable};

pub const MAX_LINE_BYTES: usize = 16 * 1024;
pub const MAX_LINES: usize = 50_000;
pub const MAX_TOTAL_BYTES: usize = 32 * 1024 * 1024;
const MAX_STRING_BYTES: usize = 4096;
const TRUNCATED: &str = " … (line truncated)";

const MAX_STYLE_CHANGES: usize = 256;
const MAX_CSI_BYTES: usize = 64;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunColor {
    Indexed(u8),
    Rgb(u8, u8, u8),
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunStyle {
    pub fg: Option<RunColor>,
    pub bg: Option<RunColor>,
    pub bold: bool,
    pub dim: bool,
    pub italic: bool,
    pub underline: bool,
    pub reverse: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSpan {
    pub start: usize,
    pub end: usize,
    pub style: RunStyle,
}

fn extended_color(numbers: &mut impl Iterator<Item = u32>) -> Option<RunColor> {
    let byte = |n: u32| u8::try_from(n).ok();
    match numbers.next()? {
        5 => Some(RunColor::Indexed(byte(numbers.next()?)?)),
        2 => Some(RunColor::Rgb(
            byte(numbers.next()?)?,
            byte(numbers.next()?)?,
            byte(numbers.next()?)?,
        )),
        _ => None,
    }
}

impl RunStyle {
    fn apply(&mut self, params: &[u8]) {
        let text = String::from_utf8_lossy(params);
        let mut numbers = text
            .split(';')
            .map(|part| part.split(':').next().unwrap_or("").parse().unwrap_or(0));
        while let Some(code) = numbers.next() {
            let low = |base: u32| u8::try_from(code - base).ok().map(RunColor::Indexed);
            let high = |base: u32| u8::try_from(code - base + 8).ok().map(RunColor::Indexed);
            match code {
                0 => *self = Self::default(),
                1 => self.bold = true,
                2 => self.dim = true,
                3 => self.italic = true,
                4 => self.underline = true,
                7 => self.reverse = true,
                22 => (self.bold, self.dim) = (false, false),
                23 => self.italic = false,
                24 => self.underline = false,
                27 => self.reverse = false,
                30..=37 => self.fg = low(30),
                38 => self.fg = extended_color(&mut numbers),
                39 => self.fg = None,
                40..=47 => self.bg = low(40),
                48 => self.bg = extended_color(&mut numbers),
                49 => self.bg = None,
                90..=97 => self.fg = high(90),
                100..=107 => self.bg = high(100),
                _ => {}
            }
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
enum Escape {
    #[default]
    Ground,
    Start,
    Intermediate,
    Csi,
    Text {
        seen: usize,
    },
    TextEnd {
        seen: usize,
    },
}

#[derive(Debug, Default)]
pub struct OutputDecoder {
    state: Escape,
    line: Vec<u8>,
    overflowed: bool,
    returned: bool,
    csi: Vec<u8>,
    style: RunStyle,
    marks: Vec<(usize, RunStyle)>,
}

impl OutputDecoder {
    #[cfg(test)]
    pub fn feed(&mut self, bytes: &[u8], mut done: impl FnMut(String)) {
        self.feed_styled(bytes, |text, _| done(text));
    }

    pub fn feed_styled(&mut self, bytes: &[u8], mut done: impl FnMut(String, Vec<RunSpan>)) {
        for &byte in bytes {
            self.state = match self.state {
                Escape::Ground => {
                    self.ground(byte, &mut done);
                    if byte == 0x1b {
                        Escape::Start
                    } else {
                        Escape::Ground
                    }
                }
                Escape::Start => match byte {
                    b'[' => {
                        self.csi.clear();
                        Escape::Csi
                    }
                    b']' | b'P' | b'X' | b'^' | b'_' => Escape::Text { seen: 0 },
                    0x20..=0x2f => Escape::Intermediate,
                    0x1b => Escape::Start,
                    _ => Escape::Ground,
                },
                Escape::Intermediate => match byte {
                    0x20..=0x2f => Escape::Intermediate,
                    0x1b => Escape::Start,
                    _ => Escape::Ground,
                },
                Escape::Csi => match byte {
                    0x20..=0x3f => {
                        if self.csi.len() < MAX_CSI_BYTES {
                            self.csi.push(byte);
                        }
                        Escape::Csi
                    }
                    0x1b => Escape::Start,
                    b'm' => {
                        self.style_changed();
                        Escape::Ground
                    }
                    _ => Escape::Ground,
                },
                Escape::Text { seen } => match byte {
                    0x07 | 0x18 | 0x1a => Escape::Ground,
                    0x1b => Escape::TextEnd { seen },
                    _ if seen >= MAX_STRING_BYTES => Escape::Ground,
                    _ => Escape::Text { seen: seen + 1 },
                },
                Escape::TextEnd { seen } => match byte {
                    b'\\' | 0x18 | 0x1a => Escape::Ground,
                    0x1b => Escape::TextEnd { seen },
                    _ => Escape::Text { seen: seen + 1 },
                },
            };
        }
    }

    fn style_changed(&mut self) {
        if !self
            .csi
            .iter()
            .all(|b| b.is_ascii_digit() || matches!(b, b';' | b':'))
        {
            return;
        }
        let params = std::mem::take(&mut self.csi);
        self.style.apply(&params);
        self.mark();
    }

    fn mark(&mut self) {
        let at = self.line.len();
        if let Some(last) = self.marks.last_mut().filter(|last| last.0 == at) {
            last.1 = self.style;
        } else if self.marks.len() < MAX_STYLE_CHANGES {
            self.marks.push((at, self.style));
        }
    }

    fn ground(&mut self, byte: u8, done: &mut impl FnMut(String, Vec<RunSpan>)) {
        match byte {
            b'\n' => {
                self.returned = false;
                let (text, spans) = self.take();
                done(text, spans);
            }
            b'\r' => self.returned = true,
            0x08 => self.backspace(),
            b'\t' => self.push(byte),
            0x00..=0x1f | 0x7f => {}
            _ => self.push(byte),
        }
    }

    fn push(&mut self, byte: u8) {
        if self.returned {
            self.returned = false;
            self.line.clear();
            self.overflowed = false;
            self.marks.clear();
            self.mark();
        }
        if self.line.len() < MAX_LINE_BYTES {
            self.line.push(byte);
        } else {
            self.overflowed = true;
        }
    }

    fn backspace(&mut self) {
        while let Some(byte) = self.line.pop() {
            if byte & 0xc0 != 0x80 {
                break;
            }
        }
        let end = self.line.len();
        self.marks.retain(|mark| mark.0 <= end);
    }

    fn take(&mut self) -> (String, Vec<RunSpan>) {
        let done = self.decoded();
        self.line.clear();
        self.overflowed = false;
        self.marks.clear();
        self.mark();
        done
    }

    fn decoded(&self) -> (String, Vec<RunSpan>) {
        let mut text = String::new();
        let mut spans = Vec::new();
        let mut chars = 0;
        let mut piece = |bytes: &[u8], style: RunStyle| {
            let start = chars;
            for c in String::from_utf8_lossy(bytes).chars() {
                if c == '\t' || !is_unprintable(c) {
                    text.push(c);
                    chars += 1;
                }
            }
            if style != RunStyle::default() && chars > start {
                spans.push(RunSpan {
                    start,
                    end: chars,
                    style,
                });
            }
        };
        let first = self.marks.first().map_or(self.line.len(), |mark| mark.0);
        piece(
            &self.line[..first.min(self.line.len())],
            RunStyle::default(),
        );
        for (n, (at, style)) in self.marks.iter().enumerate() {
            let end = self.marks.get(n + 1).map_or(self.line.len(), |next| next.0);
            piece(
                &self.line[(*at).min(self.line.len())..end.min(self.line.len())],
                *style,
            );
        }
        if self.overflowed {
            text.push_str(TRUNCATED);
        }
        (text, spans)
    }

    pub fn partial(&self) -> String {
        self.decoded().0
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLocation {
    pub path: PathBuf,
    pub line: usize,
    pub col: usize,
    pub span: (usize, usize),
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunLine {
    pub text: String,
    pub location: Option<RunLocation>,
    pub spans: Vec<RunSpan>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunOutcome {
    Exited(i32),
    Signalled(i32),
    Stopped,
    FailedToStart(String),
}

impl RunOutcome {
    pub fn is_success(&self) -> bool {
        *self == Self::Exited(0)
    }
}

impl std::fmt::Display for RunOutcome {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Exited(code) => write!(f, "Exit code {code}"),
            Self::Signalled(signal) => write!(f, "Killed by signal {signal}"),
            Self::Stopped => write!(f, "Stopped"),
            Self::FailedToStart(reason) => write!(f, "Could not start: {reason}"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunStatus {
    Running,
    Stopping,
    Finished {
        outcome: RunOutcome,
        duration_ms: u64,
    },
}

#[derive(Debug, Clone)]
pub struct RunPanel {
    pub id: u64,
    pub target: RunTarget,
    pub status: RunStatus,
    lines: VecDeque<RunLine>,
    first: usize,
    bytes: usize,
    pub partial: String,
    selected: Option<usize>,
    pub left: usize,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum RunStreamMessage {
    Snapshot(Option<RunSnapshot>),
    Started {
        id: u64,
        target: RunTarget,
    },
    Output {
        id: u64,
        lines: Vec<RunLine>,
        partial: String,
        dropped: usize,
    },
    Finished {
        id: u64,
        outcome: RunOutcome,
        duration_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSnapshot {
    pub id: u64,
    pub target: RunTarget,
    pub status: RunStatus,
    pub first: usize,
    pub lines: Vec<RunLine>,
    pub partial: String,
}

impl RunPanel {
    pub fn snapshot(&self) -> RunSnapshot {
        RunSnapshot {
            id: self.id,
            target: self.target.clone(),
            status: self.status.clone(),
            first: self.first,
            lines: self.lines.iter().cloned().collect(),
            partial: self.partial.clone(),
        }
    }

    pub fn from_snapshot(snapshot: RunSnapshot) -> Self {
        let mut panel = Self::new(snapshot.id, snapshot.target);
        panel.status = snapshot.status;
        panel.push(snapshot.lines, snapshot.partial, snapshot.first);
        panel
    }

    pub fn new(id: u64, target: RunTarget) -> Self {
        Self {
            id,
            target,
            status: RunStatus::Running,
            lines: VecDeque::new(),
            first: 0,
            bytes: 0,
            partial: String::new(),
            selected: None,
            left: 0,
        }
    }

    pub fn is_active(&self) -> bool {
        !matches!(self.status, RunStatus::Finished { .. })
    }

    pub fn push(&mut self, lines: Vec<RunLine>, partial: String, dropped: usize) {
        if dropped > 0 {
            self.first += self.lines.len() + dropped;
            self.lines.clear();
            self.bytes = 0;
        }
        for line in lines {
            self.bytes += line.text.len();
            self.lines.push_back(line);
        }
        while self.lines.len() > MAX_LINES || self.bytes > MAX_TOTAL_BYTES {
            let Some(old) = self.lines.pop_front() else {
                break;
            };
            self.bytes -= old.text.len();
            self.first += 1;
        }
        self.partial = partial;
        if let Some(selected) = &mut self.selected {
            *selected = (*selected).max(self.first);
        }
    }

    pub fn trimmed(&self) -> usize {
        self.first
    }

    pub fn first_line(&self) -> usize {
        self.first
    }

    pub fn end_line(&self) -> usize {
        self.first + self.lines.len()
    }

    pub fn line(&self, number: usize) -> Option<&RunLine> {
        self.lines.get(number.checked_sub(self.first)?)
    }

    pub fn selected(&self) -> Option<usize> {
        self.selected
    }

    pub fn select_by(&mut self, delta: isize) {
        let end = self.end_line();
        if end == self.first {
            self.selected = None;
            return;
        }
        let from = self.selected.unwrap_or(end);
        let to = from.saturating_add_signed(delta);
        self.selected = if to >= end {
            None
        } else {
            Some(to.max(self.first))
        };
    }

    pub fn select_first(&mut self) {
        self.selected = (self.end_line() > self.first).then_some(self.first);
    }

    pub fn follow_end(&mut self) {
        self.selected = None;
    }

    pub fn select_line(&mut self, number: usize) {
        if self.line(number).is_some() {
            self.selected = Some(number);
        }
    }

    pub fn next_location(&self, forward: bool) -> Option<usize> {
        let has = |n: &usize| self.line(*n).is_some_and(|l| l.location.is_some());
        if forward {
            let start = self.selected.map_or(self.first, |s| s + 1);
            (start..self.end_line()).find(has)
        } else {
            let end = self.selected.unwrap_or(self.end_line());
            (self.first..end).rev().find(has)
        }
    }
}

#[cfg(test)]
mod tests {
    use std::path::Path;

    use super::*;
    use crate::core::run::{ConfiguredRun, RunSource};

    fn decode(chunks: &[&[u8]]) -> (Vec<String>, String) {
        let mut decoder = OutputDecoder::default();
        let mut lines = Vec::new();
        for chunk in chunks {
            decoder.feed(chunk, |l| lines.push(l));
        }
        (lines, decoder.partial())
    }

    fn one(bytes: &[u8]) -> String {
        let (lines, partial) = decode(&[bytes]);
        assert!(lines.is_empty(), "{lines:?}");
        partial
    }

    #[test]
    fn plain_lines_and_a_partial_one() {
        let (lines, partial) = decode(&[b"one\ntw", b"o\nthr"]);
        assert_eq!(lines, ["one", "two"]);
        assert_eq!(partial, "thr");
    }

    #[test]
    fn escape_sequences_are_removed_in_any_chunking() {
        let input: &[u8] =
            b"\x1b[31mred\x1b[0m \x1b]0;title\x07\x1b]8;;http://x\x1b\\link\x1b]8;;\x1b\\ \x1b(Bb\x1bPdcs\x1b\\\x1b_apc\x1b\\\x1b[2J\x1b[?1049hc\x1b=d";
        let expected = "red link bcd";
        assert_eq!(one(input), expected);
        for at in 0..input.len() {
            let (lines, partial) = decode(&[&input[..at], &input[at..]]);
            assert!(lines.is_empty());
            assert_eq!(partial, expected, "split at {at}");
        }
        let mut decoder = OutputDecoder::default();
        for byte in input {
            decoder.feed(&[*byte], |_| panic!("no newline in the input"));
        }
        assert_eq!(decoder.partial(), expected);
    }

    #[test]
    fn nothing_harmful_survives_garbage() {
        let mut bytes: Vec<u8> = (0..=255).collect();
        bytes.extend((0..=255).rev());
        bytes.extend((0..=255u8).flat_map(|b| [0x1b, b]));
        bytes.extend((0..=255u8).flat_map(|b| [0xc2, b]));
        bytes.extend((0..=255u8).flat_map(|b| [0x1b, b'[', b]));
        let mut decoder = OutputDecoder::default();
        let mut lines = Vec::new();
        decoder.feed(&bytes, |l| lines.push(l));
        lines.push(decoder.partial());
        assert!(!lines.is_empty());
        for line in &lines {
            for c in line.chars() {
                assert!(c == '\t' || !is_unprintable(c), "{c:?} in {line:?}");
            }
        }
    }

    #[test]
    fn c1_controls_and_deceptive_characters_are_dropped() {
        assert_eq!(one("a\u{9b}2Jb".as_bytes()), "a2Jb");
        assert_eq!(one("a\u{202e}b\u{200b}c".as_bytes()), "abc");
        assert_eq!(one("emoji\u{200d}joined".as_bytes()), "emoji\u{200d}joined");
    }

    #[test]
    fn invalid_utf8_becomes_a_replacement_not_a_panic() {
        assert_eq!(one(b"a\xff\xfeb"), "a\u{fffd}\u{fffd}b");
        let snowman = "☃".as_bytes();
        let (_, partial) = decode(&[&snowman[..1], &snowman[1..]]);
        assert_eq!(partial, "☃");
    }

    #[test]
    fn carriage_returns_redraw_and_crlf_ends_a_line() {
        assert_eq!(one(b"10%\r20%\r30%"), "30%");
        let (lines, partial) = decode(&[b"a\r\nb\r", b"\nc"]);
        assert_eq!(lines, ["a", "b"]);
        assert_eq!(partial, "c");
        assert_eq!(one(b"done\r"), "done");
    }

    #[test]
    fn backspace_removes_a_whole_character() {
        assert_eq!(one("aж\x08b".as_bytes()), "ab");
        assert_eq!(one(b"\x08\x08a"), "a");
    }

    #[test]
    fn tabs_are_kept_for_drawing() {
        assert_eq!(one(b"a\tb"), "a\tb");
    }

    #[test]
    fn a_huge_line_is_cut_and_says_so() {
        let huge = vec![b'x'; MAX_LINE_BYTES * 3];
        let partial = one(&huge);
        assert!(partial.ends_with(TRUNCATED));
        assert!(partial.len() <= MAX_LINE_BYTES + TRUNCATED.len());
        let (lines, partial) = decode(&[&huge, b"\nnext\n"]);
        assert_eq!(lines.len(), 2);
        assert_eq!(lines[1], "next");
        assert!(partial.is_empty());
        let cyrillic = "ж".repeat(MAX_LINE_BYTES);
        assert!(one(cyrillic.as_bytes()).ends_with(TRUNCATED));
    }

    #[test]
    fn an_unterminated_string_does_not_swallow_everything() {
        let mut bytes = b"\x1b]0;".to_vec();
        bytes.extend(vec![b'x'; MAX_STRING_BYTES + 10]);
        bytes.extend(b"after");
        assert!(one(&bytes).ends_with("after"));
        assert_eq!(one(b"\x1b]0;title"), "");
    }

    fn panel() -> RunPanel {
        let entry = ConfiguredRun {
            name: "x".to_owned(),
            command: "true".to_owned(),
            cwd: None,
            from_project: false,
        };
        let target = entry.target(Path::new("/p"));
        assert_eq!(target.source, RunSource::UserConfig);
        RunPanel::new(1, target)
    }

    fn line(text: &str) -> RunLine {
        RunLine {
            spans: Vec::new(),
            text: text.to_owned(),
            location: None,
        }
    }

    fn lines(n: usize) -> Vec<RunLine> {
        (0..n).map(|i| line(&i.to_string())).collect()
    }

    #[test]
    fn the_buffer_is_bounded_by_lines_and_counts_what_it_lost() {
        let mut p = panel();
        p.push(lines(MAX_LINES + 5), String::new(), 0);
        assert_eq!(p.trimmed(), 5);
        assert_eq!(p.line(4), None);
        assert_eq!(p.line(5).map(|l| l.text.as_str()), Some("5"));
        assert_eq!(p.end_line(), MAX_LINES + 5);
    }

    #[test]
    fn the_buffer_is_bounded_by_bytes() {
        let mut p = panel();
        let big = "x".repeat(MAX_LINE_BYTES);
        let many = MAX_TOTAL_BYTES / MAX_LINE_BYTES + 10;
        p.push(vec![line(&big); many], String::new(), 0);
        assert_eq!(p.trimmed(), 10);
        assert!(p.bytes <= MAX_TOTAL_BYTES);
    }

    #[test]
    fn lines_the_service_dropped_count_as_lost_and_nothing_is_out_of_order() {
        let mut p = panel();
        p.push(lines(3), String::new(), 0);
        p.push(vec![line("new")], String::new(), 7);
        assert_eq!(p.trimmed(), 10);
        assert_eq!(p.line(10).map(|l| l.text.as_str()), Some("new"));
        assert_eq!(p.end_line(), 11);
    }

    #[test]
    fn moving_up_stops_following_and_moving_past_the_end_resumes() {
        let mut p = panel();
        p.push(lines(5), String::new(), 0);
        assert_eq!(p.selected(), None);
        p.select_by(-1);
        assert_eq!(p.selected(), Some(4));
        p.push(lines(5), String::new(), 0);
        assert_eq!(p.selected(), Some(4), "new output does not move the cursor");
        p.select_by(100);
        assert_eq!(p.selected(), None);
        p.select_by(-100);
        assert_eq!(p.selected(), Some(0));
        p.select_first();
        assert_eq!(p.selected(), Some(0));
        p.follow_end();
        assert_eq!(p.selected(), None);
    }

    #[test]
    fn a_selection_trimmed_away_moves_to_what_is_left() {
        let mut p = panel();
        p.push(lines(3), String::new(), 0);
        p.select_first();
        p.push(lines(MAX_LINES), String::new(), 0);
        assert_eq!(p.selected(), Some(3));
        assert!(p.line(3).is_some());
    }

    #[test]
    fn an_empty_panel_has_nothing_to_select() {
        let mut p = panel();
        p.select_by(-1);
        p.select_first();
        p.select_line(0);
        assert_eq!(p.selected(), None);
        assert_eq!(p.next_location(true), None);
        assert_eq!(p.next_location(false), None);
    }

    #[test]
    fn locations_are_found_in_both_directions_from_the_cursor() {
        let mut p = panel();
        let mut all = lines(6);
        for n in [1, 4] {
            all[n].location = Some(RunLocation {
                path: PathBuf::from("/p/a"),
                line: 0,
                col: 0,
                span: (0, 1),
            });
        }
        p.push(all, String::new(), 0);
        assert_eq!(p.next_location(true), Some(1));
        assert_eq!(p.next_location(false), Some(4));
        p.select_line(1);
        assert_eq!(p.next_location(true), Some(4));
        assert_eq!(p.next_location(false), None);
        p.select_line(4);
        assert_eq!(p.next_location(true), None);
        assert_eq!(p.next_location(false), Some(1));
    }

    #[test]
    fn outcomes_read_as_a_status() {
        assert_eq!(RunOutcome::Exited(3).to_string(), "Exit code 3");
        assert_eq!(RunOutcome::Signalled(9).to_string(), "Killed by signal 9");
        assert!(RunOutcome::Exited(0).is_success());
        assert!(!RunOutcome::Stopped.is_success());
    }

    fn styled(bytes: &[u8]) -> Vec<(String, Vec<RunSpan>)> {
        let mut decoder = OutputDecoder::default();
        let mut out = Vec::new();
        decoder.feed_styled(bytes, |text, spans| out.push((text, spans)));
        out
    }

    fn red() -> RunStyle {
        RunStyle {
            fg: Some(RunColor::Indexed(1)),
            ..RunStyle::default()
        }
    }

    #[test]
    fn colors_become_spans_counted_in_chars() {
        let lines = styled("ok \x1b[31mпривет\x1b[0m end\n".as_bytes());
        assert_eq!(lines[0].0, "ok привет end");
        assert_eq!(
            lines[0].1,
            [RunSpan {
                start: 3,
                end: 9,
                style: red()
            }]
        );
    }

    #[test]
    fn every_kind_of_color_and_attribute_is_understood() {
        let style = |seq: &str| {
            let line = format!("\x1b[{seq}mx\n");
            styled(line.as_bytes())[0].1[0].style
        };
        assert_eq!(style("91").fg, Some(RunColor::Indexed(9)));
        assert_eq!(style("44").bg, Some(RunColor::Indexed(4)));
        assert_eq!(style("38;5;208").fg, Some(RunColor::Indexed(208)));
        assert_eq!(style("48;2;1;2;3").bg, Some(RunColor::Rgb(1, 2, 3)));
        assert!(style("1;4").bold && style("1;4").underline);
        assert!(style("4:3").underline);
    }

    #[test]
    fn a_color_carries_into_the_next_line_and_a_reset_ends_it() {
        let lines = styled(b"\x1b[31ma\nb\x1b[0m\nc\n");
        assert_eq!(
            lines[1].1,
            [RunSpan {
                start: 0,
                end: 1,
                style: red()
            }]
        );
        assert!(lines[2].1.is_empty());
    }

    #[test]
    fn text_cannot_be_hidden_and_other_sequences_change_nothing() {
        assert!(
            styled(b"\x1b[8mx\n")[0].1.is_empty(),
            "conceal is not obeyed"
        );
        assert!(styled(b"\x1b[?25mx\n")[0].1.is_empty());
        assert!(styled(b"\x1b[2Kx\n")[0].1.is_empty());
        assert_eq!(
            styled(b"\x1b[38;5;999mx\n")[0].0,
            "x",
            "nonsense is no problem"
        );
    }

    #[test]
    fn a_sequence_split_between_reads_still_counts() {
        let mut decoder = OutputDecoder::default();
        let mut out = Vec::new();
        decoder.feed_styled(b"\x1b[3", |t, s| out.push((t, s)));
        decoder.feed_styled(b"1mx\n", |t, s| out.push((t, s)));
        assert_eq!(out[0].1[0].style, red());
    }

    #[test]
    fn rewriting_a_line_forgets_the_old_colors_and_endless_changes_are_bounded() {
        assert!(styled(b"\x1b[31mold\x1b[0m\rnew\n")[0].1.is_empty());
        let mut many = Vec::new();
        for n in 0..5000 {
            many.extend_from_slice(format!("\x1b[3{}mx", n % 8).as_bytes());
        }
        many.push(b'\n');
        let lines = styled(&many);
        assert_eq!(lines[0].0.chars().count(), 5000);
        assert!(lines[0].1.len() <= MAX_STYLE_CHANGES + 1);
    }

    #[test]
    fn a_broken_character_does_not_move_the_colors() {
        let lines = styled(b"\xff\x1b[31mab\n");
        assert_eq!(
            lines[0].1,
            [RunSpan {
                start: 1,
                end: 3,
                style: red()
            }]
        );
    }
}
