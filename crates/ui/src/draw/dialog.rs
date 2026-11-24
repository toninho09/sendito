//! Dialog chrome: the shell every modal shares (shadow, clear, frame, title),
//! the popup used for small floating lists, the input row, the button footer
//! and the status line above it.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use ratatui::widgets::{Clear, Widget};

use crate::app::{App, Hit, OverlayHit};
use crate::editor::TextEditor;
use crate::theme::Theme;

use super::kit::{button, centered, frame_with, input_box, put, shadow, title, truncate};

/// A dialog of about `w`×`h` centered in `body`, with its shadow and title.
/// Dead space inside the dialog is registered as a click target so it cannot
/// fall through to the backdrop.
pub(super) fn shell(
    app: &mut App,
    buf: &mut Buffer,
    body: Rect,
    w: u16,
    h: u16,
    text: &str,
    theme: &Theme,
) -> (Rect, Rect) {
    let r = centered(body, w, h);
    shell_at(app, buf, r, body, text, theme)
}

/// A dialog at `r` (see [`shell`]).
pub(super) fn shell_at(
    app: &mut App,
    buf: &mut Buffer,
    r: Rect,
    body: Rect,
    text: &str,
    theme: &Theme,
) -> (Rect, Rect) {
    shadow(buf, r, body, theme);
    Clear.render(r, buf);
    let inner = frame_with(buf, r, theme, true, theme.overlay());
    if !text.is_empty() {
        title(buf, r, text, true, theme);
    }
    app.hits.push((r, Hit::Overlay(OverlayHit::Dialog)));
    (r, inner)
}

/// A floating list (an open option list, the completion popup): a shadow, a
/// clear and a frame, with no title and no click target of its own.
pub(super) fn popup(buf: &mut Buffer, r: Rect, body: Rect, fill: Style, theme: &Theme) -> Rect {
    shadow(buf, r, body, theme);
    Clear.render(r, buf);
    frame_with(buf, r, theme, false, fill)
}

/// `r` moved in by `dx` columns on each side and `dy` rows.
pub(super) fn inset(r: Rect, dx: u16, dy: u16) -> Rect {
    Rect::new(
        r.x.saturating_add(dx),
        r.y.saturating_add(dy),
        r.width.saturating_sub(dx.saturating_mul(2)),
        r.height.saturating_sub(dy.saturating_mul(2)),
    )
}

/// The search row at the top of a dialog: an input box over `inner`, the
/// editor drawn in it, and its click target.
pub(super) fn input(
    app: &mut App,
    buf: &mut Buffer,
    inner: Rect,
    ed: &mut TextEditor,
    is_def: &dyn Fn(&str) -> bool,
    theme: &Theme,
) -> (Rect, Option<(u16, u16)>) {
    let ir = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), 1);
    let tr = input_box(buf, ir, true, theme);
    app.hits.push((tr, Hit::Overlay(OverlayHit::Input)));
    let cursor = ed.render(tr, buf, theme, true, is_def);
    (tr, cursor)
}

/// The button row on the last line of `inner`. `selected` is the focused
/// button, if the dialog has one. Returns the row used.
pub(super) fn buttons(
    app: &mut App,
    buf: &mut Buffer,
    inner: Rect,
    labels: &[&str],
    selected: Option<usize>,
    hit: &dyn Fn(usize) -> OverlayHit,
    theme: &Theme,
) -> u16 {
    let y = inner.y + inner.height.saturating_sub(1);
    let mut x = inner.x;
    for (i, label) in labels.iter().enumerate() {
        let start = x;
        x = button(buf, x, y, label, selected == Some(i), theme);
        app.hits
            .push((Rect::new(start, y, x - start, 1), Hit::Overlay(hit(i))));
        x += 2;
    }
    y
}

/// The line above the buttons: the dialog's error, or its unsaved marker.
pub(super) fn status(
    buf: &mut Buffer,
    inner: Rect,
    error: Option<&str>,
    dirty: bool,
    icons: &crate::icons::Icons,
    theme: &Theme,
) {
    let y = inner.y + inner.height.saturating_sub(2);
    if let Some(err) = error {
        put(
            buf,
            inner.x,
            y,
            &truncate(err, inner.width as usize),
            inner.width,
            theme.on_overlay(theme.err),
        );
    } else if dirty {
        put(
            buf,
            inner.x,
            y,
            &format!("{} unsaved changes", icons.dirty),
            inner.width,
            theme.on_overlay(theme.warn),
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn inset_moves_in_without_underflow() {
        let r = Rect::new(4, 5, 20, 10);
        assert_eq!(inset(r, 1, 0), Rect::new(5, 5, 18, 10));
        assert_eq!(inset(r, 2, 1), Rect::new(6, 6, 16, 8));
        assert_eq!(inset(Rect::new(0, 0, 1, 1), 2, 0).width, 0);
    }
}
