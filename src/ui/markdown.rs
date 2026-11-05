use std::path::{Path, PathBuf};
use std::sync::Arc;

use ratatui::Frame;
use ratatui::layout::Rect;
use ratatui::style::{Color, Modifier, Style, Stylize};
use ratatui::text::{Line, Span};

use super::mouse::Hit;
use super::render::{Ctx, MarkdownCache};
use super::theme::Theme;
use crate::core::image_art::{ArtCell, ImageRequest, ImageState, MAX_CELLS, colored, mono, zoomed};
use crate::core::{Document, DocumentId};
use crate::lang::markdown::{MdKind, MdLine, MdSpan, MdStyle, line_for_source, render};

pub const MIN_PANE: u16 = 50;
const MAX_BYTES: usize = 2 * 1024 * 1024;
const PADDING: u16 = 1;
const MAX_ART_ROWS: u16 = 200;

fn style_of(style: MdStyle, theme: &Theme) -> Style {
    let base = match style.kind {
        MdKind::Text => Style::new().fg(theme.fg_primary),
        MdKind::Heading(1 | 2) => Style::new().fg(theme.accent),
        MdKind::Heading(_) => Style::new().fg(theme.syntax.keyword),
        MdKind::InlineCode => Style::new().fg(theme.syntax.string),
        MdKind::Link => Style::new()
            .fg(theme.accent)
            .add_modifier(Modifier::UNDERLINED),
        MdKind::Url | MdKind::Mark | MdKind::Html => Style::new().fg(theme.fg_muted),
        MdKind::Quote => Style::new()
            .fg(theme.fg_muted)
            .add_modifier(Modifier::ITALIC),
        MdKind::TableHeader => Style::new()
            .fg(theme.fg_primary)
            .add_modifier(Modifier::BOLD),
        MdKind::Code(Some(capture)) => theme.capture(capture),
        MdKind::Code(None) => Style::new().fg(theme.fg_primary),
        MdKind::Image => Style::new()
            .fg(theme.syntax.number)
            .add_modifier(Modifier::ITALIC),
        MdKind::Art { ink, paper } => art_style(ink, paper, theme),
    };
    let mut out = base;
    if style.bold {
        out = out.add_modifier(Modifier::BOLD);
    }
    if style.italic {
        out = out.add_modifier(Modifier::ITALIC);
    }
    if style.strike {
        out = out.add_modifier(Modifier::CROSSED_OUT);
    }
    out
}

fn art_color(color: [u8; 3], theme: &Theme) -> Option<Color> {
    let [r, g, b] = color;
    let rgb = Color::Rgb(r, g, b);
    match theme.bg {
        Color::Rgb(..) => Some(rgb),
        Color::Indexed(_) => Some(super::theme::nearest_256(rgb)),
        _ => None,
    }
}

fn lost_on_page(ink: [u8; 3], theme: &Theme) -> bool {
    let light = |[r, g, b]: [u8; 3]| i32::from(r) * 3 + i32::from(g) * 6 + i32::from(b);
    let page = match theme.bg {
        Color::Rgb(r, g, b) => [r, g, b],
        _ if dark_page(theme) => [0; 3],
        _ => [255; 3],
    };
    (light(ink) - light(page)).abs() < 500
}

fn art_style(ink: Option<[u8; 3]>, paper: Option<[u8; 3]>, theme: &Theme) -> Style {
    let mut style = Style::new().fg(theme.fg_primary);
    let visible = |c: &[u8; 3]| paper.is_some() || !lost_on_page(*c, theme);
    if let Some(ink) = ink.filter(visible).and_then(|c| art_color(c, theme)) {
        style = style.fg(ink);
    }
    if let Some(paper) = paper.and_then(|c| art_color(c, theme)) {
        style = style.bg(paper);
    }
    style
}

