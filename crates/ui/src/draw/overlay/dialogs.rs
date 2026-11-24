//! The confirm, prompt and picker dialogs.

use super::*;

pub(super) fn draw_confirm(app: &mut App, buf: &mut Buffer, frame: Frame<'_>, c: &mut Confirm) {
    let lines = wrap_text(&c.message, 56);
    let (_, inner) = shell(
        app,
        buf,
        frame.body,
        64,
        lines.len() as u16 + 6,
        &c.title,
        frame.theme,
    );
    // Leave the button row for the buttons on a short terminal.
    let room = inner.height.saturating_sub(2) as usize;
    for (i, l) in lines.iter().take(room).enumerate() {
        put(
            buf,
            inner.x + 2,
            inner.y + 1 + i as u16,
            l,
            inner.width.saturating_sub(3),
            frame.text,
        );
    }
    // Every button names its key, as in the other dialogs.
    let keys: &[&str] = if c.buttons.len() == 3 {
        &["s", "d", "Esc"]
    } else {
        &["y", "Esc"]
    };
    let labels: Vec<String> = c
        .buttons
        .iter()
        .enumerate()
        .map(|(i, b)| match keys.get(i) {
            Some(k) => format!("{b} ({k})"),
            None => b.clone(),
        })
        .collect();
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    buttons(
        app,
        buf,
        inset(inner, 2, 0),
        &labels,
        Some(c.sel),
        &|i| OverlayHit::Button(i),
        frame.theme,
    );
}

pub(super) fn draw_prompt(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    p: &mut Prompt,
    cursor: &mut Option<(u16, u16)>,
) {
    let has_msg = p.error.is_some() || p.note.is_some();
    let h = 7 + 2 * p.choices.len() as u16 + if has_msg { 2 } else { 0 };
    let (_, inner) = shell(app, buf, frame.body, 66, h, &p.title, frame.theme);
    let lw = 10;
    let by = inner.y + inner.height.saturating_sub(1);
    // The choice rows keep their spacing when there is room; on a
    // short terminal they are packed so the buttons stay reachable.
    let step = if inner.height >= 2 * p.choices.len() as u16 + 5 {
        2
    } else {
        1
    };
    let first_choice = if step == 2 { inner.y + 3 } else { inner.y + 2 };
    if inner.height >= 3 {
        put(
            buf,
            inner.x + 2,
            inner.y + 1,
            &p.label,
            lw,
            frame.theme.dialog_dim(),
        );
        let er = input_box(
            buf,
            Rect::new(
                inner.x + 2 + lw,
                inner.y + 1,
                inner.width.saturating_sub(lw + 4),
                1,
            ),
            p.focus == 0,
            frame.theme,
        );
        let c = p.input.render(er, buf, frame.theme, p.focus == 0, is_def);
        if p.focus == 0 {
            *cursor = c;
        }
        app.hits.push((er, Hit::Overlay(OverlayHit::Input)));
    }
    let mut choices_end = inner.y + 1;
    for (k, (label, opts, i)) in p.choices.iter().enumerate() {
        let y = first_choice + step * k as u16;
        if y >= by {
            break;
        }
        put(buf, inner.x + 2, y, label, lw, frame.theme.dialog_dim());
        let v = format!(" ‹ {} › ", opts.get(*i).cloned().unwrap_or_default());
        put(
            buf,
            inner.x + 2 + lw,
            y,
            &v,
            inner.width.saturating_sub(lw + 4),
            if p.focus == k + 1 {
                frame.theme.chip(frame.theme.accent)
            } else {
                frame.theme.bar()
            },
        );
        app.hits.push((
            Rect::new(inner.x + 2 + lw, y, v.width() as u16, 1),
            Hit::Overlay(OverlayHit::Choice(k)),
        ));
        let hx = inner.x + 2 + lw + v.width() as u16 + 1;
        put(
            buf,
            hx,
            y,
            "← → change · Space list",
            (inner.x + inner.width).saturating_sub(hx),
            frame.theme.dialog_dim(),
        );
        choices_end = y;
    }
    let msg = p
        .error
        .as_ref()
        .map(|e| (e, frame.theme.fg(frame.theme.err)))
        .or_else(|| {
            p.note
                .as_ref()
                .map(|n| (n, frame.theme.fg(frame.theme.warn)))
        });
    if let Some((m, st)) = msg {
        let y0 = inner.y + inner.height.saturating_sub(4);
        if y0 > choices_end {
            put_wrapped(
                buf,
                inner.x + 2,
                y0,
                m,
                inner.width.saturating_sub(4),
                by,
                st.bg(frame.theme.overlay_bg),
            );
        }
    }
    // The confirm button says what it does.
    let verb = crate::app::prompt_verb(&p.action);
    let ok = format!("{}{} (Enter)", verb[..1].to_uppercase(), &verb[1..]);
    buttons(
        app,
        buf,
        inset(inner, 2, 0),
        &[&ok, "Cancel (Esc)"],
        None,
        &|i| OverlayHit::Button(i),
        frame.theme,
    );
    // The open option list drops down under its choice row.
    if let (Some(hl), Some((_, opts, cur))) = (
        p.open,
        p.focus.checked_sub(1).and_then(|k| p.choices.get(k)),
    ) {
        let k = (p.focus - 1) as u16;
        let row_y = first_choice + step * k;
        let y0 = row_y + 1;
        let w = (opts.iter().map(|o| o.width()).max().unwrap_or(10) as u16 + 6)
            .min(frame.area.width.saturating_sub(inner.x + 2 + lw));
        let room = (frame.area.y + frame.area.height).saturating_sub(y0 + 2);
        let shown = (opts.len() as u16).min(room);
        if shown > 0 && w >= 4 && y0 < frame.area.y + frame.area.height {
            let lr = Rect::new(inner.x + 2 + lw, y0, w, shown + 2);
            let li = popup(buf, lr, frame.body, frame.text, frame.theme);
            let first = (hl as u16 + 1).saturating_sub(shown) as usize;
            for (row, (j, o)) in opts
                .iter()
                .enumerate()
                .skip(first)
                .take(shown as usize)
                .enumerate()
            {
                let y = li.y + row as u16;
                let st = if j == hl {
                    frame.theme.selected()
                } else {
                    frame.text
                };
                fill(buf, Rect::new(li.x, y, li.width, 1), st);
                let mark = if j == *cur { "✓ " } else { "  " };
                put(buf, li.x, y, &format!("{mark}{o}"), li.width, st);
                app.hits.push((
                    Rect::new(li.x, y, li.width, 1),
                    Hit::Overlay(OverlayHit::PromptOption(j)),
                ));
            }
        }
    }
}

