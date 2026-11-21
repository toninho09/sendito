//! Color themes: `auto`, `ansi`, `dusk` (dark), `daylight` (light), `contrast`.
//! `NO_COLOR` switches to a monochrome theme that uses only text attributes.

use ratatui::style::{Color, Modifier, Style};

pub const THEME_NAMES: &[&str] = &["auto", "ansi", "dusk", "daylight", "contrast"];

#[derive(Debug, Clone)]
pub struct Theme {
    pub name: &'static str,
    pub mono: bool,
    pub bg: Color,
    pub fg: Color,
    pub dim: Color,
    pub accent: Color,
    pub border: Color,
    pub border_focus: Color,
    pub sel_bg: Color,
    pub sel_fg: Color,
    pub bar_bg: Color,
    pub ok: Color,
    pub warn: Color,
    pub err: Color,
    pub info: Color,
    pub get: Color,
    pub post: Color,
    pub put: Color,
    pub patch: Color,
    pub delete: Color,
    pub other: Color,
    pub grpc: Color,
    pub json_key: Color,
    pub json_str: Color,
    pub json_num: Color,
    pub json_lit: Color,
    pub var_ok: Color,
    pub var_missing: Color,
    pub search_bg: Color,
}

fn rgb(r: u8, g: u8, b: u8) -> Color {
    Color::Rgb(r, g, b)
}

impl Theme {
    pub fn dusk() -> Self {
        Theme {
            name: "dusk",
            mono: false,
            bg: rgb(0x1b, 0x1e, 0x28),
            fg: rgb(0xd8, 0xdc, 0xe6),
            dim: rgb(0xa0, 0xa8, 0xbd),
            accent: rgb(0x86, 0xac, 0xff),
            border: rgb(0x45, 0x4b, 0x5e),
            border_focus: rgb(0x86, 0xac, 0xff),
            sel_bg: rgb(0x2e, 0x3a, 0x5e),
            sel_fg: rgb(0xff, 0xff, 0xff),
            bar_bg: rgb(0x24, 0x28, 0x36),
            ok: rgb(0x9e, 0xce, 0x6a),
            warn: rgb(0xe0, 0xaf, 0x68),
            err: rgb(0xff, 0x8b, 0xa0),
            info: rgb(0x7d, 0xcf, 0xff),
            get: rgb(0x9e, 0xce, 0x6a),
            post: rgb(0xe0, 0xaf, 0x68),
            put: rgb(0x86, 0xac, 0xff),
            patch: rgb(0xbb, 0x9a, 0xf7),
            delete: rgb(0xff, 0x8b, 0xa0),
            other: rgb(0x7d, 0xcf, 0xff),
            grpc: rgb(0x2a, 0xc3, 0xde),
            json_key: rgb(0x86, 0xac, 0xff),
            json_str: rgb(0x9e, 0xce, 0x6a),
            json_num: rgb(0xff, 0x9e, 0x64),
            json_lit: rgb(0xbb, 0x9a, 0xf7),
            var_ok: rgb(0x2a, 0xc3, 0xde),
            var_missing: rgb(0xff, 0x8b, 0xa0),
            search_bg: rgb(0x6b, 0x5a, 0x1e),
        }
    }

    /// Every foreground color has at least 4.5:1 contrast on the background,
    /// the bar and the selection (WCAG AA).
    pub fn daylight() -> Self {
        Theme {
            name: "daylight",
            mono: false,
            bg: rgb(0xfa, 0xfa, 0xf7),
            fg: rgb(0x2b, 0x2f, 0x3a),
            dim: rgb(0x3e, 0x43, 0x4e),
            accent: rgb(0x29, 0x5b, 0xd0),
            border: rgb(0x7d, 0x84, 0x94),
            border_focus: rgb(0x29, 0x5b, 0xd0),
            sel_bg: rgb(0xd6, 0xe2, 0xff),
            sel_fg: rgb(0x10, 0x14, 0x20),
            bar_bg: rgb(0xec, 0xee, 0xf2),
            ok: rgb(0x29, 0x70, 0x2c),
            warn: rgb(0x90, 0x55, 0x00),
            err: rgb(0xbe, 0x26, 0x26),
            info: rgb(0x00, 0x6a, 0x98),
            get: rgb(0x29, 0x70, 0x2c),
            post: rgb(0x90, 0x55, 0x00),
            put: rgb(0x29, 0x5b, 0xd0),
            patch: rgb(0x7b, 0x3f, 0xc4),
            delete: rgb(0xbe, 0x26, 0x26),
            other: rgb(0x00, 0x6a, 0x98),
            grpc: rgb(0x00, 0x6d, 0x78),
            json_key: rgb(0x29, 0x5b, 0xd0),
            json_str: rgb(0x29, 0x70, 0x2c),
            json_num: rgb(0xa4, 0x46, 0x00),
            json_lit: rgb(0x7b, 0x3f, 0xc4),
            var_ok: rgb(0x00, 0x6d, 0x78),
            var_missing: rgb(0xbe, 0x26, 0x26),
            search_bg: rgb(0xff, 0xe5, 0x8f),
        }
    }

