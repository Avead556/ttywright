use std::path::Path;

use ratatui::style::{Color, Modifier, Style};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ColorDepth {
    TrueColor,
    Ansi256,
    Ansi16,
    None,
}

impl ColorDepth {
    pub fn detect(env: impl Fn(&str) -> Option<String>) -> Self {
        if env("NO_COLOR").is_some_and(|v| !v.is_empty()) {
            return Self::None;
        }
        if env("COLORTERM").is_some_and(|v| v == "truecolor" || v == "24bit") {
            return Self::TrueColor;
        }
        let term = env("TERM").unwrap_or_default();
        const TRUE_COLOR_TERMS: [&str; 5] = ["direct", "kitty", "ghostty", "alacritty", "wezterm"];
        if TRUE_COLOR_TERMS.iter().any(|t| term.contains(t)) {
            Self::TrueColor
        } else if term.contains("256color") {
            Self::Ansi256
        } else {
            Self::Ansi16
        }
    }
}

#[derive(Debug, Clone, Copy)]
pub struct Theme {
    pub bg: Color,
    pub bg_panel: Color,
    pub bg_popup: Color,
    pub selected: Style,
    pub selected_inactive: Style,
    pub bg_current_line: Color,
    pub matched: Style,
    pub diff_removed: Style,
    pub diff_added: Style,
    pub fg_primary: Color,
    pub fg_muted: Color,
    pub fg_subtle: Color,
    pub border: Color,
    pub border_popup: Color,
    pub accent: Color,
    pub error: Color,
    pub warning: Color,
    pub success: Color,
    pub syntax: SyntaxColors,
    pub diff_indexed: (u8, u8),
}

#[derive(Debug, Clone, Copy)]
pub struct SyntaxColors {
    pub keyword: Color,
    pub function: Color,
    pub type_: Color,
    pub variable: Color,
    pub property: Color,
    pub string: Color,
    pub number: Color,
    pub comment: Color,
    pub comment_doc: Color,
    pub attribute: Color,
    pub lifetime: Color,
    pub punctuation: Color,
}

fn parse_hex(text: &str) -> Option<Color> {
    let digits = text.strip_prefix('#')?;
    if digits.len() != 6 {
        return None;
    }
    let n = u32::from_str_radix(digits, 16).ok()?;
    let [_, r, g, b] = n.to_be_bytes();
    Some(Color::Rgb(r, g, b))
}

impl Theme {
    pub fn file_color(&self, path: &Path) -> Color {
        let syntax = self.syntax;
        match path.extension().and_then(|e| e.to_str()) {
            Some("rs" | "java" | "kt" | "swift") => syntax.keyword,
            Some("toml" | "yml" | "yaml" | "ini" | "lock") => syntax.attribute,
            Some("json" | "py" | "pyi" | "pyw" | "go") => syntax.number,
            Some("md" | "txt" | "rst") => syntax.function,
            Some("js" | "mjs" | "cjs" | "jsx" | "ts" | "mts" | "cts" | "tsx" | "php" | "phtml") => {
                self.warning
            }
            Some("html" | "css" | "scss" | "vue") => syntax.property,
            Some("sh" | "zsh" | "bash") => syntax.string,
            _ => self.fg_muted,
        }
    }

    pub fn category_color(&self, category: &str) -> Color {
        let syntax = self.syntax;
        match category {
            "Code" => syntax.function,
            "Git" => syntax.keyword,
            "Edit" | "Run" => self.success,
            "File" => syntax.attribute,
            "Navigate" => syntax.number,
            "Tool Windows" => syntax.property,
            _ => self.fg_muted,
        }
    }
}

impl SyntaxColors {
    fn set_color(&mut self, token: &str, color: Color) -> bool {
        match token {
            "keyword" => self.keyword = color,
            "function" => self.function = color,
            "type" => self.type_ = color,
            "variable" => self.variable = color,
            "property" => self.property = color,
            "string" => self.string = color,
            "number" => self.number = color,
            "comment" => self.comment = color,
            "comment_doc" => self.comment_doc = color,
            "attribute" => self.attribute = color,
            "lifetime" => self.lifetime = color,
            "punctuation" => self.punctuation = color,
            _ => return false,
        }
        true
    }
}

