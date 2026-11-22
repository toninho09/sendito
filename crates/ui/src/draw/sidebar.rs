//! The sidebar: request tree and history.

use ratatui::buffer::Buffer;
use ratatui::layout::{Position, Rect};
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use crate::app::*;
use crate::icons::Icons;
use crate::theme::Theme;
use crate::tree::{Row, RowKind};

use super::kit::*;

/// For each row: whether it has a later sibling, and for each ancestor level
/// whether that ancestor does (which decides `├`/`└` and the `│` guides).
fn tree_guides(rows: &[Row]) -> Vec<(bool, Vec<bool>)> {
    let mut out = vec![(false, Vec::new()); rows.len()];
    let mut open: Vec<bool> = Vec::new();
    for (i, r) in rows.iter().enumerate().rev() {
        let d = r.depth;
        if open.len() <= d {
            open.resize(d + 1, false);
        }
        out[i] = (open[d], open[..d].to_vec());
        open[d] = true;
        for o in open.iter_mut().skip(d + 1) {
            *o = false;
        }
    }
    out
}

pub(super) fn draw_sidebar(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    let focused = matches!(app.focus, Focus::Sidebar | Focus::Filter);
    let inner = frame(buf, area, theme, focused);
    app.hits.push((inner, Hit::SidebarArea));
    let icons = app.icons;
    // Mode switch in the top border.
    let labels = vec![
        format!("{}Requests", Icons::pre(icons.requests)),
        format!("{}History", Icons::pre(icons.history)),
    ];
    let active = usize::from(app.sidebar_mode == SidebarMode::History);
    let look = if focused {
        TabLook::Soft
    } else {
        TabLook::Quiet
    };
    let max_x = area.x + area.width - 1;
    let rects = tab_strip(buf, area.x + 1, area.y, max_x, &labels, active, look, theme);
    for (r, mode) in rects
        .into_iter()
        .zip([SidebarMode::Tree, SidebarMode::History])
    {
        app.hits.push((r, Hit::SidebarMode(mode)));
    }
    // A count in the bottom border.
    let count = match app.sidebar_mode {
        SidebarMode::Tree => {
            let (mut reqs, mut dirs) = (0usize, 0usize);
            fn walk(f: &sendito_store::Folder, reqs: &mut usize, dirs: &mut usize) {
                *reqs += f.requests.len();
                for s in &f.folders {
                    *dirs += 1;
                    walk(s, reqs, dirs);
                }
            }
            walk(&app.tree.root, &mut reqs, &mut dirs);
            let plural = |n: usize, w: &str| format!("{n} {w}{}", if n == 1 { "" } else { "s" });
            if dirs > 0 {
                format!(" {} · {} ", plural(reqs, "request"), plural(dirs, "folder"))
            } else {
                format!(" {} ", plural(reqs, "request"))
            }
        }
        SidebarMode::History => match &app.history {
            Some(h) => format!(
                " {} {} ",
                h.len(),
                if h.len() == 1 { "entry" } else { "entries" }
            ),
            None => String::new(),
        },
    };
    let cw = count.width() as u16;
    if cw + 4 < area.width {
        put(
            buf,
            max_x.saturating_sub(cw + 1),
            area.y + area.height - 1,
            &count,
            cw,
            theme.dim(),
        );
    }
    if inner.height == 0 {
        return;
    }
    if app.sidebar_mode == SidebarMode::History {
        draw_history(app, buf, inner, theme);
        return;
    }
    let mut tree_area = inner;
    let show_filter = app.focus == Focus::Filter || !app.tree.filter.is_empty();
    if show_filter {
        let fr = Rect::new(inner.x, inner.y, inner.width, 1);
        let f_focus = app.focus == Focus::Filter;
        let ir = input_box(buf, fr, f_focus, theme);
        let icon = if icons.search.is_empty() {
            "/"
        } else {
            icons.search
        };
        put(buf, ir.x, ir.y, icon, 1, theme.bar().patch(theme.accent()));
        let er = Rect::new(ir.x + 2, ir.y, ir.width.saturating_sub(2), 1);
        let names = app.names.clone();
        if let Some(c) = app
            .filter
            .render(er, buf, theme, f_focus, &|n| names.is_defined(n))
        {
            app.cursor = Some(c);
        }
        app.hits.push((fr, Hit::TreeFilter));
        tree_area = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
    }
    app.areas.tree = tree_area;
    if app.tree.rows.is_empty() {
        let lines: Vec<Spans> = if app.tree.filter.is_empty() {
            vec![
                vec![(
                    "No requests yet.".into(),
                    theme.fg(theme.fg).add_modifier(Modifier::BOLD),
                )],
                Vec::new(),
                vec![
                    (" n ".into(), theme.keycap()),
                    (" new request".into(), theme.dim()),
                ],
                vec![
                    (" N ".into(), theme.keycap()),
                    (" new folder".into(), theme.dim()),
                ],
                vec![
                    (
                        format!(
                            " {} ",
                            app.keymap.hint(crate::keymap::Action::Import, app.vim)
                        ),
                        theme.keycap(),
                    ),
                    (" import".into(), theme.dim()),
                ],
            ]
        } else {
            vec![
                vec![(format!("No match for \"{}\"", app.tree.filter), theme.dim())],
                vec![
                    (" Esc ".into(), theme.keycap()),
                    (" clears the filter".into(), theme.dim()),
                ],
            ]
        };
        for (i, l) in lines.iter().enumerate() {
            if (i as u16) < tree_area.height {
                put_spans(
                    buf,
                    tree_area.x + 1,
                    tree_area.y + i as u16,
                    tree_area.x + tree_area.width,
                    l,
                );
            }
        }
        return;
    }
    app.tree.ensure_visible(tree_area.height as usize);
    let dragging = matches!(app.drag, Some(Drag::Tree { active: true, .. }));
    let guides = tree_guides(&app.tree.rows);
    let filter = app.tree.filter.to_lowercase();
    for i in 0..tree_area.height as usize {
        let idx = app.tree.scroll + i;
        let Some(row) = app.tree.rows.get(idx).cloned() else {
            break;
        };
        let y = tree_area.y + i as u16;
        let rect = Rect::new(tree_area.x, y, tree_area.width, 1);
        let selected = idx == app.tree.selected;
        let hover = app
            .hover
            .is_some_and(|(x, y)| rect.contains(Position { x, y }));
        let drop =
            dragging && row.is_folder() && app.drop_target.as_deref() == Some(row.rel.as_str());
        let sel_focus = (selected && focused) || drop;
        let base = if sel_focus {
            theme.selected()
        } else if selected {
            Style::default()
                .fg(theme.fg)
                .bg(theme.surface_alt)
                .add_modifier(Modifier::BOLD)
        } else if hover {
            Style::default().fg(theme.fg).bg(theme.surface_alt)
        } else {
            theme.surface()
        };
        // The drop target is unmistakable: the selection colors, bold, and a label.
        let base = if drop {
            base.add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            base
        };
        fill(buf, rect, base);
        let bg = base.bg.unwrap_or(theme.surface);
        let line_st = if sel_focus {
            base.remove_modifier(Modifier::BOLD)
        } else {
            theme.fg(theme.border).bg(bg)
        };
        // Guides: │ for an ancestor with more children, ├/└ for the row itself.
        let (has_next, anc) = &guides[idx];
        let maxx = rect.x + rect.width;
        let mut x = rect.x;
        if row.depth > 0 {
            for d in 1..row.depth {
                let g = if anc.get(d).copied().unwrap_or(false) {
                    "│ "
                } else {
                    "  "
                };
                x = put(buf, x, y, g, maxx.saturating_sub(x), line_st);
            }
            x = put(
                buf,
                x,
                y,
                if *has_next { "├ " } else { "└ " },
                maxx.saturating_sub(x),
                line_st,
            );
        }
        let name_st;
        match &row.kind {
            RowKind::Folder { expanded, empty } => {
                // An empty folder has nothing to open, so no ▸/▾ marker.
                let icon = if *empty {
                    " "
                } else if *expanded {
                    icons.expanded
                } else {
                    icons.collapsed
                };
                x = put(
                    buf,
                    x,
                    y,
                    icon,
                    maxx.saturating_sub(x),
                    base.patch(theme.dim()),
                );
                x = put(buf, x, y, " ", 1, base);
                let folder = if *expanded {
                    icons.folder_open
                } else {
                    icons.folder_closed
                };
                if !folder.is_empty() {
                    x = put(
                        buf,
                        x,
                        y,
                        &format!("{folder} "),
                        maxx.saturating_sub(x),
                        base.patch(theme.fg(theme.on_selection(theme.accent))),
                    );
                }
                name_st = base.add_modifier(Modifier::BOLD);
                let name = format!("{}/", row.name);
                let nx = x;
                x = put(
                    buf,
                    x,
                    y,
                    &truncate(&name, maxx.saturating_sub(x) as usize),
                    maxx.saturating_sub(x),
                    name_st,
                );
                highlight_match(buf, nx, y, &row.name, &filter, maxx, theme);
            }
            RowKind::Request { badge } => {
                if row.depth == 0 {
                    x += 2;
                }
                let b = format!("{:<5}", short_method(badge));
                let mc = if sel_focus {
                    theme.on_selection(theme.method_color(badge))
                } else {
                    theme.method_color(badge)
                };
                x = put(
                    buf,
                    x,
                    y,
                    &b,
                    maxx.saturating_sub(x),
                    theme.fg(mc).bg(bg).add_modifier(Modifier::BOLD),
                );
                x = put(buf, x, y, " ", 1, base);
                let open = app
                    .tabs
                    .iter()
                    .any(|t| t.path.as_deref() == Some(row.rel.as_str()));
                // A request open in a tab reads in the accent color.
                name_st = if open && !sel_focus {
                    base.patch(theme.accent())
                } else if open {
                    base.add_modifier(Modifier::ITALIC)
                } else {
                    base
                };
                let name = truncate(&row.name, maxx.saturating_sub(x) as usize);
                let nx = x;
                x = put(buf, x, y, &name, maxx.saturating_sub(x), name_st);
                highlight_match(buf, nx, y, &row.name, &filter, maxx, theme);
            }
        }
        if drop {
            let label = " ← move here ";
            let lw = label.width() as u16;
            if rect.width > lw + 12 && x + lw < maxx {
                put(buf, maxx - lw, y, label, lw, base);
            }
        }
        app.hits.push((rect, Hit::TreeRow(idx)));
    }
    scrollbar(
        buf,
        area,
        app.tree.scroll,
        app.tree.rows.len(),
        tree_area.height as usize,
        focused,
        theme,
    );
    if dragging && app.drop_target.as_deref() == Some("") {
        put(
            buf,
            tree_area.x,
            tree_area.y + tree_area.height.saturating_sub(1),
            "↳ drop: collection root",
            tree_area.width,
            theme.accent(),
        );
    }
}