fn art_spans(cells: &[ArtCell], theme: &Theme) -> Vec<MdSpan> {
    let colors = art_color([0; 3], theme).is_some();
    let mut spans: Vec<MdSpan> = Vec::new();
    for &cell in cells {
        let (ch, kind) = if colors {
            let glyph = colored(cell);
            (
                glyph.ch,
                MdKind::Art {
                    ink: glyph.ink,
                    paper: glyph.paper,
                },
            )
        } else {
            (mono(cell, dark_page(theme)), MdKind::Code(None))
        };
        match spans.last_mut() {
            Some(last) if last.style.kind == kind => last.text.push(ch),
            _ => spans.push(MdSpan {
                text: ch.to_string(),
                style: MdStyle::of(kind),
            }),
        }
    }
    spans
}

fn line_of(line: &MdLine, theme: &Theme) -> Line<'static> {
    Line::from(
        line.spans
            .iter()
            .map(|s| Span::styled(s.text.clone(), style_of(s.style, theme)))
            .collect::<Vec<_>>(),
    )
}

fn document_dir(doc: &Document) -> PathBuf {
    doc.path()
        .parent()
        .map_or_else(PathBuf::new, Path::to_path_buf)
}

fn preview_request(dir: &Path, source: &str, width: usize) -> ImageRequest {
    ImageRequest {
        dir: dir.to_path_buf(),
        source: source.to_owned(),
        cols: u16::try_from(width)
            .unwrap_or(MAX_CELLS)
            .clamp(1, MAX_CELLS),
        rows: MAX_ART_ROWS,
    }
}

fn dark_page(theme: &Theme) -> bool {
    match theme.bg {
        Color::Rgb(r, g, b) => u32::from(r) * 3 + u32::from(g) * 6 + u32::from(b) < 1280,
        _ => true,
    }
}

fn plain_line(text: String, kind: MdKind, source: usize, image: Option<String>) -> MdLine {
    MdLine {
        spans: vec![MdSpan {
            text,
            style: MdStyle::of(kind),
        }],
        source,
        image,
    }
}

fn with_pictures(
    lines: &Arc<Vec<MdLine>>,
    doc: &Document,
    width: usize,
    ws: &crate::core::Workspace,
    theme: &Theme,
) -> Arc<Vec<MdLine>> {
    if lines.iter().all(|l| l.image.is_none()) {
        return Arc::clone(lines);
    }
    let dir = document_dir(doc);
    let mut out = Vec::with_capacity(lines.len());
    for line in lines.iter() {
        let Some(source) = &line.image else {
            out.push(line.clone());
            continue;
        };
        match ws.image_state(&preview_request(&dir, source, width)) {
            Some(ImageState::Ready(image)) => {
                out.extend((0..image.rows).map(|y| MdLine {
                    spans: art_spans(image.row(y, 0, image.cols), theme),
                    source: line.source,
                    image: None,
                }));
            }
            Some(ImageState::Failed(reason)) => {
                let text: Vec<char> = format!("[no image: {reason}]").chars().collect();
                out.extend(text.chunks(width.max(1)).map(|part| {
                    plain_line(part.iter().collect(), MdKind::Mark, line.source, None)
                }));
            }
            Some(ImageState::Decoding) | None => out.push(plain_line(
                "[decoding image…]".to_owned(),
                MdKind::Mark,
                line.source,
                Some(source.clone()),
            )),
        }
    }
    Arc::new(out)
}

