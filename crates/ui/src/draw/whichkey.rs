//! The Ctrl+W menu: while a global key sequence waits for its next key, a
//! box at the bottom right lists the keys that complete it, by group. It is
//! built from the keymap, so user overrides show. A click on a key runs it.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Hit};
use crate::keymap::{Action, Ctx, MenuGroup, menu_entry, short_keys};
use crate::theme::Theme;

use super::dialog::popup;
use super::kit::{put, truncate};

/// One row: the keys (with the action each runs) and the label.
struct Row {
    keys: Vec<(String, Action)>,
    label: &'static str,
}

impl Row {
    fn width(&self) -> u16 {
        let keys: usize = self.keys.iter().map(|(k, _)| k.width()).sum::<usize>()
            + self.keys.len().saturating_sub(1);
        (keys + 2 + self.label.width()) as u16
    }
}

/// A group of rows and its column width.
struct Group {
    title: &'static str,
    rows: Vec<Row>,
    width: u16,
}

/// The rows of each group that has some, in menu order.
fn groups(app: &App) -> Vec<Group> {
    let entries = app
        .keymap
        .continuations(&[Ctx::Global], app.vim, &app.pending);
    let mut out = Vec::new();
    for g in MenuGroup::ALL {
        let mut rows: Vec<Row> = Vec::new();
        for (chord, action) in &entries {
            let (group, label) = menu_entry(*action);
            if group != g {
                continue;
            }
            let key = (short_keys(&chord.to_string()), *action);
            // Arrow aliases (Ctrl+W Left) repeat a letter key: keep the letter.
            if rows.iter().any(|r| r.keys.iter().any(|(_, a)| a == action)) {
                continue;
            }
            match rows.iter_mut().find(|r| r.label == label) {
                Some(r) => r.keys.push(key),
                None => rows.push(Row {
                    keys: vec![key],
                    label,
                }),
            }
        }
        if !rows.is_empty() {
            let width = rows
                .iter()
                .map(Row::width)
                .max()
                .unwrap_or(0)
                .max(g.title().width() as u16);
            out.push(Group {
                title: g.title(),
                rows,
                width,
            });
        }
    }
    out
}

const COL_GAP: u16 = 3;

/// Groups placed left to right, starting a new band when the next does not
/// fit in `max_w`.
fn bands(groups: Vec<Group>, max_w: u16) -> Vec<Vec<Group>> {
    let mut out: Vec<Vec<Group>> = Vec::new();
    let mut used = 0u16;
    for g in groups {
        let need = if used == 0 {
            g.width
        } else {
            used + COL_GAP + g.width
        };
        match out.last_mut() {
            Some(band) if need <= max_w => {
                used = need;
                band.push(g);
            }
            _ => {
                used = g.width;
                out.push(vec![g]);
            }
        }
    }
    out
}

pub(super) fn draw_whichkey(app: &mut App, buf: &mut Buffer, body: Rect, theme: &Theme) {
    if app.pending.is_empty() {
        return;
    }
    let groups = groups(app);
    if groups.is_empty() {
        return;
    }
    // Inside: 2 columns of padding on each side, 1 row above and below.
    let bands = bands(groups, body.width.saturating_sub(8));
    let band_w = |b: &Vec<Group>| {
        b.iter().map(|g| g.width).sum::<u16>() + COL_GAP * (b.len() as u16).saturating_sub(1)
    };
    let band_h = |b: &Vec<Group>| 1 + b.iter().map(|g| g.rows.len()).max().unwrap_or(0) as u16;
    let inner_w = bands.iter().map(band_w).max().unwrap_or(0);
    let inner_h = bands.iter().map(band_h).sum::<u16>() + (bands.len() as u16).saturating_sub(1);
    let w = (inner_w + 6).min(body.width.saturating_sub(2));
    let h = (inner_h + 4).min(body.height);
    let x = body.x + body.width.saturating_sub(w + 1);
    let y = body.y + body.height.saturating_sub(h);
    let rect = Rect::new(x, y, w, h);
    let fill = theme.overlay();
    let inner = popup(buf, rect, body, fill, theme);
    let prefix = app
        .pending
        .iter()
        .map(|c| short_keys(&c.to_string()))
        .collect::<Vec<_>>()
        .join(" ");
    let title = format!(" {prefix} ");
    put(
        buf,
        rect.x + 2,
        rect.y,
        &title,
        w.saturating_sub(4),
        theme.bold(theme.dialog_accent()),
    );
    let foot = " Esc cancel ";
    let fx = (rect.x + w).saturating_sub(foot.width() as u16 + 2);
    put(
        buf,
        fx,
        rect.y + h - 1,
        foot,
        foot.width() as u16,
        theme.dialog_dim(),
    );
    // The menu takes clicks over its whole box, so they do not fall through.
    app.hits.push((rect, Hit::Menu(None)));
    let maxx = inner.x + inner.width;
    let maxy = inner.y + inner.height;
    let mut by = inner.y + 1;
    for band in &bands {
        let mut gx = inner.x + 2;
        for g in band {
            if by >= maxy {
                break;
            }
            put(
                buf,
                gx,
                by,
                g.title,
                maxx.saturating_sub(gx),
                theme.bold(theme.on_overlay(theme.title)),
            );
            for (i, row) in g.rows.iter().enumerate() {
                let ry = by + 1 + i as u16;
                if ry >= maxy {
                    break;
                }
                let mut cx = gx;
                for (j, (k, a)) in row.keys.iter().enumerate() {
                    if j > 0 {
                        cx = put(buf, cx, ry, " ", 1, fill);
                    }
                    let kx = cx;
                    cx = put(
                        buf,
                        cx,
                        ry,
                        k,
                        maxx.saturating_sub(cx),
                        theme.bold(theme.dialog_accent()),
                    );
                    app.hits
                        .push((Rect::new(kx, ry, cx - kx, 1), Hit::Menu(Some(*a))));
                }
                cx = put(buf, cx, ry, "  ", 2, fill);
                let lw = (gx + g.width)
                    .saturating_sub(cx)
                    .min(maxx.saturating_sub(cx));
                let lx = cx;
                put(buf, cx, ry, &truncate(row.label, lw as usize), lw, fill);
                // A single-key row runs its action from the label too.
                if let [(_, a)] = row.keys.as_slice() {
                    app.hits
                        .push((Rect::new(lx, ry, lw, 1), Hit::Menu(Some(*a))));
                }
            }
            gx += g.width + COL_GAP;
        }
        by += band_h(band) + 1;
    }
}
