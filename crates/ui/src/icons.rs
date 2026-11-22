//! Icon sets: `unicode` (any font) and `nerd` (Nerd Font icons, opt-in with
//! `ui.icons = "nerd"`). Every icon takes one terminal cell.
//!
//! Nerd icons use the Font Awesome block (U+F000-F2E0) and the Powerline
//! caps (U+E0B4/E0B6), which every Nerd Fonts release since v2 keeps at the
//! same code points.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Icons {
    pub nerd: bool,
    /// Before a folder name; empty in the unicode set (▸/▾ mark folders).
    pub folder_closed: &'static str,
    pub folder_open: &'static str,
    /// Before a gRPC request name; empty in the unicode set.
    pub grpc: &'static str,
    /// The environment chip.
    pub env: &'static str,
    pub history: &'static str,
    pub requests: &'static str,
    pub send: &'static str,
    pub cancel: &'static str,
    pub ok: &'static str,
    pub fail: &'static str,
    pub warn: &'static str,
    pub info: &'static str,
    pub lock: &'static str,
    pub search: &'static str,
    pub settings: &'static str,
    pub import: &'static str,
    pub clock: &'static str,
    /// Unsaved changes.
    pub dirty: &'static str,
    /// Close a tab.
    pub close: &'static str,
    pub expanded: &'static str,
    pub collapsed: &'static str,
    /// Rounded ends of chips and tabs (empty: square ends).
    pub cap_left: &'static str,
    pub cap_right: &'static str,
}

pub const SPINNER: [&str; 10] = ["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

impl Icons {
    pub fn unicode() -> Self {
        Icons {
            nerd: false,
            folder_closed: "",
            folder_open: "",
            grpc: "",
            env: "●",
            history: "",
            requests: "",
            send: "",
            cancel: "",
            ok: "✓",
            fail: "✗",
            warn: "!",
            info: "i",
            lock: "",
            search: "/",
            settings: "",
            import: "",
            clock: "",
            dirty: "●",
            close: "×",
            expanded: "▾",
            collapsed: "▸",
            cap_left: "",
            cap_right: "",
        }
    }

    pub fn nerd() -> Self {
        Icons {
            nerd: true,
            folder_closed: "\u{f07b}",
            folder_open: "\u{f07c}",
            grpc: "\u{f0e7}",
            env: "\u{f0ac}",
            history: "\u{f1da}",
            requests: "\u{f0c9}",
            send: "\u{f1d8}",
            cancel: "\u{f04d}",
            ok: "\u{f00c}",
            fail: "\u{f00d}",
            warn: "\u{f071}",
            info: "\u{f05a}",
            lock: "\u{f023}",
            search: "\u{f002}",
            settings: "\u{f013}",
            import: "\u{f019}",
            clock: "\u{f017}",
            dirty: "●",
            close: "\u{f00d}",
            expanded: "▾",
            collapsed: "▸",
            cap_left: "\u{e0b6}",
            cap_right: "\u{e0b4}",
        }
    }

    pub fn by_name(name: &str) -> Self {
        if name.eq_ignore_ascii_case("nerd") {
            Self::nerd()
        } else {
            Self::unicode()
        }
    }

    /// `icon` followed by a space, or nothing when the set has no such icon.
    pub fn pre(icon: &str) -> String {
        if icon.is_empty() {
            String::new()
        } else {
            format!("{icon} ")
        }
    }

    #[cfg(test)]
    fn all(&self) -> [&'static str; 22] {
        [
            self.folder_closed,
            self.folder_open,
            self.grpc,
            self.env,
            self.history,
            self.requests,
            self.send,
            self.cancel,
            self.ok,
            self.fail,
            self.warn,
            self.info,
            self.lock,
            self.search,
            self.settings,
            self.import,
            self.clock,
            self.dirty,
            self.close,
            self.expanded,
            self.collapsed,
            self.cap_left,
        ]
    }
}

pub const ICON_SETS: &[&str] = &["unicode", "nerd"];

#[cfg(test)]
mod tests {
    use super::*;
    use unicode_width::UnicodeWidthStr;

    /// Every icon is one cell wide (or absent), so layouts line up.
    #[test]
    fn icons_are_one_cell() {
        for set in [Icons::unicode(), Icons::nerd()] {
            for i in set.all().into_iter().chain([set.cap_right]) {
                assert!(i.width() <= 1 && i.chars().count() <= 1, "{i:?}");
            }
        }
        for s in SPINNER {
            assert_eq!(s.width(), 1);
        }
        assert!(Icons::by_name("Nerd").nerd && !Icons::by_name("x").nerd);
        assert_eq!(Icons::pre(""), "");
        assert_eq!(Icons::pre("✓"), "✓ ");
    }
}
