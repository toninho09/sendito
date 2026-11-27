//! Color themes. Built in: `auto`, `dusk` (dark), `daylight` (light), `ember`
//! (warm dark), `linen` (warm light), `contrast`, `ansi`, themes after popular
//! IDE themes (`dracula`, `nord`, `github-light`...) and solid-color themes
//! (`turbo`, `phosphor`, `bauhaus`...). The color tables are in `palettes`.
//! Users add their own as TOML files (see `docs/THEMES.md`). `NO_COLOR`
//! switches to a monochrome theme that uses only text attributes.
//!
//! A theme is a list of color tokens. The *base* tokens are given by every
//! built-in theme; the *derived* ones (surfaces, shadow, gutter, timing phases)
//! are computed from the base tokens unless a theme sets them.

use std::collections::BTreeSet;
use std::sync::Arc;

use ratatui::style::{Color, Modifier, Style};

mod palettes;
use palettes::*;

/// Built-in theme names, in the order Settings cycles through them.
pub const BUILTIN: &[&str] = &[
    "auto",
    "dusk",
    "daylight",
    "ember",
    "linen",
    "contrast",
    "ansi",
    // After IDE themes, dark.
    "dracula",
    "monokai",
    "nord",
    "gruvbox",
    "one-dark",
    "catppuccin",
    "tokyo-night",
    "rose-pine",
    "night-owl",
    "kanagawa",
    "everforest",
    "github-dark",
    "solarized-dark",
    // After IDE themes, light.
    "solarized-light",
    "gruvbox-light",
    "catppuccin-latte",
    "github-light",
    "one-light",
    // Solid colors.
    "turbo",
    "phosphor",
    "amber",
    "bauhaus",
    "crimson",
    "abyss",
    "grove",
    "grape",
];

