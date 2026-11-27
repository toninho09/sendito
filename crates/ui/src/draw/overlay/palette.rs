//! The command palette and the help overlay.

use super::*;

/// Character positions of `q` in `label` (case-insensitive): a substring
/// when there is one, else the letters of a fuzzy match.
/// Marks the letters of `query` in `text`, drawn at (`x`, `y`): accent,
/// bold and underlined.
pub(super) fn mark_matches(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    text: &str,
    query: &str,
    selected: bool,
    theme: &Theme,
) {
    for ci in match_positions(text, query) {
        let cx = x + text.chars().take(ci).collect::<String>().width() as u16;
        if let Some(c) = buf.cell_mut((cx, y)) {
            let fg = if selected {
                theme.on_selection(theme.accent)
            } else {
                theme.accent
            };
            c.set_fg(fg)
                .modifier
                .insert(Modifier::BOLD | Modifier::UNDERLINED);
        }
    }
}

pub(super) fn match_positions(label: &str, q: &str) -> Vec<usize> {
    let q: Vec<char> = q
        .to_lowercase()
        .chars()
        .filter(|c| !c.is_whitespace())
        .collect();
    if q.is_empty() {
        return Vec::new();
    }
    let hay: Vec<char> = label.to_lowercase().chars().collect();
    if hay.len() != label.chars().count() {
        return Vec::new();
    }
    if let Some(start) =
        (0..hay.len().saturating_sub(q.len() - 1)).find(|&i| hay[i..i + q.len()] == q[..])
    {
        return (start..start + q.len()).collect();
    }
    let mut out = Vec::new();
    let mut qi = 0;
    for (i, c) in hay.iter().enumerate() {
        if qi < q.len() && *c == q[qi] {
            out.push(i);
            qi += 1;
        }
    }
    if qi == q.len() { out } else { Vec::new() }
}

pub(super) fn draw_palette(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    p: &mut Palette,
    cursor: &mut Option<(u16, u16)>,
) {
    let h = (frame.area.height * 2 / 3)
        .max(10)
        .min(frame.body.height.saturating_sub(2));
    let w = 100.min(frame.body.width.saturating_sub(4));
    // Near the top, where the eye already is.
    let top = frame.body.y + (frame.body.height.saturating_sub(h) / 3).max(1);
    let r = Rect::new(frame.body.x + (frame.body.width - w) / 2, top, w, h);
    let (r, inner) = shell_at(app, buf, r, frame.body, "Command palette", frame.theme);
    let (tr, c) = dialog::input(app, buf, inner, &mut p.input, is_def, frame.theme);
    *cursor = c;
    let vis = p.visible();
    p.sel = p.sel.min(vis.len().saturating_sub(1));
    let lh = inner.height.saturating_sub(2) as usize;
    p.scroll = crate::list::keep_visible(p.sel, p.scroll, lh);
    if !vis.is_empty() {
        // Position in the list, so it is clear that it scrolls.
        let count = if p.armed {
            format!(" {}/{} ", p.sel + 1, vis.len())
        } else {
            format!(" {} ", vis.len())
        };
        let cw = count.width() as u16;
        if tr.width > cw + 20 {
            put(
                buf,
                tr.x + tr.width - cw,
                tr.y,
                &count,
                cw,
                frame.theme.bar().patch(frame.theme.dim()),
            );
        }
    }
    let query = p.input.text();
    let query = if query.starts_with(':') {
        ""
    } else {
        query.trim()
    };
    for (row, idx) in vis.iter().skip(p.scroll).take(lh).enumerate() {
        let y = inner.y + 2 + row as u16;
        let (item, label, detail) = &p.items[*idx];
        let sel = p.armed && row + p.scroll == p.sel;
        let rr = Rect::new(inner.x, y, inner.width, 1);
        let state = if sel {
            RowState::Selected
        } else if hovered(app, rr) {
            RowState::Hover
        } else {
            RowState::Plain
        };
        let base = fill_row(buf, rr, state, frame.text, frame.theme);
        let mut x = inner.x + 2;
        let dw = detail.width() as u16 + 1;
        // The label gives way to the key: it is cut with an ellipsis.
        let room = inner.width.saturating_sub(dw + 5).max(8);
        let shown = truncate(label, room as usize);
        let lx = x;
        // A request shows its method in color.
        let method = match item {
            PaletteItem::Open(_) => label
                .strip_prefix("Open ")
                .and_then(|s| s.split_whitespace().next())
                .map(str::to_string),
            _ => None,
        };
        x = put(buf, x, y, &shown, room, base);
        if let Some(m) = &method {
            let mx = lx + "Open ".width() as u16;
            let c = frame.theme.method_color(m);
            put(
                buf,
                mx,
                y,
                m,
                m.width() as u16,
                frame.theme.bold(base.patch(frame.theme.fg(if sel {
                    frame.theme.on_selection(c)
                } else {
                    c
                }))),
            );
        }
        // The typed letters, marked in the label.
        mark_matches(buf, lx, y, &shown, query, sel, frame.theme);
        let _ = x;
        let dx = (inner.x + inner.width).saturating_sub(dw + 1);
        if dx > lx + shown.width() as u16 {
            let st = match (sel, item) {
                (true, _) => base,
                (false, PaletteItem::Action(_)) => base.patch(frame.theme.accent()),
                (false, _) => base.patch(frame.theme.dim()),
            };
            put(buf, dx, y, detail, dw, st);
        }
        app.hits
            .push((rr, Hit::Overlay(OverlayHit::Item(row + p.scroll))));
    }
    // Rows above and below the visible window, said in the frame.
    let below = vis.len().saturating_sub(p.scroll + lh);
    if below > 0 {
        border_count(buf, r, r.y + r.height - 1, below, false, frame.theme);
    }
    if p.scroll > 0 {
        border_count(buf, r, inner.y + 1, p.scroll, true, frame.theme);
    }
    scrollbar(buf, r, p.scroll, vis.len(), lh, true, frame.theme);
    if vis.is_empty() {
        put(
            buf,
            inner.x + 2,
            inner.y + 2,
            "No match. Commands start with ':' (e.g. :env dev).",
            inner.width.saturating_sub(3),
            frame.theme.dialog_dim(),
        );
    }
}

