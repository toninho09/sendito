//! Color themes. Built in: `auto`, `dusk` (dark), `daylight` (light), `ember`
//! (warm dark), `linen` (warm light), `contrast` and `ansi`; users add their own
//! as TOML files (see `docs/THEMES.md`). `NO_COLOR` switches to a monochrome
//! theme that uses only text attributes.
//!
//! A theme is a list of color tokens. The *base* tokens are given by every
//! built-in theme; the *derived* ones (surfaces, shadow, gutter, timing phases)
//! are computed from the base tokens unless a theme sets them.

use std::collections::{BTreeMap, BTreeSet};
use std::path::PathBuf;
use std::sync::Arc;

use ratatui::style::{Color, Modifier, Style};

/// Built-in theme names, in the order Settings cycles through them.
pub const BUILTIN: &[&str] = &[
    "auto", "dusk", "daylight", "ember", "linen", "contrast", "ansi",
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

const fn h(v: u32) -> Color {
    Color::Rgb((v >> 16) as u8, (v >> 8) as u8, v as u8)
}

type Table = &'static [(&'static str, Color)];

const DUSK: Table = &[
    ("bg", h(0x1b1e28)),
    ("fg", h(0xd8dce6)),
    ("dim", h(0xa0a8bd)),
    ("accent", h(0x86acff)),
    ("border", h(0x454b5e)),
    ("border_focus", h(0x86acff)),
    ("sel_bg", h(0x2e3a5e)),
    ("sel_fg", h(0xffffff)),
    ("bar_bg", h(0x242836)),
    ("ok", h(0x9ece6a)),
    ("warn", h(0xe0af68)),
    ("err", h(0xff8ba0)),
    ("info", h(0x7dcfff)),
    ("get", h(0x9ece6a)),
    ("post", h(0xe0af68)),
    ("put", h(0x86acff)),
    ("patch", h(0xbb9af7)),
    ("delete", h(0xff8ba0)),
    ("other", h(0x7dcfff)),
    ("grpc", h(0x2ac3de)),
    ("json_key", h(0x86acff)),
    ("json_str", h(0x9ece6a)),
    ("json_num", h(0xff9e64)),
    ("json_lit", h(0xbb9af7)),
    ("var_ok", h(0x2ac3de)),
    ("var_missing", h(0xff8ba0)),
    ("search_bg", h(0x6b5a1e)),
    ("surface", h(0x1b1e28)),
    ("overlay_bg", h(0x222634)),
];

/// Every foreground color has at least 4.5:1 contrast on the background,
/// the bar and the selection (WCAG AA).
const DAYLIGHT: Table = &[
    ("bg", h(0xfafaf7)),
    ("fg", h(0x2b2f3a)),
    ("dim", h(0x3e434e)),
    ("accent", h(0x295bd0)),
    ("border", h(0x7d8494)),
    ("border_focus", h(0x295bd0)),
    ("sel_bg", h(0xd6e2ff)),
    ("sel_fg", h(0x101420)),
    ("bar_bg", h(0xeceef2)),
    ("ok", h(0x29702c)),
    ("warn", h(0x905500)),
    ("err", h(0xbe2626)),
    ("info", h(0x006a98)),
    ("get", h(0x29702c)),
    ("post", h(0x905500)),
    ("put", h(0x295bd0)),
    ("patch", h(0x7b3fc4)),
    ("delete", h(0xbe2626)),
    ("other", h(0x006a98)),
    ("grpc", h(0x006d78)),
    ("json_key", h(0x295bd0)),
    ("json_str", h(0x29702c)),
    ("json_num", h(0xa44600)),
    ("json_lit", h(0x7b3fc4)),
    ("var_ok", h(0x006d78)),
    ("var_missing", h(0xbe2626)),
    ("search_bg", h(0xffe58f)),
    ("overlay_bg", h(0xffffff)),
];

/// Warm dark: charcoal with amber and coral.
const EMBER: Table = &[
    ("bg", h(0x1f1a17)),
    ("fg", h(0xe8dccf)),
    ("dim", h(0xb0a191)),
    ("accent", h(0xf0a35e)),
    ("border", h(0x473c34)),
    ("border_focus", h(0xf0a35e)),
    ("sel_bg", h(0x3a2d24)),
    ("sel_fg", h(0xfff6ec)),
    ("bar_bg", h(0x2a231f)),
    ("ok", h(0xa9c77a)),
    ("warn", h(0xe9b44c)),
    ("err", h(0xf2837b)),
    ("info", h(0x7fc4c9)),
    ("get", h(0xa9c77a)),
    ("post", h(0xe9b44c)),
    ("put", h(0x8fb4e8)),
    ("patch", h(0xd29be0)),
    ("delete", h(0xf2837b)),
    ("other", h(0x7fc4c9)),
    ("grpc", h(0x6fd0c0)),
    ("json_key", h(0x8fb4e8)),
    ("json_str", h(0xa9c77a)),
    ("json_num", h(0xf29e6d)),
    ("json_lit", h(0xd29be0)),
    ("var_ok", h(0x6fd0c0)),
    ("var_missing", h(0xf2837b)),
    ("search_bg", h(0x6b4a1c)),
    ("overlay_bg", h(0x29221e)),
];

