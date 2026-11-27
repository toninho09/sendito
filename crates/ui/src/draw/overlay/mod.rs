//! Dialogs, pickers, the palette, help and settings. The screen behind a
//! dialog is faded (not cleared) and every dialog casts a shadow.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use crate::app::*;
use crate::editor::TextEditor;
use crate::overlay::*;
use crate::theme::Theme;
use crate::viewer::{ViewMode, Viewer};

use super::dialog;
use super::dialog::{buttons, inset, popup, shell, shell_at};
use super::editor::{self, EditorDialog, EditorHits, Head};
use super::kit::*;

mod dialogs;
mod editors;
mod keymap;
mod palette;
mod records;
mod report;
mod settings;
use dialogs::*;
use editors::*;
use keymap::*;
use palette::*;
use records::*;
use report::*;
use settings::*;

/// The screen a dialog is drawn on: the whole overlay `area`, the `body` the
/// dialog and its shadow stay in (the area without the footer), the theme
/// and the style of a line of dialog text.
#[derive(Clone, Copy)]
struct Frame<'a> {
    area: Rect,
    body: Rect,
    theme: &'a Theme,
    text: Style,
}

pub(super) fn draw_overlay(
    app: &mut App,
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
) -> Option<(u16, u16)> {
    let mut ov = app.overlay.take()?;
    let mut cursor = None;
    let names = app.names.clone();
    let is_def = move |n: &str| names.is_defined(n);
    app.hits.push((area, Hit::Overlay(OverlayHit::Outside)));
    // Dialogs leave the footer as it is: its hints describe the dialog. The
    // rest of the screen fades behind it.
    let body = Rect::new(
        area.x,
        area.y,
        area.width,
        area.height.saturating_sub(super::FOOTER_ROWS),
    );
    backdrop(buf, body, theme);
    let frame = Frame {
        area,
        body,
        theme,
        text: theme.overlay(),
    };
    match &mut ov {
        Overlay::Palette(p) => draw_palette(app, buf, frame, &is_def, p, &mut cursor),
        Overlay::Help {
            viewer,
            filter,
            page,
        } => draw_help(app, buf, frame, &is_def, viewer, filter, *page, &mut cursor),
        Overlay::Settings(v) => draw_settings(app, buf, frame, v, &mut cursor),
        Overlay::Confirm(c) => draw_confirm(app, buf, frame, c),
        Overlay::Prompt(p) => draw_prompt(app, buf, frame, &is_def, p, &mut cursor),
        Overlay::Picker(p) => draw_picker(app, buf, frame, &is_def, p, &mut cursor),
        Overlay::Env(e) => draw_env(app, buf, frame, &is_def, e, &mut cursor),
        Overlay::Records(e) => draw_records(app, buf, frame, &is_def, e, &mut cursor),
        Overlay::Keymap(e) => draw_keymap(app, buf, frame, &is_def, e, &mut cursor),
        Overlay::Import(i) => draw_import(app, buf, frame, &is_def, i, &mut cursor),
        Overlay::Report(rep) => draw_report(app, buf, frame, rep),
    }
    app.overlay = Some(ov);
    cursor
}

/// A viewer draws on the page background; inside a dialog its rows take the
/// dialog's color instead (rows that kept the page color).
fn set_bg_unset(buf: &mut Buffer, area: Rect, theme: &Theme) {
    if theme.mono {
        return;
    }
    let area = area.intersection(*buf.area());
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(c) = buf.cell_mut((x, y))
                && c.bg == theme.bg
            {
                c.set_bg(theme.overlay_bg);
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn match_positions_prefer_substrings() {
        assert_eq!(match_positions("Send the request", "the"), vec![5, 6, 7]);
        assert_eq!(
            match_positions("Open POST list users", "plu"),
            vec![1, 10, 15]
        );
        assert!(match_positions("abc", "abd").is_empty());
        assert!(match_positions("abc", "").is_empty());
    }
}