    pub fn contrast() -> Self {
        Theme {
            name: "contrast",
            mono: false,
            bg: Color::Black,
            fg: Color::White,
            dim: rgb(0xc0, 0xc0, 0xc0),
            accent: rgb(0xff, 0xd7, 0x00),
            border: rgb(0xb0, 0xb0, 0xb0),
            border_focus: rgb(0xff, 0xd7, 0x00),
            sel_bg: rgb(0xff, 0xd7, 0x00),
            sel_fg: Color::Black,
            bar_bg: rgb(0x20, 0x20, 0x20),
            ok: rgb(0x5f, 0xff, 0x5f),
            warn: rgb(0xff, 0xd7, 0x00),
            err: rgb(0xff, 0x5f, 0x5f),
            info: rgb(0x5f, 0xd7, 0xff),
            get: rgb(0x5f, 0xff, 0x5f),
            post: rgb(0xff, 0xd7, 0x00),
            put: rgb(0x5f, 0xd7, 0xff),
            patch: rgb(0xff, 0x87, 0xff),
            delete: rgb(0xff, 0x5f, 0x5f),
            other: Color::White,
            grpc: rgb(0x5f, 0xff, 0xff),
            json_key: rgb(0x5f, 0xd7, 0xff),
            json_str: rgb(0x5f, 0xff, 0x5f),
            json_num: rgb(0xff, 0xaf, 0x5f),
            json_lit: rgb(0xff, 0x87, 0xff),
            var_ok: rgb(0x5f, 0xff, 0xff),
            var_missing: rgb(0xff, 0x5f, 0x5f),
            search_bg: rgb(0x87, 0x5f, 0x00),
        }
    }

    /// Uses the terminal's own 16-color palette.
    pub fn ansi() -> Self {
        Theme {
            name: "ansi",
            mono: false,
            bg: Color::Reset,
            fg: Color::Reset,
            dim: Color::DarkGray,
            accent: Color::Blue,
            border: Color::DarkGray,
            border_focus: Color::Blue,
            sel_bg: Color::Blue,
            sel_fg: Color::White,
            bar_bg: Color::Reset,
            ok: Color::Green,
            warn: Color::Yellow,
            err: Color::Red,
            info: Color::Cyan,
            get: Color::Green,
            post: Color::Yellow,
            put: Color::Blue,
            patch: Color::Magenta,
            delete: Color::Red,
            other: Color::Gray,
            grpc: Color::Cyan,
            json_key: Color::Blue,
            json_str: Color::Green,
            json_num: Color::Yellow,
            json_lit: Color::Magenta,
            var_ok: Color::Cyan,
            var_missing: Color::Red,
            search_bg: Color::Yellow,
        }
    }

    /// No colors at all (`NO_COLOR`): focus and selection use attributes.
    pub fn mono() -> Self {
        let r = Color::Reset;
        Theme {
            name: "mono",
            mono: true,
            bg: r,
            fg: r,
            dim: r,
            accent: r,
            border: r,
            border_focus: r,
            sel_bg: r,
            sel_fg: r,
            bar_bg: r,
            ok: r,
            warn: r,
            err: r,
            info: r,
            get: r,
            post: r,
            put: r,
            patch: r,
            delete: r,
            other: r,
            grpc: r,
            json_key: r,
            json_str: r,
            json_num: r,
            json_lit: r,
            var_ok: r,
            var_missing: r,
            search_bg: r,
        }
    }

