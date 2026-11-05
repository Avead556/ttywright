use std::path::Path;

use serde::{Deserialize, Serialize};

const EXTENSIONS: [&str; 6] = ["png", "jpg", "jpeg", "gif", "webp", "bmp"];
pub const SAMPLES_X: u32 = 4;
pub const SAMPLES_Y: u32 = 8;
const SAMPLES: usize = 32;
const FLAT: u8 = 8;
const BRAILLE: u32 = 0x2800;
const FULL: u8 = 0xFF;
const DOT_BITS: [[u8; 2]; 4] = [[0x01, 0x08], [0x02, 0x10], [0x04, 0x20], [0x40, 0x80]];
const BRAILLE_HANDICAP: (u64, u64) = (3, 2);

const fn rows_mask(rows: [u8; 8]) -> u32 {
    let mut mask = 0;
    let mut y = 0;
    while y < 8 {
        mask |= (rows[y] as u32) << (y * 4);
        y += 1;
    }
    mask
}

const fn lower(n: usize) -> u32 {
    let mut rows = [0; 8];
    let mut y = 8 - n;
    while y < 8 {
        rows[y] = 0xF;
        y += 1;
    }
    rows_mask(rows)
}

const fn left(n: u32) -> u32 {
    let row = ((1u32 << n) - 1) as u8;
    rows_mask([row; 8])
}

const fn quads(top: u8, bottom: u8) -> u32 {
    rows_mask([top, top, top, top, bottom, bottom, bottom, bottom])
}

const SHAPES: [(char, u32); 26] = [
    ('\u{2581}', lower(1)),
    ('\u{2582}', lower(2)),
    ('\u{2583}', lower(3)),
    ('\u{2584}', lower(4)),
    ('\u{2585}', lower(5)),
    ('\u{2586}', lower(6)),
    ('\u{2587}', lower(7)),
    ('\u{2580}', quads(0xF, 0)),
    ('\u{2594}', rows_mask([0xF, 0, 0, 0, 0, 0, 0, 0])),
    ('\u{258E}', left(1)),
    ('\u{258C}', left(2)),
    ('\u{258A}', left(3)),
    ('\u{2590}', quads(0xC, 0xC)),
    ('\u{2595}', rows_mask([0x8; 8])),
    ('\u{2598}', quads(0x3, 0)),
    ('\u{259D}', quads(0xC, 0)),
    ('\u{2596}', quads(0, 0x3)),
    ('\u{2597}', quads(0, 0xC)),
    ('\u{259A}', quads(0x3, 0xC)),
    ('\u{259E}', quads(0xC, 0x3)),
    ('\u{2599}', quads(0x3, 0xF)),
    ('\u{259B}', quads(0xF, 0x3)),
    ('\u{259C}', quads(0xF, 0xC)),
    ('\u{259F}', quads(0xC, 0xF)),
    ('\u{2588}', u32::MAX),
    ('\u{2589}', rows_mask([0x7; 8])),
];
pub const MAX_ZOOM: u8 = 4;
pub const MAX_CELLS: u16 = 1000;
pub const MAX_KEPT: usize = 64;

pub fn has_image_extension(path: &Path) -> bool {
    path.extension()
        .and_then(|e| e.to_str())
        .is_some_and(|e| EXTENSIONS.iter().any(|x| x.eq_ignore_ascii_case(e)))
}

fn has_image_magic(bytes: &[u8]) -> bool {
    bytes.starts_with(b"\x89PNG\r\n\x1a\n")
        || bytes.starts_with(b"\xff\xd8\xff")
        || bytes.starts_with(b"GIF87a")
        || bytes.starts_with(b"GIF89a")
        || bytes.starts_with(b"BM")
        || (bytes.starts_with(b"RIFF") && bytes.get(8..12) == Some(b"WEBP"))
}

pub fn is_image(path: &Path, bytes: &[u8]) -> bool {
    has_image_extension(path) && has_image_magic(bytes)
}

pub const HEAD_BYTES: u64 = 16;

