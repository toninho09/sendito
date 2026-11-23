//! Drawing primitives and the components every part of the screen is made
//! of: panes, tab strips, chips, key caps, buttons, scrollbars, rules,
//! dialogs (backdrop + shadow) and the spinner.
//!
//! Conventions: a *solid* chip (page-colored text on a color) marks where the
//! keyboard is; a *soft* chip (colored text on a wash of the color) marks a
//! current value that does not have the keyboard.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::symbols::border;
use ratatui::widgets::{Block, Borders, Widget};
use unicode_width::UnicodeWidthStr;

use crate::app::App;
use crate::icons::{Icons, SPINNER};
use crate::theme::Theme;

/// Styled text pieces drawn one after the other.
pub(super) type Spans = Vec<(String, Style)>;

pub(super) fn put(buf: &mut Buffer, x: u16, y: u16, s: &str, max: u16, st: Style) -> u16 {
    if max == 0 {
        return x;
    }
    // Content that does not fit the screen is dropped rather than written:
    // a dialog on a terminal too short for it must not panic.
    let area = *buf.area();
    let (right, bottom) = (
        area.x.saturating_add(area.width),
        area.y.saturating_add(area.height),
    );
    if y < area.y || y >= bottom || x >= right {
        return x;
    }
    let max = max.min(right - x);
    let (nx, _) = buf.set_stringn(x, y, s, max as usize, st);
    nx
}

/// Draws `spans` from `x`, never past `max_x`. Returns the next column.
pub(super) fn put_spans(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    max_x: u16,
    spans: &[(String, Style)],
) -> u16 {
    let mut x = x;
    for (s, st) in spans {
        x = put(buf, x, y, s, max_x.saturating_sub(x), *st);
    }
    x
}

pub(super) fn spans_width(spans: &[(String, Style)]) -> u16 {
    spans.iter().map(|(s, _)| s.width() as u16).sum()
}

/// Keeps the end of `s` (the part of a URL that differs between entries).
pub(super) fn truncate_left(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    let mut tail: Vec<char> = Vec::new();
    let mut cur = 1;
    for c in s.chars().rev() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if cur + cw > w {
            break;
        }
        cur += cw;
        tail.push(c);
    }
    std::iter::once('…').chain(tail.into_iter().rev()).collect()
}

pub(super) fn truncate(s: &str, w: usize) -> String {
    if s.width() <= w {
        return s.to_string();
    }
    let mut out = String::new();
    let mut cur = 0;
    for c in s.chars() {
        let cw = unicode_width::UnicodeWidthChar::width(c).unwrap_or(0);
        if cur + cw + 1 > w {
            break;
        }
        out.push(c);
        cur += cw;
    }
    out.push('…');
    out
}

pub(super) fn fill(buf: &mut Buffer, area: Rect, st: Style) {
    let area = area.intersection(*buf.area());
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(c) = buf.cell_mut((x, y)) {
                // Reset first: set_style only adds modifiers (e.g. underline).
                c.reset();
                c.set_char(' ').set_style(st);
            }
        }
    }
}

/// Sets the background of every cell in `area`, keeping text and colors.
pub(super) fn set_bg(buf: &mut Buffer, area: Rect, bg: Color) {
    let area = area.intersection(*buf.area());
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            if let Some(c) = buf.cell_mut((x, y)) {
                c.set_bg(bg);
            }
        }
    }
}

/// A pane: a rounded border (heavy when focused in themes that ask for it)
/// around the surface color. Returns the inner area.
pub(super) fn frame(buf: &mut Buffer, area: Rect, theme: &Theme, focused: bool) -> Rect {
    frame_with(buf, area, theme, focused, theme.surface())
}

pub(super) fn frame_with(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    focused: bool,
    fill_st: Style,
) -> Rect {
    let set = if focused && theme.heavy_focus {
        border::THICK
    } else {
        border::ROUNDED
    };
    let blk = Block::default()
        .borders(Borders::ALL)
        .border_set(set)
        .border_style(theme.border(focused))
        .style(fill_st);
    let inner = blk.inner(area);
    blk.render(area, buf);
    inner
}