/// Marks where the filter text occurs in a name drawn at `x`.
fn highlight_match(
    buf: &mut Buffer,
    x: u16,
    y: u16,
    name: &str,
    filter: &str,
    maxx: u16,
    theme: &Theme,
) {
    if filter.is_empty() || theme.mono {
        return;
    }
    let lower = name.to_lowercase();
    let Some(b) = lower.find(filter) else { return };
    // Only when lowercasing kept the byte layout (plain names).
    if lower.len() != name.len() {
        return;
    }
    let start = name[..b].width() as u16;
    let len = name[b..b + filter.len()].width() as u16;
    for cx in x + start..(x + start + len).min(maxx) {
        if let Some(c) = buf.cell_mut((cx, y)) {
            c.set_bg(theme.search_bg).set_fg(theme.fg);
        }
    }
}

/// "21 Nov 2026" from an RFC 3339 timestamp.
fn day_label(ts: &str) -> String {
    let months = [
        "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
    ];
    let (Some(y), Some(m), Some(d)) = (ts.get(0..4), ts.get(5..7), ts.get(8..10)) else {
        return ts.to_string();
    };
    let month = m
        .parse::<usize>()
        .ok()
        .and_then(|i| months.get(i.wrapping_sub(1)))
        .copied()
        .unwrap_or(m);
    format!("{} {month} {y} (UTC)", d.trim_start_matches('0'))
}