pub fn is_remote(source: &str) -> bool {
    let Some((scheme, _)) = source.split_once(':') else {
        return false;
    };
    scheme.len() > 1
        && scheme
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.'))
}

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ImageRequest {
    pub dir: std::path::PathBuf,
    pub source: String,
    pub cols: u16,
    pub rows: u16,
}

pub fn fit(width: u32, height: u32, cols: u16, rows: u16) -> (u16, u16) {
    let (w, h) = (u64::from(width.max(1)), u64::from(height.max(1)));
    let (max_c, max_r) = (u64::from(cols.max(1)), u64::from(rows.max(1)));
    let at_full_width = (max_c * h).div_ceil(w * 2).max(1);
    let (c, r) = if at_full_width <= max_r {
        (max_c, at_full_width)
    } else {
        ((max_r * w * 2 / h).clamp(1, max_c), max_r)
    };
    (
        u16::try_from(c).unwrap_or(cols),
        u16::try_from(r).unwrap_or(rows),
    )
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ImageState {
    Decoding,
    Ready(std::sync::Arc<AsciiImage>),
    Failed(String),
}

pub type Rgb = [u8; 3];

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct ArtCell {
    pub ch: char,
    pub dots: u8,
    pub ink: Option<Rgb>,
    pub paper: Option<Rgb>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Glyph {
    pub ch: char,
    pub ink: Option<Rgb>,
    pub paper: Option<Rgb>,
}

fn luma(c: Rgb) -> u8 {
    let [r, g, b] = c.map(u32::from);
    u8::try_from((r * 3 + g * 6 + b) / 10).unwrap_or(u8::MAX)
}

fn mean(colors: impl Iterator<Item = Rgb>) -> Option<Rgb> {
    let (mut sum, mut count) = ([0u32; 3], 0u32);
    for c in colors {
        for (total, part) in sum.iter_mut().zip(c) {
            *total += u32::from(part);
        }
        count += 1;
    }
    (count > 0).then(|| sum.map(|t| u8::try_from(t / count).unwrap_or(u8::MAX)))
}

fn dot_char(dots: u8) -> char {
    char::from_u32(BRAILLE + u32::from(dots)).unwrap_or(' ')
}

type Samples = [[u8; 4]; SAMPLES];

fn rgb(p: &[u8; 4]) -> Rgb {
    [p[0], p[1], p[2]]
}

fn dots_of(lit: impl Fn(usize) -> bool) -> u8 {
    let mut dots = 0;
    for (row, bits) in DOT_BITS.iter().enumerate() {
        for (col, bit) in bits.iter().enumerate() {
            let (y, x) = (row * 2, col * 2);
            let square = [
                y * 4 + x,
                y * 4 + x + 1,
                (y + 1) * 4 + x,
                (y + 1) * 4 + x + 1,
            ];
            if square.iter().filter(|&&i| lit(i)).count() >= 2 {
                dots |= bit;
            }
        }
    }
    dots
}

fn dots_mask(dots: u8) -> u32 {
    let mut mask = 0;
    for (row, bits) in DOT_BITS.iter().enumerate() {
        for (col, bit) in bits.iter().enumerate() {
            if dots & bit != 0 {
                let (y, x) = (row * 2, col * 2);
                for i in [
                    y * 4 + x,
                    y * 4 + x + 1,
                    (y + 1) * 4 + x,
                    (y + 1) * 4 + x + 1,
                ] {
                    mask |= 1u32 << i;
                }
            }
        }
    }
    mask
}

fn in_mask(mask: u32, i: usize) -> bool {
    mask & (1u32 << i) != 0
}

fn shape_error(colors: &[[i32; 3]; SAMPLES], mask: u32) -> u64 {
    let (mut sum_in, mut sum_out, mut count_in) = ([0i64; 3], [0i64; 3], 0i64);
    let mut squares = 0i64;
    for (i, c) in colors.iter().enumerate() {
        let side = if in_mask(mask, i) {
            count_in += 1;
            &mut sum_in
        } else {
            &mut sum_out
        };
        for k in 0..3 {
            let v = i64::from(c[k]);
            side[k] += v;
            squares += v * v;
        }
    }
    let count_out = SAMPLES as i64 - count_in;
    let mut explained = 0i64;
    for k in 0..3 {
        if count_in > 0 {
            explained += sum_in[k] * sum_in[k] / count_in;
        }
        if count_out > 0 {
            explained += sum_out[k] * sum_out[k] / count_out;
        }
    }
    u64::try_from(squares - explained).unwrap_or(0)
}

fn mean_of(pixels: &Samples, mask: u32, inside: bool) -> Option<Rgb> {
    mean(
        pixels
            .iter()
            .enumerate()
            .filter(|(i, _)| in_mask(mask, *i) == inside)
            .map(|(_, p)| rgb(p)),
    )
}

fn best_shape(colors: &[[i32; 3]; SAMPLES], dots: u8) -> (char, u32) {
    let mut best = (' ', 0, u64::MAX);
    for &(ch, mask) in &SHAPES {
        let error = shape_error(colors, mask);
        if error < best.2 {
            best = (ch, mask, error);
        }
    }
    if dots != 0 && dots != FULL {
        let mask = dots_mask(dots);
        let (over, under) = BRAILLE_HANDICAP;
        if shape_error(colors, mask) * over / under < best.2 {
            return (dot_char(dots), mask);
        }
    }
    (best.0, best.1)
}

fn cell_over_page(pixels: &Samples) -> ArtCell {
    let opaque = |i: usize| pixels[i][3] >= 128;
    let wanted = (0..SAMPLES)
        .filter(|&i| opaque(i))
        .fold(0u32, |m, i| m | 1u32 << i);
    let dots = dots_of(opaque);
    if wanted == 0 {
        return ArtCell {
            ch: ' ',
            dots: 0,
            ink: None,
            paper: None,
        };
    }
    let wrong = |mask: u32| (mask ^ wanted).count_ones();
    let (mut ch, mut error) = SHAPES
        .iter()
        .map(|&(c, mask)| (c, wrong(mask)))
        .min_by_key(|&(_, e)| e)
        .unwrap_or((' ', u32::MAX));
    if dots != 0 && wrong(dots_mask(dots)) * 3 / 2 < error {
        (ch, error) = (dot_char(dots), 0);
    }
    let _ = error;
    ArtCell {
        ch,
        dots,
        ink: mean_of(pixels, wanted, true),
        paper: None,
    }
}

fn cell_of(pixels: &Samples) -> ArtCell {
    if pixels.iter().any(|p| p[3] < 128) {
        return cell_over_page(pixels);
    }
    let lights = pixels.map(|p| luma(rgb(&p)));
    let (low, high) = (
        lights.iter().copied().min().unwrap_or(0),
        lights.iter().copied().max().unwrap_or(0),
    );
    if high - low < FLAT {
        return ArtCell {
            ch: ' ',
            dots: 0,
            ink: None,
            paper: mean(pixels.iter().map(rgb)),
        };
    }
    let middle = low + (high - low) / 2;
    let dots = dots_of(|i| lights[i] > middle);
    let colors = pixels.map(|p| [i32::from(p[0]), i32::from(p[1]), i32::from(p[2])]);
    let (ch, mask) = best_shape(&colors, dots);
    ArtCell {
        ch,
        dots,
        ink: mean_of(pixels, mask, true),
        paper: mean_of(pixels, mask, false),
    }
}

pub fn cells_from_rgba(rgba: &[u8], cols: u16, rows: u16) -> Vec<ArtCell> {
    let (cols, rows) = (usize::from(cols), usize::from(rows));
    let (sx, sy) = (SAMPLES_X as usize, SAMPLES_Y as usize);
    let stride = cols * sx * 4;
    let mut cells = Vec::with_capacity(cols * rows);
    for row in 0..rows {
        for col in 0..cols {
            let mut pixels = [[0u8; 4]; SAMPLES];
            for (i, pixel) in pixels.iter_mut().enumerate() {
                let at = (row * sy + i / sx) * stride + (col * sx + i % sx) * 4;
                if let Some(bytes) = rgba.get(at..at + 4) {
                    pixel.copy_from_slice(bytes);
                }
            }
            cells.push(cell_of(&pixels));
        }
    }
    cells
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AsciiImage {
    pub cols: u16,
    pub rows: u16,
    pub cells: Vec<ArtCell>,
    pub width: u32,
    pub height: u32,
}

impl AsciiImage {
    pub fn row(&self, y: u16, x: u16, cols: u16) -> &[ArtCell] {
        let width = usize::from(self.cols).max(1);
        let start = usize::from(y) * width;
        let line = self.cells.get(start..start + width).unwrap_or(&[]);
        let from = usize::from(x).min(line.len());
        let to = (from + usize::from(cols)).min(line.len());
        &line[from..to]
    }
}

pub fn colored(cell: ArtCell) -> Glyph {
    Glyph {
        ch: cell.ch,
        ink: cell.ink,
        paper: cell.paper,
    }
}

pub fn mono(cell: ArtCell, dark_page: bool) -> char {
    let inked = |c: Rgb| (luma(c) >= 128) == dark_page;
    let dots = match (cell.ink, cell.paper) {
        (None, None) => 0,
        (None, Some(paper)) => {
            if inked(paper) {
                FULL
            } else {
                0
            }
        }
        (Some(_), None) => cell.dots,
        (Some(_), Some(_)) => {
            if dark_page {
                cell.dots
            } else {
                !cell.dots
            }
        }
    };
    if dots == 0 { ' ' } else { dot_char(dots) }
}

pub fn zoomed(room: u16, zoom: u8) -> u16 {
    let scaled = u32::from(room.max(1)) << zoom.min(MAX_ZOOM);
    u16::try_from(scaled.min(u32::from(MAX_CELLS))).unwrap_or(MAX_CELLS)
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PictureView {
    pub zoom: u8,
    pub x: u16,
    pub y: u16,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_shape_is_kept_for_cells_twice_as_tall_as_wide() {
        assert_eq!(fit(100, 100, 40, 40), (40, 20), "a square");
        assert_eq!(fit(100, 100, 40, 10), (20, 10), "limited by height");
        assert_eq!(fit(1000, 1, 40, 40), (40, 1), "a line stays visible");
        assert_eq!(fit(1, 1000, 40, 10), (1, 10), "so does a column");
        assert_eq!(fit(0, 0, 0, 0), (1, 1));
    }

    fn pixels(paint: impl Fn(usize, usize) -> [u8; 4]) -> Vec<u8> {
        (0..8)
            .flat_map(|y| (0..4).map(move |x| (x, y)))
            .flat_map(|(x, y)| paint(x, y))
            .collect()
    }

    const WHITE: [u8; 4] = [255, 255, 255, 255];
    const RED: [u8; 4] = [200, 0, 0, 255];

    fn one(paint: impl Fn(usize, usize) -> [u8; 4]) -> ArtCell {
        cells_from_rgba(&pixels(paint), 1, 1)[0]
    }

    #[test]
    fn an_edge_gets_the_block_that_is_exactly_it_in_both_colors() {
        let half = one(|x, _| if x < 2 { WHITE } else { RED });
        assert_eq!(colored(half).ch, '\u{258C}', "left half");
        assert_eq!((half.ink, half.paper), (Some([255; 3]), Some([200, 0, 0])));
        let low = one(|_, y| if y >= 6 { WHITE } else { RED });
        assert_eq!(colored(low).ch, '\u{2582}', "a quarter from the bottom");
        let corner = one(|x, y| if x >= 2 && y >= 4 { WHITE } else { RED });
        assert_eq!(colored(corner).ch, '\u{2597}', "lower right");
    }

    #[test]
    fn detail_finer_than_any_block_is_left_to_braille() {
        let spaced = one(|x, y| if x < 2 && y % 4 < 2 { WHITE } else { RED });
        assert_eq!(colored(spaced).ch, dot_char(0x05), "dots 1 and 3");
    }

    #[test]
    fn a_dim_line_on_a_dark_area_still_shows() {
        let cell = one(|_, y| {
            if y < 2 {
                [70, 70, 70, 255]
            } else {
                [20, 20, 20, 255]
            }
        });
        assert_ne!(colored(cell).ch, ' ');
        assert_eq!(cell.dots, 0x09, "the top row");
    }

    #[test]
    fn a_flat_cell_is_only_its_color_and_a_transparent_one_only_page() {
        let flat = one(|_, _| [10, 200, 30, 255]);
        assert_eq!(colored(flat).ch, ' ');
        assert_eq!(flat.paper, Some([10, 200, 30]));
        let clear = one(|_, _| [9, 9, 9, 0]);
        assert_eq!((clear.ch, clear.ink, clear.paper), (' ', None, None));
        let half = one(|_, y| if y < 4 { RED } else { [0, 0, 0, 0] });
        assert_eq!(colored(half).ch, '\u{2580}');
        assert_eq!((half.ink, half.paper), (Some([200, 0, 0]), None));
    }

    #[test]
    fn cells_are_read_from_the_right_pixels() {
        let mut rgba = vec![0u8; 8 * 8 * 4];
        rgba.chunks_mut(4).for_each(|p| p[3] = 255);
        for y in 4..8 {
            for x in 6..8 {
                rgba[(y * 8 + x) * 4..(y * 8 + x) * 4 + 3].fill(255);
            }
        }
        let cells = cells_from_rgba(&rgba, 2, 1);
        assert_eq!(cells[0].ch, ' ');
        assert_eq!(cells[1].ch, '\u{2597}');
    }

    #[test]
    fn without_colors_ink_is_what_differs_from_the_page() {
        let white = ArtCell {
            ch: ' ',
            dots: 0,
            ink: None,
            paper: Some([255; 3]),
        };
        assert_eq!(mono(white, true), '\u{28FF}');
        assert_eq!(mono(white, false), ' ');
        let stroke = ArtCell {
            ch: '\u{258C}',
            dots: 0x0F,
            ink: Some([255; 3]),
            paper: Some([0; 3]),
        };
        assert_eq!(mono(stroke, true), dot_char(0x0F));
        assert_eq!(mono(stroke, false), dot_char(0xF0));
    }

    #[test]
    fn a_row_is_cut_to_what_exists() {
        let cell = ArtCell {
            ch: ' ',
            dots: 1,
            ink: None,
            paper: None,
        };
        let image = AsciiImage {
            cols: 3,
            rows: 2,
            cells: vec![cell; 6],
            width: 6,
            height: 8,
        };
        assert_eq!(image.row(1, 1, 10).len(), 2);
        assert!(image.row(2, 0, 3).is_empty());
        assert!(image.row(0, 9, 3).is_empty());
    }

    #[test]
    fn zoom_doubles_and_stops_at_the_limit() {
        assert_eq!(zoomed(100, 0), 100);
        assert_eq!(zoomed(100, 2), 400);
        assert_eq!(zoomed(100, 9), MAX_CELLS);
        assert_eq!(zoomed(0, 0), 1);
    }

    #[test]
    fn a_picture_needs_both_the_name_and_the_bytes() {
        let png = b"\x89PNG\r\n\x1a\nrest";
        assert!(is_image(Path::new("a.PNG"), png));
        assert!(!is_image(Path::new("a.txt"), png));
        assert!(!is_image(Path::new("a.png"), b"plain text"));
        assert!(!is_image(Path::new("notes.txt"), b"BMW cars"));
        assert!(is_image(Path::new("a.webp"), b"RIFF\0\0\0\0WEBPrest"));
    }

    #[test]
    fn a_scheme_is_remote_and_a_path_is_not() {
        for remote in ["https://x/a.png", "data:image/png;base64,AA", "ftp://x"] {
            assert!(is_remote(remote), "{remote}");
        }
        for local in ["a.png", "img/a b.png", "../a.png", "/abs/a.png"] {
            assert!(!is_remote(local), "{local}");
        }
    }
}