/// An input field: the bar color, with a focus bar on its left edge.
/// Returns the area for the text.
pub(super) fn input_box(buf: &mut Buffer, r: Rect, focused: bool, theme: &Theme) -> Rect {
    fill(buf, r, theme.field());
    if r.width < 3 {
        return r;
    }
    if focused {
        let st = if theme.mono {
            Style::default().add_modifier(Modifier::BOLD)
        } else {
            theme.bar().patch(theme.accent())
        };
        put(buf, r.x, r.y, "▍", 1, st);
    }
    Rect::new(r.x + 1, r.y, r.width - 2, r.height)
}

/// A title in a pane's top border: a solid chip when focused.
pub(super) fn title(buf: &mut Buffer, area: Rect, text: &str, focused: bool, theme: &Theme) -> u16 {
    let st = if focused {
        theme.chip(theme.accent)
    } else {
        theme.title_style()
    };
    let max = (area.x + area.width).saturating_sub(2);
    put(
        buf,
        area.x + 2,
        area.y,
        &format!(" {text} "),
        max.saturating_sub(area.x + 2),
        st,
    )
}

/// How a tab strip shows its active tab.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum TabLook {
    /// The keyboard is on the strip: a solid chip.
    Strong,
    /// The pane has focus: a soft chip.
    Soft,
    /// The pane does not have focus: accent text.
    Quiet,
}

/// Draws tab labels from `x` (each padded by one space) and returns each
/// tab's rectangle. Labels that do not fit are left out.
#[allow(clippy::too_many_arguments)]
pub(super) fn tab_strip(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    max_x: u16,
    labels: &[String],
    active: usize,
    look: TabLook,
    theme: &Theme,
) -> Vec<Rect> {
    let mut out = Vec::new();
    let mut x = x;
    for (i, l) in labels.iter().enumerate() {
        let text = format!(" {l} ");
        let w = text.width() as u16;
        if x + w > max_x {
            break;
        }
        let st = if i != active {
            theme.dim()
        } else {
            match look {
                TabLook::Strong => theme.chip(theme.accent),
                TabLook::Soft => theme.badge(theme.accent),
                TabLook::Quiet => theme.bold(theme.accent()),
            }
        };
        put(buf, x, y, &text, w, st);
        out.push(Rect::new(x, y, w, 1));
        x += w;
    }
    out
}

/// The first set of labels whose padded width fits in `w`.
pub(super) fn fit_labels(candidates: Vec<Vec<String>>, w: u16) -> Vec<String> {
    let width = |ls: &Vec<String>| ls.iter().map(|l| l.width() as u16 + 2).sum::<u16>();
    let last = candidates.last().cloned().unwrap_or_default();
    candidates
        .into_iter()
        .find(|ls| width(ls) <= w)
        .unwrap_or(last)
}

pub(super) fn method_style(theme: &Theme, m: &str) -> Style {
    theme.bold(theme.fg(theme.method_color(m)))
}

/// Method label that fits the 5-column badge.
pub fn short_method(m: &str) -> &str {
    match m {
        "DELETE" => "DEL",
        "OPTIONS" => "OPT",
        "CONNECT" => "CONN",
        m if m.len() > 5 => &m[..5],
        m => m,
    }
}

/// A 5-column method badge, in the method's color over the row's own
/// background (the selection's text color when the row is selected).
pub(super) fn method_cell(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    method: &str,
    base: Style,
    selected: bool,
    theme: &Theme,
) -> u16 {
    let c = theme.method_color(method);
    let c = if selected { theme.on_selection(c) } else { c };
    let st = theme.bold(base.patch(theme.fg(c)));
    put(buf, x, y, &format!("{:<5}", short_method(method)), 5, st)
}