macro_rules! theme_struct {
    ($( #[doc = $doc:literal] $name:ident, )*) => {
        #[derive(Debug, Clone, PartialEq)]
        pub struct Theme {
            pub name: Arc<str>,
            /// No colors at all (`NO_COLOR`): focus and selection use attributes.
            pub mono: bool,
            /// The focused pane also gets a heavy border.
            pub heavy_focus: bool,
            /// The background is light.
            pub light: bool,
            /// Colors were reduced to the xterm-256 palette.
            pub downgraded: bool,
            $( #[doc = $doc] pub $name: Color, )*
        }

        /// Every color token with its description.
        pub const TOKENS: &[(&str, &str)] = &[$( (stringify!($name), $doc), )*];

        impl Theme {
            fn blank(name: &str) -> Self {
                Theme {
                    name: name.into(),
                    mono: false,
                    heavy_focus: false,
                    light: false,
                    downgraded: false,
                    $( $name: Color::Reset, )*
                }
            }

            /// The value of a token by name.
            pub fn color(&self, token: &str) -> Option<Color> {
                match token {
                    $( stringify!($name) => Some(self.$name), )*
                    _ => None,
                }
            }

            fn color_mut(&mut self, token: &str) -> Option<&mut Color> {
                match token {
                    $( stringify!($name) => Some(&mut self.$name), )*
                    _ => None,
                }
            }

            fn each_color_mut(&mut self, mut f: impl FnMut(&mut Color)) {
                $( f(&mut self.$name); )*
            }
        }
    };
}

theme_struct! {
    /// Page background.
    bg,
    /// Body text.
    fg,
    /// Secondary text: labels, hints, placeholders.
    dim,
    /// Keys, active tabs and other interactive accents.
    accent,
    /// Borders of panes without focus.
    border,
    /// Border of the focused pane.
    border_focus,
    /// Background of the selected row.
    sel_bg,
    /// Text of the selected row.
    sel_fg,
    /// Input fields, the top bar and the status line.
    bar_bg,
    /// Success: 2xx status, passed assertions, saved.
    ok,
    /// Warnings: 4xx status, unsaved changes, a running request.
    warn,
    /// Errors: 5xx status, failed assertions, delete buttons.
    err,
    /// Information: 3xx status, notes.
    info,
    /// GET method.
    get,
    /// POST method.
    post,
    /// PUT method.
    put,
    /// PATCH method.
    patch,
    /// DELETE method.
    delete,
    /// Other HTTP methods.
    other,
    /// gRPC requests.
    grpc,
    /// JSON object keys.
    json_key,
    /// JSON strings.
    json_str,
    /// JSON numbers.
    json_num,
    /// JSON true, false and null.
    json_lit,
    /// A `{{variable}}` that is defined.
    var_ok,
    /// A `{{variable}}` that nothing defines.
    var_missing,
    /// Background of search matches.
    search_bg,
    /// Pane background (derived: bg).
    surface,
    /// Alternate table rows and the current line (derived from bg and fg).
    surface_alt,
    /// Dialog background (derived from bg and fg).
    overlay_bg,
    /// Background behind an open dialog (derived from bg).
    backdrop,
    /// Dialog shadow (derived from bg).
    shadow,
    /// Line numbers (derived from dim and bg).
    gutter,
    /// Background of key labels in the hint row (derived from bg and fg).
    keycap_bg,
    /// Pane titles (derived: fg).
    title,
    /// Scrollbar thumb (derived: dim).
    scroll_thumb,
    /// Timing: DNS lookup (derived: info).
    phase_dns,
    /// Timing: TCP connect (derived: accent).
    phase_connect,
    /// Timing: TLS handshake (derived: patch).
    phase_tls,
    /// Timing: waiting for the first byte (derived: warn).
    phase_wait,
    /// Timing: download (derived: ok).
    phase_download,
}

/// Tokens a built-in theme must give; the rest are derived.
pub const BASE_TOKENS: usize = 27;

/// A color from its `0xRRGGBB` value.
pub const fn h(v: u32) -> Color {
    Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

/// A theme before derived tokens are filled in: its explicit colors, in
/// order (later entries win).
#[derive(Debug, Clone)]
pub struct Spec {
    pub name: String,
    pub heavy_focus: bool,
    pub colors: Vec<(String, Color)>,
}

pub fn builtin_spec(name: &str) -> Option<Spec> {
    let (table, heavy) = match name {
        "dusk" => (DUSK, false),
        "daylight" => (DAYLIGHT, false),
        "ember" => (EMBER, false),
        "linen" => (LINEN, false),
        "contrast" => (CONTRAST, true),
        "ansi" => (ANSI, false),
        "dracula" => (DRACULA, false),
        "monokai" => (MONOKAI, false),
        "nord" => (NORD, false),
        "gruvbox" => (GRUVBOX, false),
        "one-dark" => (ONE_DARK, false),
        "catppuccin" => (CATPPUCCIN, false),
        "tokyo-night" => (TOKYO_NIGHT, false),
        "rose-pine" => (ROSE_PINE, false),
        "night-owl" => (NIGHT_OWL, false),
        "kanagawa" => (KANAGAWA, false),
        "everforest" => (EVERFOREST, false),
        "github-dark" => (GITHUB_DARK, false),
        "solarized-dark" => (SOLARIZED_DARK, false),
        "solarized-light" => (SOLARIZED_LIGHT, false),
        "gruvbox-light" => (GRUVBOX_LIGHT, false),
        "catppuccin-latte" => (CATPPUCCIN_LATTE, false),
        "github-light" => (GITHUB_LIGHT, false),
        "one-light" => (ONE_LIGHT, false),
        "turbo" => (TURBO, true),
        "phosphor" => (PHOSPHOR, true),
        "amber" => (AMBER, true),
        "bauhaus" => (BAUHAUS, true),
        "crimson" => (CRIMSON, true),
        "abyss" => (ABYSS, true),
        "grove" => (GROVE, true),
        "grape" => (GRAPE, true),
        _ => return None,
    };
    Some(Spec {
        name: name.to_string(),
        heavy_focus: heavy,
        colors: table.iter().map(|(k, c)| (k.to_string(), *c)).collect(),
    })
}

/// The spec of a built-in theme. `auto` and unknown names give dusk or
/// daylight from `COLORFGBG`.
pub fn builtin_or_auto(name: &str) -> Spec {
    if let Some(s) = builtin_spec(name) {
        return s;
    }
    let base = auto_base();
    builtin_spec(base).unwrap_or_else(|| Spec {
        name: base.into(),
        heavy_focus: false,
        colors: Vec::new(),
    })
}

impl Spec {
    /// The finished theme in full color.
    pub fn build(&self) -> Theme {
        let mut t = Theme::blank(&self.name);
        t.heavy_focus = self.heavy_focus;
        let mut set = BTreeSet::new();
        for (k, c) in &self.colors {
            if let Some(slot) = t.color_mut(k) {
                *slot = *c;
                set.insert(k.as_str());
            }
        }
        // Named terminal colors (the ansi theme) count as dark.
        t.light = luminance(t.bg).is_some_and(|l| l > 0.5);
        t.derive(&set);
        t
    }
}

impl Theme {
    /// Fills in the derived tokens a theme did not set.
    fn derive(&mut self, set: &BTreeSet<&str>) {
        let free = |k: &str| !set.contains(k);
        let black = Color::Rgb(0, 0, 0);
        if free("surface") {
            self.surface = self.bg;
        }
        if free("surface_alt") {
            self.surface_alt = self.mix(self.bg, self.fg, 0.06).unwrap_or(self.bg);
        }
        if free("overlay_bg") {
            self.overlay_bg = self.mix(self.bg, self.fg, 0.04).unwrap_or(self.bg);
        }
        if free("backdrop") {
            let t = if self.light { 0.10 } else { 0.30 };
            self.backdrop = self.mix(self.bg, black, t).unwrap_or(self.bg);
        }
        if free("shadow") {
            let t = if self.light { 0.22 } else { 0.55 };
            self.shadow = self.mix(self.bg, black, t).unwrap_or(self.bg);
        }
        if free("gutter") {
            self.gutter = self.mix(self.dim, self.bg, 0.3).unwrap_or(self.dim);
        }
        if free("keycap_bg") {
            self.keycap_bg = self.mix(self.bg, self.fg, 0.10).unwrap_or(self.bar_bg);
        }
        if free("title") {
            self.title = self.fg;
        }
        if free("scroll_thumb") {
            self.scroll_thumb = self.dim;
        }
        for (k, v) in [
            ("phase_dns", self.info),
            ("phase_connect", self.accent),
            ("phase_tls", self.patch),
            ("phase_wait", self.warn),
            ("phase_download", self.ok),
        ] {
            if free(k)
                && let Some(slot) = self.color_mut(k)
            {
                *slot = v;
            }
        }
    }

    pub fn dusk() -> Self {
        builtin_or_auto("dusk").build()
    }

    pub fn daylight() -> Self {
        builtin_or_auto("daylight").build()
    }

    pub fn ember() -> Self {
        builtin_or_auto("ember").build()
    }

    pub fn linen() -> Self {
        builtin_or_auto("linen").build()
    }

    pub fn contrast() -> Self {
        builtin_or_auto("contrast").build()
    }

    /// Uses the terminal's own 16-color palette.
    pub fn ansi() -> Self {
        builtin_or_auto("ansi").build()
    }

    /// No colors at all (`NO_COLOR`): focus and selection use attributes.
    pub fn mono() -> Self {
        let mut t = Theme::blank("mono");
        t.mono = true;
        t.heavy_focus = true;
        t
    }

    /// Resolves a built-in theme name. `auto` picks dusk or daylight from
    /// `COLORFGBG`.
    pub fn by_name(name: &str) -> Self {
        builtin_or_auto(name).build().for_terminal()
    }

    /// Reduces the theme to 256 colors when the terminal has no 24-bit color.
    pub fn for_terminal(mut self) -> Self {
        if !truecolor() {
            self.downgrade();
        }
        self
    }

    /// Converts RGB colors to the nearest xterm-256 index for terminals
    /// without 24-bit color (e.g. macOS Terminal.app).
    pub fn downgrade(&mut self) {
        self.each_color_mut(|c| {
            if let Color::Rgb(r, g, b) = *c {
                *c = Color::Indexed(rgb_to_256(r, g, b));
            }
        });
        self.downgraded = true;
    }

    /// `a` mixed with `b` (`t` = 0 → a, 1 → b). `None` when either color is
    /// not a known RGB value (named terminal colors).
    pub fn mix(&self, a: Color, b: Color, t: f32) -> Option<Color> {
        let (x, y) = (to_rgb(a)?, to_rgb(b)?);
        let m = |i: usize| (x[i] as f32 * (1.0 - t) + y[i] as f32 * t).round() as u8;
        let (r, g, bb) = (m(0), m(1), m(2));
        Some(if self.downgraded {
            Color::Indexed(rgb_to_256(r, g, bb))
        } else {
            Color::Rgb(r, g, bb)
        })
    }

    /// A light wash of `c` over the surface (chip backgrounds).
    pub fn tint(&self, c: Color) -> Option<Color> {
        let t = if self.light { 0.12 } else { 0.18 };
        self.mix(self.surface, c, t)
    }

    pub fn base(&self) -> Style {
        Style::default().fg(self.fg).bg(self.bg)
    }

    /// Pane content.
    pub fn surface(&self) -> Style {
        Style::default().fg(self.fg).bg(self.surface)
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

    /// A solid label: page-colored bold text on `c` (status, buttons).
    pub fn chip(&self, c: Color) -> Style {
        if self.mono {
            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default()
                .fg(self.bg)
                .bg(c)
                .add_modifier(Modifier::BOLD)
        }
    }

    /// A soft label: bold `c` on a wash of `c` (methods, status in lists).
    /// Named terminal colors cannot be washed: the label is underlined.
    pub fn badge(&self, c: Color) -> Style {
        let st = self.fg(c).add_modifier(Modifier::BOLD);
        match self.tint(c) {
            _ if self.mono => st,
            Some(bg) => st.bg(bg),
            None => st.add_modifier(Modifier::UNDERLINED),
        }
    }

    /// A key name in the hint row and in lists of keys.
    pub fn keycap(&self) -> Style {
        if self.mono {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            Style::default()
                .fg(self.accent)
                .bg(self.keycap_bg)
                .add_modifier(Modifier::BOLD)
        }
    }

    /// The colors that show the theme at a glance (Settings, the theme list).
    pub fn swatch(&self) -> [Color; 7] {
        [
            self.accent,
            self.ok,
            self.warn,
            self.err,
            self.info,
            self.patch,
            self.grpc,
        ]
    }

    pub fn method_color(&self, m: &str) -> Color {
        match m {
            "GET" => self.get,
            "POST" => self.post,
            "PUT" => self.put,
            "PATCH" => self.patch,
            "DELETE" => self.delete,
            "GRPC" => self.grpc,
            "MCP" => self.grpc,
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

    /// The color that marks an environment: production-like names stand out.
    pub fn env_color(&self, name: &str) -> Color {
        let n = name.to_ascii_lowercase();
        if n.contains("prod") || n == "live" {
            self.err
        } else if n.starts_with("stag") || n.contains("qa") || n.contains("test") || n == "uat" {
            self.warn
        } else if n.contains("local") || n.starts_with("dev") {
            self.ok
        } else {
            self.accent
        }
    }

    pub fn bar(&self) -> Style {
        Style::default().fg(self.fg).bg(self.bar_bg)
    }

    /// A list row under the mouse.
    pub fn hover(&self) -> Style {
        Style::default().fg(self.fg).bg(self.surface_alt)
    }

    /// The top bar and chips under the mouse (and the quiet button).
    pub fn raised(&self) -> Style {
        Style::default().fg(self.fg).bg(self.keycap_bg)
    }

    /// The content of a dialog.
    pub fn overlay(&self) -> Style {
        Style::default().fg(self.fg).bg(self.overlay_bg)
    }

    /// `c` as text on a dialog's background.
    pub fn on_overlay(&self, c: Color) -> Style {
        self.fg(c).bg(self.overlay_bg)
    }

    /// Secondary text inside a dialog.
    pub fn dialog_dim(&self) -> Style {
        self.dim().bg(self.overlay_bg)
    }

    /// Emphasis inside a dialog.
    pub fn dialog_accent(&self) -> Style {
        self.accent().bg(self.overlay_bg)
    }

    /// A pane or dialog heading.
    pub fn title_style(&self) -> Style {
        self.bold(self.fg(self.title))
    }

    pub fn bold(&self, st: Style) -> Style {
        st.add_modifier(Modifier::BOLD)
    }

    pub fn italic(&self, st: Style) -> Style {
        st.add_modifier(Modifier::ITALIC)
    }

    /// An input field: the bar color, or an underline when the bar has no
    /// color of its own (the `ansi` theme).
    pub fn field(&self) -> Style {
        if !self.mono && self.bar_bg == Color::Reset {
            self.bar().add_modifier(Modifier::UNDERLINED)
        } else {
            self.bar()
        }
    }

    /// `c` for text on a selected row, or the selection's text color when
    /// `c` would not be readable there.
    pub fn on_selection(&self, c: Color) -> Color {
        match contrast(c, self.sel_bg) {
            Some(r) if r < 4.5 => self.sel_fg,
            _ => c,
        }
    }

    /// The theme as a TOML theme file with every token (`sendito theme print`).
    pub fn to_toml(&self) -> String {
        let mut s = format!(
            "# Sendito theme \"{}\". Save it as ~/.config/sendito/themes/<name>.toml,\n\
             # change what you like and remove the rest (removed tokens come from\n\
             # `extends`). Token reference: docs/THEMES.md.\nname = \"my-{}\"\nextends = \"{}\"\n\n[colors]\n",
            self.name, self.name, self.name
        );
        for (k, doc) in TOKENS {
            if let Some(c) = self.color(k) {
                s.push_str(&format!("{k} = {}  # {}\n", color_toml(c), doc.trim()));
            }
        }
        s
    }
}

/// Text tokens and the backgrounds they must stay readable on (4.5:1).
pub fn contrast_notes(t: &Theme) -> Vec<String> {
    let text = [
        "fg",
        "dim",
        "accent",
        "ok",
        "warn",
        "err",
        "info",
        "get",
        "post",
        "put",
        "patch",
        "delete",
        "grpc",
        "json_key",
        "json_str",
        "json_num",
        "json_lit",
        "var_ok",
        "var_missing",
    ];
    let mut out = Vec::new();
    for bg in ["bg", "bar_bg", "sel_bg", "surface_alt"] {
        let Some(b) = t.color(bg) else { continue };
        for k in text {
            if let Some(c) = t.color(k)
                && let Some(r) = contrast(c, b)
                && r < 4.5
            {
                out.push(format!("{k} on {bg} is {r:.1}:1 (below 4.5:1)"));
            }
        }
    }
    out
}

/// The TOML value of a color, as a theme file writes it.
pub fn color_toml(c: Color) -> String {
    match c {
        Color::Rgb(r, g, b) => format!("\"#{r:02x}{g:02x}{b:02x}\""),
        Color::Indexed(i) => i.to_string(),
        Color::Reset => "\"reset\"".into(),
        Color::Black => "\"black\"".into(),
        Color::Red => "\"red\"".into(),
        Color::Green => "\"green\"".into(),
        Color::Yellow => "\"yellow\"".into(),
        Color::Blue => "\"blue\"".into(),
        Color::Magenta => "\"magenta\"".into(),
        Color::Cyan => "\"cyan\"".into(),
        Color::Gray => "\"gray\"".into(),
        Color::DarkGray => "\"dark-gray\"".into(),
        Color::LightRed => "\"light-red\"".into(),
        Color::LightGreen => "\"light-green\"".into(),
        Color::LightYellow => "\"light-yellow\"".into(),
        Color::LightBlue => "\"light-blue\"".into(),
        Color::LightMagenta => "\"light-magenta\"".into(),
        Color::LightCyan => "\"light-cyan\"".into(),
        Color::White => "\"white\"".into(),
    }
}

/// RGB of a color whose value is known: true color and the xterm-256 cube
/// and gray ramp (the first 16 depend on the terminal).
pub fn to_rgb(c: Color) -> Option<[u8; 3]> {
    match c {
        Color::Rgb(r, g, b) => Some([r, g, b]),
        Color::Indexed(i) if i >= 232 => {
            let v = 8 + 10 * (i - 232);
            Some([v, v, v])
        }
        Color::Indexed(i) if i >= 16 => {
            let i = i - 16;
            let cv = |n: u8| if n == 0 { 0 } else { 55 + 40 * n };
            Some([cv(i / 36), cv((i / 6) % 6), cv(i % 6)])
        }
        _ => None,
    }
}

pub fn luminance(c: Color) -> Option<f64> {
    let [r, g, b] = to_rgb(c)?;
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

/// WCAG contrast ratio, when both colors are known.
pub fn contrast(a: Color, b: Color) -> Option<f64> {
    let (x, y) = (luminance(a)?, luminance(b)?);
    Some((x.max(y) + 0.05) / (x.min(y) + 0.05))
}

/// The built-in theme `auto` stands for.
pub fn auto_base() -> &'static str {
    if terminal_is_light() {
        "daylight"
    } else {
        "dusk"
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
        for n in BUILTIN {
            let t = Theme::by_name(n);
            assert!(!t.name.is_empty());
        }
        assert_eq!(rgb_to_256(0, 0, 0), 16);
        assert_eq!(rgb_to_256(255, 255, 255), 231);
        assert_eq!(rgb_to_256(128, 128, 128), 244);
        let mut t = Theme::dusk();
        t.downgrade();
        assert!(matches!(t.bg, Color::Indexed(_)));
        assert!(matches!(t.surface_alt, Color::Indexed(_)));
        // Mixing still works on a reduced palette.
        assert!(matches!(t.tint(t.ok), Some(Color::Indexed(_))));
        let m = Theme::mono();
        assert!(m.selected().add_modifier.contains(Modifier::REVERSED));
        assert_eq!(m.fg(Color::Red), Style::default());
        assert_eq!(t.method_color("GRPC"), t.grpc);
        assert_eq!(t.status_color(404), t.warn);
        assert_eq!(t.env_color("production"), t.err);
        assert_eq!(t.env_color("staging"), t.warn);
        assert_eq!(t.env_color("local"), t.ok);
        assert_eq!(t.env_color("eu-west"), t.accent);
    }

    #[test]
    fn every_builtin_gives_the_base_tokens() {
        for n in BUILTIN.iter().filter(|n| **n != "auto") {
            let s = builtin_spec(n).unwrap();
            for (k, _) in &TOKENS[..BASE_TOKENS] {
                assert!(s.colors.iter().any(|(c, _)| c == k), "{n}: {k}");
            }
        }
        assert_eq!(TOKENS[BASE_TOKENS - 1].0, "search_bg");
    }

    #[test]
    fn derived_tokens_follow_the_base() {
        let d = Theme::dusk();
        assert_eq!(d.surface, d.bg);
        assert_ne!(d.surface_alt, d.bg);
        assert_eq!(d.phase_wait, d.warn);
        assert!(!d.light && Theme::daylight().light && Theme::linen().light);
        // The ansi theme has no RGB values: derived tokens fall back.
        let a = Theme::ansi();
        assert_eq!(a.surface_alt, a.bg);
        assert_eq!(a.tint(a.ok), None);
        assert_eq!(a.badge(a.ok).bg, None);
    }

    /// Text colors stay readable (WCAG AA, 4.5:1) on the background, the bars,
    /// the selection and alternate rows; labels on chips and washes too.
    #[test]
    fn text_contrast() {
        let r = |a, b| contrast(a, b).unwrap_or(21.0);
        // ansi has no RGB values to measure.
        for n in BUILTIN.iter().filter(|n| !matches!(**n, "auto" | "ansi")) {
            let t = builtin_spec(n).unwrap().build();
            // A theme with heavy focus has a solid selection (the contrast
            // and solid-color themes): text on it switches to sel_fg (see
            // `on_selection`).
            let notes: Vec<String> = contrast_notes(&t)
                .into_iter()
                .filter(|n| !(t.heavy_focus && n.contains("on sel_bg")))
                .collect();
            assert!(notes.is_empty(), "{}: {notes:?}", t.name);
            for c in [t.ok, t.err, t.get, t.grpc, t.accent] {
                assert!(
                    r(t.on_selection(c), t.sel_bg) >= 4.5,
                    "{}: on selection",
                    t.name
                );
            }
            assert!(r(t.sel_fg, t.sel_bg) >= 4.5, "{}: selection", t.name);
            assert!(r(t.accent, t.keycap_bg) >= 4.5, "{}: keycap", t.name);
            for c in [t.ok, t.warn, t.err, t.info, t.accent] {
                assert!(r(t.bg, c) >= 4.5, "{}: chip on {c:?}", t.name);
            }
            for m in ["GET", "POST", "PUT", "PATCH", "DELETE", "GRPC"] {
                let c = t.method_color(m);
                let bg = t.tint(c).unwrap();
                assert!(r(c, bg) >= 4.5, "{}: {m} badge", t.name);
            }
            assert!(r(t.gutter, t.bg) >= 3.0, "{}: gutter", t.name);
        }
    }

    /// docs/THEMES.md lists every token.
    #[test]
    fn themes_md_lists_every_token() {
        let doc = include_str!("../../../docs/THEMES.md");
        for (k, _) in TOKENS {
            assert!(
                doc.contains(&format!("| `{k}` |")),
                "docs/THEMES.md lacks {k}"
            );
        }
        for n in BUILTIN {
            assert!(
                doc.contains(&format!("| `{n}` |")),
                "docs/THEMES.md lacks {n}"
            );
        }
    }

    #[test]
    fn ansi_falls_back_to_underline() {
        let u = |s: Style| s.add_modifier.contains(Modifier::UNDERLINED);
        let a = Theme::ansi();
        assert!(u(a.field()));
        assert!(u(a.badge(a.get)));
        let d = Theme::dusk();
        assert!(!u(d.field()));
        assert!(!u(d.badge(d.get)));
        let m = Theme::mono();
        assert!(!u(m.field()));
        assert!(!u(m.badge(m.get)));
    }
}