    /// Resolves a theme name. `auto` picks dusk or daylight from `COLORFGBG`.
    pub fn by_name(name: &str) -> Self {
        let mut t = match name {
            "ansi" => Self::ansi(),
            "dusk" => Self::dusk(),
            "daylight" => Self::daylight(),
            "contrast" => Self::contrast(),
            _ => {
                if terminal_is_light() {
                    Self::daylight()
                } else {
                    Self::dusk()
                }
            }
        };
        if !truecolor() {
            t.downgrade();
        }
        t
    }

    /// Converts RGB colors to the nearest xterm-256 index for terminals
    /// without 24-bit color (e.g. macOS Terminal.app).
    pub fn downgrade(&mut self) {
        let f = |c: &mut Color| {
            if let Color::Rgb(r, g, b) = *c {
                *c = Color::Indexed(rgb_to_256(r, g, b));
            }
        };
        for c in [
            &mut self.bg,
            &mut self.fg,
            &mut self.dim,
            &mut self.accent,
            &mut self.border,
            &mut self.border_focus,
            &mut self.sel_bg,
            &mut self.sel_fg,
            &mut self.bar_bg,
            &mut self.ok,
            &mut self.warn,
            &mut self.err,
            &mut self.info,
            &mut self.get,
            &mut self.post,
            &mut self.put,
            &mut self.patch,
            &mut self.delete,
            &mut self.other,
            &mut self.grpc,
            &mut self.json_key,
            &mut self.json_str,
            &mut self.json_num,
            &mut self.json_lit,
            &mut self.var_ok,
            &mut self.var_missing,
            &mut self.search_bg,
        ] {
            f(c);
        }
    }

    pub fn base(&self) -> Style {
        Style::default().fg(self.fg).bg(self.bg)
    }

    pub fn dim(&self) -> Style {
        if self.mono {
            Style::default().add_modifier(Modifier::DIM)
        } else {
            Style::default().fg(self.dim)
        }
    }

    pub fn selected(&self) -> Style {
        if self.mono {
            Style::default().add_modifier(Modifier::REVERSED)
        } else {
            // Bold as well as the background: on dark themes the selection
            // background alone is a small step from the page.
            Style::default()
                .fg(self.sel_fg)
                .bg(self.sel_bg)
                .add_modifier(Modifier::BOLD)
        }
    }

    pub fn border(&self, focused: bool) -> Style {
        if self.mono {
            if focused {
                Style::default().add_modifier(Modifier::BOLD)
            } else {
                Style::default().add_modifier(Modifier::DIM)
            }
        } else if focused {
            Style::default().fg(self.border_focus)
        } else {
            Style::default().fg(self.border)
        }
    }

    pub fn accent(&self) -> Style {
        if self.mono {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default().fg(self.accent)
        }
    }

    pub fn fg(&self, c: Color) -> Style {
        if self.mono {
            Style::default()
        } else {
            Style::default().fg(c)
        }
    }

    pub fn method_color(&self, m: &str) -> Color {
        match m {
            "GET" => self.get,
            "POST" => self.post,
            "PUT" => self.put,
            "PATCH" => self.patch,
            "DELETE" => self.delete,
            "GRPC" => self.grpc,
            _ => self.other,
        }
    }

    pub fn status_color(&self, code: u16) -> Color {
        match code {
            200..=299 => self.ok,
            300..=399 => self.info,
            400..=499 => self.warn,
            _ => self.err,
        }
    }

    pub fn bar(&self) -> Style {
        Style::default().fg(self.fg).bg(self.bar_bg)
    }
}

fn terminal_is_light() -> bool {
    // COLORFGBG is "fg;bg" (sometimes "fg;default;bg"); light backgrounds are 7 or 15.
    std::env::var("COLORFGBG")
        .ok()
        .and_then(|v| v.rsplit(';').next().and_then(|b| b.parse::<u8>().ok()))
        .is_some_and(|bg| bg == 7 || bg == 15)
}

fn truecolor() -> bool {
    if let Ok(v) = std::env::var("COLORTERM")
        && (v.contains("truecolor") || v.contains("24bit"))
    {
        return true;
    }
    let term_program = std::env::var("TERM_PROGRAM").unwrap_or_default();
    if term_program == "Apple_Terminal" {
        return false;
    }
    let term = std::env::var("TERM").unwrap_or_default();
    term.contains("kitty")
        || term.contains("alacritty")
        || term.contains("direct")
        || term_program == "iTerm.app"
        || term_program == "WezTerm"
        || std::env::var("VTE_VERSION").is_ok()
}

