//! The key bindings editor: a filter and the mode tabs, the actions grouped
//! with their keys, the line that explains the selection (or the keys being
//! recorded), the status line and the buttons.

use super::*;
use crate::keymap_editor::{BUTTONS, KEYMAP_MODES, KeyRow, KeymapEditor};

pub(super) fn draw_keymap(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    e: &mut KeymapEditor,
    cursor: &mut Option<(u16, u16)>,
) {
    let theme = frame.theme;
    let scope = if e.path == app.project_config_path() {
        "project config"
    } else {
        "global config"
    };
    let title = if e.dirty {
        format!("Key bindings · {scope} {} unsaved", app.icons.dirty)
    } else {
        format!("Key bindings · {scope}")
    };
    let w = frame.area.width.saturating_sub(4).min(110);
    let h = frame.area.height.saturating_sub(4).max(14);
    let (r, inner) = shell(app, buf, frame.body, w, h, &title, theme);
    let inner = inset(inner, 1, 0);
    if inner.height < 8 || inner.width < 40 {
        return;
    }
    // The filter on the left of the first row, the mode tabs on its right.
    let tabs: Vec<String> = KEYMAP_MODES.iter().map(|m| m.to_string()).collect();
    let tabs_w: u16 = tabs.iter().map(|t| t.width() as u16 + 3).sum::<u16>() + 6;
    let fr = Rect::new(inner.x, inner.y, inner.width.saturating_sub(tabs_w + 1), 1);
    let tr = input_box(buf, fr, e.capture.is_none(), theme);
    let c = e.filter.render(tr, buf, theme, e.capture.is_none(), is_def);
    if e.capture.is_none() {
        *cursor = c;
    }
    let tx = inner.x + inner.width.saturating_sub(tabs_w);
    let x = put(buf, tx, inner.y, "Mode ", 5, theme.dialog_dim());
    let rects = tab_strip(
        buf,
        x,
        inner.y,
        inner.x + inner.width,
        &tabs,
        e.mode,
        TabLook::Strong,
        theme,
    );
    for (i, rr) in rects.into_iter().enumerate() {
        app.hits.push((rr, Hit::Overlay(OverlayHit::KeyTab(i))));
    }
    // The list: group titles and actions.
    let top = inner.y + 2;
    let lh = inner.height.saturating_sub(6) as usize;
    let rows = e.rows();
    let sel = e.selected();
    // The row of the selected action, to keep it in view.
    let sel_row = rows
        .iter()
        .position(|r| Some(*r) == sel.map(KeyRow::Action))
        .unwrap_or(0);
    e.scroll = crate::list::keep_visible(sel_row, e.scroll, lh);
    let mut action_index = rows[..e.scroll.min(rows.len())]
        .iter()
        .filter(|r| matches!(r, KeyRow::Action(_)))
        .count();
    let key_x = inner.x + (inner.width * 3 / 5);
    for (n, row) in rows.iter().skip(e.scroll).take(lh).enumerate() {
        let y = top + n as u16;
        let rr = Rect::new(inner.x, y, inner.width, 1);
        match row {
            KeyRow::Group(ctx) => {
                put(
                    buf,
                    inner.x,
                    y,
                    ctx.title(),
                    inner.width,
                    theme.bold(theme.on_overlay(theme.accent)),
                );
            }
            KeyRow::Action(a) => {
                let selected = Some(*a) == sel;
                let state = if selected {
                    RowState::Selected
                } else if hovered(app, rr) {
                    RowState::Hover
                } else {
                    RowState::Plain
                };
                let base = fill_row(buf, rr, state, frame.text, theme);
                put(
                    buf,
                    inner.x + 2,
                    y,
                    &truncate(a.description(), (key_x - inner.x - 4) as usize),
                    key_x - inner.x - 3,
                    base,
                );
                let recording = e.capture.as_ref().filter(|c| c.action == *a);
                let (text, st) = match recording {
                    Some(c) if c.keys.is_empty() => (
                        "press the keys…".to_string(),
                        base.patch(theme.fg(theme.warn)),
                    ),
                    Some(c) => (
                        crate::keymap::KeySeq(c.keys.clone()).to_string(),
                        theme.bold(base.patch(theme.fg(theme.warn))),
                    ),
                    None => {
                        let (keys, set) = e.keys(*a);
                        let text = if keys.is_empty() {
                            "no key".to_string()
                        } else {
                            keys.join(", ")
                        };
                        let st = if set {
                            theme.bold(base.patch(theme.fg(theme.accent)))
                        } else {
                            base.patch(theme.dim())
                        };
                        (
                            if set {
                                format!("{text}  (changed)")
                            } else {
                                text
                            },
                            st,
                        )
                    }
                };
                let kw = (inner.x + inner.width).saturating_sub(key_x);
                put(buf, key_x, y, &truncate(&text, kw as usize), kw, st);
                app.hits
                    .push((rr, Hit::Overlay(OverlayHit::KeyRow(action_index))));
                action_index += 1;
            }
        }
    }
    let total = rows.len();
    scrollbar(
        buf,
        Rect::new(r.x, top.saturating_sub(1), r.width, lh as u16 + 2),
        e.scroll,
        total,
        lh,
        true,
        theme,
    );
    if rows.is_empty() {
        put(
            buf,
            inner.x + 2,
            top,
            "No action matches the filter.",
            inner.width,
            theme.dialog_dim(),
        );
    }
    // The line under the list.
    let (line, st) = if let Some(c) = &e.capture {
        (
            format!(
                "Recording the keys of \"{}\": press them; a pause of 1 s keeps them, Esc cancels.",
                c.action.description()
            ),
            theme.on_overlay(theme.warn),
        )
    } else if let Some(n) = &e.note {
        let warn = n.starts_with('⚠');
        (
            n.clone(),
            theme.on_overlay(if warn { theme.warn } else { theme.ok }),
        )
    } else {
        let mode = match e.mode {
            0 => "all: both editing modes",
            1 => "conventional: this mode only; wins over all",
            _ => "vim: this mode only; wins over all",
        };
        (
            format!(
                "Enter records new keys, Ctrl+A adds one, Del restores the default. Tab: {mode}."
            ),
            theme.on_overlay(theme.info),
        )
    };
    put(
        buf,
        inner.x,
        inner.y + inner.height.saturating_sub(3),
        &truncate(&line, inner.width as usize),
        inner.width,
        st,
    );
    dialog::status(buf, inner, e.error.as_deref(), e.dirty, &app.icons, theme);
    buttons(
        app,
        buf,
        inner,
        BUTTONS,
        None,
        &|i| OverlayHit::KeyButton(i),
        theme,
    );
}
