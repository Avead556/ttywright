use std::ops::Range;
use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::{Constraint, Layout as Split, Position, Rect};
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};
use ropey::Rope;
use unicode_width::UnicodeWidthStr;

use super::glyphs;
use super::history::age;
use super::mouse::Hit;
use super::render::{Ctx, GitMarks, ensure_visible};
use super::text::{ellipsize, styled_text};
use super::theme::Theme;
use crate::core::conflict::is_conflict_marker;
use crate::core::debug::{Breakpoint, BreakpointState};
use crate::core::{
    Action, Axis, BlameView, Conflict, Diagnostic, Document, DocumentId, FindOptions, Focus,
    Layout, LineMark, PaneId, Severity, line_marks, text,
    wrap::{Metrics, Row},
};
use crate::lang::syntax::Span as HighlightSpan;

const TAB_CLOSE_CELLS: u16 = 3;

const BLAME_AUTHOR: usize = 10;
const BLAME_AGE: usize = 10;
const BLAME_HASH: usize = 7;
const BLAME_FULL_PANE: u16 = 80;

const MAX_MARKED_BYTES: usize = 1024 * 1024;

pub fn render_layout(frame: &mut Frame, area: Rect, node: &Layout, cx: &mut Ctx<'_>) {
    match node {
        Layout::Pane(id) => render_pane(frame, area, *id, cx),
        Layout::Split { axis, children } => {
            let n = children.len();
            let mut constraints = Vec::with_capacity(n * 2);
            for i in 0..n {
                if i > 0 {
                    constraints.push(Constraint::Length(1));
                }
                constraints.push(Constraint::Fill(1));
            }
            let cells = match axis {
                Axis::Columns => Split::horizontal(constraints).split(area),
                Axis::Rows => Split::vertical(constraints).split(area),
            };
            for (i, cell) in cells.iter().enumerate() {
                if i % 2 == 1 {
                    let line = match axis {
                        Axis::Columns => glyphs::V_LINE,
                        Axis::Rows => glyphs::H_LINE,
                    };
                    let fill = line.repeat(usize::from(cell.width.max(1)));
                    for y in cell.y..cell.y + cell.height {
                        let row = Rect::new(cell.x, y, cell.width, 1);
                        frame.render_widget(
                            Line::from(fill.clone()).fg(cx.theme.border).bg(cx.theme.bg),
                            row,
                        );
                    }
                } else {
                    render_layout(frame, *cell, &children[i / 2], cx);
                }
            }
        }
    }
}