impl Theme {
    pub fn named(name: &str, dir: Option<&Path>, warnings: &mut Vec<String>) -> Self {
        match name {
            "darcula" => Self::DARCULA,
            "light" => Self::LIGHT,
            custom => {
                let file = dir.map(|d| d.join(format!("{custom}.toml")));
                let text = file.as_ref().and_then(|f| std::fs::read_to_string(f).ok());
                let Some(text) = text else {
                    warnings.push(format!("theme {custom} was not found"));
                    return Self::DARCULA;
                };
                Self::parse_custom(custom, &text, warnings)
            }
        }
    }

    fn parse_custom(name: &str, text: &str, warnings: &mut Vec<String>) -> Self {
        let table: toml::Table = match text.parse() {
            Ok(table) => table,
            Err(e) => {
                let first = e.message().lines().next().unwrap_or("invalid TOML");
                warnings.push(format!("theme {name}: {first}"));
                return Self::DARCULA;
            }
        };
        let mut theme = match table.get("base").and_then(toml::Value::as_str) {
            Some("light") => Self::LIGHT,
            Some("darcula") | None => Self::DARCULA,
            Some(other) => {
                warnings.push(format!("theme {name}: unknown base {other}"));
                Self::DARCULA
            }
        };
        for (section, colors) in &table {
            let Some(colors) = colors.as_table() else {
                if section != "base" {
                    warnings.push(format!("theme {name}: unknown setting {section}"));
                }
                continue;
            };
            for (token, value) in colors {
                let color = value.as_str().and_then(parse_hex);
                let Some(color) = color else {
                    warnings.push(format!("theme {name}: {token} must look like #rrggbb"));
                    continue;
                };
                let known = match section.as_str() {
                    "colors" => theme.set_color(token, color),
                    "syntax" => theme.syntax.set_color(token, color),
                    _ => false,
                };
                if !known {
                    warnings.push(format!("theme {name}: unknown color {section}.{token}"));
                }
            }
        }
        theme
    }

    fn set_color(&mut self, token: &str, color: Color) -> bool {
        match token {
            "bg" => self.bg = color,
            "bg_panel" => self.bg_panel = color,
            "bg_popup" => self.bg_popup = color,
            "selected" => self.selected = self.selected.bg(color),
            "selected_inactive" => self.selected_inactive = self.selected_inactive.bg(color),
            "bg_current_line" => self.bg_current_line = color,
            "matched" => self.matched = self.matched.bg(color),
            "diff_removed" => self.diff_removed = self.diff_removed.bg(color),
            "diff_added" => self.diff_added = self.diff_added.bg(color),
            "fg_primary" => self.fg_primary = color,
            "fg_muted" => self.fg_muted = color,
            "fg_subtle" => self.fg_subtle = color,
            "border" => self.border = color,
            "border_popup" => self.border_popup = color,
            "accent" => self.accent = color,
            "error" => self.error = color,
            "warning" => self.warning = color,
            "success" => self.success = color,
            _ => return false,
        }
        true
    }

    pub const DARCULA: Self = Self {
        bg: hex(0x1E1F22),
        bg_panel: hex(0x2B2D30),
        bg_popup: hex(0x2B2D30),
        selected: Style::new().bg(hex(0x2E436E)),
        selected_inactive: Style::new().bg(hex(0x393B40)),
        bg_current_line: hex(0x26282E),
        matched: Style::new().bg(hex(0x32593D)),
        diff_removed: Style::new().fg(hex(0xDFE1E5)).bg(hex(0x5A2D2D)),
        diff_added: Style::new().fg(hex(0xDFE1E5)).bg(hex(0x2D5A3D)),
        fg_primary: hex(0xDFE1E5),
        fg_muted: hex(0x868A91),
        fg_subtle: hex(0x5A5D63),
        border: hex(0x393B40),
        border_popup: hex(0x4E5157),
        accent: hex(0x3574F0),
        error: hex(0xF75464),
        warning: hex(0xF2C55C),
        success: hex(0x5FB865),
        syntax: SyntaxColors {
            keyword: hex(0xCF8E6D),
            function: hex(0x56A8F5),
            type_: hex(0xBCBEC4),
            variable: hex(0xBCBEC4),
            property: hex(0xC77DBB),
            string: hex(0x6AAB73),
            number: hex(0x2AACB8),
            comment: hex(0x7A7E85),
            comment_doc: hex(0x5F826B),
            attribute: hex(0xB3AE60),
            lifetime: hex(0x20999D),
            punctuation: hex(0xBCBEC4),
        },
        diff_indexed: (52, 22),
    };

