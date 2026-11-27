//! Theme files: the registry of built-in and user themes, built from the
//! theme files the store reads. The theme itself is in `sendito_widgets::theme`.

use std::collections::BTreeMap;
use std::path::PathBuf;

use ratatui::style::Color;
pub use sendito_widgets::theme::*;

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

    /// What kind of theme `name` is, for the lists that show it ("dark",
    /// "light · solid selection", "yours · dark"...). Typing a kind in the
    /// filter finds the themes of that kind.
    pub fn kind(&self, name: &str) -> String {
        match name {
            "auto" => return format!("follows the terminal ({})", auto_base()),
            "ansi" => return "your terminal's 16 colors".into(),
            _ => {}
        }
        let t = self.theme_full(name);
        let mut s = String::new();
        if self.path(name).is_some() {
            s.push_str("yours · ");
        }
        s.push_str(if t.light { "light" } else { "dark" });
        if t.heavy_focus {
            s.push_str(" · solid selection");
        }
        s
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
        if builtin_spec(name).is_none()
            && let Some((s, _)) = self.user.iter().find(|(s, _)| s.name == name)
        {
            return s.clone();
        }
        builtin_or_auto(name)
    }

    /// The theme in full color, whatever the terminal supports.
    pub fn theme_full(&self, name: &str) -> Theme {
        self.spec(name).build()
    }

    /// The theme for this terminal (reduced to 256 colors when needed).
    pub fn theme(&self, name: &str) -> Theme {
        self.theme_full(name).for_terminal()
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

#[cfg(test)]
mod tests {
    use super::*;
    use sendito_store::themes::{ColorValue, ThemeFile};

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
}