/// The color of a call's result, wherever it is shown: the status code when
/// there is one, otherwise what is known about the call.
pub(super) fn result_color(
    theme: &Theme,
    code: Option<u16>,
    ok: bool,
    cancelled: bool,
    running: bool,
) -> Color {
    match code {
        Some(c) => theme.status_color(c),
        // A call in progress is not an error.
        None if running => theme.accent,
        None if cancelled => theme.warn,
        None if ok => theme.ok,
        None => theme.err,
    }
}

/// A key cap padded to `w` columns, so a column of keys lines up.
pub(super) fn keycap_w(buf: &mut Buffer, x: u16, y: u16, key: &str, w: u16, theme: &Theme) -> u16 {
    let text = format!(" {key:<pad$} ", pad = w.saturating_sub(2) as usize);
    put(
        buf,
        x,
        y,
        &text,
        w.max(text.width() as u16).min(w.max(1)),
        theme.keycap(),
    )
}

/// A key name as a key cap (" F5 "). Returns the next column.
pub(super) fn keycap(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    key: &str,
    max_x: u16,
    theme: &Theme,
) -> u16 {
    put(
        buf,
        x,
        y,
        &format!(" {key} "),
        max_x.saturating_sub(x),
        theme.keycap(),
    )
}

/// A solid chip with optional rounded caps. Returns the next column.
#[allow(clippy::too_many_arguments)]
pub(super) fn chip(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    text: &str,
    st: Style,
    max_x: u16,
    icons: &Icons,
    theme: &Theme,
) -> u16 {
    let mut x = x;
    let cap = st.bg.filter(|_| !icons.cap_left.is_empty() && !theme.mono);
    if let Some(bg) = cap {
        x = put(
            buf,
            x,
            y,
            icons.cap_left,
            max_x.saturating_sub(x),
            Style::default().fg(bg),
        );
        x = put(buf, x, y, text, max_x.saturating_sub(x), st);
        put(
            buf,
            x,
            y,
            icons.cap_right,
            max_x.saturating_sub(x),
            Style::default().fg(bg),
        )
    } else {
        put(buf, x, y, &format!(" {text} "), max_x.saturating_sub(x), st)
    }
}

/// How a button reads.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(super) enum Btn {
    Primary,
    Secondary,
    Danger,
}

impl Btn {
    /// From its label: Delete/Discard are dangerous, Cancel/Close are not
    /// the main action.
    pub(super) fn of(label: &str) -> Self {
        if label.starts_with("Delete") || label.starts_with("Discard") {
            Btn::Danger
        } else if label.starts_with("Cancel") || label.starts_with("Close") {
            Btn::Secondary
        } else {
            Btn::Primary
        }
    }
}

pub(super) fn button_style(kind: Btn, selected: bool, theme: &Theme) -> Style {
    if theme.mono {
        return if selected {
            Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
        } else {
            Style::default().add_modifier(Modifier::BOLD)
        };
    }
    match (kind, selected) {
        (Btn::Primary, true) => theme.chip(theme.accent),
        (Btn::Primary, false) => theme.badge(theme.accent),
        (Btn::Danger, true) => theme.chip(theme.err),
        (Btn::Danger, false) => theme.badge(theme.err),
        (Btn::Secondary, true) => theme.chip(theme.fg),
        (Btn::Secondary, false) => theme.bold(theme.raised()),
    }
}

/// A dialog button; its look follows from the label (see [`Btn::of`]).
pub(super) fn button(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    label: &str,
    selected: bool,
    theme: &Theme,
) -> u16 {
    let text = format!(" {label} ");
    let st = button_style(Btn::of(label), selected, theme);
    put(buf, x, y, &text, text.width() as u16, st)
}

