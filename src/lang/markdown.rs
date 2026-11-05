use std::ops::Range;

use pulldown_cmark::{Alignment, CodeBlockKind, Event, Options, Parser, Tag, TagEnd};
use ropey::Rope;
use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

use super::languages::Language;
use super::syntax::Syntax;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MdKind {
    Text,
    Heading(u8),
    InlineCode,
    Link,
    Url,
    Quote,
    Mark,
    TableHeader,
    Code(Option<&'static str>),
    Html,
    Image,
    Art {
        ink: Option<[u8; 3]>,
        paper: Option<[u8; 3]>,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MdStyle {
    pub kind: MdKind,
    pub bold: bool,
    pub italic: bool,
    pub strike: bool,
}

impl MdStyle {
    pub fn of(kind: MdKind) -> Self {
        Self {
            kind,
            bold: false,
            italic: false,
            strike: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MdSpan {
    pub text: String,
    pub style: MdStyle,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct MdLine {
    pub spans: Vec<MdSpan>,
    pub source: usize,
    pub image: Option<String>,
}

impl MdLine {
    pub fn text(&self) -> String {
        self.spans.iter().map(|s| s.text.as_str()).collect()
    }
}

pub fn line_for_source(lines: &[MdLine], source: usize) -> usize {
    lines
        .partition_point(|l| l.source < source)
        .min(lines.len().saturating_sub(1))
}

struct Writer {
    width: usize,
    out: Vec<MdLine>,
    inline: Vec<MdSpan>,
    source: usize,
    frames: Vec<Frame>,
    bold: u32,
    italic: u32,
    strike: u32,
    heading: Option<u8>,
    link: Option<String>,
    image: u32,
    image_source: Option<String>,
    lists: Vec<Option<u64>>,
    code: Option<(String, Language)>,
    chart: bool,
    table: Option<Table>,
}

#[derive(Debug, Clone)]
enum Frame {
    Quote,
    Item { mark: String, used: bool },
}

#[derive(Default)]
struct Table {
    aligns: Vec<Alignment>,
    head: Vec<Vec<MdSpan>>,
    rows: Vec<Vec<Vec<MdSpan>>>,
    row: Vec<Vec<MdSpan>>,
    source: usize,
}

fn width_of(spans: &[MdSpan]) -> usize {
    spans.iter().map(|s| s.text.width()).sum()
}

impl Writer {
    fn style(&self) -> MdStyle {
        let kind = if self.image > 0 {
            MdKind::Image
        } else if self.link.is_some() {
            MdKind::Link
        } else if let Some(level) = self.heading {
            MdKind::Heading(level)
        } else if self.frames.iter().any(|f| matches!(f, Frame::Quote)) {
            MdKind::Quote
        } else {
            MdKind::Text
        };
        MdStyle {
            kind,
            bold: self.bold > 0 || self.heading.is_some(),
            italic: self.italic > 0,
            strike: self.strike > 0,
        }
    }

    fn push_styled(&mut self, text: &str, style: MdStyle) {
        match self.inline.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => self.inline.push(MdSpan {
                text: text.to_owned(),
                style,
            }),
        }
    }

    fn push(&mut self, text: &str) {
        self.push_styled(text, self.style());
    }

    fn prefix(&mut self) -> Vec<MdSpan> {
        let mut spans = Vec::new();
        for frame in &mut self.frames {
            let (text, kind) = match frame {
                Frame::Quote => ("│ ".to_owned(), MdKind::Quote),
                Frame::Item { mark, used } => {
                    let text = if *used {
                        " ".repeat(mark.width())
                    } else {
                        mark.clone()
                    };
                    *used = true;
                    (text, MdKind::Mark)
                }
            };
            spans.push(MdSpan {
                text,
                style: MdStyle::of(kind),
            });
        }
        spans
    }

    fn prefix_width(&self) -> usize {
        self.frames
            .iter()
            .map(|f| match f {
                Frame::Quote => 2,
                Frame::Item { mark, .. } => mark.width(),
            })
            .sum()
    }

    fn row(&mut self, spans: Vec<MdSpan>, source: usize) {
        let mut all = self.prefix();
        all.extend(spans);
        let source = self.after_last(source);
        self.out.push(MdLine {
            spans: all,
            source,
            image: None,
        });
    }

    fn after_last(&self, source: usize) -> usize {
        source.max(self.out.last().map_or(0, |l| l.source))
    }

    fn blank(&mut self, source: usize) {
        let source = self.after_last(source);
        if self
            .out
            .last()
            .is_some_and(|l| !l.spans.is_empty() || l.image.is_some())
        {
            self.out.push(MdLine {
                spans: Vec::new(),
                source,
                image: None,
            });
        }
    }

    fn flush(&mut self) {
        if self.inline.is_empty() {
            return;
        }
        let spans = std::mem::take(&mut self.inline);
        let room = self.width.saturating_sub(self.prefix_width()).max(1);
        for row in wrap(&spans, room) {
            self.row(row, self.source);
        }
    }

    fn chart_room(&self) -> usize {
        self.width.saturating_sub(self.prefix_width() + 2).max(1)
    }

    fn chart_rows(&mut self, rows: Vec<String>, source: usize) {
        for (n, text) in rows.into_iter().enumerate() {
            let spans = vec![
                MdSpan {
                    text: "  ".to_owned(),
                    style: MdStyle::of(MdKind::Mark),
                },
                MdSpan {
                    text,
                    style: MdStyle::of(MdKind::Text),
                },
            ];
            self.row(spans, source + 1 + n);
        }
    }

    fn code_block(&mut self, text: &str, language: Language, source: usize) {
        let rope = Rope::from_str(text);
        let highlights =
            Syntax::new(language, &rope).map(|s| s.highlight(&rope, 0..rope.len_lines()));
        let lines: Vec<&str> = text
            .strip_suffix('\n')
            .unwrap_or(text)
            .split('\n')
            .collect();
        for (n, line) in lines.iter().enumerate() {
            let line = line.trim_end_matches('\r');
            let captures = highlights.as_ref().and_then(|h| h.get(n));
            let mut spans = vec![MdSpan {
                text: "  ".to_owned(),
                style: MdStyle::of(MdKind::Mark),
            }];
            spans.extend(code_spans(
                &line.replace('\t', "    "),
                line,
                captures.map(Vec::as_slice),
            ));
            self.row(spans, source + 1 + n);
        }
    }

    fn table(&mut self, table: Table) {
        let columns = table
            .rows
            .iter()
            .map(Vec::len)
            .chain([table.head.len()])
            .max()
            .unwrap_or(0);
        if columns == 0 {
            return;
        }
        let wanted: Vec<usize> = (0..columns)
            .map(|c| {
                table
                    .rows
                    .iter()
                    .chain([&table.head])
                    .filter_map(|r| r.get(c))
                    .map(|cell| width_of(cell))
                    .max()
                    .unwrap_or(0)
                    .max(1)
            })
            .collect();
        let room = self
            .width
            .saturating_sub(self.prefix_width() + columns * 3 + 1)
            .max(columns);
        let widths = fit_columns(&wanted, room);
        let mark = |text: String| MdSpan {
            text,
            style: MdStyle::of(MdKind::Mark),
        };
        let border = |left: &str, mid: &str, right: &str| {
            let parts: Vec<String> = widths.iter().map(|w| "─".repeat(w + 2)).collect();
            mark(format!("{left}{}{right}", parts.join(mid)))
        };
        let source = table.source;
        self.row(vec![border("┌", "┬", "┐")], source);
        let draw = |this: &mut Self, cells: &[Vec<MdSpan>], source: usize| {
            let mut spans = Vec::new();
            for (c, width) in widths.iter().enumerate() {
                spans.push(mark(if c == 0 { "│ " } else { " │ " }.to_owned()));
                let empty = Vec::new();
                let cell = clip(cells.get(c).unwrap_or(&empty), *width);
                let pad = width.saturating_sub(width_of(&cell));
                let (before, after) = match table.aligns.get(c) {
                    Some(Alignment::Right) => (pad, 0),
                    Some(Alignment::Center) => (pad / 2, pad - pad / 2),
                    _ => (0, pad),
                };
                spans.push(mark(" ".repeat(before)));
                spans.extend(cell);
                spans.push(mark(" ".repeat(after)));
            }
            spans.push(mark(" │".to_owned()));
            this.row(spans, source);
        };
        draw(self, &table.head, source);
        self.row(vec![border("├", "┼", "┤")], source);
        for (n, cells) in table.rows.iter().enumerate() {
            draw(self, cells, source + 2 + n);
        }
        self.row(vec![border("└", "┴", "┘")], source + 1 + table.rows.len());
    }

    fn start(&mut self, tag: &Tag<'_>, source: usize) {
        match tag {
            Tag::Paragraph => self.source = source,
            Tag::Heading { level, .. } => {
                self.blank(source);
                self.source = source;
                self.heading = Some(*level as u8);
            }
            Tag::BlockQuote(_) => {
                self.blank(source);
                self.frames.push(Frame::Quote);
            }
            Tag::CodeBlock(kind) => {
                self.flush();
                self.chart = matches!(&kind, CodeBlockKind::Fenced(info)
                    if info.split_whitespace().next() == Some("mermaid"));
                let language = match kind {
                    CodeBlockKind::Fenced(info) => Language::from_fence(info),
                    CodeBlockKind::Indented => Language::PlainText,
                };
                self.source = source;
                self.code = Some((String::new(), language));
            }
            Tag::List(first) => {
                self.flush();
                if self.lists.is_empty() {
                    self.blank(source);
                }
                self.lists.push(*first);
            }
            Tag::Item => {
                self.flush();
                self.source = source;
                let mark = match self.lists.last_mut() {
                    Some(Some(number)) => {
                        let mark = format!("{number}. ");
                        *number += 1;
                        mark
                    }
                    _ => "• ".to_owned(),
                };
                self.frames.push(Frame::Item { mark, used: false });
            }
            Tag::Table(aligns) => {
                self.blank(source);
                self.table = Some(Table {
                    aligns: aligns.clone(),
                    source,
                    ..Table::default()
                });
            }
            Tag::TableHead | Tag::TableRow => {
                if let Some(table) = &mut self.table {
                    table.row = Vec::new();
                }
            }
            Tag::TableCell => self.inline.clear(),
            Tag::Emphasis => self.italic += 1,
            Tag::Strong => self.bold += 1,
            Tag::Strikethrough => self.strike += 1,
            Tag::Link { dest_url, .. } => self.link = Some(dest_url.to_string()),
            Tag::Image { dest_url, .. } => {
                self.image += 1;
                self.image_source = Some(dest_url.to_string());
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd, source: usize) {
        match tag {
            TagEnd::Paragraph => {
                self.flush();
                self.blank(source);
            }
            TagEnd::Heading(level) => {
                self.flush();
                self.heading = None;
                if (level as u8) <= 2 {
                    let rule = if (level as u8) == 1 { "═" } else { "─" };
                    let text = rule.repeat(self.width.saturating_sub(self.prefix_width()));
                    self.row(
                        vec![MdSpan {
                            text,
                            style: MdStyle::of(MdKind::Mark),
                        }],
                        source,
                    );
                }
                self.blank(source);
            }
            TagEnd::BlockQuote(_) => {
                self.flush();
                self.frames.pop();
                self.blank(source);
            }
            TagEnd::CodeBlock => {
                if let Some((text, language)) = self.code.take() {
                    let chart = std::mem::take(&mut self.chart)
                        .then(|| super::mermaid::render(&text, self.chart_room()))
                        .flatten();
                    match chart {
                        Some(rows) => self.chart_rows(rows, self.source),
                        None => self.code_block(&text, language, self.source),
                    }
                }
                self.blank(source);
            }
            TagEnd::List(_) => {
                self.flush();
                self.lists.pop();
                if self.lists.is_empty() {
                    self.blank(source);
                }
            }
            TagEnd::Item => {
                self.flush();
                if matches!(self.frames.last(), Some(Frame::Item { used: false, .. })) {
                    self.row(Vec::new(), source);
                }
                self.frames.pop();
            }
            TagEnd::TableCell => {
                let cell = std::mem::take(&mut self.inline);
                if let Some(table) = &mut self.table {
                    table.row.push(cell);
                }
            }
            TagEnd::TableHead => {
                if let Some(table) = &mut self.table {
                    table.head = std::mem::take(&mut table.row);
                }
            }
            TagEnd::TableRow => {
                if let Some(table) = &mut self.table {
                    let row = std::mem::take(&mut table.row);
                    table.rows.push(row);
                }
            }
            TagEnd::Table => {
                if let Some(table) = self.table.take() {
                    self.table(table);
                }
                self.blank(source);
            }
            TagEnd::Emphasis => self.italic = self.italic.saturating_sub(1),
            TagEnd::Strong => self.bold = self.bold.saturating_sub(1),
            TagEnd::Strikethrough => self.strike = self.strike.saturating_sub(1),
            TagEnd::Link => {
                if let Some(url) = self.link.take() {
                    let shown: String = self
                        .inline
                        .iter()
                        .rev()
                        .take(1)
                        .map(|s| s.text.clone())
                        .collect();
                    if !shown.ends_with(&url) {
                        self.push_styled(&format!(" ({url})"), MdStyle::of(MdKind::Url));
                    }
                }
            }
            TagEnd::Image => {
                self.image = self.image.saturating_sub(1);
                let Some(address) = self.image_source.take() else {
                    return;
                };
                if crate::core::image_art::is_remote(&address) {
                    self.push_styled(&format!(" ({address})"), MdStyle::of(MdKind::Url));
                } else if self.table.is_none() {
                    self.flush();
                    let source = self.after_last(self.source);
                    self.out.push(MdLine {
                        spans: Vec::new(),
                        source,
                        image: Some(address),
                    });
                }
            }
            _ => {}
        }
    }

    fn event(&mut self, event: Event<'_>, source: usize) {
        match event {
            Event::Start(tag) => self.start(&tag, source),
            Event::End(tag) => self.end(tag, source),
            Event::Text(text) => {
                if let Some((code, _)) = &mut self.code {
                    code.push_str(&text);
                } else {
                    self.push(&text);
                }
            }
            Event::Code(text) => {
                let style = MdStyle {
                    kind: MdKind::InlineCode,
                    ..self.style()
                };
                self.push_styled(&text, style);
            }
            Event::SoftBreak => self.push(" "),
            Event::HardBreak => self.flush_hard(source),
            Event::Rule => {
                self.blank(source);
                let text = "─".repeat(self.width.saturating_sub(self.prefix_width()));
                self.row(
                    vec![MdSpan {
                        text,
                        style: MdStyle::of(MdKind::Mark),
                    }],
                    source,
                );
                self.blank(source);
            }
            Event::TaskListMarker(done) => {
                let mark = if done { "[x] " } else { "[ ] " };
                self.push_styled(mark, MdStyle::of(MdKind::Mark));
            }
            Event::Html(text) | Event::InlineHtml(text) => {
                self.flush_if_block(&text);
                self.push_styled(text.trim_end_matches('\n'), MdStyle::of(MdKind::Html));
                if text.ends_with('\n') {
                    self.source = source;
                    self.flush();
                }
            }
            _ => {}
        }
    }

    fn flush_hard(&mut self, source: usize) {
        self.flush();
        self.source = source;
    }

    fn flush_if_block(&mut self, text: &str) {
        if text.ends_with('\n') {
            self.flush();
        }
    }
}

fn code_spans(shown: &str, raw: &str, captures: Option<&[super::syntax::Span]>) -> Vec<MdSpan> {
    let Some(captures) = captures.filter(|c| !c.is_empty() && !raw.contains('\t')) else {
        return vec![MdSpan {
            text: shown.to_owned(),
            style: MdStyle::of(MdKind::Code(None)),
        }];
    };
    let chars: Vec<char> = raw.chars().collect();
    let mut spans: Vec<MdSpan> = Vec::new();
    let mut at = 0;
    let piece = |range: Range<usize>, capture| MdSpan {
        text: chars[range].iter().collect(),
        style: MdStyle::of(MdKind::Code(capture)),
    };
    let mut sorted: Vec<&super::syntax::Span> = captures.iter().collect();
    sorted.sort_by_key(|s| s.chars.start);
    for span in sorted {
        let start = span.chars.start.min(chars.len());
        let end = span.chars.end.min(chars.len());
        if start < at || start >= end {
            continue;
        }
        if at < start {
            spans.push(piece(at..start, None));
        }
        spans.push(piece(start..end, Some(span.capture)));
        at = end;
    }
    if at < chars.len() {
        spans.push(piece(at..chars.len(), None));
    }
    spans
}

fn fit_columns(wanted: &[usize], room: usize) -> Vec<usize> {
    let mut widths = wanted.to_vec();
    while widths.iter().sum::<usize>() > room {
        let Some(widest) = widths.iter_mut().max() else {
            break;
        };
        if *widest <= 1 {
            break;
        }
        *widest -= 1;
    }
    widths
}

fn clip(spans: &[MdSpan], width: usize) -> Vec<MdSpan> {
    if width_of(spans) <= width {
        return spans.to_vec();
    }
    let mut out: Vec<MdSpan> = Vec::new();
    let mut used = 0;
    'spans: for span in spans {
        let mut text = String::new();
        for c in span.text.chars() {
            let w = c.width().unwrap_or(0);
            if used + w + 1 > width {
                if !text.is_empty() {
                    out.push(MdSpan {
                        text,
                        style: span.style,
                    });
                }
                break 'spans;
            }
            text.push(c);
            used += w;
        }
        out.push(MdSpan {
            text,
            style: span.style,
        });
    }
    let style = spans.first().map_or(MdStyle::of(MdKind::Text), |s| s.style);
    out.push(MdSpan {
        text: "…".to_owned(),
        style,
    });
    out
}

fn wrap(spans: &[MdSpan], width: usize) -> Vec<Vec<MdSpan>> {
    let mut rows: Vec<Vec<MdSpan>> = vec![Vec::new()];
    let mut used = 0;
    let push = |rows: &mut Vec<Vec<MdSpan>>, text: &str, style: MdStyle| {
        let Some(row) = rows.last_mut() else {
            return;
        };
        match row.last_mut() {
            Some(last) if last.style == style => last.text.push_str(text),
            _ => row.push(MdSpan {
                text: text.to_owned(),
                style,
            }),
        }
    };
    for span in spans {
        for piece in span.text.split_inclusive(' ') {
            let word = piece.trim_end_matches(' ');
            if used > 0 && used + word.width() > width {
                rows.push(Vec::new());
                used = 0;
            }
            if piece.width() <= width.saturating_sub(used)
                || word.width() <= width.saturating_sub(used)
            {
                push(&mut rows, piece, span.style);
                used += piece.width();
                continue;
            }
            for c in piece.chars() {
                let w = c.width().unwrap_or(0);
                if used + w > width && used > 0 {
                    rows.push(Vec::new());
                    used = 0;
                }
                let mut buffer = [0; 4];
                push(&mut rows, c.encode_utf8(&mut buffer), span.style);
                used += w;
            }
        }
    }
    rows
}

struct Lines(Vec<usize>);

impl Lines {
    fn new(text: &str) -> Self {
        let mut starts = vec![0];
        starts.extend(
            text.bytes()
                .enumerate()
                .filter(|(_, b)| *b == b'\n')
                .map(|(i, _)| i + 1),
        );
        Self(starts)
    }

    fn of(&self, byte: usize) -> usize {
        self.0.partition_point(|s| *s <= byte).saturating_sub(1)
    }
}

pub fn render(text: &str, width: usize) -> Vec<MdLine> {
    let width = width.max(1);
    let options =
        Options::ENABLE_TABLES | Options::ENABLE_STRIKETHROUGH | Options::ENABLE_TASKLISTS;
    let lines = Lines::new(text);
    let mut writer = Writer {
        width,
        out: Vec::new(),
        inline: Vec::new(),
        source: 0,
        frames: Vec::new(),
        bold: 0,
        italic: 0,
        strike: 0,
        heading: None,
        link: None,
        image: 0,
        image_source: None,
        lists: Vec::new(),
        code: None,
        chart: false,
        table: None,
    };
    for (event, range) in Parser::new_ext(text, options).into_offset_iter() {
        writer.event(event, lines.of(range.start));
    }
    writer.flush();
    while writer
        .out
        .last()
        .is_some_and(|l| l.spans.is_empty() && l.image.is_none())
    {
        writer.out.pop();
    }
    writer.out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn rows(text: &str, width: usize) -> Vec<String> {
        render(text, width).iter().map(MdLine::text).collect()
    }

    fn span_with<'a>(lines: &'a [MdLine], needle: &str) -> &'a MdSpan {
        lines
            .iter()
            .flat_map(|l| &l.spans)
            .find(|s| s.text.contains(needle))
            .unwrap_or_else(|| panic!("no span with {needle:?} in {lines:?}"))
    }

    #[test]
    fn headings_are_marked_by_style_and_a_rule_not_by_hashes() {
        let lines = render("# Big\n\n## Mid\n\n### Small\n", 8);
        let text: Vec<String> = lines.iter().map(MdLine::text).collect();
        assert_eq!(
            text,
            ["Big", "════════", "", "Mid", "────────", "", "Small"]
        );
        let big = span_with(&lines, "Big");
        assert_eq!(big.style.kind, MdKind::Heading(1));
        assert!(big.style.bold);
        assert_eq!(span_with(&lines, "Small").style.kind, MdKind::Heading(3));
    }

    #[test]
    fn bold_italic_and_strikethrough_nest() {
        let lines = render("plain **bold *both*** _it_ ~~gone~~\n", 80);
        assert_eq!(lines.len(), 1);
        assert_eq!(lines[0].text(), "plain bold both it gone");
        assert!(!span_with(&lines, "plain").style.bold);
        let bold = span_with(&lines, "bold");
        assert!(bold.style.bold && !bold.style.italic);
        let both = span_with(&lines, "both");
        assert!(both.style.bold && both.style.italic);
        assert!(span_with(&lines, "it").style.italic);
        assert!(span_with(&lines, "gone").style.strike);
    }

    #[test]
    fn lists_nest_number_and_hang_their_text() {
        let text = "- one\n- two\n  - inner\n\n1. first\n2. second\n\n- [x] done\n- [ ] todo\n";
        assert_eq!(
            rows(text, 40),
            [
                "• one",
                "• two",
                "  • inner",
                "",
                "1. first",
                "2. second",
                "",
                "• [x] done",
                "• [ ] todo"
            ]
        );
        assert_eq!(rows("- aaa bbb ccc\n", 7), ["• aaa ", "  bbb ", "  ccc"]);
        assert_eq!(
            rows("7. x\n8. y\n", 9),
            ["7. x", "8. y"],
            "starts where it says"
        );
    }

    #[test]
    fn quotes_keep_their_bar_on_every_row_and_can_nest() {
        assert_eq!(
            rows("> aaa bbb ccc\n>\n> > deep\n", 7),
            ["│ aaa ", "│ bbb ", "│ ccc", "", "│ │ dee", "│ │ p"]
        );
        let lines = render("> said\n", 20);
        assert_eq!(span_with(&lines, "said").style.kind, MdKind::Quote);
    }

    #[test]
    fn code_is_highlighted_by_the_existing_grammars() {
        let lines = render("```rust\nfn main() {}\n```\n", 40);
        assert_eq!(lines[0].text(), "  fn main() {}");
        let keyword = span_with(&lines, "fn");
        assert_eq!(keyword.style.kind, MdKind::Code(Some("keyword")));
        let plain = render("```nonsense\nfn main() {}\n```\n", 40);
        assert_eq!(span_with(&plain, "fn main").style.kind, MdKind::Code(None));
        let py = render("``` Python extra words\ndef f(): pass\n```\n", 40);
        assert_eq!(
            span_with(&py, "def").style.kind,
            MdKind::Code(Some("keyword"))
        );
        let inline = render("use `x` here\n", 40);
        assert_eq!(span_with(&inline, "x").style.kind, MdKind::InlineCode);
    }

    #[test]
    fn a_mermaid_block_is_drawn_and_one_it_cannot_draw_stays_its_source() {
        let shown = |text: &str, width| {
            render(text, width)
                .iter()
                .map(|l| l.spans.iter().map(|s| s.text.as_str()).collect::<String>())
                .collect::<Vec<_>>()
                .join("\n")
        };
        let chart = "```mermaid\nflowchart TD\nA[Start] --> B[End]\n```\n";
        let wide = shown(chart, 60);
        assert!(wide.contains("┌───────┐"), "{wide}");
        assert!(wide.contains('▼') && !wide.contains("-->"), "{wide}");
        let narrow = shown(chart, 8);
        assert!(narrow.contains("-->"), "no room: the source\n{narrow}");
        let other = shown("```mermaid\nsequenceDiagram\nA->>B: hi\n```\n", 60);
        assert!(other.contains("sequenceDiagram"), "{other}");
    }

    #[test]
    fn code_keeps_its_lines_unwrapped_and_tabs_visible() {
        let long = "x".repeat(50);
        let lines = render(&format!("```\n{long}\n\tTab\n```\n"), 20);
        assert_eq!(
            lines[0].text().trim(),
            long,
            "cut by the screen, not broken"
        );
        assert_eq!(lines[1].text(), "      Tab");
    }

    #[test]
    fn links_show_text_and_address_and_an_autolink_only_once() {
        let lines = render("see [the docs](https://a.io/b) now\n", 80);
        assert_eq!(lines[0].text(), "see the docs (https://a.io/b) now");
        assert_eq!(span_with(&lines, "the docs").style.kind, MdKind::Link);
        assert_eq!(span_with(&lines, "https").style.kind, MdKind::Url);
        assert_eq!(rows("<https://x.io>\n", 80), ["https://x.io"]);
    }

    #[test]
    fn an_image_shows_as_its_alt_text() {
        let lines = render("![a cat sits](cat.png)\n", 80);
        assert_eq!(
            lines[0].text(),
            "a cat sits (cat.png)".split(" (").next().unwrap()
        );
        assert_eq!(span_with(&lines, "a cat").style.kind, MdKind::Image);
    }

    #[test]
    fn tables_line_up_with_alignment_and_wide_characters() {
        let text = "| A | Беее |\n|:--|---:|\n| 日本 | 1 |\n| x | 22 |\n";
        let table = rows(text, 40);
        assert_eq!(
            table,
            [
                "┌──────┬──────┐",
                "│ A    │ Беее │",
                "├──────┼──────┤",
                "│ 日本 │    1 │",
                "│ x    │   22 │",
                "└──────┴──────┘"
            ]
        );
        assert_eq!(
            span_with(&render(text, 40), "Беее").style.kind,
            MdKind::Text
        );
    }

    #[test]
    fn a_table_too_wide_gives_way_with_a_mark_and_stays_in_its_width() {
        let text = "| first column | second column here |\n|---|---|\n| aaaaaaaaaaaaaaaaaa | b |\n";
        for width in [30, 20, 12] {
            let table = rows(text, width);
            let widths: Vec<usize> = table.iter().map(|r| r.width()).collect();
            assert!(widths.iter().all(|w| *w == widths[0]), "{table:?}");
            assert!(widths[0] <= width.max(9), "{table:?}");
            assert!(table.iter().any(|r| r.contains('…')), "{table:?}");
        }
    }

    #[test]
    fn rules_fill_the_width() {
        assert_eq!(rows("a\n\n---\n\nb\n", 3), ["a", "", "───", "", "b"]);
    }

    #[test]
    fn words_wrap_across_styles_and_long_words_are_cut() {
        assert_eq!(rows("aaa **bbb** ccc\n", 4), ["aaa ", "bbb ", "ccc"]);
        assert_eq!(rows("abcdefgh\n", 3), ["abc", "def", "gh"]);
        for row in rows("日本語のテキストです\n", 5) {
            assert!(row.width() <= 5, "{row}");
        }
    }

    #[test]
    fn each_line_knows_where_it_came_from_and_the_cursor_finds_it() {
        let text = "# Title\n\nfirst para\n\n- item\n\n```\ncode\nmore\n```\n\nlast\n";
        let lines = render(text, 40);
        let of = |needle: &str| {
            lines
                .iter()
                .find(|l| l.text().contains(needle))
                .map(|l| l.source)
                .unwrap_or(usize::MAX)
        };
        assert_eq!(of("Title"), 0);
        assert_eq!(of("first"), 2);
        assert_eq!(of("item"), 4);
        assert_eq!(of("code"), 7);
        assert_eq!(of("more"), 8);
        assert_eq!(of("last"), 11);
        let sources: Vec<usize> = lines.iter().map(|l| l.source).collect();
        assert!(sources.windows(2).all(|p| p[0] <= p[1]), "{sources:?}");
        let at = line_for_source(&lines, 8);
        assert!(lines[at].text().contains("more"));
        assert!(lines[line_for_source(&lines, 999)].text().contains("last"));
        assert_eq!(line_for_source(&[], 3), 0);
    }

    #[test]
    fn html_is_shown_as_written_and_crlf_does_not_leak() {
        assert_eq!(rows("<div>raw</div>\n", 40), ["<div>raw</div>"]);
        assert_eq!(
            rows("a\r\nb\r\n\r\n```\r\nc\r\n```\r\n", 40),
            ["a b", "", "  c"]
        );
    }

    #[test]
    fn nothing_panics_on_odd_input_or_width() {
        for text in [
            "",
            "\n\n",
            "# ",
            "|a|\n|-|\n",
            "- \n",
            "> \n",
            "```\n",
            "[x](",
            "**",
            "1. \n   - \n     > ```\n",
            "\u{1b}[31mred\u{1b}[0m\n",
            "| a | b |\n|---|---|\n| 1 |\n",
        ] {
            for width in [0, 1, 2, 5, 80] {
                let _ = render(text, width);
            }
        }
        assert!(render("", 10).is_empty());
    }
}

#[cfg(test)]
mod picture_tests {
    use super::*;

    #[test]
    fn a_local_picture_gets_its_own_row_under_the_alt_text() {
        let lines = render("![a cat](img/cat.png)\n\nnext", 40);
        let at = lines.iter().position(|l| l.image.is_some()).expect("a row");
        assert_eq!(lines[at].image.as_deref(), Some("img/cat.png"));
        assert_eq!(lines[at - 1].text(), "a cat");
        assert!(lines.iter().any(|l| l.text() == "next"));
    }

    #[test]
    fn every_picture_gets_a_row() {
        let lines = render("![one](a.png)\n\n![two](b.png)\n", 40);
        let found: Vec<_> = lines.iter().filter_map(|l| l.image.as_deref()).collect();
        assert_eq!(found, ["a.png", "b.png"], "{lines:?}");
    }

    #[test]
    fn a_remote_picture_is_its_alt_text_and_address_and_nothing_is_fetched() {
        let lines = render("![a cat](https://x.test/cat.png)", 60);
        assert!(lines.iter().all(|l| l.image.is_none()));
        assert_eq!(lines[0].text(), "a cat (https://x.test/cat.png)");
    }
}
