//! The command palette and the help overlay.

use super::*;

/// Character positions of `q` in `label` (case-insensitive): a substring
/// when there is one, else the letters of a fuzzy match.
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
        let plain = frame.text;
        let base = row_style(
            if sel {
                RowState::Selected
            } else if hovered(app, rr) {
                RowState::Hover
            } else {
                RowState::Plain
            },
            plain,
            frame.theme,
        );
        fill(buf, rr, base);
        if sel {
            selection_bar(buf, rr, base, frame.theme);
        }
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
        for ci in match_positions(&shown, query) {
            let cx = lx + shown.chars().take(ci).collect::<String>().width() as u16;
            if let Some(c) = buf.cell_mut((cx, y)) {
                let fg = if sel {
                    frame.theme.on_selection(frame.theme.accent)
                } else {
                    frame.theme.accent
                };
                c.set_fg(fg)
                    .modifier
                    .insert(Modifier::BOLD | Modifier::UNDERLINED);
            }
        }
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
    viewer: &mut Viewer,
    filter: &mut TextEditor,
    cursor: &mut Option<(u16, u16)>,
) {
    // Wide screens read the keys in two columns, like a newspaper.
    let two_cols = frame.area.width >= 180;
    let mw = if two_cols {
        frame.area.width.saturating_sub(8).min(200)
    } else {
        110
    };
    let (r, inner) = shell(
        app,
        buf,
        frame.body,
        mw,
        frame.area.height.saturating_sub(2),
        "Help — keys and actions (Esc closes)",
        frame.theme,
    );
    let ir = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), 1);
    put(buf, ir.x, ir.y, "Search ", 7, frame.theme.dialog_dim());
    let er = input_box(
        buf,
        Rect::new(ir.x + 7, ir.y, ir.width.saturating_sub(7), 1),
        true,
        frame.theme,
    );
    app.hits.push((er, Hit::Overlay(OverlayHit::Input)));
    *cursor = filter.render(er, buf, frame.theme, true, is_def);
    // Scroll position, so it is clear that more keys follow.
    let pos = format!(" {} · ↑↓ PgUp PgDn ", viewer.status());
    let pw = pos.width() as u16;
    if er.width > pw + 20 {
        put(
            buf,
            er.x + er.width - pw,
            er.y,
            &pos,
            pw,
            frame.theme.bar().patch(frame.theme.dim()),
        );
    }
    let vr = Rect::new(
        inner.x + 1,
        inner.y + 2,
        inner.width.saturating_sub(2),
        inner.height.saturating_sub(2),
    );
    viewer.syntax = crate::viewer::Syntax::Help;
    let shown = if two_cols {
        let (left, right) = help_columns(buf, vr, viewer, frame.theme);
        app.hits.push((left, Hit::Overlay(OverlayHit::Item(0))));
        app.hits
            .push((right, Hit::Overlay(OverlayHit::Item(left.height as usize))));
        left.height as usize * 2
    } else {
        viewer.render(vr, buf, frame.theme, true);
        app.hits.push((vr, Hit::Overlay(OverlayHit::Item(0))));
        vr.height as usize
    };
    set_bg_unset(buf, vr, frame.theme);
    scrollbar(
        buf,
        r,
        viewer.scroll,
        viewer.row_count(),
        shown,
        true,
        frame.theme,
    );
}

/// Renders `viewer` as one column twice the height of `area` and lays its
/// halves side by side, so scrolling, paging and search work unchanged.
/// Returns the left and right column.
pub(super) fn help_columns(
    buf: &mut Buffer,
    area: Rect,
    viewer: &mut crate::viewer::Viewer,
    theme: &Theme,
) -> (Rect, Rect) {
    let gap: u16 = 3;
    let col_w = area.width.saturating_sub(gap) / 2;
    let left = Rect::new(area.x, area.y, col_w, area.height);
    let right = Rect::new(area.x + col_w + gap, area.y, col_w, area.height);
    let mut tall = Buffer::empty(Rect::new(0, 0, col_w, area.height.saturating_mul(2)));
    viewer.render(*tall.area(), &mut tall, theme, true);
    for y in 0..area.height {
        for x in 0..col_w {
            for (dst, sy) in [(left, y), (right, y + area.height)] {
                if let (Some(src), Some(cell)) =
                    (tall.cell((x, sy)), buf.cell_mut((dst.x + x, dst.y + y)))
                {
                    *cell = src.clone();
                }
            }
        }
    }
    // The gap between the columns: a faint rule.
    let rx = area.x + col_w + gap / 2;
    for y in area.y..area.y + area.height {
        put(buf, rx, y, "│", 1, theme.on_overlay(theme.border));
    }
    (left, right)
}