/// A scrollbar on the right border of `outer`: a heavy thumb over the line.
pub(super) fn scrollbar(
    buf: &mut Buffer,
    outer: Rect,
    pos: usize,
    len: usize,
    view: usize,
    focused: bool,
    theme: &Theme,
) {
    if outer.height < 4 || len <= view || view == 0 {
        return;
    }
    let track = (outer.height - 2) as usize;
    let size = (track * view / len).clamp(1, track);
    let max_pos = len - view;
    let off = (pos.min(max_pos) * (track - size) + max_pos / 2)
        .checked_div(max_pos)
        .unwrap_or(0);
    let x = outer.x + outer.width - 1;
    let thumb = if theme.heavy_focus && focused {
        "█"
    } else {
        "┃"
    };
    let st = if theme.mono {
        Style::default().add_modifier(Modifier::BOLD)
    } else if focused {
        Style::default().fg(theme.border_focus)
    } else {
        Style::default().fg(theme.scroll_thumb)
    };
    for i in 0..size {
        let y = outer.y + 1 + (off + i) as u16;
        put(buf, x, y, thumb, 1, st);
    }
}

/// "── Label ─────" across `w` columns.
pub(super) fn rule(buf: &mut Buffer, x: u16, y: u16, w: u16, label: &str, theme: &Theme) {
    let area = *buf.area();
    if w == 0 || y < area.y || y >= area.y.saturating_add(area.height) {
        return;
    }
    let line = theme.fg(theme.border);
    let max_x = x.saturating_add(w).min(area.x.saturating_add(area.width));
    if max_x <= x {
        return;
    }
    let mut cx = put(buf, x, y, "──", max_x - x, line);
    if !label.is_empty() {
        cx = put(
            buf,
            cx,
            y,
            &format!(" {label} "),
            max_x.saturating_sub(cx),
            theme.title_style(),
        );
    }
    while cx < max_x {
        let next = put(buf, cx, y, "─", 1, line);
        if next <= cx {
            break;
        }
        cx = next;
    }
}

pub(super) fn spinner(frame: u64) -> &'static str {
    SPINNER[(frame % SPINNER.len() as u64) as usize]
}

/// Lines centered horizontally and vertically in `area`.
pub(super) fn centered_lines(buf: &mut Buffer, area: Rect, lines: &[Spans]) {
    let h = lines.len() as u16;
    let y0 = area.y + area.height.saturating_sub(h) / 2;
    for (i, l) in lines.iter().enumerate() {
        let y = y0 + i as u16;
        if y >= area.y + area.height {
            break;
        }
        let w = spans_width(l).min(area.width);
        let x = area.x + (area.width - w) / 2;
        put_spans(buf, x, y, area.x + area.width, l);
    }
}

/// Writes `text` word-wrapped to `w` columns starting at (`x`, `y`); every
/// line starts at `x`. Stops before `bottom`. Returns the next free row.
pub(super) fn put_wrapped(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    text: &str,
    w: u16,
    bottom: u16,
    st: Style,
) -> u16 {
    let mut y = y;
    let lines = wrap_text(text, w.max(1) as usize);
    for l in lines {
        // Also bounded by the screen: `bottom` may lie past it on a short
        // terminal.
        if y >= bottom || y >= buf.area().y.saturating_add(buf.area().height) {
            break;
        }
        put(buf, x, y, &truncate(&l, w as usize), w, st);
        y += 1;
    }
    y
}

pub(super) fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// Fades everything already drawn in `area` so a dialog stands out while the
/// screen behind it stays recognizable. Monochrome: the area is cleared.
pub(super) fn backdrop(buf: &mut Buffer, area: Rect, theme: &Theme) {
    let area = area.intersection(*buf.area());
    if theme.mono {
        fill(buf, area, theme.base());
        return;
    }
    let bd = theme.backdrop;
    let known = crate::theme::to_rgb(bd).is_some();
    for y in area.y..area.y + area.height {
        for x in area.x..area.x + area.width {
            let Some(c) = buf.cell_mut((x, y)) else {
                continue;
            };
            let fg = if c.fg == Color::Reset { theme.fg } else { c.fg };
            if known {
                let faded = theme.mix(fg, bd, 0.62).unwrap_or(theme.dim);
                c.set_fg(faded).set_bg(bd);
                c.modifier
                    .remove(Modifier::BOLD | Modifier::UNDERLINED | Modifier::REVERSED);
            } else {
                c.modifier.remove(Modifier::BOLD | Modifier::REVERSED);
                c.modifier.insert(Modifier::DIM);
            }
        }
    }
}