/// Warm light: paper with ink blue and teal.
const LINEN: Table = &[
    ("bg", h(0xf6f1e7)),
    ("fg", h(0x2e2a24)),
    ("dim", h(0x4f493f)),
    ("accent", h(0x2c5a8c)),
    ("border", h(0xa89d8b)),
    ("border_focus", h(0x2c5a8c)),
    ("sel_bg", h(0xe5dcc8)),
    ("sel_fg", h(0x1d1a15)),
    ("bar_bg", h(0xece5d6)),
    ("ok", h(0x35601f)),
    ("warn", h(0x7a4a00)),
    ("err", h(0xa3321f)),
    ("info", h(0x1a5a68)),
    ("get", h(0x35601f)),
    ("post", h(0x7a4a00)),
    ("put", h(0x2c5a8c)),
    ("patch", h(0x73398a)),
    ("delete", h(0xa3321f)),
    ("other", h(0x1a5a68)),
    ("grpc", h(0x0f5c56)),
    ("json_key", h(0x2c5a8c)),
    ("json_str", h(0x35601f)),
    ("json_num", h(0x8a3f0c)),
    ("json_lit", h(0x73398a)),
    ("var_ok", h(0x0f5c56)),
    ("var_missing", h(0xa3321f)),
    ("search_bg", h(0xf5d77a)),
    ("overlay_bg", h(0xfbf8f2)),
];

const CONTRAST: Table = &[
    ("bg", h(0x000000)),
    ("fg", h(0xffffff)),
    ("dim", h(0xc0c0c0)),
    ("accent", h(0xffd700)),
    ("border", h(0xb0b0b0)),
    ("border_focus", h(0xffd700)),
    ("sel_bg", h(0xffd700)),
    ("sel_fg", h(0x000000)),
    ("bar_bg", h(0x202020)),
    ("ok", h(0x5fff5f)),
    ("warn", h(0xffd700)),
    ("err", h(0xff5f5f)),
    ("info", h(0x5fd7ff)),
    ("get", h(0x5fff5f)),
    ("post", h(0xffd700)),
    ("put", h(0x5fd7ff)),
    ("patch", h(0xff87ff)),
    ("delete", h(0xff5f5f)),
    ("other", h(0xffffff)),
    ("grpc", h(0x5fffff)),
    ("json_key", h(0x5fd7ff)),
    ("json_str", h(0x5fff5f)),
    ("json_num", h(0xffaf5f)),
    ("json_lit", h(0xff87ff)),
    ("var_ok", h(0x5fffff)),
    ("var_missing", h(0xff5f5f)),
    ("search_bg", h(0x875f00)),
    ("surface_alt", h(0x161616)),
    ("keycap_bg", h(0x2a2a2a)),
];

/// The terminal's own 16-color palette.
const ANSI: Table = &[
    ("bg", Color::Reset),
    ("fg", Color::Reset),
    ("dim", Color::DarkGray),
    ("accent", Color::Blue),
    ("border", Color::DarkGray),
    ("border_focus", Color::Blue),
    ("sel_bg", Color::Blue),
    ("sel_fg", Color::White),
    ("bar_bg", Color::Reset),
    ("ok", Color::Green),
    ("warn", Color::Yellow),
    ("err", Color::Red),
    ("info", Color::Cyan),
    ("get", Color::Green),
    ("post", Color::Yellow),
    ("put", Color::Blue),
    ("patch", Color::Magenta),
    ("delete", Color::Red),
    ("other", Color::Gray),
    ("grpc", Color::Cyan),
    ("json_key", Color::Blue),
    ("json_str", Color::Green),
    ("json_num", Color::Yellow),
    ("json_lit", Color::Magenta),
    ("var_ok", Color::Cyan),
    ("var_missing", Color::Red),
    ("search_bg", Color::Yellow),
];

