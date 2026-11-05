use std::collections::HashMap;

use unicode_width::{UnicodeWidthChar, UnicodeWidthStr};

const MAX_NODES: usize = 200;
const MAX_LABEL: usize = 60;
const GAP_X: usize = 3;
const GAP_Y: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Shape {
    Box,
    Round,
    Diamond,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Node {
    id: String,
    label: String,
    shape: Shape,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct Edge {
    from: usize,
    to: usize,
    label: Option<String>,
}

#[derive(Debug, Default)]
struct Graph {
    nodes: Vec<Node>,
    ids: HashMap<String, usize>,
    edges: Vec<Edge>,
}

impl Graph {
    fn node(&mut self, id: &str, label: Option<(String, Shape)>) -> usize {
        let at = *self.ids.entry(id.to_owned()).or_insert_with(|| {
            self.nodes.push(Node {
                id: id.to_owned(),
                label: id.to_owned(),
                shape: Shape::Box,
            });
            self.nodes.len() - 1
        });
        if let Some((text, shape)) = label {
            self.nodes[at].label = text;
            self.nodes[at].shape = shape;
        }
        at
    }
}

fn clean_label(text: &str) -> String {
    let text = text
        .trim()
        .trim_matches('"')
        .replace("<br/>", " ")
        .replace("<br>", " ");
    text.chars()
        .filter(|c| !c.is_control() && c.width().unwrap_or(0) > 0)
        .take(MAX_LABEL)
        .collect()
}

fn parse_node(text: &str) -> Option<(String, Option<(String, Shape)>)> {
    let text = text.trim();
    let is_id = |c: char| c.is_alphanumeric() || c == '_' || c == '-';
    let end = text.find(|c: char| !is_id(c)).unwrap_or(text.len());
    let (id, rest) = text.split_at(end);
    if id.is_empty() {
        return None;
    }
    if rest.is_empty() {
        return Some((id.to_owned(), None));
    }
    let (shape, inner) = if let Some(i) = rest.strip_prefix('[').and_then(|r| r.strip_suffix(']')) {
        (Shape::Box, i)
    } else if let Some(i) = rest.strip_prefix('{').and_then(|r| r.strip_suffix('}')) {
        (Shape::Diamond, i)
    } else if let Some(i) = rest.strip_prefix('(').and_then(|r| r.strip_suffix(')')) {
        (Shape::Round, i.trim_matches(['(', ')', '[', ']']))
    } else {
        return None;
    };
    Some((id.to_owned(), Some((clean_label(inner), shape))))
}

fn find_arrow(line: &str) -> Option<(usize, usize, Option<String>)> {
    let bytes = line.as_bytes();
    let mut depth = 0i32;
    let mut i = 0;
    while i < bytes.len() {
        match bytes[i] {
            b'[' | b'(' | b'{' => depth += 1,
            b']' | b')' | b'}' => depth -= 1,
            b'"' => {
                i += line[i + 1..].find('"').map_or(0, |n| n + 1);
            }
            _ => {}
        }
        if depth == 0 {
            let rest = &line[i..];
            for arrow in ["-->", "---", "==>", "-.->"] {
                if rest.starts_with(arrow) {
                    return Some((i, arrow.len(), None));
                }
            }
            if rest.starts_with("-- ")
                && let Some(end) = rest.find("-->")
            {
                let label = clean_label(&rest[3..end]);
                return Some((i, end + 3, Some(label)));
            }
        }
        i += 1;
    }
    None
}

fn parse_line(line: &str, graph: &mut Graph) -> Option<()> {
    let rest = line.trim().trim_end_matches(';');
    let Some((at, len, mut label)) = find_arrow(rest) else {
        let (id, shape) = parse_node(rest)?;
        graph.node(&id, shape);
        return Some(());
    };
    let (id, shape) = parse_node(&rest[..at])?;
    let mut from = graph.node(&id, shape);
    let mut rest = rest[at + len..].trim_start();
    loop {
        if let Some(after) = rest.strip_prefix('|') {
            let end = after.find('|')?;
            label = Some(clean_label(&after[..end]));
            rest = after[end + 1..].trim_start();
        }
        let next = find_arrow(rest);
        let end = next.as_ref().map_or(rest.len(), |n| n.0);
        let (id, shape) = parse_node(&rest[..end])?;
        let to = graph.node(&id, shape);
        graph.edges.push(Edge {
            from,
            to,
            label: label.take(),
        });
        let Some((at, len, next_label)) = next else {
            return Some(());
        };
        from = to;
        label = next_label;
        rest = rest[at + len..].trim_start();
    }
}

fn parse(source: &str) -> Option<Graph> {
    let mut lines = source
        .lines()
        .map(str::trim)
        .filter(|l| !l.is_empty() && !l.starts_with("%%"));
    let head = lines.next()?;
    let mut words = head.split_whitespace();
    if !matches!(words.next()?, "flowchart" | "graph") {
        return None;
    }
    let mut graph = Graph::default();
    for line in lines {
        parse_line(line, &mut graph)?;
        if graph.nodes.len() > MAX_NODES {
            return None;
        }
    }
    (!graph.nodes.is_empty()).then_some(graph)
}

fn layers(graph: &Graph) -> (Vec<usize>, Vec<usize>) {
    let n = graph.nodes.len();
    let mut state = vec![0u8; n];
    let mut back = Vec::new();
    fn visit(at: usize, graph: &Graph, state: &mut [u8], back: &mut Vec<usize>) {
        state[at] = 1;
        for (i, e) in graph.edges.iter().enumerate() {
            if e.from != at {
                continue;
            }
            match state[e.to] {
                0 => visit(e.to, graph, state, back),
                1 => back.push(i),
                _ => {}
            }
        }
        state[at] = 2;
    }
    for at in 0..n {
        if state[at] == 0 {
            visit(at, graph, &mut state, &mut back);
        }
    }
    let mut layer = vec![0usize; n];
    for _ in 0..n {
        for (i, e) in graph.edges.iter().enumerate() {
            if !back.contains(&i) && layer[e.to] <= layer[e.from] {
                layer[e.to] = layer[e.from] + 1;
            }
        }
    }
    (layer, back)
}

const UP: u8 = 1;
const DOWN: u8 = 2;
const LEFT: u8 = 4;
const RIGHT: u8 = 8;

fn line_char(bits: u8) -> char {
    match bits {
        b if b == UP | DOWN || b == UP || b == DOWN => '│',
        b if b == LEFT | RIGHT || b == LEFT || b == RIGHT => '─',
        b if b == DOWN | RIGHT => '┌',
        b if b == DOWN | LEFT => '┐',
        b if b == UP | RIGHT => '└',
        b if b == UP | LEFT => '┘',
        b if b == UP | DOWN | RIGHT => '├',
        b if b == UP | DOWN | LEFT => '┤',
        b if b == LEFT | RIGHT | DOWN => '┬',
        b if b == LEFT | RIGHT | UP => '┴',
        _ => '┼',
    }
}

struct Canvas {
    width: usize,
    cells: Vec<char>,
    lines: Vec<u8>,
}

impl Canvas {
    fn new(width: usize, height: usize) -> Self {
        Self {
            width,
            cells: vec![' '; width * height],
            lines: vec![0; width * height],
        }
    }

    fn put(&mut self, x: usize, y: usize, ch: char) {
        if let Some(cell) = self
            .cells
            .get_mut(y * self.width + x)
            .filter(|_| x < self.width)
        {
            *cell = ch;
        }
    }

    fn text(&mut self, x: usize, y: usize, text: &str) {
        let mut at = x;
        for ch in text.chars() {
            self.put(at, y, ch);
            for extra in 1..ch.width().unwrap_or(1) {
                self.put(at + extra, y, '\0');
            }
            at += ch.width().unwrap_or(1);
        }
    }

    fn link(&mut self, x: usize, y: usize, bits: u8) {
        if let Some(cell) = self
            .lines
            .get_mut(y * self.width + x)
            .filter(|_| x < self.width)
        {
            *cell |= bits;
        }
    }

    fn rows(&self) -> Vec<String> {
        let mut rows: Vec<String> = self
            .cells
            .chunks(self.width)
            .zip(self.lines.chunks(self.width))
            .map(|(cells, lines)| {
                cells
                    .iter()
                    .zip(lines)
                    .filter(|(c, _)| **c != '\0')
                    .map(|(c, l)| {
                        if *c == ' ' && *l != 0 {
                            line_char(*l)
                        } else {
                            *c
                        }
                    })
                    .collect::<String>()
                    .trim_end()
                    .to_owned()
            })
            .collect();
        while rows.last().is_some_and(String::is_empty) {
            rows.pop();
        }
        rows
    }
}

pub fn render(source: &str, width: usize) -> Option<Vec<String>> {
    let graph = parse(source)?;
    let (layer, back) = layers(&graph);
    let count = layer.iter().max().copied().unwrap_or(0) + 1;
    let mut rows: Vec<Vec<usize>> = vec![Vec::new(); count];
    for (node, &l) in layer.iter().enumerate() {
        rows[l].push(node);
    }
    let box_width = |n: usize| graph.nodes[n].label.width() + 4;
    let row_width = |row: &Vec<usize>| {
        row.iter().map(|&n| box_width(n)).sum::<usize>() + GAP_X * row.len().saturating_sub(1)
    };
    let total = rows.iter().map(row_width).max()?;
    if total > width {
        return None;
    }
    let height = count * 3 + count.saturating_sub(1) * GAP_Y;
    let mut canvas = Canvas::new(total, height);
    let mut place = vec![(0usize, 0usize); graph.nodes.len()];
    for (l, row) in rows.iter().enumerate() {
        let y = l * (3 + GAP_Y);
        let mut x = (total - row_width(row)) / 2;
        for &n in row {
            let w = box_width(n);
            place[n] = (x, y);
            let node = &graph.nodes[n];
            let (corners, side) = match node.shape {
                Shape::Box => (['┌', '┐', '└', '┘'], '│'),
                Shape::Round => (['╭', '╮', '╰', '╯'], '│'),
                Shape::Diamond => (['╱', '╲', '╲', '╱'], '◇'),
            };
            canvas.put(x, y, corners[0]);
            canvas.put(x + w - 1, y, corners[1]);
            canvas.put(x, y + 2, corners[2]);
            canvas.put(x + w - 1, y + 2, corners[3]);
            for i in 1..w - 1 {
                canvas.put(x + i, y, '─');
                canvas.put(x + i, y + 2, '─');
            }
            canvas.put(x, y + 1, side);
            canvas.put(x + w - 1, y + 1, side);
            canvas.text(x + 2, y + 1, &node.label);
            x += w + GAP_X;
        }
    }
    let center = |n: usize| place[n].0 + box_width(n) / 2;
    for (i, edge) in graph.edges.iter().enumerate() {
        if back.contains(&i) {
            continue;
        }
        let (fx, fy) = (center(edge.from), place[edge.from].1 + 3);
        let (tx, ty) = (center(edge.to), place[edge.to].1);
        let turn = ty - GAP_Y + 1;
        for y in fy..turn {
            canvas.link(fx, y, UP | DOWN);
        }
        canvas.link(fx, turn - 1, DOWN);
        let (low, high) = (fx.min(tx), fx.max(tx));
        for x in low..=high {
            let mut bits = 0;
            if x > low {
                bits |= LEFT;
            }
            if x < high {
                bits |= RIGHT;
            }
            if x == fx {
                bits |= UP;
            }
            if x == tx {
                bits |= DOWN;
            }
            canvas.link(x, turn, bits);
        }
        for y in turn + 1..ty {
            canvas.link(tx, y, UP | DOWN);
        }
        canvas.put(tx, ty - 1, '▼');
        if let Some(label) = &edge.label {
            let at = tx + 2;
            if at + label.width() <= total {
                canvas.text(at, ty - 1, label);
            } else {
                let start = tx.saturating_sub(label.width() + 1);
                canvas.text(start, ty - 1, label);
            }
        }
    }
    let mut out = canvas.rows();
    for &i in &back {
        let edge = &graph.edges[i];
        let label = edge
            .label
            .as_deref()
            .map_or_else(String::new, |l| format!(" ({l})"));
        out.push(format!(
            "↩ {} → {}{label}",
            graph.nodes[edge.from].label, graph.nodes[edge.to].label
        ));
    }
    Some(out)
}

#[cfg(test)]
mod tests {
    use super::*;

    const LAMP: &str = "flowchart TD\n  A[Lamp doesn't work] --> B{Plugged in?}\n  B -- No --> C[Plug in lamp]\n  B -->|Yes| D(Bulb burned out?)\n";

    #[test]
    fn a_chart_keeps_every_label_as_real_text_with_lines_between() {
        let rows = render(LAMP, 80).expect("drawn");
        let text = rows.join("\n");
        for label in [
            "Lamp doesn't work",
            "Plugged in?",
            "Plug in lamp",
            "Bulb burned out?",
            "No",
            "Yes",
        ] {
            assert!(text.contains(label), "{label}\n{text}");
        }
        assert!(text.contains('▼'), "{text}");
        let y = |label: &str| rows.iter().position(|r| r.contains(label)).unwrap();
        assert!(y("Lamp doesn't") < y("Plugged in?"), "{text}");
        assert!(y("Plugged in?") < y("Plug in lamp"), "{text}");
        assert_eq!(y("Plug in lamp"), y("Bulb burned"), "siblings side by side");
    }

    #[test]
    fn chains_and_repeated_names_are_one_node() {
        let rows = render("graph LR\nA-->B-->C\nA-->C\n", 60).unwrap();
        let text = rows.join("\n");
        assert_eq!(text.matches('A').count(), 1, "{text}");
        assert_eq!(text.matches('C').count(), 1, "{text}");
    }

    #[test]
    fn a_cycle_is_said_in_words_and_does_not_hang() {
        let rows = render("flowchart TD\nA-->B\nB-->A\n", 40).unwrap();
        assert!(rows.iter().any(|r| r.contains("↩ B → A")), "{rows:?}");
        assert!(
            render("flowchart TD\nA-->A\n", 40)
                .unwrap()
                .iter()
                .any(|r| r.contains('↩'))
        );
    }

    #[test]
    fn what_it_cannot_draw_stays_source() {
        assert_eq!(
            render("sequenceDiagram\nA->>B: hi\n", 80),
            None,
            "another kind"
        );
        assert_eq!(
            render("flowchart TD\nA ~~> B\n", 80),
            None,
            "an unknown arrow"
        );
        assert_eq!(render("flowchart TD\n", 80), None, "nothing in it");
        assert_eq!(render(LAMP, 10), None, "no room");
        assert_eq!(render("", 80), None);
    }

    #[test]
    fn wide_labels_keep_the_box_aligned() {
        let rows = render("flowchart TD\nA[Лампа 灯]\n", 40).unwrap();
        let widths: Vec<usize> = rows.iter().map(|r| r.width()).collect();
        assert_eq!(widths[0], widths[1], "{rows:?}");
        assert_eq!(widths[1], widths[2], "{rows:?}");
    }

    #[test]
    fn hostile_labels_cannot_reach_the_screen_as_control_codes() {
        let rows = render("flowchart TD\nA[\u{1b}]0;x\u{7}evil]\n", 60);
        if let Some(rows) = rows {
            assert!(!rows.join("").contains('\u{1b}'));
        }
    }

    #[test]
    fn a_huge_chart_is_refused_rather_than_slow() {
        let mut source = String::from("flowchart TD\n");
        for i in 0..=MAX_NODES {
            source.push_str(&format!("N{i}-->N{}\n", i + 1));
        }
        assert_eq!(render(&source, 10_000), None);
    }
}