fn render_pane(frame: &mut Frame, area: Rect, pane_id: PaneId, cx: &mut Ctx<'_>) {
    let ws = cx.ws;
    let theme = cx.theme;
    let Some(pane) = ws.pane(pane_id) else {
        return;
    };
    let is_active = ws.active_pane() == pane_id;
    let show_find = is_active && ws.find_bar().is_some();
    let replacing = ws.find_bar().is_some_and(|b| b.replacement.is_some());
    let find_rows = u16::from(show_find) + u16::from(show_find && replacing);
    let conflict = pane
        .active_view()
        .and_then(|v| ws.document(ws.view(v)?.doc)?.conflict());
    let merge_conflict = pane.active_view().and_then(|v| {
        let view = ws.view(v)?;
        let line = ws.document(view.doc)?.text().char_to_line(view.cursor.idx);
        ws.conflict_position(view.doc, line)
    });
    let [tabs, underline, banner, find, body] = Split::vertical([
        Constraint::Length(1),
        Constraint::Length(1),
        Constraint::Length(u16::from(conflict.is_some() || merge_conflict.is_some())),
        Constraint::Length(find_rows),
        Constraint::Fill(1),
    ])
    .areas(area);
    let preview = pane
        .active_view()
        .and_then(|v| ws.view(v))
        .filter(|v| ws.markdown_preview(v.doc))
        .map(|v| v.doc);
    let alone = preview.is_some() && body.width < super::markdown::MIN_PANE;
    if is_active {
        cx.ui.preview_alone = preview.filter(|_| alone);
    }
    let (body, preview_area) = if alone {
        (body, Some(body))
    } else if preview.is_some() {
        let [text, line, shown] = Split::horizontal([
            Constraint::Fill(1),
            Constraint::Length(1),
            Constraint::Fill(1),
        ])
        .areas(body);
        for y in line.y..line.bottom() {
            frame.render_widget(
                Line::from(glyphs::V_LINE).fg(theme.border).bg(theme.bg),
                Rect::new(line.x, y, 1, 1),
            );
        }
        (text, Some(shown))
    } else {
        (body, None)
    };

    if pane.views.is_empty() {
        frame.render_widget(Line::default().bg(theme.bg_panel), tabs);
        frame.render_widget(Line::default().bg(theme.bg), underline);
        render_empty_hints(frame, body, cx);
        return;
    }

    if let Some(conflict) = conflict {
        let text = match conflict {
            Conflict::Modified => {
                " File changed on disk: palette → Reload from Disk, Keep My Version, Show Local History"
            }
            Conflict::Deleted => " File deleted on disk: Save or Keep My Version recreates it",
        };
        frame.render_widget(Line::from(text).fg(theme.error).bg(theme.bg_panel), banner);
    } else if let Some((at, of)) = merge_conflict {
        let text = if of == 0 {
            " No markers left: Space in the commit view marks it resolved".to_owned()
        } else {
            format!(" Conflict {at} of {of}: in the commit view N/P step, O ours, T theirs, B both")
        };
        frame.render_widget(
            Line::from(text).fg(theme.warning).bg(theme.bg_panel),
            banner,
        );
    }

    let labels: Vec<(String, bool, Color, bool)> = pane
        .views
        .iter()
        .enumerate()
        .filter_map(|(i, v)| {
            let doc = ws.document(ws.view(*v)?.doc)?;
            let active = i == pane.active;
            let marker = match (doc.is_dirty(), active) {
                (true, _) => format!(" {}", glyphs::MODIFIED),
                (false, true) => format!(" {}", glyphs::CLOSE),
                (false, false) => String::new(),
            };
            let color = theme.file_color(doc.path());
            let label = format!(" {}{marker} ", doc.file_name());
            Some((label, active, color, doc.is_dirty()))
        })
        .collect();
    let width = usize::from(tabs.width);
    let mut first = 0;
    while first < pane.active
        && labels[first..=pane.active]
            .iter()
            .map(|(l, ..)| l.width())
            .sum::<usize>()
            > width
    {
        first += 1;
    }
    let mut spans = Vec::new();
    let mut underline_at = None;
    let mut x = 0;
    for ((label, active, color, dirty), view) in labels[first..].iter().zip(&pane.views[first..]) {
        let cell = Rect::new(tabs.x + x as u16, tabs.y, label.width() as u16, 1).intersection(tabs);
        cx.ui.hits.push(cell, Hit::Tab { view: *view });
        if *active || *dirty {
            let target = TAB_CLOSE_CELLS.min(cell.width);
            let close = Rect::new(cell.right() - target, cell.y, target, 1);
            cx.ui.hits.push(close, Hit::CloseTab { view: *view });
        }
        let style = if *active {
            Style::new()
                .fg(*color)
                .bg(theme.bg)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.fg_muted)
        };
        if *active {
            underline_at = Some((x, label.width()));
        }
        x += label.width();
        let dot = format!("{} ", glyphs::MODIFIED);
        match label.strip_suffix(&dot).filter(|_| *dirty) {
            Some(name) => {
                spans.push(Span::styled(name.to_owned(), style));
                spans.push(Span::styled(dot, style.fg(theme.warning)));
            }
            None => spans.push(Span::styled(label.clone(), style)),
        }
    }
    frame.render_widget(Line::from(spans).bg(theme.bg_panel), tabs);
    let markdown_open = pane
        .active_view()
        .and_then(|v| ws.view(v))
        .and_then(|v| ws.document(v.doc))
        .is_some_and(|d| d.language() == crate::core::Language::Markdown);
    if is_active && markdown_open {
        let label = if preview.is_some() {
            " ☰ Text only "
        } else {
            " ◧ Preview "
        };
        let width = label.width() as u16;
        let cell =
            Rect::new(tabs.right().saturating_sub(width), tabs.y, width, 1).intersection(tabs);
        frame.render_widget(Line::from(label).fg(theme.accent).bg(theme.bg_panel), cell);
        cx.ui.hits.push(cell, Hit::MarkdownSwitch);
    }
    let bar_color = if is_active && ws.focus() == Focus::Editor {
        theme.accent
    } else {
        theme.fg_subtle
    };
    let (ux, uw) = underline_at.unwrap_or((0, 0));
    frame.render_widget(
        Line::from(format!(
            "{}{}",
            " ".repeat(ux),
            glyphs::TAB_UNDERLINE.repeat(uw)
        ))
        .fg(bar_color)
        .bg(theme.bg),
        underline,
    );

    let Some(view_id) = pane.active_view() else {
        return;
    };
    let (Some(view), Some(doc)) = (
        ws.view(view_id),
        ws.view(view_id).and_then(|v| ws.document(v.doc)),
    ) else {
        return;
    };
    if doc.is_image() {
        super::markdown::render_picture(frame, body, doc, cx);
        return;
    }
    let rope = doc.text();
    let cursor = view.cursor.idx;
    if let (Some(area), Some(id)) = (preview_area, preview) {
        super::markdown::render_preview(frame, area, id, doc, rope.char_to_line(cursor), cx);
    }
    if alone {
        if show_find {
            render_find_bar(frame, find, cursor, cx);
        }
        return;
    }

    if show_find {
        render_find_bar(frame, find, cursor, cx);
    }

    let cursor_line = rope.char_to_line(cursor);
    let cursor_col = text::col_of(rope, cursor, ws.tab_width());
    let digits = rope.len_lines().to_string().len().max(3);
    let blame = ws.blame(view.doc);
    let blame_cols = blame.as_ref().map_or(0, |_| blame_width(body.width));
    let gutter = digits + 4 + blame_cols;
    let text_cols = usize::from(body.width).saturating_sub(gutter);
    let rows = usize::from(body.height);
    let metrics = Metrics {
        tab_width: ws.tab_width(),
        wrap: (ws.soft_wrap() && text_cols > 0).then_some(text_cols),
    };
    let (_, cursor_row, cursor_row_col) = metrics.position(rope, cursor);
    let marker = (cursor, doc.version(), metrics.wrap);
    let before = cx.ui.followed.insert(view_id, marker);
    let scroll = cx.ui.scroll.entry(view_id).or_default();
    if before.is_some_and(|b| b.2.is_some() != metrics.wrap.is_some()) {
        scroll.1 = 0;
    }
    if before != Some(marker) {
        *scroll = match metrics.wrap {
            Some(_) => metrics.follow(rope, *scroll, (cursor_line, cursor_row), rows),
            None => ensure_visible(*scroll, (cursor_line, cursor_col), (rows, text_cols)),
        };
    } else {
        scroll.0 = scroll.0.min(rope.len_lines().saturating_sub(1));
    }
    let (scroll_row, scroll_col) = *scroll;
    cx.ui.hits.push(
        body,
        Hit::Text {
            view: view_id,
            origin: Position::new(body.x + gutter as u16, body.y),
            top: scroll_row,
            left: scroll_col,
            wrap: metrics.wrap,
        },
    );
    if is_active {
        cx.ui.viewport = text_cols;
    }
    if is_active && ws.focus() == Focus::Editor {
        cx.ui.page_rows = rows;
    }

    let visible = scroll_row..(scroll_row + rows).min(rope.len_lines());
    let highlights = doc.syntax().map(|s| s.highlight(rope, visible.clone()));
    let matches = ws
        .find_bar()
        .filter(|_| is_active)
        .map(|b| b.matches.as_slice())
        .unwrap_or(&[]);
    let base = Style::new().fg(theme.fg_primary);
    let selection = ws.view(view_id).and_then(|v| v.cursor.selection());
    let marks = git_marks(view.doc, doc, cx);
    let paint = LinePaint {
        theme,
        base,
        highlights: highlights.as_deref(),
        first_line: scroll_row,
        matches,
        cursor,
        selection,
        diagnostics: ws.diagnostics(doc.path()),
    };

    let stop_line = ws.debug_line(doc.path());
    let mut screen_row = 0;
    let mut line_idx = scroll_row;
    let mut first_row = if metrics.wrap.is_some() {
        scroll_col
    } else {
        0
    };
    while screen_row < rows && line_idx < rope.len_lines() {
        let content = text::line_content(rope, line_idx);
        let mut styles = paint.styles(rope, line_idx, content.chars().count());
        if merge_conflict.is_some() && is_conflict_marker(&content) {
            styles.fill(Style::new().fg(theme.warning).add_modifier(Modifier::BOLD));
        }
        let line_rows = if metrics.wrap.is_some() {
            metrics.rows(rope, line_idx)
        } else {
            vec![Row {
                chars: 0..content.chars().count(),
                cols: scroll_col..scroll_col + text_cols,
            }]
        };
        let is_current = line_idx == cursor_line;
        let number_style = if is_current {
            Style::new()
                .fg(theme.fg_primary)
                .add_modifier(Modifier::BOLD)
        } else {
            Style::new().fg(theme.fg_subtle)
        };
        let number_style = if cx.ws.is_bookmarked(doc.path(), line_idx) {
            number_style
                .fg(theme.warning)
                .add_modifier(Modifier::REVERSED)
        } else {
            number_style
        };
        let stopped_here = stop_line == Some(line_idx);
        let breakpoint = cx.ws.breakpoint_at(doc.path(), line_idx);
        let gutter_mark = if stopped_here {
            Span::raw(glyphs::EXECUTION).fg(theme.warning).bold()
        } else {
            breakpoint.map_or_else(|| Span::raw(" "), |b| breakpoint_mark(b, theme))
        };
        let covered = cx.ws.coverage_mark(doc, line_idx);
        let edge = match marks.get(line_idx).copied().flatten() {
            _ if covered == Some(true) => Span::raw(glyphs::COVERED).fg(theme.success),
            _ if covered == Some(false) => Span::raw(glyphs::NOT_COVERED).fg(theme.error),
            Some(LineMark::Added) => Span::raw(glyphs::V_MARK).fg(theme.success),
            Some(LineMark::Modified) => Span::raw(glyphs::V_MARK).fg(theme.accent),
            Some(LineMark::DeletedBelow) => Span::raw(glyphs::DELETED_MARK).fg(theme.error),
            None => Span::raw(glyphs::V_LINE).fg(theme.border),
        };
        let bg = if stopped_here {
            theme.selected_inactive.bg.unwrap_or(theme.bg_current_line)
        } else if is_current {
            theme.bg_current_line
        } else {
            theme.bg
        };
        let inline = cx.ws.debug_inline_values(doc.path(), line_idx, &content);
        for (row_idx, cell) in line_rows.iter().enumerate().skip(first_row) {
            if screen_row == rows {
                break;
            }
            let area = Rect::new(body.x, body.y + screen_row as u16, body.width, 1);
            let number = if row_idx == 0 {
                (line_idx + 1).to_string()
            } else {
                String::new()
            };
            let mut spans = Vec::new();
            if let Some(blame) = &blame {
                spans.push(blame_cell(
                    blame, line_idx, row_idx, screen_row, blame_cols, cx,
                ));
            }
            let number_x = body.x + blame_cols as u16;
            cx.ui.hits.push(
                Rect::new(number_x, area.y, digits as u16 + 3, 1).intersection(body),
                Hit::Gutter {
                    view: view_id,
                    line: line_idx,
                },
            );
            let mark = if row_idx == 0 {
                gutter_mark.clone()
            } else {
                Span::raw(" ")
            };
            spans.extend([
                mark,
                Span::styled(format!("{number:>digits$} "), number_style),
                edge.clone(),
                Span::raw(" "),
            ]);
            spans.extend(styled_text(
                &content,
                &styles,
                base,
                cell.cols.start,
                cell.cols.len(),
                ws.tab_width(),
            ));
            if row_idx + 1 == line_rows.len() && !inline.is_empty() {
                spans.push(
                    Span::raw(format!("   {}", inline.join("  ")))
                        .fg(theme.fg_subtle)
                        .italic(),
                );
            }
            frame.render_widget(Line::from(spans).bg(bg), area);
            if is_active
                && is_current
                && row_idx == cursor_row
                && ws.focus() == Focus::Editor
                && ws.picker().is_none()
            {
                let col = if metrics.wrap.is_some() {
                    cursor_row_col.min(text_cols.saturating_sub(1))
                } else {
                    cursor_col.saturating_sub(scroll_col)
                };
                if let Ok(x) = u16::try_from(usize::from(body.x) + gutter + col) {
                    let at = Position::new(x, area.y);
                    frame.set_cursor_position(at);
                    cx.ui.editor_cursor = Some(at);
                }
            }
            screen_row += 1;
        }
        first_row = 0;
        line_idx += 1;
    }
    while screen_row < rows {
        let area = Rect::new(body.x, body.y + screen_row as u16, body.width, 1);
        frame.render_widget(Line::default().bg(theme.bg), area);
        screen_row += 1;
    }
}