pub fn render_picture(frame: &mut Frame, area: Rect, doc: &Document, cx: &mut Ctx<'_>) {
    let theme = cx.theme;
    frame
        .buffer_mut()
        .set_style(area, Style::new().bg(theme.bg));
    let (room_cols, room_rows) = (
        area.width.saturating_sub(PADDING * 2),
        area.height.saturating_sub(2),
    );
    let view = cx
        .ws
        .active()
        .map_or_else(Default::default, |(v, _)| cx.ws.picture_view(v.doc));
    let request = ImageRequest {
        dir: document_dir(doc),
        source: doc.path().display().to_string(),
        cols: zoomed(room_cols, view.zoom),
        rows: zoomed(room_rows, view.zoom),
    };
    let name = doc.file_name();
    let state = cx.ws.image_state(&request);
    let caption = match state {
        Some(ImageState::Ready(image)) => format!(
            "{name}  {}×{}  {}%  + - zoom, 0 fit, arrows move, o open",
            image.width,
            image.height,
            100u32 << view.zoom,
        ),
        Some(ImageState::Failed(reason)) => format!("{name}: cannot show it, {reason}"),
        Some(ImageState::Decoding) | None => format!("{name}: decoding…"),
    };
    let row_at = |y| {
        Rect::new(
            area.x + PADDING,
            y,
            area.width.saturating_sub(PADDING * 2),
            1,
        )
    };
    frame.render_widget(
        Line::from(caption).fg(theme.fg_muted),
        row_at(area.y).intersection(area),
    );
    if let Some(ImageState::Ready(image)) = state {
        for (y, row) in (area.y + 2..area.bottom()).zip(view.y..image.rows) {
            let spans = art_spans(image.row(row, view.x, room_cols), theme);
            let line = line_of(
                &MdLine {
                    spans,
                    source: 0,
                    image: None,
                },
                theme,
            );
            frame.render_widget(line, row_at(y));
        }
    } else if state.is_none() {
        cx.ui.image_wants.push(request);
    }
}

pub fn render_preview(
    frame: &mut Frame,
    area: Rect,
    id: DocumentId,
    doc: &Document,
    cursor: usize,
    cx: &mut Ctx<'_>,
) {
    let theme = cx.theme;
    frame
        .buffer_mut()
        .set_style(area, Style::new().bg(theme.bg));
    cx.ui.hits.push(area, Hit::Preview { doc: id });
    if doc.text().len_bytes() > MAX_BYTES {
        frame.render_widget(Line::from(" Too large to preview").fg(theme.fg_muted), area);
        return;
    }
    let width = usize::from(area.width.saturating_sub(PADDING * 2));
    let cache = cx.ui.markdown.entry(id).or_insert_with(|| MarkdownCache {
        version: u64::MAX,
        width: 0,
        lines: Arc::new(Vec::new()),
        top: 0,
        followed: None,
    });
    if cache.version != doc.version() || cache.width != width {
        cache.lines = Arc::new(render(&doc.text().to_string(), width));
        cache.version = doc.version();
        cache.width = width;
    }
    let lines = with_pictures(&Arc::clone(&cache.lines), doc, width, cx.ws, theme);
    let Some(cache) = cx.ui.markdown.get_mut(&id) else {
        return;
    };
    let height = usize::from(area.height);
    if cache.followed != Some(cursor) {
        cache.followed = Some(cursor);
        let target = line_for_source(&lines, cursor);
        if target < cache.top || target >= cache.top + height {
            cache.top = target.saturating_sub(height / 3);
        }
    }
    cache.top = cache
        .top
        .min(lines.len().saturating_sub(height.min(lines.len())));
    let top = cache.top;
    let dir = document_dir(doc);
    for line in lines.iter().skip(top).take(height) {
        if let Some(source) = &line.image {
            let request = preview_request(&dir, source, width);
            if cx.ws.image_state(&request).is_none() {
                cx.ui.image_wants.push(request);
            }
        }
    }
    for (y, line) in (area.y..area.bottom()).zip(lines.iter().skip(top)) {
        let row = Rect::new(
            area.x + PADDING,
            y,
            area.width.saturating_sub(PADDING * 2),
            1,
        );
        frame.render_widget(line_of(line, theme), row);
    }
}

#[cfg(test)]
mod art_tests {
    use super::*;

    #[test]
    fn a_line_as_dark_as_the_page_gets_the_text_color_unless_it_has_its_own_paper() {
        let theme = Theme::DARCULA;
        let black = Some([0; 3]);
        assert_eq!(art_style(black, None, &theme).fg, Some(theme.fg_primary));
        assert_eq!(
            art_style(black, Some([255, 255, 0]), &theme).fg,
            Some(Color::Rgb(0, 0, 0))
        );
        let red = Some([230, 40, 40]);
        assert_eq!(
            art_style(red, None, &theme).fg,
            Some(Color::Rgb(230, 40, 40))
        );
    }
}