    pub const LIGHT: Self = Self {
        bg: hex(0xFFFFFF),
        bg_panel: hex(0xF7F8FA),
        bg_popup: hex(0xFFFFFF),
        selected: Style::new().bg(hex(0xD5E1FF)),
        selected_inactive: Style::new().bg(hex(0xE4E6EB)),
        bg_current_line: hex(0xF5F8FE),
        matched: Style::new().bg(hex(0xC8E6C9)),
        diff_removed: Style::new().fg(hex(0x1E1F22)).bg(hex(0xFFD6D6)),
        diff_added: Style::new().fg(hex(0x1E1F22)).bg(hex(0xCFF5D6)),
        fg_primary: hex(0x1E1F22),
        fg_muted: hex(0x6C707E),
        fg_subtle: hex(0xA8ADBD),
        border: hex(0xEBECF0),
        border_popup: hex(0xC9CCD6),
        accent: hex(0x3574F0),
        error: hex(0xE55765),
        warning: hex(0xC27D04),
        success: hex(0x208A3C),
        syntax: SyntaxColors {
            keyword: hex(0x0033B3),
            function: hex(0x00627A),
            type_: hex(0x000000),
            variable: hex(0x000000),
            property: hex(0x871094),
            string: hex(0x067D17),
            number: hex(0x1750EB),
            comment: hex(0x8C8C8C),
            comment_doc: hex(0x8C8C8C),
            attribute: hex(0x9E880D),
            lifetime: hex(0x20999D),
            punctuation: hex(0x000000),
        },
        diff_indexed: (224, 194),
    };

    pub const ANSI16: Self = Self {
        bg: Color::Reset,
        bg_panel: Color::Reset,
        bg_popup: Color::Reset,
        selected: Style::new().add_modifier(Modifier::REVERSED),
        selected_inactive: Style::new().add_modifier(Modifier::UNDERLINED),
        bg_current_line: Color::Reset,
        matched: Style::new().add_modifier(Modifier::UNDERLINED),
        diff_removed: Style::new().fg(Color::Red),
        diff_added: Style::new().fg(Color::Green),
        fg_primary: Color::Reset,
        fg_muted: Color::DarkGray,
        fg_subtle: Color::DarkGray,
        border: Color::DarkGray,
        border_popup: Color::Reset,
        accent: Color::Blue,
        error: Color::Red,
        warning: Color::Yellow,
        success: Color::Green,
        syntax: SyntaxColors {
            keyword: Color::Yellow,
            function: Color::Blue,
            type_: Color::Reset,
            variable: Color::Reset,
            property: Color::Magenta,
            string: Color::Green,
            number: Color::Cyan,
            comment: Color::DarkGray,
            comment_doc: Color::DarkGray,
            attribute: Color::Yellow,
            lifetime: Color::Cyan,
            punctuation: Color::Reset,
        },
        diff_indexed: (52, 22),
    };

    pub fn for_depth(depth: ColorDepth) -> Self {
        Self::DARCULA.adapted(depth)
    }

    pub fn adapted(self, depth: ColorDepth) -> Self {
        match depth {
            ColorDepth::TrueColor => self,
            ColorDepth::Ansi256 => {
                let mut theme = self.map_colors(nearest_256);
                theme.diff_removed = theme.diff_removed.bg(Color::Indexed(self.diff_indexed.0));
                theme.diff_added = theme.diff_added.bg(Color::Indexed(self.diff_indexed.1));
                theme
            }
            ColorDepth::Ansi16 => Self::ANSI16,
            ColorDepth::None => Self::ANSI16.map_colors(|_| Color::Reset),
        }
    }