fn breakpoint_mark(b: &Breakpoint, theme: &Theme) -> Span<'static> {
    let shape = if !b.enabled || matches!(b.state, BreakpointState::Pending(_)) {
        glyphs::BREAKPOINT_OFF
    } else if b.log_message.is_some() {
        glyphs::LOGPOINT
    } else if b.condition.is_some() {
        glyphs::BREAKPOINT_CONDITION
    } else {
        glyphs::BREAKPOINT
    };
    let color = if b.enabled {
        theme.error
    } else {
        theme.fg_subtle
    };
    Span::raw(shape).fg(color)
}

fn blame_width(pane: u16) -> usize {
    if pane >= BLAME_FULL_PANE {
        BLAME_AUTHOR + BLAME_AGE + BLAME_HASH + 3
    } else {
        BLAME_HASH + 1
    }
}

fn blame_cell(
    blame: &BlameView<'_>,
    line: usize,
    row: usize,
    screen_row: usize,
    cols: usize,
    cx: &Ctx<'_>,
) -> Span<'static> {
    let theme = cx.theme;
    let blank = " ".repeat(cols);
    let Some(known) = blame.blame else {
        if screen_row == 0 {
            let text = pad_to(&ellipsize("stale", cols.saturating_sub(1)), cols);
            return Span::raw(text).fg(theme.fg_subtle).italic();
        }
        return Span::raw(blank);
    };
    let Some(commit) = known.line(line).filter(|_| row == 0) else {
        return Span::raw(blank);
    };
    if commit.is_uncommitted() {
        let text = pad_to(&ellipsize("not committed", cols.saturating_sub(1)), cols);
        return Span::raw(text).fg(theme.fg_subtle);
    }
    let hash: String = commit.hash.chars().take(BLAME_HASH).collect();
    if cols <= BLAME_HASH + 1 {
        return Span::raw(pad_to(&hash, cols)).fg(theme.syntax.attribute);
    }
    let author = pad_to(&ellipsize(&commit.author, BLAME_AUTHOR), BLAME_AUTHOR);
    let when = pad_to(&age(blame.now, commit.time), BLAME_AGE);
    Span::raw(format!("{author} {when} {hash} ")).fg(theme.fg_muted)
}

