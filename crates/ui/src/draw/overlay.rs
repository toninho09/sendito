//! Dialogs, pickers, the palette, help and settings. The screen behind a
//! dialog is faded (not cleared) and every dialog casts a shadow.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use ratatui::widgets::{Clear, Widget};
use unicode_width::UnicodeWidthStr;

use crate::app::*;
use crate::overlay::*;
use crate::theme::Theme;
use crate::viewer::ViewMode;

use super::kit::*;
use super::request::draw_table;

/// Character positions of `q` in `label` (case-insensitive): a substring
/// when there is one, else the letters of a fuzzy match.
fn match_positions(label: &str, q: &str) -> Vec<usize> {
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

/// Settings grouped under headings: (heading before this row, row).
fn settings_rows() -> Vec<(Option<&'static str>, usize)> {
    SETTINGS
        .iter()
        .enumerate()
        .map(|(i, (id, _))| {
            let h = match id {
                SettingId::Theme => Some("Appearance"),
                SettingId::Vim => Some("Editing"),
                SettingId::Sidebar => Some("Layout"),
                SettingId::Mouse => Some("Behavior"),
                _ => None,
            };
            (h, i)
        })
        .collect()
}

/// A line of the settings list: a heading or a setting row (its index in
/// `SETTINGS`).
enum SettingsLine {
    Head(&'static str),
    Row(usize),
}

/// Settings grouped under headings, flattened in the order they are drawn.
fn settings_lines(rows: &[(Option<&'static str>, usize)]) -> Vec<SettingsLine> {
    let mut out = Vec::new();
    for (heading, i) in rows {
        if let Some(h) = heading {
            out.push(SettingsLine::Head(h));
        }
        out.push(SettingsLine::Row(*i));
    }
    out
}

fn overlay_style(theme: &Theme) -> Style {
    Style::default().fg(theme.fg).bg(theme.overlay_bg)
}

fn hovered(app: &App, r: Rect) -> bool {
    app.hover
        .is_some_and(|(x, y)| r.contains(Position { x, y }))
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
    let push =
        |hits: &mut Vec<(Rect, Hit)>, r: Rect, h: OverlayHit| hits.push((r, Hit::Overlay(h)));
    push(&mut app.hits, area, OverlayHit::Outside);
    // Dialogs leave the hint and status rows as they are: they describe the
    // dialog. The rest of the screen fades behind it.
    let body = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(2));
    backdrop(buf, body, theme);
    let ost = overlay_style(theme);
    match &mut ov {
        Overlay::Palette(p) => {
            let h = (area.height * 2 / 3)
                .max(10)
                .min(body.height.saturating_sub(2));
            let w = 100.min(body.width.saturating_sub(4));
            // Near the top, where the eye already is.
            let top = body.y + (body.height.saturating_sub(h) / 3).max(1);
            let r = Rect::new(body.x + (body.width - w) / 2, top, w, h);
            let (r, inner) = modal_at(buf, r, body, "Command palette", theme);
            push(&mut app.hits, r, OverlayHit::Dialog);
            let ir = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), 1);
            let tr = input_box(buf, ir, true, theme);
            push(&mut app.hits, tr, OverlayHit::Input);
            cursor = p.input.render(tr, buf, theme, true, &is_def);
            let vis = p.visible();
            p.sel = p.sel.min(vis.len().saturating_sub(1));
            let lh = inner.height.saturating_sub(2) as usize;
            if p.sel < p.scroll {
                p.scroll = p.sel;
            } else if p.sel >= p.scroll + lh {
                p.scroll = p.sel + 1 - lh;
            }
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
                        theme.bar().patch(theme.dim()),
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
                let base = if sel {
                    theme.selected()
                } else if hovered(app, rr) {
                    ost.bg(theme.surface_alt)
                } else {
                    ost
                };
                fill(buf, rr, base);
                if sel {
                    put(
                        buf,
                        inner.x,
                        y,
                        "▌",
                        1,
                        base.patch(theme.fg(theme.on_selection(theme.accent))),
                    );
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
                    let c = theme.method_color(m);
                    put(
                        buf,
                        mx,
                        y,
                        m,
                        m.width() as u16,
                        base.patch(theme.fg(if sel { theme.on_selection(c) } else { c }))
                            .add_modifier(Modifier::BOLD),
                    );
                }
                // The typed letters, marked in the label.
                for ci in match_positions(&shown, query) {
                    let cx = lx + shown.chars().take(ci).collect::<String>().width() as u16;
                    if let Some(c) = buf.cell_mut((cx, y)) {
                        let fg = if sel {
                            theme.on_selection(theme.accent)
                        } else {
                            theme.accent
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
                        (false, PaletteItem::Action(_)) => base.patch(theme.accent()),
                        (false, _) => base.patch(theme.dim()),
                    };
                    put(buf, dx, y, detail, dw, st);
                }
                push(&mut app.hits, rr, OverlayHit::Item(row + p.scroll));
            }
            // Rows above and below the visible window, said in the frame.
            let below = vis.len().saturating_sub(p.scroll + lh);
            if below > 0 {
                let t = format!(" ↓ {below} more ");
                let tw = t.width() as u16;
                put(
                    buf,
                    r.x + r.width - tw - 2,
                    r.y + r.height - 1,
                    &t,
                    tw,
                    theme.accent().bg(theme.overlay_bg),
                );
            }
            if p.scroll > 0 {
                let t = format!(" ↑ {} more ", p.scroll);
                let tw = t.width() as u16;
                put(
                    buf,
                    inner.x + inner.width - tw,
                    inner.y + 1,
                    &t,
                    tw,
                    theme.dim().bg(theme.overlay_bg),
                );
            }
            scrollbar(buf, r, p.scroll, vis.len(), lh, true, theme);
            if vis.is_empty() {
                put(
                    buf,
                    inner.x + 2,
                    inner.y + 2,
                    "No match. Commands start with ':' (e.g. :env dev).",
                    inner.width.saturating_sub(3),
                    theme.dim().bg(theme.overlay_bg),
                );
            }
        }
        Overlay::Help { viewer, filter } => {
            // Wide screens read the keys in two columns, like a newspaper.
            let two_cols = area.width >= 180;
            let mw = if two_cols {
                area.width.saturating_sub(8).min(200)
            } else {
                110
            };
            let (r, inner) = modal(
                buf,
                body,
                mw,
                area.height.saturating_sub(2),
                "Help — keys and actions (Esc closes)",
                theme,
            );
            push(&mut app.hits, r, OverlayHit::Dialog);
            let ir = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), 1);
            put(
                buf,
                ir.x,
                ir.y,
                "Search ",
                7,
                theme.dim().bg(theme.overlay_bg),
            );
            let er = input_box(
                buf,
                Rect::new(ir.x + 7, ir.y, ir.width.saturating_sub(7), 1),
                true,
                theme,
            );
            push(&mut app.hits, er, OverlayHit::Input);
            cursor = filter.render(er, buf, theme, true, &is_def);
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
                    theme.bar().patch(theme.dim()),
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
                let (left, right) = help_columns(buf, vr, viewer, theme);
                push(&mut app.hits, left, OverlayHit::Item(0));
                push(&mut app.hits, right, OverlayHit::Item(left.height as usize));
                left.height as usize * 2
            } else {
                viewer.render(vr, buf, theme, true);
                push(&mut app.hits, vr, OverlayHit::Item(0));
                vr.height as usize
            };
            set_bg_unset(buf, vr, theme);
            scrollbar(
                buf,
                r,
                viewer.scroll,
                viewer.row_count(),
                shown,
                true,
                theme,
            );
        }
        Overlay::Settings { sel } => {
            let rows = settings_rows();
            let lines = settings_lines(&rows);
            let last = lines.len() as u16;
            let h = last + 10;
            let (r, inner) = modal(buf, body, 70, h, "Settings", theme);
            push(&mut app.hits, r, OverlayHit::Dialog);
            // The path is pinned to the bottom and the description block sits
            // above it; the list takes what is left and scrolls to keep the
            // selection visible on a short terminal.
            let footer = if inner.height >= 5 { 2 } else { 0 };
            let info = if inner.height >= last + 1 + footer + 4 {
                4
            } else {
                0
            };
            let top = inner.y + 1;
            let bottom = inner.y + inner.height - footer - info;
            let list_h = bottom.saturating_sub(top) as usize;
            let sel_line = lines
                .iter()
                .position(|l| matches!(l, SettingsLine::Row(i) if *i == *sel))
                .unwrap_or(0);
            let scroll = sel_line.saturating_sub(list_h.saturating_sub(1));
            let mut y = top;
            for line in lines.iter().skip(scroll).take(list_h) {
                let SettingsLine::Row(i) = line else {
                    if let SettingsLine::Head(hd) = line {
                        put(
                            buf,
                            inner.x + 2,
                            y,
                            hd,
                            inner.width.saturating_sub(2),
                            theme
                                .dim()
                                .bg(theme.overlay_bg)
                                .add_modifier(Modifier::BOLD),
                        );
                        y += 1;
                    }
                    continue;
                };
                let (id, label) = SETTINGS[*i];
                let s = *i == *sel;
                let rr = Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), 1);
                let base = if s {
                    theme.selected()
                } else if hovered(app, rr) {
                    ost.bg(theme.surface_alt)
                } else {
                    ost
                };
                fill(buf, rr, base);
                // A many-valued setting shows where it is among its values.
                let val = app.setting_value(id);
                let v = match id {
                    SettingId::Theme => {
                        let names = app.themes.names();
                        let i = names.iter().position(|n| *n == val).unwrap_or(0);
                        format!("‹ {val} › ({} of {})", i + 1, names.len())
                    }
                    _ => format!("‹ {val} ›"),
                };
                let vw = v.width() as u16;
                let vx = (inner.x + inner.width).saturating_sub(vw + 3);
                // The label gives way to the value on a narrow terminal.
                let room = vx.saturating_sub(inner.x + 4);
                put(
                    buf,
                    inner.x + 4,
                    y,
                    &truncate(label, room as usize),
                    room,
                    base,
                );
                put(
                    buf,
                    vx,
                    y,
                    &v,
                    vw,
                    if s {
                        base.add_modifier(Modifier::BOLD)
                    } else {
                        base.patch(theme.accent())
                    },
                );
                // The current theme's colors at a glance.
                if id == SettingId::Theme && !theme.mono {
                    let sw = [
                        theme.accent,
                        theme.ok,
                        theme.warn,
                        theme.err,
                        theme.info,
                        theme.patch,
                        theme.grpc,
                    ];
                    let mut sx = vx.saturating_sub(sw.len() as u16 + 2);
                    for c in sw {
                        sx = put(
                            buf,
                            sx,
                            y,
                            "█",
                            1,
                            Style::default()
                                .fg(c)
                                .bg(base.bg.unwrap_or(theme.overlay_bg)),
                        );
                    }
                }
                push(&mut app.hits, rr, OverlayHit::Item(*i));
                push(
                    &mut app.hits,
                    Rect::new(vx, y, vw, 1),
                    OverlayHit::Value(*i),
                );
                y += 1;
            }
            if info > 0 {
                rule(
                    buf,
                    inner.x + 1,
                    y,
                    inner.width.saturating_sub(2),
                    "",
                    theme,
                );
                set_bg(
                    buf,
                    Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), 1),
                    theme.overlay_bg,
                );
                // What the selected setting does, and the values it can take.
                let id = SETTINGS.get(*sel).map(|(id, _)| *id);
                let what = match id {
                    Some(SettingId::Vim) => {
                        "Vim keys in editors and lists (normal / insert modes).".to_string()
                    }
                    Some(SettingId::Theme) => {
                        let name = app.setting_value(SettingId::Theme);
                        match app.themes.notes(&name) {
                            [] if app.themes.path(&name).is_some() => format!(
                                "Your theme, from {}.",
                                crate::app::display_path(
                                    app.themes.path(&name).map(|p| p.as_path()).unwrap_or(std::path::Path::new("")),
                                    app.home.as_deref()
                                )
                            ),
                            [] => "Colors: auto follows the terminal; ansi uses your terminal palette; contrast is high-contrast. Add your own in themes/ (sendito theme print)."
                                .to_string(),
                            notes => format!("{name}: {}", notes.join("; ")),
                        }
                    }
                    Some(SettingId::Icons) => {
                        "unicode works with any font; nerd uses Nerd Font icons (install one first)."
                            .to_string()
                    }
                    Some(SettingId::Sidebar) => {
                        "Which side the Requests / History sidebar is on.".to_string()
                    }
                    Some(SettingId::Mouse) => {
                        "Clicks, wheel scrolling, dragging tree items and borders.".to_string()
                    }
                    Some(SettingId::ConfirmQuit) => {
                        "Ask before quitting while tabs have unsaved changes.".to_string()
                    }
                    Some(SettingId::History) => {
                        "Keep sent requests in History (F7) to reopen or resend.".to_string()
                    }
                    Some(SettingId::WatchInterval) => {
                        "The :watch command re-sends the active request on this interval."
                            .to_string()
                    }
                    None => String::new(),
                };
                let values = match id {
                    Some(SettingId::Theme) => app.themes.names().join(" / "),
                    Some(SettingId::Icons) => crate::icons::ICON_SETS.join(" / "),
                    Some(SettingId::Sidebar) => "left / right".into(),
                    Some(SettingId::WatchInterval) => "5 / 10 / 30 / 60 s".into(),
                    Some(_) => "On / Off".into(),
                    None => String::new(),
                };
                let tw = inner.width.saturating_sub(4);
                put_wrapped(
                    buf,
                    inner.x + 2,
                    y + 1,
                    &what,
                    tw,
                    y + 3,
                    theme.fg(theme.fg).bg(theme.overlay_bg),
                );
                let vline = format!(
                    "{}: {values}  (← → / Space)",
                    SETTINGS.get(*sel).map(|(_, l)| *l).unwrap_or("")
                );
                put(
                    buf,
                    inner.x + 2,
                    y + 3,
                    &truncate(&vline, tw as usize),
                    tw,
                    theme.dim().bg(theme.overlay_bg),
                );
            }
            if footer > 0 {
                let path = app
                    .project
                    .global_config
                    .as_ref()
                    .map(|p| crate::app::display_path(p, app.home.as_deref()))
                    .unwrap_or_else(|| "(not saved)".into());
                let tw = inner.width.saturating_sub(4);
                put(
                    buf,
                    inner.x + 2,
                    inner.y + inner.height.saturating_sub(2),
                    "Changes apply at once and are saved to",
                    tw,
                    theme.dim().bg(theme.overlay_bg),
                );
                put(
                    buf,
                    inner.x + 2,
                    inner.y + inner.height.saturating_sub(1),
                    &truncate_left(&path, tw as usize),
                    tw,
                    theme.dim().bg(theme.overlay_bg),
                );
            }
        }
        Overlay::Confirm(c) => {
            let lines = wrap_text(&c.message, 56);
            let (r, inner) = modal(buf, body, 64, lines.len() as u16 + 6, &c.title, theme);
            push(&mut app.hits, r, OverlayHit::Dialog);
            // Leave the button row for the buttons on a short terminal.
            let room = inner.height.saturating_sub(2) as usize;
            for (i, l) in lines.iter().take(room).enumerate() {
                put(
                    buf,
                    inner.x + 2,
                    inner.y + 1 + i as u16,
                    l,
                    inner.width.saturating_sub(3),
                    ost,
                );
            }
            let by = inner.y + inner.height.saturating_sub(1);
            let mut x = inner.x + 2;
            // Every button names its key, as in the other dialogs.
            let keys: &[&str] = if c.buttons.len() == 3 {
                &["s", "d", "Esc"]
            } else {
                &["y", "Esc"]
            };
            for (i, b) in c.buttons.iter().enumerate() {
                let start = x;
                let lab = match keys.get(i) {
                    Some(k) => format!("{b} ({k})"),
                    None => b.clone(),
                };
                x = button(buf, x, by, &lab, i == c.sel, theme);
                push(
                    &mut app.hits,
                    Rect::new(start, by, x - start, 1),
                    OverlayHit::Button(i),
                );
                x += 2;
            }
        }
        Overlay::Prompt(p) => {
            let has_msg = p.error.is_some() || p.note.is_some();
            let h = 7 + 2 * p.choices.len() as u16 + if has_msg { 2 } else { 0 };
            let (r, inner) = modal(buf, body, 66, h, &p.title, theme);
            push(&mut app.hits, r, OverlayHit::Dialog);
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
                    theme.dim().bg(theme.overlay_bg),
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
                    theme,
                );
                let c = p.input.render(er, buf, theme, p.focus == 0, &is_def);
                if p.focus == 0 {
                    cursor = c;
                }
                push(&mut app.hits, er, OverlayHit::Input);
            }
            let mut choices_end = inner.y + 1;
            for (k, (label, opts, i)) in p.choices.iter().enumerate() {
                let y = first_choice + step * k as u16;
                if y >= by {
                    break;
                }
                put(
                    buf,
                    inner.x + 2,
                    y,
                    label,
                    lw,
                    theme.dim().bg(theme.overlay_bg),
                );
                let v = format!(" ‹ {} › ", opts.get(*i).cloned().unwrap_or_default());
                put(
                    buf,
                    inner.x + 2 + lw,
                    y,
                    &v,
                    inner.width.saturating_sub(lw + 4),
                    if p.focus == k + 1 {
                        theme.chip(theme.accent)
                    } else {
                        theme.bar()
                    },
                );
                push(
                    &mut app.hits,
                    Rect::new(inner.x + 2 + lw, y, v.width() as u16, 1),
                    OverlayHit::Choice(k),
                );
                let hx = inner.x + 2 + lw + v.width() as u16 + 1;
                put(
                    buf,
                    hx,
                    y,
                    "← → change · Space list",
                    (inner.x + inner.width).saturating_sub(hx),
                    theme.dim().bg(theme.overlay_bg),
                );
                choices_end = y;
            }
            let msg = p
                .error
                .as_ref()
                .map(|e| (e, theme.fg(theme.err)))
                .or_else(|| p.note.as_ref().map(|n| (n, theme.fg(theme.warn))));
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
                        st.bg(theme.overlay_bg),
                    );
                }
            }
            let x0 = inner.x + 2;
            // The confirm button says what it does.
            let verb = crate::app::prompt_verb(&p.action);
            let ok = format!("{}{} (Enter)", verb[..1].to_uppercase(), &verb[1..]);
            let x1 = button(buf, x0, by, &ok, false, theme);
            push(
                &mut app.hits,
                Rect::new(x0, by, x1 - x0, 1),
                OverlayHit::Button(0),
            );
            let x2 = button(buf, x1 + 2, by, "Cancel (Esc)", false, theme);
            push(
                &mut app.hits,
                Rect::new(x1 + 2, by, x2 - x1 - 2, 1),
                OverlayHit::Button(1),
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
                    .min(area.width.saturating_sub(inner.x + 2 + lw));
                let room = (area.y + area.height).saturating_sub(y0 + 2);
                let shown = (opts.len() as u16).min(room);
                if shown > 0 && w >= 4 && y0 < area.y + area.height {
                    let lr = Rect::new(inner.x + 2 + lw, y0, w, shown + 2);
                    shadow(buf, lr, body, theme);
                    Clear.render(lr, buf);
                    let li = frame_with(buf, lr, theme, true, ost);
                    let first = (hl as u16 + 1).saturating_sub(shown) as usize;
                    for (row, (j, o)) in opts
                        .iter()
                        .enumerate()
                        .skip(first)
                        .take(shown as usize)
                        .enumerate()
                    {
                        let y = li.y + row as u16;
                        let st = if j == hl { theme.selected() } else { ost };
                        fill(buf, Rect::new(li.x, y, li.width, 1), st);
                        let mark = if j == *cur { "✓ " } else { "  " };
                        put(buf, li.x, y, &format!("{mark}{o}"), li.width, st);
                        push(
                            &mut app.hits,
                            Rect::new(li.x, y, li.width, 1),
                            OverlayHit::PromptOption(j),
                        );
                    }
                }
            }
        }
        Overlay::Picker(p) => {
            let vis = p.visible();
            let has_details = p.details.iter().any(|d| !d.is_empty())
                && !matches!(p.action, PickAction::MoveTo(_));
            let h = (vis.len() as u16 + 6 + u16::from(has_details))
                .min(area.height.saturating_sub(4))
                .max(4);
            let w = p
                .items
                .iter()
                .zip(p.details.iter().chain(std::iter::repeat(&String::new())))
                .map(|(a, b)| a.width() + b.width() + 8)
                .max()
                .unwrap_or(30)
                .clamp(38, 130) as u16;
            let (r, inner) = modal(buf, body, w, h, &p.title, theme);
            push(&mut app.hits, r, OverlayHit::Dialog);
            let ir = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), 1);
            let tr = input_box(buf, ir, true, theme);
            push(&mut app.hits, tr, OverlayHit::Input);
            cursor = p.filter.render(tr, buf, theme, true, &is_def);
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
                    theme.accent().bg(theme.overlay_bg),
                );
            }
            if p.sel < p.scroll {
                p.scroll = p.sel;
            } else if p.sel >= p.scroll + lh {
                p.scroll = p.sel + 1 - lh;
            }
            let methods = matches!(p.action, PickAction::Method);
            let envs = matches!(p.action, PickAction::Env);
            for (row, idx) in vis.iter().skip(p.scroll).take(lh).enumerate() {
                let y = inner.y + 2 + row as u16;
                let s = row + p.scroll == p.sel;
                let rr = Rect::new(inner.x, y, inner.width, 1);
                let base = if s {
                    theme.selected()
                } else if hovered(app, rr) {
                    ost.bg(theme.surface_alt)
                } else {
                    ost
                };
                fill(buf, rr, base);
                if s {
                    put(
                        buf,
                        inner.x,
                        y,
                        "▌",
                        1,
                        base.patch(theme.fg(theme.on_selection(theme.accent))),
                    );
                }
                let item = &p.items[*idx];
                let mut x = inner.x + 2;
                // Methods in their colors; environments with their marker.
                let item_st = if methods {
                    let c = theme.method_color(item);
                    base.patch(theme.fg(if s { theme.on_selection(c) } else { c }))
                        .add_modifier(Modifier::BOLD)
                } else {
                    base
                };
                if envs {
                    let real = !item.starts_with('(') && !item.is_empty();
                    let c = if real {
                        theme.env_color(item)
                    } else {
                        theme.dim
                    };
                    x = put(
                        buf,
                        x,
                        y,
                        &format!("{} ", app.icons.env),
                        2,
                        base.patch(theme.fg(if s { theme.on_selection(c) } else { c })),
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
                        if s { base } else { base.patch(theme.dim()) },
                    );
                }
                push(&mut app.hits, rr, OverlayHit::Item(row + p.scroll));
            }
            scrollbar(buf, r, p.scroll, vis.len(), lh, true, theme);
            if vis.is_empty() {
                put(
                    buf,
                    inner.x + 2,
                    inner.y + 2,
                    "No match",
                    inner.width.saturating_sub(3),
                    theme.dim().bg(theme.overlay_bg),
                );
            }
        }
        Overlay::Env(e) => {
            // Unsaved edits are marked, as on request tabs.
            let title = if e.dirty {
                "Environments ● unsaved"
            } else {
                "Environments"
            };
            let (mr, inner) = modal(
                buf,
                body,
                112,
                area.height.saturating_sub(4).max(12),
                title,
                theme,
            );
            // The frame itself is inert: dead space inside the dialog must
            // not fall through to the Outside that leaves the editor.
            push(&mut app.hits, mr, OverlayHit::Dialog);
            let inner = Rect::new(
                inner.x + 1,
                inner.y,
                inner.width.saturating_sub(2),
                inner.height,
            );
            let labels: Vec<String> = e
                .envs
                .iter()
                .map(|name| {
                    if name == sendito_store::env::COLLECTION_ENV {
                        "Collection variables".to_string()
                    } else if app.active_env.as_deref() == Some(name.as_str()) {
                        // The highlight shows the env being edited; this marks the one in use.
                        format!("{name} (active)")
                    } else {
                        name.clone()
                    }
                })
                .collect();
            let rects = tab_strip(
                buf,
                inner.x,
                inner.y,
                inner.x + inner.width,
                &labels,
                e.current,
                TabLook::Strong,
                theme,
            );
            for (i, r) in rects.into_iter().enumerate() {
                push(&mut app.hits, r, OverlayHit::EnvTab(i));
            }
            let legend = "Source says where a value lives (Space changes it). For secrets \
                and keychain, select Secret value and type the value. ✕ deletes a row (Ctrl+D). \
                Ctrl+S saves; Esc asks before discarding unsaved changes.";
            let ly = put_wrapped(
                buf,
                inner.x,
                inner.y + 2,
                legend,
                inner.width,
                (inner.y + 7).min(inner.y + inner.height.saturating_sub(4)),
                theme.dim().bg(theme.overlay_bg),
            );
            let tr = Rect::new(
                inner.x,
                ly + 1,
                inner.width,
                (inner.y + inner.height).saturating_sub(ly + 1 + 4),
            );
            // What the selected row's source means.
            let src = e
                .table
                .rows
                .get(e.table.sel_row)
                .and_then(|r| r.cells.get(1))
                .map(|c| c.choice());
            let explain = match src {
                Some(0) => "plain: written in this file, shared through git.",
                Some(1) => {
                    "secrets: in your .sendito/secrets.toml, never committed. Type the value in Secret value."
                }
                Some(2) => "keychain: in your OS keychain. Type the value in Secret value.",
                Some(3) => "file: read from the file at the path.",
                _ => "Type a name in the last row to add a variable.",
            };
            put(
                buf,
                inner.x,
                inner.y + inner.height.saturating_sub(3),
                &truncate(explain, inner.width as usize),
                inner.width,
                theme.fg(theme.info).bg(theme.overlay_bg),
            );
            let mut tmp_hits = Vec::new();
            cursor = draw_table(
                buf,
                tr,
                theme,
                &mut e.table,
                true,
                "",
                &is_def,
                &mut tmp_hits,
                0,
                app.hover,
            );
            for (rect, h) in tmp_hits {
                match h {
                    Hit::TableCell { row, col, .. } => {
                        push(&mut app.hits, rect, OverlayHit::EnvCell { row, col });
                    }
                    Hit::TableDelete { row, .. } => {
                        push(&mut app.hits, rect, OverlayHit::EnvDelete(row));
                    }
                    _ => {}
                }
            }
            if let Some(err) = &e.error {
                put(
                    buf,
                    inner.x,
                    inner.y + inner.height.saturating_sub(2),
                    &truncate(err, inner.width as usize),
                    inner.width,
                    theme.fg(theme.err).bg(theme.overlay_bg),
                );
            } else if e.dirty {
                put(
                    buf,
                    inner.x,
                    inner.y + inner.height.saturating_sub(2),
                    "● unsaved changes",
                    inner.width,
                    theme.fg(theme.warn).bg(theme.overlay_bg),
                );
            }
            let by = inner.y + inner.height.saturating_sub(1);
            let mut bx = inner.x;
            for (i, label) in ["Save (Ctrl+S)", "New env (Ctrl+N)", "Close (Esc)"]
                .iter()
                .enumerate()
            {
                let start = bx;
                bx = button(buf, bx, by, label, false, theme);
                push(
                    &mut app.hits,
                    Rect::new(start, by, bx - start, 1),
                    OverlayHit::Button(i),
                );
                bx += 2;
            }
        }
        Overlay::Mcp(m) => {
            let title = if m.dirty {
                "MCP servers ● unsaved"
            } else {
                "MCP servers"
            };
            let (mr, inner) = modal(
                buf,
                body,
                108,
                // The frame grows with the list: a few servers get a short
                // dialog, a long list scrolls inside the screen height.
                (m.table.rows.len() as u16 + 12).min(area.height.saturating_sub(4).max(12)),
                title,
                theme,
            );
            push(&mut app.hits, mr, OverlayHit::Dialog);
            let inner = Rect::new(
                inner.x + 1,
                inner.y,
                inner.width.saturating_sub(2),
                inner.height,
            );
            let legend = "Each row is a [mcp.servers] entry: a name, the kind (Space changes it) \
                and the URL or command. Typing in the last row adds one; ✕ deletes a row (Ctrl+D). \
                Args, env, headers and auth are edited in the config file. Ctrl+S saves to \
                .sendito/config.toml; Esc asks before discarding unsaved changes.";
            let ly = put_wrapped(
                buf,
                inner.x,
                inner.y + 1,
                legend,
                inner.width,
                inner.y + 7,
                theme.dim().bg(theme.overlay_bg),
            );
            let tr = Rect::new(
                inner.x,
                ly + 1,
                inner.width,
                // Three rows are left for the explanation, the unsaved marker
                // and the buttons; the rest shows the table and its add row.
                (inner.y + inner.height).saturating_sub(ly + 1 + 3),
            );
            let explain = match m.table.rows.get(m.table.sel_row) {
                Some(r) => match r.cells.get(1).map(|c| c.choice()) {
                    Some(1) => "stdio: started as a child process; the value is the command.",
                    _ => "http: a URL the tool calls go to (variables allowed).",
                },
                None => "Type a name in the last row to add a server.",
            };
            put(
                buf,
                inner.x,
                inner.y + inner.height.saturating_sub(3),
                &truncate(explain, inner.width as usize),
                inner.width,
                theme.fg(theme.info).bg(theme.overlay_bg),
            );
            let mut tmp_hits = Vec::new();
            cursor = draw_table(
                buf,
                tr,
                theme,
                &mut m.table,
                true,
                "",
                &is_def,
                &mut tmp_hits,
                0,
                app.hover,
            );
            for (rect, h) in tmp_hits {
                match h {
                    Hit::TableCell { row, col, .. } => {
                        push(&mut app.hits, rect, OverlayHit::McpCell { row, col });
                    }
                    Hit::TableDelete { row, .. } => {
                        push(&mut app.hits, rect, OverlayHit::McpDelete(row));
                    }
                    _ => {}
                }
            }
            if let Some(err) = &m.error {
                put(
                    buf,
                    inner.x,
                    inner.y + inner.height.saturating_sub(2),
                    &truncate(err, inner.width as usize),
                    inner.width,
                    theme.fg(theme.err).bg(theme.overlay_bg),
                );
            } else if m.dirty {
                put(
                    buf,
                    inner.x,
                    inner.y + inner.height.saturating_sub(2),
                    "● unsaved changes",
                    inner.width,
                    theme.fg(theme.warn).bg(theme.overlay_bg),
                );
            }
            let by = inner.y + inner.height.saturating_sub(1);
            let mut bx = inner.x;
            for (i, label) in ["Save (Ctrl+S)", "Add row (Ctrl+N)", "Close (Esc)"]
                .iter()
                .enumerate()
            {
                let start = bx;
                bx = button(buf, bx, by, label, false, theme);
                push(
                    &mut app.hits,
                    Rect::new(start, by, bx - start, 1),
                    OverlayHit::McpButton(i),
                );
                bx += 2;
            }
        }
        Overlay::Import(i) => {
            let (r, inner) = modal(buf, body, 100, 17, "Import", theme);
            push(&mut app.hits, r, OverlayHit::Dialog);
            let tw = inner.width.saturating_sub(4);
            let note = theme.dim().bg(theme.overlay_bg);
            put(
                buf,
                inner.x + 2,
                inner.y + 1,
                "Paste a curl command, or enter a Postman/Insomnia JSON file or Bruno directory.",
                tw,
                note,
            );
            put(
                buf,
                inner.x + 2,
                inner.y + 2,
                "Relative paths: the folder where you started Sendito, then the collection.",
                tw,
                note,
            );
            put(
                buf,
                inner.x + 2,
                inner.y + 3,
                "Enter = new line; F5 / Ctrl+S imports.",
                tw,
                note,
            );
            let er = Rect::new(inner.x + 2, inner.y + 5, tw, inner.height.saturating_sub(8));
            let tr = input_box(buf, er, true, theme);
            cursor = i.input.render(tr, buf, theme, true, &is_def);
            push(&mut app.hits, tr, OverlayHit::Input);
            if let Some(e) = &i.error {
                put(
                    buf,
                    inner.x + 2,
                    inner.y + inner.height.saturating_sub(2),
                    &truncate(e, tw as usize),
                    tw,
                    theme.fg(theme.err).bg(theme.overlay_bg),
                );
            }
            let by = inner.y + inner.height.saturating_sub(1);
            let x1 = button(buf, inner.x + 2, by, "Import (F5 / Ctrl+S)", false, theme);
            push(
                &mut app.hits,
                Rect::new(inner.x + 2, by, x1 - inner.x - 2, 1),
                OverlayHit::Button(0),
            );
            let x2 = button(buf, x1 + 2, by, "Cancel (Esc)", false, theme);
            push(
                &mut app.hits,
                Rect::new(x1 + 2, by, x2 - x1 - 2, 1),
                OverlayHit::Button(1),
            );
        }
        Overlay::Report(rep) => {
            let w = area.width.min(100).saturating_sub(8).max(10) as usize;
            let lines: usize = (0..rep.viewer.doc.line_count(ViewMode::Raw))
                .map(|i| wrap_text(&rep.viewer.doc.line_ref(ViewMode::Raw, i), w).len())
                .sum();
            let lines = lines.min(u16::MAX as usize) as u16;
            let (r, inner) = modal(
                buf,
                body,
                100,
                (lines.saturating_add(6))
                    .min(area.height.saturating_sub(2))
                    .max(4),
                &rep.title,
                theme,
            );
            push(&mut app.hits, r, OverlayHit::Dialog);
            // Padding around the text; a blank row separates it from the buttons.
            let vr = Rect::new(
                inner.x + 2,
                inner.y + 1,
                inner.width.saturating_sub(4),
                inner.height.saturating_sub(3),
            );
            rep.viewer.wrap = true;
            rep.viewer.render(vr, buf, theme, false);
            set_bg_unset(buf, vr, theme);
            scrollbar(
                buf,
                r,
                rep.viewer.scroll,
                rep.viewer.row_count(),
                vr.height as usize,
                true,
                theme,
            );
            let by = inner.y + inner.height.saturating_sub(1);
            let copy = if rep.copied {
                "Copy again (y)"
            } else {
                "Copy (y)"
            };
            let x1 = button(buf, inner.x + 2, by, copy, false, theme);
            push(
                &mut app.hits,
                Rect::new(inner.x + 2, by, x1 - inner.x - 2, 1),
                OverlayHit::Button(0),
            );
            let x2 = button(buf, x1 + 2, by, "Close (Esc)", false, theme);
            // Where the view is, so a long report reads as scrollable.
            if rep.viewer.row_count() > vr.height as usize {
                let pos = format!("{} · ↑↓ PgUp PgDn scroll", rep.viewer.status());
                let pw = pos.width() as u16;
                if x2 + pw + 2 < inner.x + inner.width {
                    put(
                        buf,
                        inner.x + inner.width - pw - 2,
                        by,
                        &pos,
                        pw,
                        theme.dim().bg(theme.overlay_bg),
                    );
                }
            }
            push(
                &mut app.hits,
                Rect::new(x1 + 2, by, x2 - x1 - 2, 1),
                OverlayHit::Button(1),
            );
        }
    }
    app.overlay = Some(ov);
    cursor
}

/// Renders `viewer` as one column twice the height of `area` and lays its
/// halves side by side, so scrolling, paging and search work unchanged.
/// Returns the left and right column.
fn help_columns(
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
        put(
            buf,
            rx,
            y,
            "│",
            1,
            theme.fg(theme.border).bg(theme.overlay_bg),
        );
    }
    (left, right)
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