/// A one-cell shadow to the right of and under `r`, inside `limit`.
pub(super) fn shadow(buf: &mut Buffer, r: Rect, limit: Rect, theme: &Theme) {
    if theme.mono || crate::theme::to_rgb(theme.shadow).is_none() {
        return;
    }
    let right = Rect::new(r.x + r.width, r.y + 1, 1, r.height);
    let below = Rect::new(r.x + 1, r.y + r.height, r.width, 1);
    for part in [right, below] {
        let p = part.intersection(limit);
        for y in p.y..p.y + p.height {
            for x in p.x..p.x + p.width {
                if let Some(c) = buf.cell_mut((x, y)) {
                    c.reset();
                    c.set_char(' ').set_bg(theme.shadow);
                }
            }
        }
    }
}

/// Whether the mouse is over `r`.
pub(super) fn hovered(app: &App, r: Rect) -> bool {
    app.hover
        .is_some_and(|(x, y)| r.contains(Position { x, y }))
}

/// How a list row reads.
#[derive(Clone, Copy, PartialEq, Eq)]
pub(super) enum RowState {
    Plain,
    /// Under the mouse.
    Hover,
    /// Selected, and the list has the keyboard.
    Selected,
    /// Selected, but the keyboard is elsewhere.
    SelectedQuiet,
}

/// The style of a list row; `plain` is the row's own background (a pane's
/// surface, or a dialog's).
pub(super) fn row_style(state: RowState, plain: Style, theme: &Theme) -> Style {
    match state {
        RowState::Plain => plain,
        RowState::Hover => theme.hover(),
        RowState::Selected => theme.selected(),
        RowState::SelectedQuiet => theme.bold(theme.hover()),
    }
}

/// The accent bar that marks the selected row.
pub(super) fn selection_bar(buf: &mut Buffer, r: Rect, base: Style, theme: &Theme) {
    put(
        buf,
        r.x,
        r.y,
        "▌",
        1,
        base.patch(theme.fg(theme.on_selection(theme.accent))),
    );
}

/// "↑ 3 more" / "↓ 3 more" in a dialog's border row.
#[allow(clippy::too_many_arguments)]
pub(super) fn border_count(
    buf: &mut Buffer,
    r: Rect,
    y: u16,
    count: usize,
    up: bool,
    theme: &Theme,
) {
    let text = if up {
        format!(" ↑ {count} more ")
    } else {
        format!(" ↓ {count} more ")
    };
    let w = text.width() as u16;
    let right = r.x + r.width;
    let x = right.saturating_sub(w + 2);
    let st = if up {
        theme.dialog_dim()
    } else {
        theme.dialog_accent()
    };
    put(buf, x, y, &text, w, st);
}