fn pad_to(text: &str, cols: usize) -> String {
    format!("{text}{}", " ".repeat(cols.saturating_sub(text.width())))
}

struct LinePaint<'a> {
    theme: &'a Theme,
    base: Style,
    highlights: Option<&'a [Vec<HighlightSpan>]>,
    first_line: usize,
    matches: &'a [Range<usize>],
    cursor: usize,
    selection: Option<Range<usize>>,
    diagnostics: &'a [Diagnostic],
}

impl LinePaint<'_> {
    fn styles(&self, rope: &Rope, line_idx: usize, len: usize) -> Vec<Style> {
        let theme = self.theme;
        let mut styles = vec![self.base; len];
        let spans = self
            .highlights
            .and_then(|h| h.get(line_idx.checked_sub(self.first_line)?));
        for span in spans.into_iter().flatten() {
            let style = theme.capture(span.capture);
            for s in styles
                .iter_mut()
                .take(span.chars.end.min(len))
                .skip(span.chars.start)
            {
                *s = style;
            }
        }
        let line_start = rope.line_to_char(line_idx);
        for m in self
            .matches
            .iter()
            .filter(|m| m.end > line_start && m.start < line_start + len)
        {
            let highlight = if m.start == self.cursor {
                theme.selected
            } else {
                theme.matched
            };
            let from = m.start.saturating_sub(line_start);
            let to = (m.end - line_start).min(len);
            for s in &mut styles[from..to] {
                *s = s.patch(highlight);
            }
        }
        if let Some(selected) = &self.selection
            && selected.end > line_start
            && selected.start < line_start + len
        {
            let from = selected.start.saturating_sub(line_start);
            let to = (selected.end - line_start).min(len);
            for s in &mut styles[from..to] {
                *s = s.patch(theme.selected);
            }
        }
        for d in self.diagnostics {
            let Some(chars) = &d.chars else { continue };
            if chars.end <= line_start || chars.start >= line_start + len.max(1) {
                continue;
            }
            let from = chars.start.saturating_sub(line_start).min(len);
            let to = (chars.end - line_start)
                .clamp(from + 1, len.max(from + 1))
                .min(len);
            let style = Style::new()
                .fg(severity_color(d.raw.severity, theme))
                .add_modifier(Modifier::UNDERLINED);
            for s in styles.iter_mut().take(to).skip(from) {
                *s = s.patch(style);
            }
        }
        styles
    }
}