    fn map_colors(self, f: impl Fn(Color) -> Color) -> Self {
        let style = |s: Style| Style {
            fg: s.fg.map(&f),
            bg: s.bg.map(&f),
            ..s
        };
        let syntax = self.syntax;
        Self {
            diff_indexed: self.diff_indexed,
            bg: f(self.bg),
            bg_panel: f(self.bg_panel),
            bg_popup: f(self.bg_popup),
            selected: style(self.selected),
            selected_inactive: style(self.selected_inactive),
            bg_current_line: f(self.bg_current_line),
            matched: style(self.matched),
            diff_removed: style(self.diff_removed),
            diff_added: style(self.diff_added),
            fg_primary: f(self.fg_primary),
            fg_muted: f(self.fg_muted),
            fg_subtle: f(self.fg_subtle),
            border: f(self.border),
            border_popup: f(self.border_popup),
            accent: f(self.accent),
            error: f(self.error),
            warning: f(self.warning),
            success: f(self.success),
            syntax: SyntaxColors {
                keyword: f(syntax.keyword),
                function: f(syntax.function),
                type_: f(syntax.type_),
                variable: f(syntax.variable),
                property: f(syntax.property),
                string: f(syntax.string),
                number: f(syntax.number),
                comment: f(syntax.comment),
                comment_doc: f(syntax.comment_doc),
                attribute: f(syntax.attribute),
                lifetime: f(syntax.lifetime),
                punctuation: f(syntax.punctuation),
            },
        }
    }

    pub fn capture(&self, name: &str) -> Style {
        let s = &self.syntax;
        let italic = |c: Color| Style::new().fg(c).add_modifier(Modifier::ITALIC);
        let plain = |c: Color| Style::new().fg(c);
        let has = |prefix: &str| name == prefix || name.starts_with(&format!("{prefix}."));
        if has("string.special.key") || has("property") || has("field") {
            plain(s.property)
        } else if has("comment.documentation") || has("comment.doc") {
            italic(s.comment_doc)
        } else if has("comment") {
            italic(s.comment)
        } else if has("function.macro") {
            italic(s.function)
        } else if has("function") || has("method") || has("text.title") || has("markup.heading") {
            plain(s.function)
        } else if has("type.builtin")
            || has("variable.builtin")
            || has("keyword")
            || has("tag")
            || has("conditional")
            || has("repeat")
        {
            plain(s.keyword)
        } else if has("type") || has("constructor") {
            plain(s.type_)
        } else if has("string")
            || has("escape")
            || has("char")
            || has("text.literal")
            || has("markup.raw")
        {
            plain(s.string)
        } else if has("number")
            || has("constant")
            || has("boolean")
            || has("float")
            || has("text.uri")
        {
            plain(s.number)
        } else if has("attribute") {
            plain(s.attribute)
        } else if has("label") {
            italic(s.lifetime)
        } else if has("punctuation") || has("operator") || has("delimiter") {
            plain(s.punctuation)
        } else {
            plain(s.variable)
        }
    }
}

const fn hex(rgb: u32) -> Color {
    Color::Rgb((rgb >> 16) as u8, (rgb >> 8) as u8, rgb as u8)
}

