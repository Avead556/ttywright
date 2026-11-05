use std::collections::HashSet;

use regex_syntax::hir::{Class, Hir, HirKind};

const MAX_FILTER_BYTES: usize = 2 * 1024;
const MIN_FILTER_BYTES: usize = 8;
const BITS_PER_TRIGRAM: usize = 10;
const KELVIN: char = '\u{212A}';
const LONG_S: char = '\u{17F}';

fn trigram(a: u8, b: u8, c: u8) -> u32 {
    u32::from(a) << 16 | u32::from(b) << 8 | u32::from(c)
}

fn add_trigrams(text: &str, set: &mut HashSet<u32>) {
    let mut last = (0, 0);
    for (i, byte) in text.bytes().enumerate() {
        let byte = byte.to_ascii_lowercase();
        if i >= 2 {
            set.insert(trigram(last.0, last.1, byte));
        }
        last = (last.1, byte);
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TrigramFilter {
    bits: Vec<u8>,
}

impl TrigramFilter {
    pub fn of_text(text: &str) -> Self {
        let mut set = HashSet::new();
        add_trigrams(text, &mut set);
        if text.contains([KELVIN, LONG_S]) {
            add_trigrams(&text.replace(KELVIN, "k").replace(LONG_S, "s"), &mut set);
        }
        let wanted = (set.len() * BITS_PER_TRIGRAM).div_ceil(8);
        let size = wanted
            .next_power_of_two()
            .clamp(MIN_FILTER_BYTES, MAX_FILTER_BYTES);
        let mut filter = Self {
            bits: vec![0; size],
        };
        for t in set {
            for bit in filter.positions(t) {
                filter.bits[bit / 8] |= 1 << (bit % 8);
            }
        }
        filter
    }

    pub fn from_bytes(bits: Vec<u8>) -> Option<Self> {
        (bits.len().is_power_of_two() && bits.len() <= MAX_FILTER_BYTES).then_some(Self { bits })
    }

    pub fn as_bytes(&self) -> &[u8] {
        &self.bits
    }

    fn positions(&self, t: u32) -> [usize; 3] {
        let mask = self.bits.len() * 8 - 1;
        let a = t.wrapping_mul(0x9E37_79B1) as usize;
        let b = t.wrapping_mul(0x85EB_CA77) as usize;
        let c = t.wrapping_mul(0xC2B2_AE3D) as usize;
        [(a >> 7) & mask, (b >> 11) & mask, (c >> 5) & mask]
    }

    fn may_contain(&self, t: u32) -> bool {
        self.positions(t)
            .iter()
            .all(|bit| self.bits[bit / 8] & (1 << (bit % 8)) != 0)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TextFilter {
    Anything,
    Contains(u32),
    All(Vec<TextFilter>),
    AnyOf(Vec<TextFilter>),
}

impl TextFilter {
    pub fn may_match(&self, filter: &TrigramFilter) -> bool {
        match self {
            Self::Anything => true,
            Self::Contains(t) => filter.may_contain(*t),
            Self::All(all) => all.iter().all(|f| f.may_match(filter)),
            Self::AnyOf(any) => any.iter().any(|f| f.may_match(filter)),
        }
    }

    pub fn of_regex(hir: &Hir) -> Self {
        let mut run = Run::default();
        let mut parts = Vec::new();
        run.visit(hir, &mut parts);
        run.finish(&mut parts);
        all_of(parts)
    }
}

fn all_of(parts: Vec<TextFilter>) -> TextFilter {
    let mut parts: Vec<TextFilter> = parts
        .into_iter()
        .filter(|p| *p != TextFilter::Anything)
        .collect();
    match parts.len() {
        0 => TextFilter::Anything,
        1 => parts.remove(0),
        _ => TextFilter::All(parts),
    }
}

fn one_letter(class: &Class) -> Option<(u8, bool)> {
    let Class::Unicode(class) = class else {
        return None;
    };
    let mut letter = None;
    let mut folds = false;
    for range in class.ranges() {
        if u32::from(range.end()) - u32::from(range.start()) > 2 {
            return None;
        }
        for c in range.start()..=range.end() {
            let lower = match c {
                KELVIN => {
                    folds = true;
                    b'k'
                }
                LONG_S => {
                    folds = true;
                    b's'
                }
                c if c.is_ascii() => (c as u8).to_ascii_lowercase(),
                _ => return None,
            };
            if letter.is_some_and(|l| l != lower) {
                return None;
            }
            letter = Some(lower);
        }
    }
    letter.map(|l| (l, folds))
}

#[derive(Default)]
struct Run {
    bytes: Vec<u8>,
    outside_ascii: bool,
    folded: bool,
}

impl Run {
    fn push(&mut self, byte: u8, folded: bool, parts: &mut Vec<TextFilter>) {
        let outside = !byte.is_ascii();
        if (outside && self.folded) || (folded && self.outside_ascii) {
            self.finish(parts);
        }
        self.outside_ascii |= outside;
        self.folded |= folded;
        self.bytes.push(byte.to_ascii_lowercase());
    }

    fn finish(&mut self, parts: &mut Vec<TextFilter>) {
        for w in self.bytes.windows(3) {
            parts.push(TextFilter::Contains(trigram(w[0], w[1], w[2])));
        }
        *self = Self::default();
    }

    fn visit(&mut self, hir: &Hir, parts: &mut Vec<TextFilter>) {
        match hir.kind() {
            HirKind::Literal(literal) => {
                for &byte in &literal.0 {
                    if byte == b'\n' || byte == b'\r' {
                        self.finish(parts);
                    } else {
                        self.push(byte, false, parts);
                    }
                }
            }
            HirKind::Class(class) => match one_letter(class) {
                Some((letter, folded)) if letter != b'\n' && letter != b'\r' => {
                    self.push(letter, folded, parts);
                }
                _ => self.finish(parts),
            },
            HirKind::Concat(subs) => {
                for sub in subs {
                    self.visit(sub, parts);
                }
            }
            HirKind::Capture(capture) => self.visit(&capture.sub, parts),
            HirKind::Repetition(repeat) => {
                self.finish(parts);
                if repeat.min >= 1 {
                    parts.push(TextFilter::of_regex(&repeat.sub));
                }
            }
            HirKind::Alternation(subs) => {
                self.finish(parts);
                let any: Vec<TextFilter> = subs.iter().map(TextFilter::of_regex).collect();
                if !any.contains(&TextFilter::Anything) {
                    parts.push(TextFilter::AnyOf(any));
                }
            }
            HirKind::Empty | HirKind::Look(_) => self.finish(parts),
        }
    }
}