/// A theme before derived tokens are filled in: its explicit colors, in
/// order (later entries win).
#[derive(Debug, Clone)]
pub struct Spec {
    pub name: String,
    pub light: bool,
    pub heavy_focus: bool,
    pub colors: Vec<(String, Color)>,
}

fn builtin_spec(name: &str) -> Option<Spec> {
    let (table, light, heavy) = match name {
        "dusk" => (DUSK, false, false),
        "daylight" => (DAYLIGHT, true, false),
        "ember" => (EMBER, false, false),
        "linen" => (LINEN, true, false),
        "contrast" => (CONTRAST, false, true),
        "ansi" => (ANSI, false, false),
        _ => return None,
    };
    Some(Spec {
        name: name.to_string(),
        light,
        heavy_focus: heavy,
        colors: table.iter().map(|(k, c)| (k.to_string(), *c)).collect(),
    })
}

impl Spec {
    /// The finished theme in full color.
    pub fn build(&self) -> Theme {
        let mut t = Theme::blank(&self.name);
        t.light = self.light;
        t.heavy_focus = self.heavy_focus;
        let mut set = BTreeSet::new();
        for (k, c) in &self.colors {
            if let Some(slot) = t.color_mut(k) {
                *slot = *c;
                set.insert(k.as_str());
            }
        }
        if let Some(l) = luminance(t.bg) {
            t.light = l > 0.5;
        }
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
        Registry::builtin().theme_full("dusk")
    }

    pub fn daylight() -> Self {
        Registry::builtin().theme_full("daylight")
    }

    pub fn ember() -> Self {
        Registry::builtin().theme_full("ember")
    }

    pub fn linen() -> Self {
        Registry::builtin().theme_full("linen")
    }

    pub fn contrast() -> Self {
        Registry::builtin().theme_full("contrast")
    }