pub(super) fn draw_picker(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    p: &mut Picker,
    cursor: &mut Option<(u16, u16)>,
) {
    let vis = p.visible();
    let has_details =
        p.details.iter().any(|d| !d.is_empty()) && !matches!(p.action, PickAction::MoveTo(_));
    let h = (vis.len() as u16 + 6 + u16::from(has_details))
        .min(frame.area.height.saturating_sub(4))
        .max(4);
    let w = p
        .items
        .iter()
        .zip(p.details.iter().chain(std::iter::repeat(&String::new())))
        .map(|(a, b)| a.width() + b.width() + 8)
        .max()
        .unwrap_or(30)
        .clamp(38, 130) as u16;
    let (r, inner) = shell(app, buf, frame.body, w, h, &p.title, frame.theme);
    let (_tr, c) = dialog::input(app, buf, inner, &mut p.filter, is_def, frame.theme);
    *cursor = c;
    p.sel = p.sel.min(vis.len().saturating_sub(1));
    let lh = inner.height.saturating_sub(2 + u16::from(has_details)) as usize;
    // The selected item's full detail (rows may cut it short).
    if has_details && let Some(d) = vis.get(p.sel).and_then(|i| p.details.get(*i)) {
        let dy = inner.y + inner.height.saturating_sub(1);
        put(
            buf,
            inner.x + 2,
            dy,
            &truncate(
                &format!("Selected: {d}"),
                inner.width.saturating_sub(3) as usize,
            ),
            inner.width.saturating_sub(3),
            frame.theme.dialog_accent(),
        );
    }
    p.scroll = crate::list::keep_visible(p.sel, p.scroll, lh);
    let methods = matches!(p.action, PickAction::Method);
    let envs = matches!(p.action, PickAction::Env);
    for (row, idx) in vis.iter().skip(p.scroll).take(lh).enumerate() {
        let y = inner.y + 2 + row as u16;
        let s = row + p.scroll == p.sel;
        let rr = Rect::new(inner.x, y, inner.width, 1);
        let base = row_style(
            if s {
                RowState::Selected
            } else if hovered(app, rr) {
                RowState::Hover
            } else {
                RowState::Plain
            },
            frame.text,
            frame.theme,
        );
        fill(buf, rr, base);
        if s {
            selection_bar(buf, rr, base, frame.theme);
        }
        let item = &p.items[*idx];
        let mut x = inner.x + 2;
        // Methods in their colors; environments with their marker.
        let item_st = if methods {
            let c = frame.theme.method_color(item);
            frame.theme.bold(base.patch(frame.theme.fg(if s {
                frame.theme.on_selection(c)
            } else {
                c
            })))
        } else {
            base
        };
        if envs {
            let real = !item.starts_with('(') && !item.is_empty();
            let c = if real {
                frame.theme.env_color(item)
            } else {
                frame.theme.dim
            };
            x = put(
                buf,
                x,
                y,
                &format!("{} ", app.icons.env),
                2,
                base.patch(
                    frame
                        .theme
                        .fg(if s { frame.theme.on_selection(c) } else { c }),
                ),
            );
        }
        x = put(
            buf,
            x,
            y,
            item,
            (inner.x + inner.width).saturating_sub(x + 1),
            item_st,
        );
        if let Some(d) = p.details.get(*idx)
            && !matches!(p.action, PickAction::MoveTo(_))
        {
            put(
                buf,
                x + 2,
                y,
                &truncate(d, (inner.x + inner.width).saturating_sub(x + 3) as usize),
                (inner.x + inner.width).saturating_sub(x + 2),
                if s {
                    base
                } else {
                    base.patch(frame.theme.dim())
                },
            );
        }
        app.hits
            .push((rr, Hit::Overlay(OverlayHit::Item(row + p.scroll))));
    }
    scrollbar(buf, r, p.scroll, vis.len(), lh, true, frame.theme);
    if vis.is_empty() {
        put(
            buf,
            inner.x + 2,
            inner.y + 2,
            "No match",
            inner.width.saturating_sub(3),
            frame.theme.dialog_dim(),
        );
    }
}