fn git_marks(id: DocumentId, doc: &Document, cx: &mut Ctx<'_>) -> Arc<Vec<Option<LineMark>>> {
    let (base, stamp) = cx.ws.git_base(doc.path());
    let Some(base) = base.filter(|_| doc.text().len_bytes() <= MAX_MARKED_BYTES) else {
        return Arc::default();
    };
    let cached = cx.ui.git_marks.get(&id);
    if let Some(found) = cached.filter(|c| c.version == doc.version() && c.base_stamp == stamp) {
        return Arc::clone(&found.marks);
    }
    let marks = Arc::new(line_marks(base, &doc.text().to_string()));
    cx.ui.git_marks.insert(
        id,
        GitMarks {
            version: doc.version(),
            base_stamp: stamp,
            marks: Arc::clone(&marks),
        },
    );
    marks
}

fn render_find_bar(frame: &mut Frame, area: Rect, cursor: usize, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    let Some(bar) = cx.ws.find_bar() else {
        return;
    };
    let query = bar.input.text();
    let counter = if let Some(error) = &bar.error {
        Span::raw(error.clone()).fg(theme.error)
    } else if query.is_empty() {
        Span::raw("")
    } else if bar.matches.is_empty() {
        Span::raw("No matches").fg(theme.error)
    } else {
        let n = bar
            .current(cursor)
            .map_or("?".to_owned(), |i| (i + 1).to_string());
        Span::raw(format!("{n}/{}", bar.matches.len())).fg(theme.fg_muted)
    };
    let prompt = format!(" {} ", glyphs::PROMPT);
    let line = Line::from(vec![
        Span::raw(prompt.clone()).fg(theme.accent),
        Span::raw(query.to_owned()).fg(theme.fg_primary),
        Span::raw("   "),
        counter,
    ]);
    let query_row = Rect::new(area.x, area.y, area.width, 1);
    frame.render_widget(line.bg(theme.bg_panel), query_row);
    frame.render_widget(find_options(cx.ws.bar_options(), theme), query_row);
    let replace_row = Rect::new(area.x, area.y + 1, area.width, 1);
    if let Some(replacement) = bar.replacement.as_ref().filter(|_| area.height > 1) {
        let all = cx.keymap.label(Action::ReplaceAll).unwrap_or("palette");
        let hint = format!("⏎ replace  {all} all  Tab switch");
        let line = Line::from(vec![
            Span::raw(prompt.clone()).fg(theme.accent),
            Span::raw(replacement.text().to_owned()).fg(theme.fg_primary),
        ]);
        frame.render_widget(line.bg(theme.bg_panel), replace_row);
        frame.render_widget(
            Line::from(hint).fg(theme.fg_muted).right_aligned(),
            replace_row,
        );
    }
    if cx.ws.focus() == Focus::FindBar && cx.ws.picker().is_none() {
        let (field, row) = match &bar.replacement {
            Some(replacement) if bar.in_replacement && area.height > 1 => {
                (replacement, replace_row)
            }
            _ => (&bar.input, query_row),
        };
        let before: String = field.text().chars().take(field.cursor()).collect();
        let x = row.x + (prompt.width() + before.width()) as u16;
        frame.set_cursor_position(Position::new(x.min(row.right().saturating_sub(1)), row.y));
    }
}