pub(super) fn draw_history(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    let focused = app.focus == Focus::Sidebar;
    let Some(h) = &app.history else {
        put(buf, area.x + 1, area.y, "Loading…", area.width, theme.dim());
        return;
    };
    if h.is_empty() {
        put(
            buf,
            area.x + 1,
            area.y,
            "No requests sent yet.",
            area.width.saturating_sub(1),
            theme.dim(),
        );
        return;
    }
    // Entries take two rows; a day heading goes before the first entry of
    // each day (and at the top).
    let layout = |scroll: usize| -> Vec<(usize, u16, bool)> {
        let mut out = Vec::new();
        let mut y = 0u16;
        let mut prev: Option<&str> = None;
        for (idx, e) in h.iter().enumerate().skip(scroll) {
            let day = e.ts.get(0..10);
            let heading = prev != day;
            let need = 2 + u16::from(heading);
            if y + need > area.height {
                break;
            }
            out.push((idx, y + u16::from(heading), heading));
            y += need;
            prev = day;
        }
        out
    };
    if app.history_sel < app.history_scroll {
        app.history_scroll = app.history_sel;
    }
    let mut rows = layout(app.history_scroll);
    while app.history_scroll < app.history_sel
        && rows.last().is_none_or(|(i, _, _)| *i < app.history_sel)
    {
        app.history_scroll += 1;
        rows = layout(app.history_scroll);
    }
    let h = app.history.as_deref().unwrap_or(&[]);
    for (idx, ry, heading) in rows {
        let Some(e) = h.get(idx) else { break };
        let y = area.y + ry;
        if heading {
            rule(buf, area.x, y - 1, area.width, &day_label(&e.ts), theme);
        }
        let sel = idx == app.history_sel;
        let rect = Rect::new(area.x, y, area.width, 2);
        let hover = app
            .hover
            .is_some_and(|(x, y)| rect.contains(Position { x, y }));
        let base = if sel && focused {
            theme.selected()
        } else if sel || hover {
            Style::default().fg(theme.fg).bg(theme.surface_alt)
        } else {
            theme.surface()
        };
        let bg = base.bg.unwrap_or(theme.surface);
        let sel_focus = sel && focused;
        fill(buf, rect, base);
        let mc = theme.method_color(&e.badge);
        let mut x = put(
            buf,
            area.x + 1,
            y,
            &format!("{:<5}", short_method(&e.badge)),
            5,
            theme
                .fg(if sel_focus {
                    theme.on_selection(mc)
                } else {
                    mc
                })
                .bg(bg)
                .add_modifier(Modifier::BOLD),
        );
        x = put(buf, x, y, " ", 1, base);
        let status = match (e.status, &e.error) {
            (_, Some(err)) if e.badge != "GRPC" || e.status.is_none() => {
                format!("ERR {}", truncate(err, 20))
            }
            (Some(s), _) => s.to_string(),
            (None, Some(err)) => format!("ERR {}", truncate(err, 20)),
            (None, None) => "—".into(),
        };
        let sc = e
            .status
            .map(|s| {
                if e.badge == "GRPC" {
                    if s == 0 { theme.ok } else { theme.err }
                } else {
                    theme.status_color(s as u16)
                }
            })
            .unwrap_or(theme.err);
        let sst = if sel_focus {
            theme
                .fg(theme.on_selection(sc))
                .bg(bg)
                .add_modifier(Modifier::BOLD)
        } else {
            theme.badge(sc)
        };
        x = put(
            buf,
            x,
            y,
            &format!(" {status} "),
            area.width.saturating_sub(x - area.x),
            sst,
        );
        let time = e.ts.get(11..19).unwrap_or("").to_string();
        // The environment tells apart the same path sent to different servers.
        let env = e
            .env
            .as_deref()
            .map(|v| format!("{v} · "))
            .unwrap_or_default();
        // A narrow sidebar keeps the path readable: only the time on the right.
        let ms = if area.width < 40 {
            time.to_string()
        } else if e.duration_ms == 0 {
            format!("{env}{time} <1 ms")
        } else {
            format!("{env}{time} {} ms", e.duration_ms)
        };
        // Line 1: method, status, name. Line 2: the URL as written … time and
        // duration (UTC).
        let name_room = (area.x + area.width).saturating_sub(x + 2);
        put(
            buf,
            x + 1,
            y,
            &truncate(&e.name, name_room as usize),
            name_room,
            base,
        );
        let url = if e.template.is_empty() {
            &e.target
        } else {
            &e.template
        };
        let mw = ms.width() as u16 + 1;
        let url_room = area.width.saturating_sub(3 + mw);
        put(
            buf,
            area.x + 2,
            y + 1,
            &truncate_left(&crate::tab::url_path(url), url_room as usize),
            url_room,
            base.patch(theme.dim()),
        );
        if area.width > mw + 8 {
            let slow = e.duration_ms >= 1000;
            put(
                buf,
                area.x + area.width - mw,
                y + 1,
                &ms,
                mw,
                if slow && !sel_focus {
                    base.patch(theme.fg(theme.warn))
                } else {
                    base.patch(theme.dim())
                },
            );
        }
        app.hits.push((rect, Hit::HistoryRow(idx)));
    }
    let _ = Icons::pre("");
}
