//! The editable table every form is made of (params, headers, scripts, the
//! environment variables, the MCP servers). It is shared by the request pane
//! and by the dialogs, so it takes its click targets from the caller.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use crate::form::{Cell, ColKind, Table};
use crate::icons::Icons;
use crate::theme::Theme;

use super::kit::{fill, put, set_bg, truncate};
use super::request::auto_value;

/// What a click on a table asks for.
pub(super) enum TableHit {
    Cell { row: usize, col: usize },
    Toggle { row: usize },
    Delete { row: usize },
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_table(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    t: &mut Table,
    focused: bool,
    label: &str,
    is_def: &dyn Fn(&str) -> bool,
    hit: &mut dyn FnMut(Rect, TableHit),
    hover: Option<(u16, u16)>,
) -> Option<(u16, u16)> {
    if area.height == 0 {
        return None;
    }
    let check_w: u16 = if t.has_enable { 4 } else { 0 };
    // "│✕": the delete button is a column of its own, separated like the others.
    let del_w: u16 = 2;
    let ncol = t.columns.len() as u16;
    let seps = ncol.saturating_sub(1);
    let fixed: u16 = t
        .columns
        .iter()
        .map(|c| match c.kind {
            ColKind::Choice(opts) => opts.iter().map(|o| o.len()).max().unwrap_or(4) as u16 + 3,
            ColKind::Toggle => 5,
            ColKind::Text => 0,
        })
        .sum();
    let text_weights: u16 = t
        .columns
        .iter()
        .filter(|c| c.kind == ColKind::Text)
        .map(|c| c.weight)
        .sum::<u16>()
        .max(1);
    let avail = area.width.saturating_sub(check_w + del_w + seps + fixed);
    let widths: Vec<u16> = t
        .columns
        .iter()
        .map(|c| match c.kind {
            ColKind::Choice(opts) => opts.iter().map(|o| o.len()).max().unwrap_or(4) as u16 + 3,
            ColKind::Toggle => 5,
            ColKind::Text => (avail as u32 * c.weight as u32 / text_weights as u32) as u16,
        })
        .collect();
    let sep_st = theme.fg(theme.border);
    // Header row.
    let mut x = area.x;
    if !label.is_empty() {
        let lw = (label.width() as u16 + 1).min(check_w.max(label.width() as u16 + 1));
        put(buf, x, area.y, label, lw, theme.bold(theme.accent()));
        if x < area.x + check_w {
            x = area.x + check_w;
        }
    } else {
        x += check_w;
    }
    for (i, c) in t.columns.iter().enumerate() {
        let w = if i == 0 && x > area.x + check_w {
            widths[0].saturating_sub(x - area.x - check_w)
        } else {
            widths[i]
        };
        put(
            buf,
            x,
            area.y,
            &truncate(c.title, w as usize),
            w,
            theme.bold(theme.dim()),
        );
        x += w + 1;
    }
    let body_h = area.height.saturating_sub(1) as usize;
    if focused {
        t.ensure_visible(body_h);
    }
    let total = t.rows.len() + 1;
    let mut cursor = None;
    let icons = Icons::unicode();
    for vis in 0..body_h {
        let r = t.scroll + vis;
        if r >= total {
            break;
        }
        let y = area.y + 1 + vis as u16;
        let row_rect = Rect::new(area.x, y, area.width, 1);
        let row_sel = focused && r == t.sel_row;
        let is_new = r == t.rows.len();
        let row_hover =
            hover.is_some_and(|(hx, hy)| hy == y && hx >= area.x && hx < area.x + area.width);
        // Alternate rows are shaded so wide rows stay readable.
        let zebra = r % 2 == 1 && !theme.mono && !is_new;
        let row_bg = if zebra {
            theme.surface_alt
        } else {
            theme.surface
        };
        let base = Style::default().fg(theme.fg).bg(row_bg);
        fill(buf, row_rect, base);
        let mut x = area.x;
        if t.has_enable {
            if is_new {
                put(buf, x, y, " +  ", check_w, theme.accent().bg(row_bg));
            } else {
                let on = t.rows[r].enabled;
                let x2 = put(buf, x, y, "[", 1, base.patch(theme.dim()));
                let x2 = put(
                    buf,
                    x2,
                    y,
                    if on { icons.ok } else { " " },
                    1,
                    theme.bold(base.patch(theme.fg(theme.ok))),
                );
                put(buf, x2, y, "] ", 2, base.patch(theme.dim()));
                hit(Rect::new(x, y, 3, 1), TableHit::Toggle { row: r });
            }
            x += check_w;
        }
        let enabled = is_new || t.rows[r].enabled;
        for (c, w) in widths.iter().enumerate() {
            let cell_rect = Rect::new(x, y, *w, 1);
            let cell_sel = row_sel && c == t.sel_col;
            if is_new {
                let ph = t.placeholder.get(c).copied().unwrap_or("");
                let st = if cell_sel {
                    theme.selected()
                } else {
                    theme.italic(theme.dim().bg(row_bg))
                };
                if cell_sel {
                    fill(buf, cell_rect, theme.selected());
                }
                let text = if c == 0 {
                    format!("add {}", ph.to_lowercase())
                } else {
                    String::new()
                };
                put(buf, x, y, &truncate(&text, *w as usize), *w, st);
                if cell_sel {
                    cursor = Some((x, y));
                }
            } else {
                let kind = t.columns[c].kind;
                // No guess from a half-typed path.
                let auto = if row_sel && t.sel_col == 2 {
                    None
                } else {
                    auto_value(t, r, c)
                };
                match (&mut t.rows[r].cells[c], kind) {
                    (Cell::Text(ed), _) => {
                        if cell_sel {
                            fill(buf, cell_rect, theme.bar());
                        }
                        if let Some(cp) = ed.render(cell_rect, buf, theme, cell_sel, is_def) {
                            cursor = Some(cp);
                        }
                        // An empty cell whose value is derived shows it.
                        if let Some(a) = auto.filter(|_| ed.text().is_empty() && !cell_sel) {
                            // Too narrow for the "(auto)" marker: show the bare value.
                            let a = if a.chars().count() > *w as usize {
                                a.strip_suffix(" (auto)").unwrap_or(&a).to_string()
                            } else {
                                a
                            };
                            put(
                                buf,
                                x,
                                y,
                                &truncate(&a, *w as usize),
                                *w,
                                theme.italic(theme.dim()),
                            );
                        }
                        // The row's background, then the selected cell on top.
                        set_bg(
                            buf,
                            cell_rect,
                            if cell_sel && !theme.mono {
                                theme.sel_bg
                            } else {
                                row_bg
                            },
                        );
                        if !enabled {
                            for xx in cell_rect.x..cell_rect.x + cell_rect.width {
                                if let Some(cell) = buf.cell_mut((xx, y)) {
                                    cell.set_style(
                                        theme.dim().add_modifier(Modifier::CROSSED_OUT).bg(row_bg),
                                    );
                                }
                            }
                        }
                    }
                    (Cell::Choice(i), ColKind::Choice(opts)) => {
                        // ‹ › marks a value that Space cycles (as in dialogs and
                        // settings); ▾ is kept for real menus.
                        let label = format!("‹{}›", opts.get(*i).copied().unwrap_or(""));
                        put(
                            buf,
                            x,
                            y,
                            &label,
                            *w,
                            if cell_sel {
                                theme.selected()
                            } else {
                                theme.on_overlay(theme.info).bg(row_bg)
                            },
                        );
                    }
                    (Cell::Toggle(b), _) => {
                        let mark = if *b {
                            format!("[{}]", icons.ok)
                        } else {
                            "[ ]".to_string()
                        };
                        put(
                            buf,
                            x,
                            y,
                            &mark,
                            *w,
                            if cell_sel { theme.selected() } else { base },
                        );
                    }
                    _ => {}
                }
            }
            hit(cell_rect, TableHit::Cell { row: r, col: c });
            x += w;
            if c + 1 < widths.len() {
                put(buf, x, y, "│", 1, sep_st.bg(row_bg));
                x += 1;
            }
        }
        if !is_new {
            let dx = area.x + area.width - del_w;
            put(buf, dx, y, "│", 1, sep_st.bg(row_bg));
            // The delete button is quiet until its row is selected or pointed at.
            let del_st = if row_sel || row_hover {
                theme.bold(theme.fg(theme.err))
            } else {
                theme.dim()
            };
            put(buf, dx + 1, y, icons.close, 1, del_st.bg(row_bg));
            hit(Rect::new(dx, y, del_w, 1), TableHit::Delete { row: r });
        }
    }
    if focused { cursor } else { None }
}