pub fn find_options(options: FindOptions, theme: &Theme) -> Line<'static> {
    let chip = |on: bool, label: &str| {
        if on {
            Span::raw(format!("[{label}] ")).fg(theme.accent).bold()
        } else {
            Span::raw(format!(" {label}  ")).fg(theme.fg_subtle)
        }
    };
    if options.structural {
        return Line::from(chip(true, "$X$")).right_aligned();
    }
    Line::from(vec![
        chip(options.case_sensitive, "Aa"),
        chip(options.whole_word, "W"),
        chip(options.regex, ".*"),
    ])
    .right_aligned()
}

fn render_empty_hints(frame: &mut Frame, area: Rect, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    frame.render_widget(Line::default().bg(theme.bg), area);
    let hints = [
        ("Go to File", cx.keymap.label(crate::core::Action::GotoFile)),
        (
            "Find Action",
            cx.keymap.label(crate::core::Action::OpenPalette),
        ),
        (
            "Find in Files",
            cx.keymap.label(crate::core::Action::FindInProject),
        ),
        (
            "Project",
            cx.keymap.label(crate::core::Action::ToggleProject),
        ),
    ];
    let top = area.y + area.height.saturating_sub(hints.len() as u16) / 2;
    for (i, (title, key)) in hints.iter().enumerate() {
        let y = top + i as u16;
        if y >= area.bottom() {
            break;
        }
        let line = Line::from(vec![
            Span::raw(format!("{title}  ")).fg(theme.fg_muted),
            Span::raw(key.unwrap_or("")).fg(theme.accent),
        ]);
        frame.render_widget(line.centered(), Rect::new(area.x, y, area.width, 1));
    }
}

pub fn severity_color(severity: Severity, theme: &Theme) -> ratatui::style::Color {
    match severity {
        Severity::Error => theme.error,
        Severity::Warning => theme.warning,
        Severity::Info | Severity::Hint => theme.fg_muted,
    }
}