pub(super) fn draw_help(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    h: &mut Help,
    cursor: &mut Option<(u16, u16)>,
) {
    let theme = frame.theme;
    // The sidebar fits the longest name; the text fits the longest function
    // row. One width for every section, so Tab does not resize the dialog.
    let side_w = h
        .sections
        .iter()
        .map(|s| (s.name.width() + 4).max(s.group.width() + 2))
        .max()
        .unwrap_or(0) as u16;
    let mw = frame.area.width.saturating_sub(8).min(side_w + 1 + 132);
    let (r, inner) = shell(
        app,
        buf,
        frame.body,
        mw,
        frame.area.height.saturating_sub(2),
        "Help (Tab: next section, Esc closes)",
        theme,
    );
    // The sidebar: a heading row where the group changes, then one row per
    // section. The open row stays in view.
    let mut side: Vec<(Option<usize>, &str)> = Vec::new();
    for (i, s) in h.sections.iter().enumerate() {
        if i == 0 || h.sections[i - 1].group != s.group {
            side.push((None, s.group));
        }
        side.push((Some(i), s.group));
    }
    let rows = inner.height as usize;
    let open = side.iter().position(|r| r.0 == Some(h.sel)).unwrap_or(0);
    let first = (open + 1).saturating_sub(rows);
    for (n, &(row, group)) in side.iter().enumerate().skip(first).take(rows) {
        let rr = Rect::new(inner.x, inner.y + (n - first) as u16, side_w, 1);
        let Some(i) = row else {
            // A heading names the group of the rows under it.
            let style = theme.dialog_dim().add_modifier(Modifier::BOLD);
            put(buf, rr.x + 1, rr.y, group, side_w.saturating_sub(2), style);
            continue;
        };
        let state = if i == h.sel {
            RowState::Selected
        } else if hovered(app, rr) {
            RowState::Hover
        } else {
            RowState::Plain
        };
        let base = fill_row(buf, rr, state, frame.text, theme);
        let name = h.sections[i].name;
        put(buf, rr.x + 3, rr.y, name, side_w.saturating_sub(4), base);
        app.hits.push((rr, Hit::Overlay(OverlayHit::Choice(i))));
    }
    let rx = inner.x + side_w;
    for y in inner.y..inner.y + inner.height {
        put(buf, rx, y, "│", 1, theme.on_overlay(theme.border));
    }
    // The text: the search row, then the open section.
    let tx = rx + 2;
    let tw = (inner.x + inner.width).saturating_sub(tx + 1);
    put(buf, tx, inner.y, "Search ", 7, theme.dialog_dim());
    let er = input_box(
        buf,
        Rect::new(tx + 7, inner.y, tw.saturating_sub(7), 1),
        true,
        theme,
    );
    app.hits.push((er, Hit::Overlay(OverlayHit::Input)));
    *cursor = h.filter.render(er, buf, theme, true, is_def);
    // Scroll position, so it is clear that more lines follow.
    let pos = format!(" {} · ↑↓ PgUp PgDn ", h.viewer.status());
    let pw = pos.width() as u16;
    if er.width > pw + 20 {
        put(
            buf,
            er.x + er.width - pw,
            er.y,
            &pos,
            pw,
            theme.bar().patch(theme.dim()),
        );
    }
    let vr = Rect::new(tx, inner.y + 2, tw, inner.height.saturating_sub(2));
    h.viewer.syntax = crate::viewer::Syntax::Help;
    h.viewer.render(vr, buf, theme, true);
    app.hits.push((vr, Hit::Overlay(OverlayHit::Item(0))));
    set_bg_unset(buf, vr, theme);
    scrollbar(
        buf,
        r,
        h.viewer.scroll,
        h.viewer.row_count(),
        vr.height as usize,
        true,
        theme,
    );
}