pub(super) fn nearest_256(color: Color) -> Color {
    let Color::Rgb(r, g, b) = color else {
        return color;
    };
    const LEVELS: [i32; 6] = [0, 95, 135, 175, 215, 255];
    let level = |v: u8| {
        (0..6)
            .min_by_key(|&i| (LEVELS[i] - i32::from(v)).abs())
            .unwrap_or(0)
    };
    let (cr, cg, cb) = (level(r), level(g), level(b));
    let dist = |x: i32, y: i32, z: i32| {
        (x - i32::from(r)).pow(2) + (y - i32::from(g)).pow(2) + (z - i32::from(b)).pow(2)
    };
    let cube = dist(LEVELS[cr], LEVELS[cg], LEVELS[cb]);
    let gray_value = |i: u8| 8 + 10 * i32::from(i);
    let gray = (0..24u8)
        .min_by_key(|&i| dist(gray_value(i), gray_value(i), gray_value(i)))
        .unwrap_or(0);
    let gray_dist = dist(gray_value(gray), gray_value(gray), gray_value(gray));
    Color::Indexed(if gray_dist < cube {
        232 + gray
    } else {
        (16 + 36 * cr + 6 * cg + cb) as u8
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_theme_knows_every_capture_of_every_language() {
        let mut theme = Theme::named("darcula", None, &mut Vec::new());
        theme.syntax.variable = Color::Rgb(1, 2, 3);
        let plain = theme.capture("no.such.capture");
        let meant_to_be_plain = ["variable", "embedded", "module", "none", "spell", "text"];
        let mut unknown = Vec::new();
        for row in crate::lang::languages::LANGUAGES {
            for name in crate::lang::syntax::capture_names(row.language) {
                let first = name.split('.').next().unwrap_or(name);
                let is_helper = name.starts_with('_');
                if theme.capture(name) == plain && !meant_to_be_plain.contains(&first) && !is_helper
                {
                    unknown.push(format!("{}: {name}", row.name));
                }
            }
        }
        assert!(unknown.is_empty(), "{unknown:#?}");
    }

    fn depth(vars: &[(&str, &str)]) -> ColorDepth {
        ColorDepth::detect(|name| {
            vars.iter()
                .find(|(k, _)| *k == name)
                .map(|(_, v)| (*v).to_owned())
        })
    }

    #[test]
    fn color_depth_follows_the_environment() {
        let truecolor = [("COLORTERM", "truecolor"), ("TERM", "xterm-256color")];
        assert_eq!(depth(&truecolor), ColorDepth::TrueColor);
        assert_eq!(depth(&[("TERM", "xterm-kitty")]), ColorDepth::TrueColor);
        assert_eq!(
            depth(&[
                ("TERM", "xterm-256color"),
                ("TERM_PROGRAM", "Apple_Terminal")
            ]),
            ColorDepth::Ansi256
        );
        assert_eq!(depth(&[("TERM", "tmux-256color")]), ColorDepth::Ansi256);
        assert_eq!(depth(&[("TERM", "linux")]), ColorDepth::Ansi16);
        assert_eq!(depth(&[]), ColorDepth::Ansi16);
    }

    #[test]
    fn no_color_wins_only_when_not_empty() {
        let set = [("NO_COLOR", "1"), ("COLORTERM", "truecolor")];
        assert_eq!(depth(&set), ColorDepth::None);
        let empty = [("NO_COLOR", ""), ("COLORTERM", "truecolor")];
        assert_eq!(depth(&empty), ColorDepth::TrueColor);
    }

    #[test]
    fn rgb_maps_to_the_nearest_xterm_color() {
        let nearest = |rgb| nearest_256(hex(rgb));
        assert_eq!(nearest(0x000000), Color::Indexed(16));
        assert_eq!(nearest(0xFFFFFF), Color::Indexed(231));
        assert_eq!(nearest(0xFF0000), Color::Indexed(196));
        assert_eq!(nearest(0x808080), Color::Indexed(244));
        assert_eq!(nearest_256(Color::Reset), Color::Reset);
    }

    #[test]
    fn the_256_theme_has_no_rgb_and_keeps_surfaces_distinct() {
        let t = Theme::for_depth(ColorDepth::Ansi256);
        assert!(!format!("{t:?}").contains("Rgb"));
        assert_ne!(t.bg, t.bg_current_line);
        assert_ne!(t.bg, t.bg_panel);
        assert_ne!(t.selected, t.matched);
    }

    #[test]
    fn diff_rows_stay_red_and_green_in_256_colors() {
        let t = Theme::for_depth(ColorDepth::Ansi256);
        assert_eq!(t.diff_removed.bg, Some(Color::Indexed(52)));
        assert_eq!(t.diff_added.bg, Some(Color::Indexed(22)));
    }

    #[test]
    fn no_color_theme_sets_no_color_but_keeps_highlights_visible() {
        let t = Theme::for_depth(ColorDepth::None);
        for name in ["Rgb", "Indexed", "Blue", "Red", "Green", "DarkGray"] {
            assert!(!format!("{t:?}").contains(name), "{name} in {t:?}");
        }
        assert_eq!(t.selected.add_modifier, Modifier::REVERSED);
        assert_eq!(t.matched.add_modifier, Modifier::UNDERLINED);
    }

    #[test]
    fn captures_map_to_design_tokens() {
        let t = Theme::DARCULA;
        assert_eq!(t.capture("keyword").fg, Some(t.syntax.keyword));
        assert_eq!(t.capture("function.macro").add_modifier, Modifier::ITALIC);
        assert_eq!(t.capture("string.special.key").fg, Some(t.syntax.property));
        assert_eq!(t.capture("string").fg, Some(t.syntax.string));
        assert_eq!(t.capture("type.builtin").fg, Some(t.syntax.keyword));
        assert_eq!(t.capture("unknown").fg, Some(t.syntax.variable));
    }

    #[test]
    fn built_in_themes_go_by_name() {
        let mut warnings = Vec::new();
        assert_eq!(
            Theme::named("light", None, &mut warnings).bg,
            Theme::LIGHT.bg
        );
        assert_eq!(
            Theme::named("darcula", None, &mut warnings).bg,
            Theme::DARCULA.bg
        );
        assert!(warnings.is_empty());
    }

    #[test]
    fn a_missing_theme_warns_and_falls_back() {
        let mut warnings = Vec::new();
        let theme = Theme::named("nope", None, &mut warnings);
        assert_eq!(theme.bg, Theme::DARCULA.bg);
        assert!(warnings[0].contains("nope"));
    }

    #[test]
    fn a_custom_theme_starts_from_its_base_and_overrides_tokens() {
        let dir = tempfile::tempdir().expect("temp dir");
        std::fs::write(
            dir.path().join("mine.toml"),
            "base = \"light\"\n[colors]\nbg = \"#102030\"\nselected = \"#010203\"\n\
             [syntax]\nkeyword = \"#AABBCC\"\n",
        )
        .expect("writes");
        let mut warnings = Vec::new();
        let theme = Theme::named("mine", Some(dir.path()), &mut warnings);
        assert_eq!(warnings, Vec::<String>::new());
        assert_eq!(theme.bg, Color::Rgb(0x10, 0x20, 0x30));
        assert_eq!(theme.selected.bg, Some(Color::Rgb(1, 2, 3)));
        assert_eq!(theme.syntax.keyword, Color::Rgb(0xAA, 0xBB, 0xCC));
        assert_eq!(
            theme.fg_primary,
            Theme::LIGHT.fg_primary,
            "the rest is the base"
        );
    }

    #[test]
    fn bad_colors_in_a_custom_theme_warn_one_by_one() {
        let mut warnings = Vec::new();
        let theme = Theme::parse_custom(
            "m",
            "[colors]\nbg = \"red\"\nnope = \"#000000\"\nerror = \"#112233\"",
            &mut warnings,
        );
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert_eq!(theme.bg, Theme::DARCULA.bg);
        assert_eq!(theme.error, Color::Rgb(0x11, 0x22, 0x33));
    }

    #[test]
    fn the_light_theme_keeps_its_tints_in_256_colors() {
        let theme = Theme::LIGHT.adapted(ColorDepth::Ansi256);
        assert_eq!(theme.diff_removed.bg, Some(Color::Indexed(224)));
        assert_eq!(theme.diff_added.bg, Some(Color::Indexed(194)));
        assert!(!matches!(theme.bg, Color::Rgb(..)));
    }

    #[test]
    fn file_kinds_have_colors_and_unknown_ones_stay_quiet() {
        let t = Theme::DARCULA;
        assert_eq!(t.file_color(Path::new("/a/main.rs")), t.syntax.keyword);
        assert_eq!(t.file_color(Path::new("Cargo.toml")), t.syntax.attribute);
        assert_eq!(t.file_color(Path::new("no_extension")), t.fg_muted);
        assert_ne!(t.category_color("Git"), t.category_color("Edit"));
    }
}