pub(super) fn wrap_text(s: &str, w: usize) -> Vec<String> {
    let mut out = Vec::new();
    for para in s.split('\n') {
        let mut line = String::new();
        // Plain spaces only: a no-break space (U+00A0) keeps a name whole.
        for word in para.split([' ', '\t']).filter(|w| !w.is_empty()) {
            if !line.is_empty() && line.width() + word.width() + 1 > w {
                out.push(std::mem::take(&mut line));
            }
            if !line.is_empty() {
                line.push(' ');
            }
            line.push_str(word);
        }
        out.push(line);
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn labels_fit_and_scrollbar_thumb() {
        let c = vec![
            vec!["Params (2)".to_string(), "Headers".to_string()],
            vec!["P(2)".to_string(), "H".to_string()],
        ];
        assert_eq!(fit_labels(c.clone(), 30)[0], "Params (2)");
        assert_eq!(fit_labels(c.clone(), 8)[0], "P(2)");
        assert_eq!(fit_labels(c, 2)[0], "P(2)");
        let theme = Theme::dusk();
        let mut buf = Buffer::empty(Rect::new(0, 0, 10, 12));
        scrollbar(
            &mut buf,
            Rect::new(0, 0, 10, 12),
            90,
            100,
            10,
            false,
            &theme,
        );
        // At the end of the list the thumb sits at the bottom of the track.
        assert_eq!(buf[(9, 10)].symbol(), "┃");
        assert_ne!(buf[(9, 1)].symbol(), "┃");
        assert_eq!(Btn::of("Delete"), Btn::Danger);
        assert_eq!(Btn::of("Cancel (Esc)"), Btn::Secondary);
        assert_eq!(Btn::of("Save (Enter)"), Btn::Primary);
    }

    #[test]
    fn backdrop_fades_and_mono_clears() {
        let theme = Theme::dusk();
        let mut buf = Buffer::empty(Rect::new(0, 0, 4, 1));
        put(
            &mut buf,
            0,
            0,
            "ab",
            2,
            theme.fg(theme.ok).add_modifier(Modifier::BOLD),
        );
        backdrop(&mut buf, Rect::new(0, 0, 4, 1), &theme);
        assert_eq!(buf[(0, 0)].symbol(), "a");
        assert_eq!(buf[(0, 0)].bg, theme.backdrop);
        assert_ne!(buf[(0, 0)].fg, theme.ok);
        assert!(!buf[(0, 0)].modifier.contains(Modifier::BOLD));
        let mono = Theme::mono();
        backdrop(&mut buf, Rect::new(0, 0, 4, 1), &mono);
        assert_eq!(buf[(0, 0)].symbol(), " ");
    }

    #[test]
    fn row_states_read_in_order() {
        let theme = Theme::dusk();
        let plain = theme.overlay();
        assert_eq!(row_style(RowState::Plain, plain, &theme), plain);
        assert_eq!(row_style(RowState::Hover, plain, &theme), theme.hover());
        assert_eq!(
            row_style(RowState::Selected, plain, &theme),
            theme.selected()
        );
        assert_eq!(
            row_style(RowState::SelectedQuiet, plain, &theme),
            theme.bold(theme.hover())
        );
        // Over a dialog, the plain row keeps the dialog's background.
        assert_eq!(
            row_style(RowState::Plain, plain, &theme).bg,
            Some(theme.overlay_bg)
        );
    }

    #[test]
    fn result_color_prefers_the_code() {
        let theme = Theme::dusk();
        assert_eq!(
            result_color(&theme, Some(204), true, false, false),
            theme.ok
        );
        assert_eq!(
            result_color(&theme, Some(404), false, false, false),
            theme.warn
        );
        assert_eq!(
            result_color(&theme, Some(500), false, false, false),
            theme.err
        );
        // A call in progress outranks the rest.
        assert_eq!(result_color(&theme, None, false, false, true), theme.accent);
        assert_eq!(result_color(&theme, None, false, true, false), theme.warn);
        assert_eq!(result_color(&theme, None, true, false, false), theme.ok);
        assert_eq!(result_color(&theme, None, false, false, false), theme.err);
    }

    #[test]
    fn method_cell_pads_and_follows_the_selection() {
        let theme = Theme::dusk();
        let mut buf = Buffer::empty(Rect::new(0, 0, 6, 1));
        assert_eq!(
            method_cell(&mut buf, 0, 0, "DELETE", theme.bar(), false, &theme),
            5
        );
        assert_eq!(buf[(0, 0)].symbol(), "D");
        assert_eq!(buf[(1, 0)].symbol(), "E");
        assert_eq!(buf[(3, 0)].symbol(), " ");
        method_cell(&mut buf, 0, 0, "GET", theme.selected(), true, &theme);
        assert_eq!(
            buf[(0, 0)].fg,
            theme.on_selection(theme.method_color("GET"))
        );
        assert_eq!(buf[(0, 0)].bg, theme.selected().bg.unwrap_or(theme.bg));
        // The pad keeps every badge five columns wide.
        let mut b = Buffer::empty(Rect::new(0, 0, 6, 1));
        assert_eq!(
            method_cell(&mut b, 0, 0, "GET", theme.bar(), false, &theme),
            5
        );
        assert_eq!(b[(3, 0)].symbol(), " ");
    }
}