pub fn rgb_to_256(r: u8, g: u8, b: u8) -> u8 {
    let to6 = |v: u8| -> u8 {
        if v < 48 {
            0
        } else if v < 115 {
            1
        } else {
            (v - 35) / 40
        }
    };
    let (r6, g6, b6) = (to6(r), to6(g), to6(b));
    let cube = 16 + 36 * r6 + 6 * g6 + b6;
    let cv = |i: u8| if i == 0 { 0u8 } else { 55 + 40 * i };
    let cube_err = dist((r, g, b), (cv(r6), cv(g6), cv(b6)));
    let avg = (r as u16 + g as u16 + b as u16) / 3;
    let gi = if avg > 238 {
        23
    } else {
        ((avg.saturating_sub(3)) / 10) as u8
    };
    let gv = 8 + 10 * gi;
    let gray_err = dist((r, g, b), (gv, gv, gv));
    if gray_err < cube_err { 232 + gi } else { cube }
}

fn dist(a: (u8, u8, u8), b: (u8, u8, u8)) -> u32 {
    let d = |x: u8, y: u8| (x as i32 - y as i32).pow(2) as u32;
    d(a.0, b.0) + d(a.1, b.1) + d(a.2, b.2)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn themes() {
        for n in THEME_NAMES {
            let t = Theme::by_name(n);
            assert!(!t.name.is_empty());
        }
        assert_eq!(rgb_to_256(0, 0, 0), 16);
        assert_eq!(rgb_to_256(255, 255, 255), 231);
        assert_eq!(rgb_to_256(128, 128, 128), 244);
        let mut t = Theme::dusk();
        t.downgrade();
        assert!(matches!(t.bg, Color::Indexed(_)));
        let m = Theme::mono();
        assert!(m.selected().add_modifier.contains(Modifier::REVERSED));
        assert_eq!(m.fg(Color::Red), Style::default());
        assert_eq!(t.method_color("GRPC"), t.grpc);
        assert_eq!(t.status_color(404), t.warn);
    }

    fn luminance(c: Color) -> Option<f64> {
        let Color::Rgb(r, g, b) = c else { return None };
        let f = |v: u8| {
            let c = v as f64 / 255.0;
            if c <= 0.03928 {
                c / 12.92
            } else {
                ((c + 0.055) / 1.055).powf(2.4)
            }
        };
        Some(0.2126 * f(r) + 0.7152 * f(g) + 0.0722 * f(b))
    }

    fn ratio(a: Color, b: Color) -> f64 {
        match (luminance(a), luminance(b)) {
            (Some(x), Some(y)) => (x.max(y) + 0.05) / (x.min(y) + 0.05),
            _ => 21.0,
        }
    }

    /// Text colors stay readable (WCAG AA, 4.5:1) on the background, the bars
    /// and the selection.
    #[test]
    fn text_contrast() {
        for t in [Theme::dusk(), Theme::daylight()] {
            let text = [
                ("fg", t.fg),
                ("dim", t.dim),
                ("accent", t.accent),
                ("ok", t.ok),
                ("warn", t.warn),
                ("err", t.err),
                ("info", t.info),
                ("get", t.get),
                ("post", t.post),
                ("put", t.put),
                ("patch", t.patch),
                ("delete", t.delete),
                ("grpc", t.grpc),
                ("var_ok", t.var_ok),
                ("var_missing", t.var_missing),
                ("json_key", t.json_key),
                ("json_str", t.json_str),
                ("json_num", t.json_num),
                ("json_lit", t.json_lit),
            ];
            for (bg_name, bg) in [("bg", t.bg), ("bar", t.bar_bg), ("selection", t.sel_bg)] {
                for (n, c) in text {
                    let r = ratio(c, bg);
                    assert!(r >= 4.5, "{}: {n} on {bg_name} is {r:.2}:1", t.name);
                }
            }
            assert!(ratio(t.sel_fg, t.sel_bg) >= 4.5, "{}: selection", t.name);
        }
    }
}