    /// Uses the terminal's own 16-color palette.
    pub fn ansi() -> Self {
        Registry::builtin().theme_full("ansi")
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
        Registry::builtin().theme(name)
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

/// Built-in themes plus the user's theme files.
#[derive(Debug, Clone, Default)]
pub struct Registry {
    user: Vec<(Spec, PathBuf)>,
    /// Problems per user theme (bad tokens, low contrast).
    notes: BTreeMap<String, Vec<String>>,
    /// Problems with files that could not be used at all.
    pub warnings: Vec<String>,
}

impl Registry {
    pub fn builtin() -> Self {
        Self::default()
    }

    /// Builds the registry from parsed theme files.
    pub fn from_files(
        files: Vec<sendito_store::themes::ThemeFile>,
        file_warnings: Vec<String>,
    ) -> Self {
        let mut reg = Registry {
            warnings: file_warnings,
            ..Default::default()
        };
        let by_name: BTreeMap<String, &sendito_store::themes::ThemeFile> = files
            .iter()
            .filter(|f| !BUILTIN.contains(&f.name.as_str()))
            .map(|f| (f.name.clone(), f))
            .collect();
        for f in &files {
            if BUILTIN.contains(&f.name.as_str()) || f.name == "mono" {
                reg.warnings.push(format!(
                    "{}: \"{}\" is a built-in theme name; rename the theme",
                    f.path.display(),
                    f.name
                ));
                continue;
            }
            let mut notes = Vec::new();
            let spec = resolve(&f.name, &by_name, &mut Vec::new(), &mut notes);
            let theme = spec.build();
            notes.extend(contrast_notes(&theme));
            if !notes.is_empty() {
                reg.notes.insert(f.name.clone(), notes);
            }
            reg.user.push((spec, f.path.clone()));
        }
        reg
    }

    /// Every theme name: built-ins first, then the user's, by name.
    pub fn names(&self) -> Vec<String> {
        BUILTIN
            .iter()
            .map(|s| s.to_string())
            .chain(self.user.iter().map(|(s, _)| s.name.clone()))
            .collect()
    }

    pub fn is_known(&self, name: &str) -> bool {
        BUILTIN.contains(&name) || self.user.iter().any(|(s, _)| s.name == name)
    }

    /// Problems found in a user theme.
    pub fn notes(&self, name: &str) -> &[String] {
        self.notes.get(name).map(Vec::as_slice).unwrap_or(&[])
    }

    pub fn path(&self, name: &str) -> Option<&PathBuf> {
        self.user
            .iter()
            .find(|(s, _)| s.name == name)
            .map(|(_, p)| p)
    }

    fn spec(&self, name: &str) -> Spec {
        if let Some(s) = builtin_spec(name) {
            return s;
        }
        if let Some((s, _)) = self.user.iter().find(|(s, _)| s.name == name) {
            return s.clone();
        }
        // `auto` and unknown names.
        let base = auto_base();
        builtin_spec(base).unwrap_or_else(|| Spec {
            name: base.into(),
            light: false,
            heavy_focus: false,
            colors: Vec::new(),
        })
    }

    /// The theme in full color, whatever the terminal supports.
    pub fn theme_full(&self, name: &str) -> Theme {
        self.spec(name).build()
    }

    /// The theme for this terminal (reduced to 256 colors when needed).
    pub fn theme(&self, name: &str) -> Theme {
        let mut t = self.theme_full(name);
        if !truecolor() {
            t.downgrade();
        }
        t
    }
}

/// The spec of theme `name`: for a user theme, its parent's colors, then
/// its own.
fn resolve(
    name: &str,
    files: &BTreeMap<String, &sendito_store::themes::ThemeFile>,
    seen: &mut Vec<String>,
    notes: &mut Vec<String>,
) -> Spec {
    let fallback = || {
        builtin_spec("dusk").unwrap_or_else(|| Spec {
            name: "dusk".into(),
            light: false,
            heavy_focus: false,
            colors: Vec::new(),
        })
    };
    if let Some(s) = builtin_spec(name) {
        return s;
    }
    if name == "auto" {
        return builtin_spec(auto_base()).unwrap_or_else(fallback);
    }
    let Some(f) = files.get(name) else {
        notes.push(format!("unknown theme \"{name}\" in extends; using dusk"));
        return fallback();
    };
    if seen.iter().any(|s| s == name) {
        notes.push(format!("\"extends\" loops back to \"{name}\"; using dusk"));
        return fallback();
    }
    seen.push(name.to_string());
    let parent = f.extends.clone().unwrap_or_else(|| "dusk".into());
    let mut spec = resolve(&parent, files, seen, notes);
    spec.name = name.to_string();
    for (k, v) in &f.colors {
        if !TOKENS.iter().any(|(t, _)| t == k) {
            notes.push(format!("unknown token \"{k}\""));
            continue;
        }
        match parse_color(v) {
            Ok(c) => spec.colors.push((k.clone(), c)),
            Err(e) => notes.push(format!("{k}: {e}")),
        }
    }
    spec
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

/// Parses a color as written in a theme file.
pub fn parse_color(v: &sendito_store::themes::ColorValue) -> Result<Color, String> {
    use sendito_store::themes::ColorValue;
    match v {
        ColorValue::Index(i) => u8::try_from(*i)
            .map(Color::Indexed)
            .map_err(|_| format!("{i} is not a color index (0-255)")),
        ColorValue::Text(s) => {
            let t = s.trim();
            if let Some(hex) = t.strip_prefix('#') {
                let digits = |s: &str| u32::from_str_radix(s, 16).ok();
                return match (hex.len(), digits(hex)) {
                    (6, Some(v)) => Ok(h(v)),
                    (3, Some(v)) => {
                        let e = |n: u32| ((n & 0xf) * 17) as u8;
                        Ok(Color::Rgb(e(v >> 8), e(v >> 4), e(v)))
                    }
                    _ => Err(format!("\"{t}\" is not #rgb or #rrggbb")),
                };
            }
            let n: String = t
                .to_ascii_lowercase()
                .chars()
                .filter(|c| c.is_ascii_alphanumeric())
                .collect::<String>()
                .replace("bright", "light")
                .replace("grey", "gray");
            Ok(match n.as_str() {
                "reset" | "default" | "none" => Color::Reset,
                "black" => Color::Black,
                "red" => Color::Red,
                "green" => Color::Green,
                "yellow" => Color::Yellow,
                "blue" => Color::Blue,
                "magenta" => Color::Magenta,
                "cyan" => Color::Cyan,
                "gray" => Color::Gray,
                "darkgray" | "lightblack" => Color::DarkGray,
                "lightred" => Color::LightRed,
                "lightgreen" => Color::LightGreen,
                "lightyellow" => Color::LightYellow,
                "lightblue" => Color::LightBlue,
                "lightmagenta" => Color::LightMagenta,
                "lightcyan" => Color::LightCyan,
                "white" | "lightwhite" => Color::White,
                _ => return Err(format!("\"{t}\" is not a color")),
            })
        }
    }
}

fn color_toml(c: Color) -> String {
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
fn auto_base() -> &'static str {
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
    use sendito_store::themes::{ColorValue, ThemeFile};

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
        for n in ["dusk", "daylight", "ember", "linen", "contrast", "ansi"] {
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
        for t in [
            Theme::dusk(),
            Theme::daylight(),
            Theme::ember(),
            Theme::linen(),
            Theme::contrast(),
        ] {
            // The contrast theme's selection is a solid yellow: text on it
            // switches to sel_fg (see `on_selection`).
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
    fn colors_parse() {
        let p = |s: &str| parse_color(&ColorValue::Text(s.into()));
        assert_eq!(p("#102030"), Ok(Color::Rgb(0x10, 0x20, 0x30)));
        assert_eq!(p("#fff"), Ok(Color::Rgb(255, 255, 255)));
        assert_eq!(p("Bright-Blue"), Ok(Color::LightBlue));
        assert_eq!(p("dark_grey"), Ok(Color::DarkGray));
        assert_eq!(p("reset"), Ok(Color::Reset));
        assert!(p("#12").is_err() && p("#gggggg").is_err() && p("teal").is_err());
        assert_eq!(
            parse_color(&ColorValue::Index(240)),
            Ok(Color::Indexed(240))
        );
        assert!(parse_color(&ColorValue::Index(256)).is_err());
        for c in [
            h(0x123456),
            Color::Indexed(3),
            Color::DarkGray,
            Color::Reset,
        ] {
            let text = color_toml(c);
            let v = match text.trim_matches('"').parse::<i64>() {
                Ok(i) => ColorValue::Index(i),
                Err(_) => ColorValue::Text(text.trim_matches('"').into()),
            };
            assert_eq!(parse_color(&v), Ok(c), "{text}");
        }
    }

    fn file(name: &str, extends: Option<&str>, colors: &[(&str, ColorValue)]) -> ThemeFile {
        ThemeFile {
            name: name.into(),
            extends: extends.map(Into::into),
            colors: colors
                .iter()
                .map(|(k, v)| (k.to_string(), v.clone()))
                .collect(),
            path: PathBuf::from(format!("/t/{name}.toml")),
        }
    }

    #[test]
    fn user_themes() {
        let t = |s: &str| ColorValue::Text(s.into());
        let reg = Registry::from_files(
            vec![
                file("harbor", Some("daylight"), &[("accent", t("#004488"))]),
                file("harbor-night", Some("harbor"), &[("bg", t("#000000"))]),
                file("loop-a", Some("loop-b"), &[]),
                file("loop-b", Some("loop-a"), &[]),
                file(
                    "odd",
                    Some("nowhere"),
                    &[("nonsense", t("red")), ("fg", t("??"))],
                ),
                file("dusk", None, &[]),
            ],
            vec!["/t/broken.toml: bad".into()],
        );
        let names = reg.names();
        assert_eq!(&names[..BUILTIN.len()], BUILTIN);
        assert!(
            names.contains(&"harbor".to_string())
                && !names[BUILTIN.len()..].contains(&"dusk".to_string())
        );
        assert!(reg.warnings.iter().any(|w| w.contains("built-in")));
        assert!(reg.warnings.iter().any(|w| w.contains("broken")));
        let h1 = reg.theme_full("harbor");
        assert_eq!(h1.accent, Color::Rgb(0, 0x44, 0x88));
        assert_eq!(h1.fg, Theme::daylight().fg);
        assert!(h1.light);
        // The child inherits the parent's accent and re-derives from its own bg.
        let h2 = reg.theme_full("harbor-night");
        assert_eq!(h2.accent, h1.accent);
        assert_eq!(h2.surface, Color::Rgb(0, 0, 0));
        assert!(!h2.light);
        // Low contrast is reported, not refused.
        assert!(
            reg.notes("harbor-night")
                .iter()
                .any(|n| n.contains("below 4.5:1"))
        );
        assert!(reg.notes("loop-a").iter().any(|n| n.contains("loops")));
        let odd = reg.notes("odd");
        assert!(odd.iter().any(|n| n.contains("nowhere")));
        assert!(odd.iter().any(|n| n.contains("nonsense")));
        assert!(odd.iter().any(|n| n.starts_with("fg:")));
        assert!(reg.is_known("odd") && !reg.is_known("nope"));
        assert_eq!(reg.path("odd"), Some(&PathBuf::from("/t/odd.toml")));
        // A theme printed as TOML reads back as the same theme.
        let text = Theme::ember().to_toml();
        let f = sendito_store::themes::parse(&text, std::path::Path::new("/t/x.toml")).unwrap();
        let back = Registry::from_files(vec![f], vec![]).theme_full("my-ember");
        let mut e = Theme::ember();
        e.name = back.name.clone();
        assert_eq!(back, e);
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
