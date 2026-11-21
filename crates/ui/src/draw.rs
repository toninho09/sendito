//! Rendering. Every clickable region is registered in `app.hits`.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use ratatui::symbols::border;
use ratatui::text::{Line, Span};
use ratatui::widgets::{Block, Borders, Clear, Paragraph, Widget};
use unicode_width::UnicodeWidthStr;

use crate::app::*;
use crate::form::{Cell, ColKind, Table};
use crate::keymap::Action;
use crate::overlay::*;
use crate::tab::{Field, Kind};
use crate::theme::Theme;
use crate::tree::RowKind;
use crate::viewer::ViewMode;

fn put(buf: &mut Buffer, x: u16, y: u16, s: &str, max: u16, st: Style) -> u16 {
    if max == 0 {
        return x;
    }
    let (nx, _) = buf.set_stringn(x, y, s, max as usize, st);
    nx
}

/// Keeps the end of `s` (the part of a URL that differs between entries).
fn truncate_left(s: &str, w: usize) -> String {
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

fn truncate(s: &str, w: usize) -> String {
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

fn fill(buf: &mut Buffer, area: Rect, st: Style) {
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

fn block<'a>(theme: &Theme, title: &'a str, focused: bool) -> Block<'a> {
    Block::default()
        .borders(Borders::ALL)
        // The focused pane gets a heavy border as well as the accent color.
        .border_set(if focused {
            border::THICK
        } else {
            border::ROUNDED
        })
        .border_style(theme.border(focused))
        .title(Span::styled(
            format!(" {title} "),
            if focused {
                theme.accent().add_modifier(Modifier::BOLD)
            } else {
                theme.dim()
            },
        ))
        .style(theme.base())
}

pub fn draw(app: &mut App, f: &mut Frame) {
    app.hits.clear();
    let area = f.area();
    app.size = (area.width, area.height);
    let theme = app.theme.clone();
    let buf = f.buffer_mut();
    fill(buf, area, theme.base());
    if area.height < 8 || area.width < 40 {
        put(
            buf,
            0,
            0,
            "Sendito needs at least 40×8 cells",
            area.width,
            theme.base(),
        );
        return;
    }
    let top = Rect::new(area.x, area.y, area.width, 1);
    let body = Rect::new(area.x, area.y + 1, area.width, area.height - 3);
    let hint = Rect::new(area.x, area.y + area.height - 2, area.width, 1);
    let status = Rect::new(area.x, area.y + area.height - 1, area.width, 1);

    draw_tabbar(app, buf, top, &theme);

    let sidebar_visible = app.layout.sidebar_visible;
    let right = app.project.config.ui.sidebar == "right";
    let sw = if sidebar_visible {
        let max = (body.width / 2).max(16);
        let w = app.layout.sidebar_width.clamp(16, max);
        if body.width < 100 {
            w.min((body.width / 3).max(20))
        } else {
            w
        }
    } else {
        0
    };
    let (side, main) = if sw == 0 {
        (Rect::default(), body)
    } else if right {
        (
            Rect::new(body.x + body.width - sw, body.y, sw, body.height),
            Rect::new(body.x, body.y, body.width - sw, body.height),
        )
    } else {
        (
            Rect::new(body.x, body.y, sw, body.height),
            Rect::new(body.x + sw, body.y, body.width - sw, body.height),
        )
    };
    app.areas.sidebar = side;
    app.areas.main = main;
    let zoom_side = app.zoom && matches!(app.focus, Focus::Sidebar | Focus::Filter);
    if sw > 0 {
        let s = if zoom_side { body } else { side };
        draw_sidebar(app, buf, s, &theme);
        // Draggable border column.
        let bx = if right {
            side.x
        } else {
            side.x + side.width - 1
        };
        app.hits.push((
            Rect::new(bx, side.y + 1, 1, side.height.saturating_sub(2)),
            Hit::SidebarBorder,
        ));
    }
    if !zoom_side {
        draw_main(app, buf, main, &theme);
    }
    draw_hints(app, buf, hint, &theme);
    let cursor = draw_status(app, buf, status, &theme);
    let mut cursor = cursor.or(app.cursor.take());
    if app.overlay.is_some() {
        cursor = draw_overlay(app, buf, area, &theme);
    }
    if let Some((x, y)) = cursor {
        f.set_cursor_position((x, y));
    }
    app.last_cursor = cursor;
}

fn method_style(theme: &Theme, m: &str) -> Style {
    theme.fg(theme.method_color(m)).add_modifier(Modifier::BOLD)
}

fn draw_tabbar(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    fill(buf, area, theme.bar());
    let env = app.active_env.clone().unwrap_or_else(|| "no env".into());
    let env_label = format!(" env: {env} ▾ ");
    let help_label = " F1 all keys ";
    let right_w = (env_label.width() + help_label.width()) as u16;
    let tabs_w = area.width.saturating_sub(right_w + 1);
    // Make the active tab visible.
    let labels: Vec<(String, bool)> = app
        .tabs
        .iter()
        .map(|t| {
            (
                format!(
                    " {} {}{}{} ",
                    t.badge(),
                    truncate(&t.title(), 22),
                    // A history copy is told apart from the saved request.
                    if t.origin.is_some() { " (copy)" } else { "" },
                    if t.dirty() { " ●" } else { "" }
                ),
                t.running.is_some(),
            )
        })
        .collect();
    let widths: Vec<u16> = labels.iter().map(|(l, _)| l.width() as u16 + 2).collect();
    let mut start = 0;
    while !widths.is_empty() {
        let used: u16 = widths[start..=app.active.min(widths.len().saturating_sub(1))]
            .iter()
            .sum::<u16>()
            + 4;
        if used <= tabs_w || start >= app.active {
            break;
        }
        start += 1;
    }
    let mut x = area.x;
    if start > 0 {
        x = put(buf, x, area.y, "‹", 1, theme.dim().patch(theme.bar()));
    }
    for (i, (label, running)) in labels.iter().enumerate().skip(start) {
        let w = label.width() as u16 + 2;
        if x + w + 3 > area.x + tabs_w {
            put(buf, x, area.y, "›", 1, theme.dim().patch(theme.bar()));
            break;
        }
        let active = i == app.active;
        let st = if active {
            theme.selected().add_modifier(Modifier::BOLD)
        } else {
            theme.bar()
        };
        let tab = &app.tabs[i];
        let badge = tab.badge();
        let start_x = x;
        x = put(buf, x, area.y, " ", 1, st);
        x = put(
            buf,
            x,
            area.y,
            &badge,
            badge.width() as u16,
            method_style(theme, &badge).patch(if active {
                Style::default().bg(theme.sel_bg)
            } else {
                Style::default().bg(theme.bar_bg)
            }),
        );
        let rest = &label[badge.len() + 1..];
        x = put(buf, x, area.y, rest, rest.width() as u16, st);
        // "↻" while a request runs ("…" only ever means "cut short").
        let close = if *running { "↻" } else { "×" };
        let cx = x;
        x = put(buf, x, area.y, close, 1, st.patch(theme.dim()));
        x = put(buf, x, area.y, " ", 1, st);
        app.hits
            .push((Rect::new(start_x, area.y, cx - start_x, 1), Hit::Tab(i)));
        app.hits
            .push((Rect::new(cx, area.y, 1, 1), Hit::TabClose(i)));
        x = put(buf, x, area.y, "│", 1, theme.dim().patch(theme.bar()));
    }
    let px = x;
    x = put(
        buf,
        x,
        area.y,
        " + ",
        3,
        theme
            .accent()
            .patch(theme.bar())
            .add_modifier(Modifier::BOLD),
    );
    app.hits
        .push((Rect::new(px, area.y, x - px, 1), Hit::NewTab));
    let ex = area.x + area.width - right_w;
    let env_st = if app.active_env.is_some() {
        theme.accent().patch(theme.bar())
    } else {
        theme.dim().patch(theme.bar())
    };
    let hx = put(
        buf,
        ex,
        area.y,
        &env_label,
        env_label.width() as u16,
        env_st.add_modifier(Modifier::BOLD),
    );
    app.hits.push((
        Rect::new(ex, area.y, env_label.width() as u16, 1),
        Hit::EnvSelector,
    ));
    put(
        buf,
        hx,
        area.y,
        help_label,
        help_label.width() as u16,
        theme.dim().patch(theme.bar()),
    );
    app.hits.push((
        Rect::new(hx, area.y, help_label.width() as u16, 1),
        Hit::HelpButton,
    ));
}

fn draw_sidebar(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    let focused = matches!(app.focus, Focus::Sidebar | Focus::Filter);
    let blk = block(theme, "", focused);
    let inner = blk.inner(area);
    blk.render(area, buf);
    app.hits.push((inner, Hit::SidebarArea));
    // Mode switch in the top border.
    let mut x = area.x + 2;
    for (label, mode) in [
        ("Requests", SidebarMode::Tree),
        ("History", SidebarMode::History),
    ] {
        let active = app.sidebar_mode == mode;
        let st = if active {
            theme.accent().add_modifier(Modifier::BOLD)
        } else {
            theme.dim()
        };
        let text = format!(" {label} ");
        let w = (text.width() as u16).min((area.x + area.width).saturating_sub(x + 1));
        put(
            buf,
            x,
            area.y,
            &text,
            w,
            if active && focused {
                st.add_modifier(Modifier::UNDERLINED)
            } else {
                st
            },
        );
        app.hits
            .push((Rect::new(x, area.y, w, 1), Hit::SidebarMode(mode)));
        x += w;
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
        put(buf, fr.x, fr.y, "/ ", 2, theme.accent());
        let er = Rect::new(fr.x + 2, fr.y, fr.width.saturating_sub(2), 1);
        let names = app.names.clone();
        if let Some(c) = app
            .filter
            .render(er, buf, theme, app.focus == Focus::Filter, &|n| {
                names.is_defined(n)
            })
        {
            app.cursor = Some(c);
        }
        app.hits.push((fr, Hit::TreeFilter));
        tree_area = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
    }
    app.areas.tree = tree_area;
    if app.tree.rows.is_empty() {
        let msg = if app.tree.filter.is_empty() {
            vec![
                "No requests yet.".to_string(),
                String::new(),
                "n  new request".into(),
                "N  new folder".into(),
                "Ctrl+W i  import".into(),
            ]
        } else {
            vec![
                format!("No match for \"{}\"", app.tree.filter),
                "Esc clears the filter".into(),
            ]
        };
        for (i, l) in msg.iter().enumerate() {
            if (i as u16) < tree_area.height {
                put(
                    buf,
                    tree_area.x + 1,
                    tree_area.y + i as u16,
                    l,
                    tree_area.width.saturating_sub(1),
                    theme.dim(),
                );
            }
        }
        return;
    }
    app.tree.ensure_visible(tree_area.height as usize);
    let dragging = matches!(app.drag, Some(Drag::Tree { active: true, .. }));
    for i in 0..tree_area.height as usize {
        let idx = app.tree.scroll + i;
        let Some(row) = app.tree.rows.get(idx).cloned() else {
            break;
        };
        let y = tree_area.y + i as u16;
        let rect = Rect::new(tree_area.x, y, tree_area.width, 1);
        let selected = idx == app.tree.selected;
        let drop =
            dragging && row.is_folder() && app.drop_target.as_deref() == Some(row.rel.as_str());
        let base = if selected && focused {
            theme.selected()
        } else if selected {
            theme.base().add_modifier(Modifier::BOLD)
        } else {
            theme.base()
        };
        // The drop target is unmistakable: the selection colors, bold, and a label.
        let base = if drop {
            theme
                .selected()
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            base
        };
        fill(buf, rect, base);
        if drop {
            let label = " ← move here ";
            let lw = label.width() as u16;
            if rect.width > lw + 12 {
                put(buf, rect.x + rect.width - lw, y, label, lw, base);
            }
        }
        // A guide line per level shows which folder a row belongs to.
        for d in 0..row.depth as u16 {
            let gx = rect.x + d * 2;
            if gx + 1 < rect.x + rect.width {
                put(buf, gx, y, "│", 1, base.patch(theme.fg(theme.border)));
            }
        }
        let mut x = rect.x + (row.depth as u16) * 2;
        let maxx = rect.x + rect.width;
        match &row.kind {
            RowKind::Folder { expanded, empty } => {
                // An empty folder has nothing to open, so no ▸/▾ marker.
                let icon = if *empty {
                    "  "
                } else if *expanded {
                    "▾ "
                } else {
                    "▸ "
                };
                x = put(
                    buf,
                    x,
                    y,
                    icon,
                    maxx.saturating_sub(x),
                    base.patch(theme.dim()),
                );
                let name = format!("{}/", row.name);
                put(
                    buf,
                    x,
                    y,
                    &name,
                    maxx.saturating_sub(x),
                    base.add_modifier(Modifier::BOLD),
                );
            }
            RowKind::Request { badge } => {
                x += 2;
                let b = format!("{:<5}", short_method(badge));
                x = put(
                    buf,
                    x,
                    y,
                    &b,
                    maxx.saturating_sub(x),
                    method_style(theme, badge)
                        .patch(Style::default().bg(base.bg.unwrap_or(theme.bg))),
                );
                x = put(buf, x, y, " ", 1, base);
                let open = app
                    .tabs
                    .iter()
                    .any(|t| t.path.as_deref() == Some(row.rel.as_str()));
                let name = truncate(&row.name, maxx.saturating_sub(x) as usize);
                put(
                    buf,
                    x,
                    y,
                    &name,
                    maxx.saturating_sub(x),
                    if open {
                        base.add_modifier(Modifier::ITALIC)
                    } else {
                        base
                    },
                );
            }
        }
        app.hits.push((rect, Hit::TreeRow(idx)));
    }
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

fn draw_history(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
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
    let rows_per = 2usize;
    let visible = (area.height as usize / rows_per).max(1);
    if app.history_sel < app.history_scroll {
        app.history_scroll = app.history_sel;
    } else if app.history_sel >= app.history_scroll + visible {
        app.history_scroll = app.history_sel + 1 - visible;
    }
    for i in 0..visible {
        let idx = app.history_scroll + i;
        let Some(e) = h.get(idx) else { break };
        let y = area.y + (i * rows_per) as u16;
        let sel = idx == app.history_sel;
        let base = if sel && focused {
            theme.selected()
        } else {
            theme.base()
        };
        let rect = Rect::new(area.x, y, area.width, rows_per as u16);
        fill(buf, rect, base);
        let mut x = put(
            buf,
            area.x,
            y,
            &format!("{:<5}", short_method(&e.badge)),
            5,
            method_style(theme, &e.badge).patch(Style::default().bg(base.bg.unwrap_or(theme.bg))),
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
        x = put(
            buf,
            x,
            y,
            &status,
            area.width.saturating_sub(x - area.x),
            theme
                .fg(sc)
                .patch(Style::default().bg(base.bg.unwrap_or(theme.bg))),
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
            x + 2,
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
        let url_room = area.width.saturating_sub(2 + mw);
        put(
            buf,
            area.x + 1,
            y + 1,
            &truncate_left(&crate::tab::url_path(url), url_room as usize),
            url_room,
            base.patch(theme.dim()),
        );
        if area.width > mw + 8 {
            put(
                buf,
                area.x + area.width - mw,
                y + 1,
                &ms,
                mw,
                base.patch(theme.dim()),
            );
        }
        app.hits.push((rect, Hit::HistoryRow(idx)));
    }
}

fn draw_main(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    if app.tabs.is_empty() {
        draw_welcome(app, buf, area, theme);
        return;
    }
    let req_focus = matches!(app.focus, Focus::Bar(_) | Focus::ReqTabs | Focus::ReqBody);
    let resp_focus = matches!(app.focus, Focus::RespTabs | Focus::RespBody);
    let (req_area, resp_area) = if app.zoom && req_focus {
        (area, Rect::default())
    } else if app.zoom && resp_focus {
        (Rect::default(), area)
    } else {
        let rh = ((area.height as u32 * app.layout.request_pct as u32) / 100) as u16;
        let rh = rh.clamp(6.min(area.height), area.height.saturating_sub(5).max(6));
        (
            Rect::new(area.x, area.y, area.width, rh),
            Rect::new(
                area.x,
                area.y + rh,
                area.width,
                area.height.saturating_sub(rh),
            ),
        )
    };
    app.areas.request = req_area;
    app.areas.response = resp_area;
    if req_area.height > 0 {
        draw_request(app, buf, req_area, theme);
    }
    if resp_area.height > 0 {
        draw_response(app, buf, resp_area, theme);
        if req_area.height > 0 {
            app.hits.push((
                Rect::new(
                    resp_area.x + 1,
                    resp_area.y,
                    resp_area.width.saturating_sub(2),
                    1,
                ),
                Hit::SplitBorder,
            ));
        }
    }
}

fn draw_welcome(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    let blk = block(theme, "Sendito", false);
    let inner = blk.inner(area);
    blk.render(area, buf);
    let k = |a: Action| app.keymap.hint(a, app.vim);
    let lines = vec![
        ("".to_string(), format!("Collection: {}", app.project.name)),
        ("".to_string(), String::new()),
        (k(Action::NewRequest), "new HTTP request tab".to_string()),
        (
            "n / N".to_string(),
            "new request / folder in the sidebar".to_string(),
        ),
        ("Enter".to_string(), "open the selected request".to_string()),
        (
            k(Action::Palette),
            "command palette (actions, requests, :commands)".to_string(),
        ),
        (
            k(Action::Import),
            "import a Postman collection or a curl command".to_string(),
        ),
        (k(Action::SelectEnv), "choose the environment".to_string()),
        (
            "m".to_string(),
            "move the selected request to a folder (or drag it with the mouse)".to_string(),
        ),
        (
            k(Action::Settings),
            "settings: vim mode, theme, sidebar side".to_string(),
        ),
        (k(Action::Help), "all keys".to_string()),
        (k(Action::Quit), "quit".to_string()),
    ];
    let bottom = inner.y + inner.height;
    let mut y = inner.y + 1;
    for (key, desc) in &lines {
        if y >= bottom {
            break;
        }
        let x = put(
            buf,
            inner.x + 2,
            y,
            &format!("{key:<14}"),
            14.min(inner.width.saturating_sub(2)),
            theme.accent().add_modifier(Modifier::BOLD),
        );
        let st = if key.is_empty() {
            theme.base().add_modifier(Modifier::BOLD)
        } else {
            theme.base()
        };
        y = put_wrapped(
            buf,
            x,
            y,
            desc,
            inner.width.saturating_sub(x - inner.x),
            bottom,
            st,
        );
    }
}

/// Writes `text` word-wrapped to `w` columns starting at (`x`, `y`); every
/// line starts at `x`. Stops before `bottom`. Returns the next free row.
fn put_wrapped(
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
        if y >= bottom {
            break;
        }
        put(buf, x, y, &truncate(&l, w as usize), w, st);
        y += 1;
    }
    y
}

fn short_label(f: Field) -> &'static str {
    match f {
        Field::BodyKind => "Body type",
        Field::RawContentType | Field::FileContentType => "Content-Type",
        Field::FilePath => "File",
        Field::AuthKind => "Auth type",
        Field::AuthUser => "Username",
        Field::AuthPass => "Password",
        Field::AuthToken => "Token",
        Field::AuthKey => "Key name",
        Field::AuthValue => "Key value",
        Field::AuthIn => "Send in",
        Field::Timeout => "Timeout ms",
        Field::FollowRedirects => "Redirects",
        Field::VerifyTls | Field::GrpcVerify => "Verify TLS",
        Field::SchemaSource => "Schema",
        Field::ProtoFiles => "Proto files",
        Field::ImportPaths => "Import paths",
        Field::Tls => "Use TLS",
        Field::Deadline => "Deadline ms",
        Field::ScriptSection => "Section",
        Field::ScriptPre | Field::ScriptExtract | Field::ScriptAssert => "",
        _ => "",
    }
}

/// A note shown under a field: (text, is a warning). `editing`: a text cell
/// of this field is selected, so a path may still be half typed.
fn field_note(
    f: Field,
    model: &sendito_core::Request,
    root: &std::path::Path,
    editing: bool,
) -> Option<(String, bool)> {
    let sendito_core::Request::Http(h) = model else {
        return None;
    };
    let body_on_get =
        matches!(h.method.as_str(), "GET" | "HEAD") && !matches!(h.body, sendito_core::Body::None);
    match f {
        Field::Params => {
            let q = h.url.split_once('?').map(|(_, q)| q)?;
            let q = q.split('#').next().unwrap_or(q);
            Some((
                format!(
                    "The URL's own query (?{q}) is sent as written; rows here are added after it."
                ),
                false,
            ))
        }
        Field::Parts => {
            let missing = match &h.body {
                _ if editing => None,
                sendito_core::Body::Multipart { parts } => {
                    parts.iter().find_map(|p| match &p.kind {
                        sendito_core::PartKind::File { path, .. }
                            if p.enabled
                                && !path.is_empty()
                                && !path.contains("{{")
                                && !sendito_store::env::expand(root, path).is_file() =>
                        {
                            Some(path.clone())
                        }
                        _ => None,
                    })
                }
                _ => None,
            };
            // The first file part that exists, with its size.
            let found = match &h.body {
                sendito_core::Body::Multipart { parts } => {
                    parts.iter().find_map(|p| match &p.kind {
                        sendito_core::PartKind::File { path, .. }
                            if p.enabled && !path.is_empty() =>
                        {
                            std::fs::metadata(sendito_store::env::expand(root, path))
                                .ok()
                                .filter(|m| m.is_file())
                                .map(|m| (path.clone(), m.len()))
                        }
                        _ => None,
                    })
                }
                _ => None,
            };
            Some(match (missing, body_on_get, found) {
                (Some(p), _, _) => (format!("File not found: {p} (paths are relative to the collection folder)"), true),
                (None, true, found) => (
                    format!(
                        "{} requests usually have no body; many servers ignore it.{}",
                        h.method,
                        found
                            .map(|(p, n)| format!(" File found: {p} ({}).", human_size(n)))
                            .unwrap_or_default()
                    ),
                    true,
                ),
                (None, false, Some((p, n))) => (
                    format!("File found: {p} ({}). Values marked (auto) come from the file.", human_size(n)),
                    false,
                ),
                (None, false, None) => (
                    "File paths are relative to the collection folder; an empty Filename or Content-Type comes from the file.".into(),
                    false,
                ),
            })
        }
        Field::BodyText | Field::FormFields | Field::FilePath if body_on_get => Some((
            format!(
                "{} requests usually have no body; many servers ignore it.",
                h.method
            ),
            true,
        )),
        _ => None,
    }
}

/// What an empty multipart cell resolves to for a file part: the file name
/// and the content type guessed from it.
fn auto_value(t: &Table, r: usize, c: usize) -> Option<String> {
    let title = t.columns.get(c)?.title;
    if !matches!(title, "Filename" | "Content-Type") {
        return None;
    }
    let row = t.rows.get(r)?;
    if row.cells.get(1)?.choice() != 1 {
        return None;
    }
    let path = row.cells.get(2)?.text();
    let name = path.rsplit(['/', '\\']).next().unwrap_or("").to_string();
    if name.is_empty() {
        return None;
    }
    let filename = match row.cells.get(3).map(|c| c.text()) {
        Some(f) if !f.is_empty() => f,
        _ => name,
    };
    // Shown in italics with "auto:": a derived value, not one typed in the cell.
    Some(format!(
        "{} (auto)",
        if title == "Filename" {
            filename
        } else {
            sendito_http::guess_mime(&filename)
        }
    ))
}

fn table_count(t: &Table) -> usize {
    t.values().len()
}

fn draw_request(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    let focused = matches!(app.focus, Focus::Bar(_) | Focus::ReqTabs | Focus::ReqBody);
    let title = {
        let t = &app.tabs[app.active];
        let path = match (&t.path, &t.origin) {
            (Some(p), _) => p.clone(),
            (None, Some(o)) => format!("{} · {o}", t.name),
            (None, None) => format!(
                "{} · no file yet ({} saves it to a file)",
                t.title(),
                app.keymap.hint(Action::Save, app.vim)
            ),
        };
        // Unsaved changes are marked once, on the tab (and counted in the
        // status line).
        format!("Request · {path}")
    };
    let blk = block(theme, &title, focused);
    let inner = blk.inner(area);
    blk.render(area, buf);
    if inner.height < 2 || inner.width < 10 {
        return;
    }
    let names = app.names.clone();
    let is_def = move |n: &str| names.is_defined(n);
    let vim = app.vim;
    let focus = app.focus;
    let running = app.tabs[app.active].running.is_some();
    let streaming = app.tabs[app.active]
        .running
        .as_ref()
        .is_some_and(|r| r.streaming_input);
    // URL bar.
    let y = inner.y;
    let kind = app.tabs[app.active].ed.kind;
    // A gRPC call needs a method first; until then Call is shown disabled.
    let has_method = match &app.tabs[app.active].model {
        sendito_core::Request::Grpc(g) => !g.service.is_empty() && !g.method.is_empty(),
        sendito_core::Request::Http(_) => true,
    };
    // Without a method the button is dimmed (the hint row says why).
    let send_label = if streaming {
        " Send message "
    } else if running {
        " Sending "
    } else {
        " Send "
    };
    let send_w = send_label.width() as u16;
    let send_x = inner.x + inner.width - send_w;
    let mut cursor = None;
    match kind {
        Kind::Http => {
            let method = app.tabs[app.active].ed.method.clone();
            let ml = format!(" {:<7}▾", truncate(&method, 7));
            let mw = ml.width() as u16;
            let mst = method_style(theme, &method).patch(if focus == Focus::Bar(0) {
                theme.selected()
            } else {
                theme.bar()
            });
            put(buf, inner.x, y, &ml, mw, mst.add_modifier(Modifier::BOLD));
            app.hits.push((Rect::new(inner.x, y, mw, 1), Hit::Bar(0)));
            let ur = Rect::new(
                inner.x + mw + 1,
                y,
                send_x.saturating_sub(inner.x + mw + 2),
                1,
            );
            fill(buf, ur, theme.bar());
            app.areas.bar_url = ur;
            let tab = &mut app.tabs[app.active];
            if let Some(c) = tab
                .ed
                .url
                .render(ur, buf, theme, focus == Focus::Bar(1), &is_def)
            {
                cursor = Some(c);
            }
            app.hits.push((ur, Hit::Bar(1)));
        }
        Kind::Grpc => {
            let (svc, m) = (
                app.tabs[app.active].ed.service.clone(),
                app.tabs[app.active].ed.grpc_method.clone(),
            );
            let ml = if svc.is_empty() {
                " none ▾".to_string()
            } else {
                format!(" {}/{} ▾", svc.rsplit('.').next().unwrap_or(&svc), m)
            };
            // "target" labels the address field, as the method menu does for HTTP.
            let label = " target ";
            let lw = label.width() as u16;
            put(
                buf,
                inner.x,
                y,
                label,
                lw,
                theme.fg(theme.grpc).add_modifier(Modifier::BOLD),
            );
            // "method" labels the method menu (dropped when the bar is narrow).
            let mlabel = " method ";
            let avail = inner.width.saturating_sub(send_w + 4 + lw);
            let mlw = if avail > 30 { mlabel.width() as u16 } else { 0 };
            let avail = avail.saturating_sub(mlw);
            let mw = (ml.width() as u16).min(avail / 2).max(10);
            let tw = avail.saturating_sub(mw);
            let tr = Rect::new(inner.x + lw, y, tw, 1);
            fill(buf, tr, theme.bar());
            app.areas.bar_url = tr;
            let tab = &mut app.tabs[app.active];
            if let Some(c) = tab
                .ed
                .target
                .render(tr, buf, theme, focus == Focus::Bar(0), &is_def)
            {
                cursor = Some(c);
            }
            app.hits.push((tr, Hit::Bar(0)));
            if mlw > 0 {
                put(
                    buf,
                    inner.x + lw + tw + 1,
                    y,
                    mlabel,
                    mlw,
                    theme.fg(theme.grpc).add_modifier(Modifier::BOLD),
                );
            }
            let mx = inner.x + lw + tw + 1 + mlw;
            let mst = if focus == Focus::Bar(1) {
                theme.selected()
            } else {
                theme.bar().patch(theme.fg(theme.grpc))
            };
            put(
                buf,
                mx,
                y,
                &truncate(&ml, mw as usize),
                mw,
                mst.add_modifier(Modifier::BOLD),
            );
            app.hits.push((Rect::new(mx, y, mw, 1), Hit::Bar(1)));
        }
    }
    let send_st = if !has_method {
        theme.bar().patch(theme.dim())
    } else if focus == Focus::Bar(2) {
        theme.selected()
    } else if theme.mono {
        Style::default().add_modifier(Modifier::REVERSED)
    } else {
        Style::default().fg(theme.bg).bg(if running && !streaming {
            theme.warn
        } else {
            theme.accent
        })
    };
    put(
        buf,
        send_x,
        y,
        send_label,
        send_w,
        send_st.add_modifier(Modifier::BOLD),
    );
    app.hits
        .push((Rect::new(send_x, y, send_w, 1), Hit::Bar(2)));

    // Sub-tabs.
    let ty = inner.y + 1;
    let (names_list, counts): (Vec<&str>, Vec<usize>) = {
        let e = &app.tabs[app.active].ed;
        let n = e.tab_names().to_vec();
        let c = match e.kind {
            Kind::Http => vec![
                table_count(&e.params),
                table_count(&e.headers),
                usize::from(e.body_kind != sendito_core::BodyKind::None),
                usize::from(e.auth_kind != sendito_core::AuthKind::None),
                table_count(&e.pre) + table_count(&e.extract) + table_count(&e.asserts),
                0,
            ],
            Kind::Grpc => vec![
                0,
                table_count(&e.metadata),
                usize::from(e.auth_kind != sendito_core::AuthKind::None),
                table_count(&e.pre) + table_count(&e.extract) + table_count(&e.asserts),
                0,
            ],
        };
        (n, c)
    };
    let req_tab = app.tabs[app.active].req_tab;
    let mut x = inner.x;
    // Pick the most informative label style that fits. Tables show their row
    // count, "Params (2)"; Body and Auth show "•" when set. Only when even the
    // compact form does not fit do counts become "•" too.
    let style_labels = |style: usize| -> Vec<String> {
        names_list
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let set = counts[i] > 0;
                let counted = set && !matches!(*name, "Body" | "Auth");
                match style {
                    0 if counted => format!(" {name} ({}) ", counts[i]),
                    1 if counted => format!("{name} ({})", counts[i]),
                    2 if counted => format!("{name}({})", counts[i]),
                    0 if set => format!(" {name}• "),
                    0 => format!(" {name} "),
                    _ if set => format!("{name}•"),
                    _ => name.to_string(),
                }
            })
            .collect()
    };
    let labels = (0..4)
        .map(style_labels)
        .find(|ls| ls.iter().map(|l| l.width() as u16 + 1).sum::<u16>() <= inner.width)
        .unwrap_or_else(|| style_labels(3));
    for (i, label) in labels.iter().enumerate() {
        let w = (label.width() as u16).min((inner.x + inner.width).saturating_sub(x));
        if w == 0 {
            break;
        }
        let active = i == req_tab;
        let st = if active && focus == Focus::ReqTabs {
            theme.selected().add_modifier(Modifier::BOLD)
        } else if active {
            theme
                .accent()
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            theme.dim()
        };
        put(buf, x, ty, label, w, st);
        app.hits.push((Rect::new(x, ty, w, 1), Hit::ReqTab(i)));
        x += w;
        if i + 1 < labels.len() && x < inner.x + inner.width {
            x = put(buf, x, ty, "│", 1, theme.dim());
        }
    }
    let content = Rect::new(
        inner.x,
        inner.y + 2,
        inner.width,
        inner.height.saturating_sub(2),
    );
    app.areas.req_body = content;
    if let Some(c) = draw_fields(app, buf, content, theme, &is_def, vim) {
        cursor = Some(c);
    }
    if focused {
        app.cursor = cursor;
    }
}

fn draw_fields(
    app: &mut App,
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    is_def: &dyn Fn(&str) -> bool,
    _vim: bool,
) -> Option<(u16, u16)> {
    let focus_body = app.focus == Focus::ReqBody;
    let root = app.project.root.clone();
    let tab = &mut app.tabs[app.active];
    let fields = tab.fields();
    app.areas.field_rects.clear();
    if fields.is_empty() || area.height == 0 {
        return None;
    }
    // Fixed one-row fields; tables and multi-line editors share the rest.
    let flex: Vec<usize> = fields
        .iter()
        .enumerate()
        .filter(|(_, f)| f.is_table() || f.is_multiline())
        .map(|(i, _)| i)
        .collect();
    let fixed = (fields.len() - flex.len()) as u16;
    let remaining = area.height.saturating_sub(fixed);
    let per_flex = if flex.is_empty() {
        0
    } else {
        (remaining / flex.len() as u16).max(2)
    };
    let mut y = area.y;
    let label_w: u16 = 14;
    let mut cursor = None;
    let selected = tab.field;
    let mut rects = Vec::new();
    for (i, f) in fields.iter().enumerate() {
        let h = if f.is_table() || f.is_multiline() {
            if i == *flex.last().unwrap_or(&0) {
                (area.y + area.height).saturating_sub(y)
            } else {
                per_flex
            }
        } else {
            1
        };
        if y >= area.y + area.height {
            rects.push(Rect::default());
            continue;
        }
        let h = h.min(area.y + area.height - y);
        let rect = Rect::new(area.x, y, area.width, h);
        rects.push(rect);
        let is_sel = focus_body && i == selected;
        // A one-line note under some tables.
        let editing = is_sel && tab.ed.table_mut(*f).is_some_and(|t| t.editing_text());
        let note = field_note(*f, &tab.model, &root, editing);
        // The note wraps onto two lines when the table has room for them.
        let rect = match &note {
            Some((text, warn)) if rect.height >= 3 => {
                let lines = wrap_text(text, rect.width.max(1) as usize).len() as u16;
                let nh = if lines > 1 && rect.height >= 5 { 2 } else { 1 };
                let ny = rect.y + rect.height - nh;
                let st = if *warn {
                    theme.fg(theme.warn)
                } else {
                    theme.dim()
                };
                if nh == 2 {
                    put_wrapped(buf, rect.x, ny, text, rect.width, ny + 2, st);
                } else {
                    put(
                        buf,
                        rect.x,
                        ny,
                        &truncate(text, rect.width as usize),
                        rect.width,
                        st,
                    );
                }
                Rect::new(rect.x, rect.y, rect.width, rect.height - nh)
            }
            _ => rect,
        };
        if f.is_table() {
            let label = short_label(*f);
            let tbl = tab.ed.table_mut(*f);
            if let Some(tbl) = tbl
                && let Some(c) = draw_table(
                    buf,
                    rect,
                    theme,
                    tbl,
                    is_sel,
                    label,
                    is_def,
                    &mut app.hits,
                    i,
                )
            {
                cursor = Some(c);
            }
        } else if f.is_multiline() {
            let ed = tab.ed.text_mut(*f);
            if let Some(ed) = ed
                && let Some(c) = ed.render(rect, buf, theme, is_sel, is_def)
            {
                cursor = Some(c);
            }
            app.hits.push((rect, Hit::TextArea { field: i }));
        } else {
            let lst = if is_sel {
                theme.accent().add_modifier(Modifier::BOLD)
            } else {
                theme.dim()
            };
            put(
                buf,
                rect.x,
                y,
                &truncate(short_label(*f), label_w as usize - 1),
                label_w,
                lst,
            );
            let vr = Rect::new(rect.x + label_w, y, rect.width.saturating_sub(label_w), 1);
            if f.is_choice() {
                let label = format!(" {} ▾ ", tab.ed.choice_label(*f));
                let st = if is_sel {
                    theme.selected()
                } else {
                    theme.bar()
                };
                put(
                    buf,
                    vr.x,
                    y,
                    &label,
                    vr.width,
                    st.add_modifier(Modifier::BOLD),
                );
                let hint = choice_hint(*f);
                let hx = vr.x + label.width() as u16 + 1;
                let room = (vr.x + vr.width).saturating_sub(hx);
                put(
                    buf,
                    hx,
                    y,
                    &truncate(hint, room as usize),
                    room,
                    theme.dim(),
                );
                app.hits.push((rect, Hit::ReqField(i)));
            } else if f.is_toggle() {
                let on = tab.ed.choice_label(*f) == "On";
                let label = format!(
                    " [{}] {} ",
                    if on { "x" } else { " " },
                    if on { "On" } else { "Off" }
                );
                let st = if is_sel {
                    theme.selected()
                } else {
                    theme.bar()
                };
                put(
                    buf,
                    vr.x,
                    y,
                    &label,
                    vr.width,
                    st.add_modifier(Modifier::BOLD),
                );
                let hx = vr.x + label.width() as u16 + 1;
                put(
                    buf,
                    hx,
                    y,
                    &truncate(
                        toggle_hint(*f),
                        (vr.x + vr.width).saturating_sub(hx) as usize,
                    ),
                    (vr.x + vr.width).saturating_sub(hx),
                    theme.dim(),
                );
                app.hits.push((rect, Hit::ReqField(i)));
            } else if let Some(ed) = tab.ed.text_mut(*f) {
                fill(buf, vr, theme.bar());
                if let Some(c) = ed.render(vr, buf, theme, is_sel, is_def) {
                    cursor = Some(c);
                }
                app.hits.push((vr, Hit::TextArea { field: i }));
            }
        }
        y += h;
    }
    // Headers tab: show the headers Sendito adds automatically.
    if tab.ed.kind == Kind::Http && tab.req_tab == 1 {
        let auto = tab.ed.auto_headers();
        if !auto.is_empty() {
            let rows = auto.len().min(3) as u16 + 1;
            if let Some(r) = rects.first().copied() {
                let used = tab.ed.headers.rows.len() as u16 + 2;
                if r.height > used + rows {
                    let ay = r.y + r.height - rows;
                    put(
                        buf,
                        r.x,
                        ay,
                        "Added automatically:",
                        r.width,
                        theme.dim().add_modifier(Modifier::ITALIC),
                    );
                    for (j, (n, v)) in auto.iter().take(3).enumerate() {
                        put(
                            buf,
                            r.x + 2,
                            ay + 1 + j as u16,
                            &truncate(&format!("{n}: {v}"), r.width.saturating_sub(2) as usize),
                            r.width.saturating_sub(2),
                            theme.dim(),
                        );
                    }
                }
            }
        }
    }
    app.areas.field_rects = rects;
    cursor
}

fn choice_hint(f: Field) -> &'static str {
    match f {
        Field::BodyKind => "Enter or Space to change",
        Field::AuthKind => "Enter or Space to change · values may use {{variables}}",
        Field::AuthIn => "header or query parameter",
        Field::SchemaSource => "reflection asks the server; proto files are compiled locally",
        Field::ScriptSection => "Enter to switch",
        _ => "",
    }
}

fn toggle_hint(f: Field) -> &'static str {
    match f {
        Field::FollowRedirects => "follow 3xx responses (up to 10)",
        Field::VerifyTls | Field::GrpcVerify => "turn off for self-signed certificates",
        Field::Tls => "or prefix the target with https://",
        _ => "",
    }
}

#[allow(clippy::too_many_arguments)]
fn draw_table(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    t: &mut Table,
    focused: bool,
    label: &str,
    is_def: &dyn Fn(&str) -> bool,
    hits: &mut Vec<(Rect, Hit)>,
    field: usize,
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
    // Header row.
    let mut x = area.x;
    if !label.is_empty() {
        let lw = (label.width() as u16 + 1).min(check_w.max(label.width() as u16 + 1));
        put(
            buf,
            x,
            area.y,
            label,
            lw,
            theme.accent().add_modifier(Modifier::BOLD),
        );
        x += lw;
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
            theme.dim().add_modifier(Modifier::BOLD),
        );
        x += w + 1;
    }
    let body_h = area.height.saturating_sub(1) as usize;
    if focused {
        t.ensure_visible(body_h);
    }
    let total = t.rows.len() + 1;
    let mut cursor = None;
    for vis in 0..body_h {
        let r = t.scroll + vis;
        if r >= total {
            break;
        }
        let y = area.y + 1 + vis as u16;
        let row_sel = focused && r == t.sel_row;
        let is_new = r == t.rows.len();
        let mut x = area.x;
        if t.has_enable {
            if is_new {
                put(buf, x, y, " +  ", check_w, theme.dim());
            } else {
                let on = t.rows[r].enabled;
                put(
                    buf,
                    x,
                    y,
                    if on { "[x] " } else { "[ ] " },
                    check_w,
                    if on { theme.base() } else { theme.dim() },
                );
                hits.push((Rect::new(x, y, 3, 1), Hit::TableToggle { field, row: r }));
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
                    theme.dim().add_modifier(Modifier::ITALIC)
                };
                fill(
                    buf,
                    cell_rect,
                    if cell_sel {
                        theme.selected()
                    } else {
                        theme.base()
                    },
                );
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
                                theme.dim().add_modifier(Modifier::ITALIC),
                            );
                        }
                        // The selected cell stands out in every theme.
                        if cell_sel && !theme.mono {
                            for xx in cell_rect.x..cell_rect.x + cell_rect.width {
                                if let Some(c) = buf.cell_mut((xx, cell_rect.y)) {
                                    c.set_bg(theme.sel_bg);
                                }
                            }
                        }
                        if !enabled {
                            for xx in cell_rect.x..cell_rect.x + cell_rect.width {
                                if let Some(cell) = buf.cell_mut((xx, y)) {
                                    cell.set_style(theme.dim().add_modifier(Modifier::CROSSED_OUT));
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
                                theme.fg(theme.info)
                            },
                        );
                    }
                    (Cell::Toggle(b), _) => {
                        put(
                            buf,
                            x,
                            y,
                            if *b { "[x]" } else { "[ ]" },
                            *w,
                            if cell_sel {
                                theme.selected()
                            } else {
                                theme.base()
                            },
                        );
                    }
                    _ => {}
                }
            }
            hits.push((
                cell_rect,
                Hit::TableCell {
                    field,
                    row: r,
                    col: c,
                },
            ));
            x += w;
            if c + 1 < widths.len() {
                put(buf, x, y, "│", 1, theme.dim());
                x += 1;
            }
        }
        if !is_new {
            let dx = area.x + area.width - del_w;
            put(buf, dx, y, "│", 1, theme.dim());
            put(buf, dx + 1, y, "✕", 1, theme.fg(theme.err));
            hits.push((
                Rect::new(dx, y, del_w, 1),
                Hit::TableDelete { field, row: r },
            ));
        }
    }
    if focused { cursor } else { None }
}

fn draw_response(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    let focused = matches!(app.focus, Focus::RespTabs | Focus::RespBody);
    let tab = &app.tabs[app.active];
    let running = tab
        .running
        .as_ref()
        .map(|r| (r.started.elapsed(), r.streaming_input));
    let summary = tab
        .response
        .as_ref()
        .map(|r| r.summary.clone())
        .unwrap_or_default();
    let blk = block(theme, "Response", focused);
    let inner = blk.inner(area);
    blk.render(area, buf);
    // Summary in the top border.
    let mut spans: Vec<(String, Style)> = Vec::new();
    if let Some((el, streaming)) = running {
        let what = match tab.response.as_ref().and_then(|r| r.grpc_kind) {
            Some(k) if streaming => format!("{} open", k.label()),
            Some(k) => format!("↻ {} running", k.label()),
            None => "↻ sending".to_string(),
        };
        spans.push((
            format!(" {what} {:.1}s ", el.as_secs_f64()),
            theme.fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
    }
    if !summary.status.is_empty() {
        let color = match summary.code {
            Some(c) => theme.status_color(c),
            // A call in progress is not an error.
            None if running.is_some() => theme.accent,
            None if summary.error.as_deref() == Some("cancelled") => theme.warn,
            None if summary.ok => theme.ok,
            None => theme.err,
        };
        let st = if theme.mono {
            Style::default().add_modifier(Modifier::BOLD | Modifier::REVERSED)
        } else {
            Style::default()
                .fg(theme.bg)
                .bg(color)
                .add_modifier(Modifier::BOLD)
        };
        spans.push((format!(" {} ", truncate(&summary.status, 40)), st));
        if let Some(t) = summary.time_ms {
            spans.push((format!(" {} ", crate::tab::fmt_ms(t)), theme.base()));
        }
        if let Some(s) = summary.size {
            // gRPC: the size of the response messages, said explicitly.
            let what = if tab.ed.kind == Kind::Grpc {
                " received"
            } else {
                ""
            };
            spans.push((
                format!(
                    "{}{}{what} ",
                    if summary.time_ms.is_some() { "" } else { " " },
                    human_size(s)
                ),
                theme.dim(),
            ));
        }
    }
    let total: u16 = spans.iter().map(|(s, _)| s.width() as u16).sum();
    let mut x = (area.x + area.width)
        .saturating_sub(total + 2)
        .max(area.x + 12);
    for (s, st) in &spans {
        x = put(
            buf,
            x,
            area.y,
            s,
            (area.x + area.width - 1).saturating_sub(x),
            *st,
        );
    }
    if inner.height < 2 {
        return;
    }
    // Tabs and view controls.
    let ty = inner.y;
    let resp_tab = tab.resp_tab;
    let kind = tab.ed.kind;
    let mut x = inner.x;
    for (i, name) in crate::tab::resp_tab_names(tab.ed.kind).iter().enumerate() {
        let badge = match (kind, i, tab.response.as_ref()) {
            (Kind::Http, 3, Some(r)) => r.results_badge.as_deref(),
            _ => None,
        };
        let label = match badge {
            Some(b) => format!(" {name} {b} "),
            None => format!(" {name} "),
        };
        let w = (label.width() as u16).min((inner.x + inner.width).saturating_sub(x));
        let active = i == resp_tab;
        let st = if active && app.focus == Focus::RespTabs {
            theme.selected().add_modifier(Modifier::BOLD)
        } else if active {
            theme
                .accent()
                .add_modifier(Modifier::BOLD | Modifier::UNDERLINED)
        } else {
            theme.dim()
        };
        put(buf, x, ty, &label, w, st);
        app.hits.push((Rect::new(x, ty, w, 1), Hit::RespTab(i)));
        x += w;
        if i + 1 < 4 && x < inner.x + inner.width {
            x = put(buf, x, ty, "│", 1, theme.dim());
        }
    }
    let has_resp = tab.response.is_some();
    if has_resp && resp_tab == 0 {
        let mode = tab
            .response
            .as_ref()
            .map(|r| r.body.mode)
            .unwrap_or(ViewMode::Raw);
        // "Pretty" only for bodies that have a pretty form (JSON).
        let has_pretty = tab
            .response
            .as_ref()
            .is_some_and(|r| r.body.doc.pretty.is_some());
        let modes: Vec<(ViewMode, &str)> = [
            (ViewMode::Pretty, "Pretty"),
            (ViewMode::Raw, "Raw"),
            (ViewMode::Hex, "Hex"),
        ]
        .into_iter()
        .filter(|(m, _)| has_pretty || *m != ViewMode::Pretty)
        .collect();
        let w: u16 = modes.iter().map(|(_, l)| l.len() as u16 + 2).sum();
        let mut mx = (inner.x + inner.width).saturating_sub(w);
        if mx > x + 1 {
            for (m, l) in modes {
                let lab = format!(" {l} ");
                let st = if m == mode {
                    theme
                        .accent()
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED)
                } else {
                    theme.dim()
                };
                let lw = lab.width() as u16;
                put(buf, mx, ty, &lab, lw, st);
                app.hits.push((Rect::new(mx, ty, lw, 1), Hit::ViewMode(m)));
                mx += lw;
            }
        } else {
            // Too narrow for all three: the active view alone stays visible.
            let name = match mode {
                ViewMode::Pretty => "Pretty",
                ViewMode::Raw => "Raw",
                ViewMode::Hex => "Hex",
            };
            let lab = format!(" {name} ");
            let lw = lab.width() as u16;
            let cx = (inner.x + inner.width).saturating_sub(lw);
            if cx > x + 1 {
                put(
                    buf,
                    cx,
                    ty,
                    &lab,
                    lw,
                    theme
                        .accent()
                        .add_modifier(Modifier::BOLD | Modifier::REVERSED),
                );
                app.hits
                    .push((Rect::new(cx, ty, lw, 1), Hit::ViewMode(mode)));
            }
        }
    }
    let view = Rect::new(inner.x, inner.y + 1, inner.width, inner.height - 1);
    let mut resp_hit = view;
    app.areas.resp_body = view;
    let k = app.send_hint();
    let verb = app.send_verb();
    let tab = &mut app.tabs[app.active];
    match &mut tab.response {
        None => {
            let lines = [
                format!("Press {} to {verb}.", k.replace('/', " or ")),
                String::new(),
                "The response appears here.".to_string(),
            ];
            for (i, l) in lines.iter().enumerate() {
                if (i as u16) < view.height {
                    put(
                        buf,
                        view.x + 1,
                        view.y + i as u16,
                        l,
                        view.width.saturating_sub(1),
                        theme.dim(),
                    );
                }
            }
        }
        Some(r) => {
            let empty =
                r.viewer_mut(resp_tab).doc.is_empty() && r.viewer_mut(resp_tab).doc.raw.is_empty();
            if running.is_some() && r.grpc_events.is_empty() && empty {
                let k = app.keymap.hint(Action::Cancel, app.vim);
                put(
                    buf,
                    view.x + 1,
                    view.y,
                    &format!("Waiting for the response… ({k} cancels)"),
                    view.width.saturating_sub(1),
                    theme.fg(theme.warn),
                );
            } else if empty && resp_tab == 0 {
                let msg = if summary.code.is_some() {
                    "(empty body)"
                } else if summary.error.as_deref() == Some("cancelled") {
                    "Cancelled before a response arrived."
                } else if kind == Kind::Grpc {
                    "(no message yet)"
                } else {
                    "(no response)"
                };
                put(
                    buf,
                    view.x + 1,
                    view.y,
                    msg,
                    view.width.saturating_sub(1),
                    theme.dim(),
                );
            } else {
                // The gRPC Log keeps its legend pinned above the scrolling log.
                let legend = kind == Kind::Grpc && resp_tab == 3 && view.height > 3;
                if legend {
                    put(
                        buf,
                        view.x + 1,
                        view.y,
                        "→ sent  ← received  ■ final status  +s since start",
                        view.width.saturating_sub(1),
                        theme.dim(),
                    );
                }
                let top = u16::from(legend);
                let v = r.viewer_mut(resp_tab);
                let h = view.height.saturating_sub(1 + top);
                let vr = Rect::new(view.x, view.y + top, view.width, h.max(1));
                resp_hit = Rect::new(view.x, view.y + top, view.width, view.height - top);
                v.render(vr, buf, theme, app.focus == Focus::RespBody);
                if h >= 1 {
                    // Rows past the bottom of the view are announced.
                    let more = v.scroll + (vr.height as usize) < v.row_count();
                    let st = format!(
                        " {}{}{} ",
                        v.status(),
                        if v.wrap && resp_tab != 2 {
                            " · wrap on"
                        } else {
                            ""
                        },
                        if more { " · ↓ more" } else { "" }
                    );
                    let sw = st.width() as u16;
                    put(
                        buf,
                        (view.x + view.width).saturating_sub(sw),
                        view.y + view.height - 1,
                        &st,
                        sw,
                        theme.dim(),
                    );
                }
            }
        }
    }
    app.hits.push((resp_hit, Hit::RespView));
}

pub fn human_size(n: u64) -> String {
    crate::app::human_size(n)
}

fn draw_hints(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    fill(buf, area, theme.base());
    let hints = app.hints();
    let mut x = area.x + 1;
    let maxx = area.x + area.width;
    let mut skipped = false;
    for (i, (k, label)) in hints.iter().enumerate() {
        let need = (k.width() + label.width() + 4) as u16;
        // Always keep the last two hints (palette, help) visible, plus room
        // for the "…" that says some hints did not fit.
        let reserve: u16 = if i + 2 < hints.len() {
            hints[hints.len() - 2..]
                .iter()
                .map(|(k, l)| (k.width() + l.width() + 4) as u16)
                .sum::<u16>()
                + 3
        } else {
            0
        };
        if x + need + reserve > maxx && i + 2 < hints.len() {
            skipped = true;
            continue;
        }
        // More keys exist than fit: "F1 all keys" (always last) covers them;
        // in a dialog without it, an ellipsis says so.
        if skipped && i + 2 == hints.len() && hints[i + 1].1 != "all keys" {
            x = put(buf, x, area.y, "…  ", 3, theme.dim());
        }
        x = put(
            buf,
            x,
            area.y,
            k,
            maxx.saturating_sub(x),
            theme.accent().add_modifier(Modifier::BOLD),
        );
        x = put(buf, x, area.y, " ", 1, theme.base());
        x = put(buf, x, area.y, label, maxx.saturating_sub(x), theme.dim());
        x = put(buf, x, area.y, "  ", 2, theme.base());
    }
}

fn draw_status(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) -> Option<(u16, u16)> {
    fill(buf, area, theme.bar());
    let mut x = area.x;
    let maxx = area.x + area.width;
    let env = app.active_env.clone().unwrap_or_else(|| "none".into());
    x = put(
        buf,
        x,
        area.y,
        &format!(" env: {env} "),
        maxx - x,
        theme.bar().add_modifier(Modifier::BOLD),
    );
    let dirty = app.tabs.iter().filter(|t| t.dirty()).count();
    if dirty > 0 {
        x = put(
            buf,
            x,
            area.y,
            &format!("│ ● {dirty} unsaved "),
            maxx.saturating_sub(x),
            theme.bar().patch(theme.fg(theme.warn)),
        );
    }
    // Variables the current request uses but nothing defines.
    if let Some(t) = app.tab() {
        let un = crate::runner::unresolved(&t.model, &app.names);
        if !un.is_empty() {
            let text = match un.as_slice() {
                [one] => format!("│ {{{{{one}}}}} undefined "),
                many => format!("│ {} undefined variables ", many.len()),
            };
            x = put(
                buf,
                x,
                area.y,
                &text,
                maxx.saturating_sub(x),
                theme.bar().patch(theme.fg(theme.err)),
            );
        }
    }
    // While a request runs, say so before the last result.
    if let Some(t) = app.tab()
        && t.running.is_some()
    {
        let streaming = t.running.as_ref().is_some_and(|r| r.streaming_input);
        let text = if streaming {
            "│ stream open ".to_string()
        } else {
            format!(
                "│ sending… ({} cancels) ",
                app.keymap.hint(Action::Cancel, app.vim)
            )
        };
        x = put(
            buf,
            x,
            area.y,
            &text,
            maxx.saturating_sub(x),
            theme.bar().patch(theme.fg(theme.accent)),
        );
    }
    if let Some(t) = app.tab()
        && let Some(r) = &t.response
    {
        let s = &r.summary;
        if !s.status.is_empty() {
            let color = match s.code {
                Some(c) => theme.status_color(c),
                None if t.running.is_some() => theme.accent,
                None if s.error.as_deref() == Some("cancelled") => theme.warn,
                None if s.ok => theme.ok,
                None => theme.err,
            };
            x = put(buf, x, area.y, "│ last request: ", 16, theme.bar());
            x = put(
                buf,
                x,
                area.y,
                &truncate(&s.status, 28),
                maxx.saturating_sub(x),
                theme
                    .bar()
                    .patch(theme.fg(color))
                    .add_modifier(Modifier::BOLD),
            );
            if let Some(ms) = s.time_ms {
                x = put(
                    buf,
                    x,
                    area.y,
                    &format!(" │ {}", crate::tab::fmt_ms(ms)),
                    maxx.saturating_sub(x),
                    theme.bar(),
                );
            }
            if let Some(sz) = s.size {
                x = put(
                    buf,
                    x,
                    area.y,
                    &format!(" │ {}", human_size(sz)),
                    maxx.saturating_sub(x),
                    theme.bar(),
                );
            }
            x = put(buf, x, area.y, " ", 1, theme.bar());
        }
    }
    // Right side: vim mode and message.
    let mut right = String::new();
    if app.vim {
        let mode = vim_mode_label(app);
        right = format!(" -- {mode} -- ");
    }
    let rw = right.width() as u16;
    if rw > 0 && maxx > rw {
        put(
            buf,
            maxx - rw,
            area.y,
            &right,
            rw,
            theme
                .bar()
                .patch(theme.accent())
                .add_modifier(Modifier::BOLD),
        );
    }
    // Confirmations and notes expire; warnings and errors stay until replaced.
    if let Some((_, StatusKind::Info | StatusKind::Ok, at)) = &app.status
        && at.elapsed() > std::time::Duration::from_secs(10)
    {
        app.status = None;
    }
    if let Some((msg, kind, _)) = app.status.clone() {
        let color = match kind {
            StatusKind::Info => theme.info,
            StatusKind::Ok => theme.ok,
            StatusKind::Warn => theme.warn,
            StatusKind::Error => theme.err,
        };
        let room = maxx.saturating_sub(x + rw + 2);
        put(
            buf,
            x + 1,
            area.y,
            &truncate(&msg, room as usize),
            room,
            theme.bar().patch(theme.fg(color)),
        );
    }
    None
}

fn vim_mode_label(app: &mut App) -> &'static str {
    use crate::editor::VimMode;
    let focus = app.focus;
    let Some(tab) = app.tabs.get_mut(app.active) else {
        return "NORMAL";
    };
    let mode = match focus {
        Focus::Bar(1) if tab.ed.kind == Kind::Http => Some(tab.ed.url.vim),
        Focus::Bar(0) if tab.ed.kind == Kind::Grpc => Some(tab.ed.target.vim),
        Focus::ReqBody => match tab.current_field() {
            Some(f) if f.is_table() => tab
                .ed
                .table_mut(f)
                .and_then(|t| t.editor_mut().map(|e| e.vim)),
            Some(f) => tab.ed.text_mut(f).map(|e| e.vim),
            None => None,
        },
        _ => None,
    };
    mode.unwrap_or(VimMode::Normal).label()
}

fn centered(area: Rect, w: u16, h: u16) -> Rect {
    let w = w.min(area.width.saturating_sub(2));
    let h = h.min(area.height.saturating_sub(2));
    Rect::new(
        area.x + (area.width - w) / 2,
        area.y + (area.height - h) / 2,
        w,
        h,
    )
}

/// Clears a dialog's area plus a one-cell margin around it.
fn modal_clear(buf: &mut Buffer, r: Rect, area: Rect, theme: &Theme) {
    let x0 = r.x.saturating_sub(1).max(area.x);
    let y0 = r.y.saturating_sub(1).max(area.y);
    let x1 = (r.x + r.width + 1).min(area.x + area.width);
    // Never into the hint and status rows.
    let y1 = (r.y + r.height + 1)
        .min((area.y + area.height).saturating_sub(2))
        .max(r.y + r.height);
    fill(buf, Rect::new(x0, y0, x1 - x0, y1 - y0), theme.base());
    Clear.render(r, buf);
}

fn button(buf: &mut Buffer, x: u16, y: u16, label: &str, selected: bool, theme: &Theme) -> u16 {
    let text = format!(" {label} ");
    let st = if selected {
        theme.selected().add_modifier(Modifier::BOLD)
    } else {
        theme.bar()
    };
    // A destructive button reads as one.
    let st = if label.starts_with("Delete") || label.starts_with("Discard") {
        st.patch(theme.fg(theme.err)).add_modifier(Modifier::BOLD)
    } else {
        st
    };
    put(buf, x, y, &text, text.width() as u16, st)
}

fn draw_overlay(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) -> Option<(u16, u16)> {
    let mut ov = app.overlay.take()?;
    let mut cursor = None;
    let names = app.names.clone();
    let is_def = move |n: &str| names.is_defined(n);
    let push =
        |hits: &mut Vec<(Rect, Hit)>, r: Rect, h: OverlayHit| hits.push((r, Hit::Overlay(h)));
    push(&mut app.hits, area, OverlayHit::Outside);
    // Dialogs leave the hint and status rows visible: they describe the dialog.
    let body = Rect::new(area.x, area.y, area.width, area.height.saturating_sub(2));
    // Behind a dialog the screen is cleared, so no fragments read as part of
    // it; the hint and status rows stay because they describe the dialog.
    if area.height > 2 {
        fill(
            buf,
            Rect::new(area.x, area.y, area.width, area.height - 2),
            theme.base(),
        );
    }
    match &mut ov {
        Overlay::Palette(p) => {
            let r = centered(body, 100, (area.height * 2 / 3).max(10));
            modal_clear(buf, r, area, theme);
            let blk = block(theme, "Command palette", true);
            let inner = blk.inner(r);
            blk.render(r, buf);
            push(&mut app.hits, r, OverlayHit::Close);
            let ir = Rect::new(inner.x, inner.y, inner.width, 1);
            fill(buf, ir, theme.bar());
            cursor = p.input.render(ir, buf, theme, true, &is_def);
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
                if ir.width > cw + 20 {
                    put(
                        buf,
                        ir.x + ir.width - cw,
                        ir.y,
                        &count,
                        cw,
                        theme.bar().patch(theme.dim()),
                    );
                }
            }
            for (row, idx) in vis.iter().skip(p.scroll).take(lh).enumerate() {
                let y = inner.y + 2 + row as u16;
                let (_, label, detail) = &p.items[*idx];
                let sel = p.armed && row + p.scroll == p.sel;
                let rr = Rect::new(inner.x, y, inner.width, 1);
                fill(buf, rr, if sel { theme.selected() } else { theme.base() });
                let dw = detail.width() as u16;
                // The label gives way to the key: it is cut with an ellipsis.
                let room = inner.width.saturating_sub(dw + 4).max(8);
                let x = put(
                    buf,
                    inner.x + 1,
                    y,
                    &truncate(label, room as usize),
                    room,
                    if sel { theme.selected() } else { theme.base() },
                );
                if x + dw + 2 <= inner.x + inner.width {
                    put(
                        buf,
                        inner.x + inner.width - dw - 1,
                        y,
                        detail,
                        dw,
                        if sel {
                            theme.selected()
                        } else if matches!(p.items[*idx].0, PaletteItem::Action(_)) {
                            theme.accent()
                        } else {
                            theme.dim()
                        },
                    );
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
                    theme.accent(),
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
                    theme.dim(),
                );
            }
            if vis.is_empty() {
                put(
                    buf,
                    inner.x + 1,
                    inner.y + 2,
                    "No match. Commands start with ':' (e.g. :env dev).",
                    inner.width.saturating_sub(2),
                    theme.dim(),
                );
            }
        }
        Overlay::Help { viewer, filter } => {
            let r = centered(body, 110, area.height.saturating_sub(2));
            modal_clear(buf, r, area, theme);
            let blk = block(theme, "Help — keys and actions (Esc closes)", true);
            let inner = blk.inner(r);
            blk.render(r, buf);
            let ir = Rect::new(inner.x, inner.y, inner.width, 1);
            put(buf, ir.x, ir.y, "Search: ", 8, theme.dim());
            let er = Rect::new(ir.x + 8, ir.y, ir.width.saturating_sub(8), 1);
            fill(buf, er, theme.bar());
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
                inner.x,
                inner.y + 1,
                inner.width,
                inner.height.saturating_sub(1),
            );
            viewer.render(vr, buf, theme, true);
            push(&mut app.hits, vr, OverlayHit::Item(0));
        }
        Overlay::Settings { sel } => {
            let r = centered(body, 64, SETTINGS.len() as u16 + 9);
            modal_clear(buf, r, area, theme);
            let blk = block(theme, "Settings", true);
            let inner = blk.inner(r);
            blk.render(r, buf);
            for (i, (id, label)) in SETTINGS.iter().enumerate() {
                let y = inner.y + 1 + i as u16;
                let s = i == *sel;
                let rr = Rect::new(inner.x, y, inner.width, 1);
                fill(buf, rr, if s { theme.selected() } else { theme.base() });
                put(
                    buf,
                    inner.x + 2,
                    y,
                    label,
                    inner.width.saturating_sub(2),
                    if s { theme.selected() } else { theme.base() },
                );
                // A many-valued setting shows where it is among its values.
                let val = app.setting_value(*id);
                let v = match id {
                    SettingId::Theme => {
                        let names = crate::theme::THEME_NAMES;
                        let i = names.iter().position(|n| *n == val).unwrap_or(0);
                        format!("‹ {val} › ({} of {})", i + 1, names.len())
                    }
                    _ => format!("‹ {val} ›"),
                };
                let vw = v.width() as u16;
                put(
                    buf,
                    inner.x + inner.width - vw - 2,
                    y,
                    &v,
                    vw,
                    if s {
                        theme.selected().add_modifier(Modifier::BOLD)
                    } else {
                        theme.accent()
                    },
                );
                push(&mut app.hits, rr, OverlayHit::Item(i));
            }
            let path = app
                .project
                .global_config
                .as_ref()
                .map(|p| crate::app::display_path(p, app.home.as_deref()))
                .unwrap_or_else(|| "(not saved)".into());
            put(
                buf,
                inner.x + 1,
                inner.y + inner.height - 2,
                "Changes apply at once and are saved to",
                inner.width.saturating_sub(2),
                theme.dim(),
            );
            put(
                buf,
                inner.x + 1,
                inner.y + inner.height - 1,
                &truncate_left(&path, inner.width as usize - 2),
                inner.width.saturating_sub(2),
                theme.dim(),
            );
            // What the selected setting does.
            let what = match SETTINGS.get(*sel).map(|(id, _)| *id) {
                Some(SettingId::Vim) => "Vim keys in editors and lists (normal / insert modes).",
                Some(SettingId::Theme) => {
                    "Colors: auto follows the terminal background; ansi uses your terminal palette; contrast is high-contrast."
                }
                Some(SettingId::Sidebar) => "Which side the Requests / History sidebar is on.",
                Some(SettingId::Mouse) => {
                    "Clicks, wheel scrolling, dragging tree items and borders."
                }
                Some(SettingId::ConfirmQuit) => {
                    "Ask before quitting while tabs have unsaved changes."
                }
                Some(SettingId::History) => {
                    "Keep sent requests in History (F7) to reopen or resend."
                }
                None => "",
            };
            put(
                buf,
                inner.x + 1,
                inner.y + inner.height - 5,
                &truncate(what, inner.width as usize - 2),
                inner.width.saturating_sub(2),
                theme.base(),
            );
            // The values the selected setting can take.
            let values = match SETTINGS.get(*sel).map(|(id, _)| *id) {
                Some(SettingId::Theme) => crate::theme::THEME_NAMES.join(" / "),
                Some(SettingId::Sidebar) => "left / right".into(),
                Some(_) => "On / Off".into(),
                None => String::new(),
            };
            put(
                buf,
                inner.x + 1,
                inner.y + inner.height - 4,
                &truncate(
                    &format!(
                        "{}: {values}  (← → / Space)",
                        SETTINGS.get(*sel).map(|(_, l)| *l).unwrap_or("")
                    ),
                    inner.width as usize - 2,
                ),
                inner.width.saturating_sub(2),
                theme.dim(),
            );
        }
        Overlay::Confirm(c) => {
            let lines = wrap_text(&c.message, 56);
            let r = centered(body, 62, lines.len() as u16 + 6);
            modal_clear(buf, r, area, theme);
            let blk = block(theme, &c.title, true);
            let inner = blk.inner(r);
            blk.render(r, buf);
            for (i, l) in lines.iter().enumerate() {
                put(
                    buf,
                    inner.x + 2,
                    inner.y + 1 + i as u16,
                    l,
                    inner.width.saturating_sub(3),
                    theme.base(),
                );
            }
            let by = inner.y + inner.height - 1;
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
            let r = centered(body, 64, h);
            modal_clear(buf, r, area, theme);
            let blk = block(theme, &p.title, true);
            let inner = blk.inner(r);
            blk.render(r, buf);
            let lw = 10;
            put(buf, inner.x + 1, inner.y + 1, &p.label, lw, theme.dim());
            let er = Rect::new(
                inner.x + 1 + lw,
                inner.y + 1,
                inner.width.saturating_sub(lw + 2),
                1,
            );
            fill(buf, er, theme.bar());
            let c = p.input.render(er, buf, theme, p.focus == 0, &is_def);
            if p.focus == 0 {
                cursor = c;
            }
            push(&mut app.hits, er, OverlayHit::Input);
            for (k, (label, opts, i)) in p.choices.iter().enumerate() {
                let y = inner.y + 3 + 2 * k as u16;
                put(buf, inner.x + 1, y, label, lw, theme.dim());
                let v = format!(" ‹ {} › ", opts.get(*i).cloned().unwrap_or_default());
                put(
                    buf,
                    inner.x + 1 + lw,
                    y,
                    &v,
                    inner.width.saturating_sub(lw + 2),
                    if p.focus == k + 1 {
                        theme.selected()
                    } else {
                        theme.bar()
                    },
                );
                push(
                    &mut app.hits,
                    Rect::new(inner.x + 1 + lw, y, v.width() as u16, 1),
                    OverlayHit::Choice(k),
                );
                {
                    let hx = inner.x + 1 + lw + v.width() as u16 + 1;
                    let hint = "← → change · Space list";
                    put(
                        buf,
                        hx,
                        y,
                        hint,
                        (inner.x + inner.width).saturating_sub(hx),
                        theme.dim(),
                    );
                }
            }
            let msg = p
                .error
                .as_ref()
                .map(|e| (e, theme.fg(theme.err)))
                .or_else(|| p.note.as_ref().map(|n| (n, theme.fg(theme.warn))));
            if let Some((m, st)) = msg {
                let y0 = inner.y + inner.height - 4;
                put_wrapped(buf, inner.x + 1, y0, m, inner.width - 2, y0 + 2, st);
            }
            let by = inner.y + inner.height - 1;
            let x0 = inner.x + 1;
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
                let y0 = inner.y + 4 + 2 * k;
                let w = (opts.iter().map(|o| o.width()).max().unwrap_or(10) as u16 + 6)
                    .min(area.width.saturating_sub(inner.x + 1 + lw));
                let room = (area.y + area.height).saturating_sub(y0 + 2);
                let shown = (opts.len() as u16).min(room).max(1);
                let lr = Rect::new(inner.x + 1 + lw, y0, w, shown + 2);
                Clear.render(lr, buf);
                let lb = block(theme, "", true);
                let li = lb.inner(lr);
                lb.render(lr, buf);
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
                        theme.selected()
                    } else {
                        theme.base()
                    };
                    fill(buf, Rect::new(li.x, y, li.width, 1), st);
                    let mark = if j == *cur { "• " } else { "  " };
                    put(buf, li.x, y, &format!("{mark}{o}"), li.width, st);
                }
            }
        }
        Overlay::Picker(p) => {
            let vis = p.visible();
            let has_details = p.details.iter().any(|d| !d.is_empty())
                && !matches!(p.action, PickAction::MoveTo(_));
            let h = (vis.len() as u16 + 5 + u16::from(has_details))
                .clamp(7, area.height.saturating_sub(4));
            let w = p
                .items
                .iter()
                .zip(p.details.iter().chain(std::iter::repeat(&String::new())))
                .map(|(a, b)| a.width() + b.width() + 6)
                .max()
                .unwrap_or(30)
                .clamp(36, 130) as u16;
            let r = centered(body, w, h);
            modal_clear(buf, r, area, theme);
            let blk = block(theme, &p.title, true);
            let inner = blk.inner(r);
            blk.render(r, buf);
            let ir = Rect::new(inner.x, inner.y, inner.width, 1);
            fill(buf, ir, theme.bar());
            cursor = p.filter.render(ir, buf, theme, true, &is_def);
            p.sel = p.sel.min(vis.len().saturating_sub(1));
            let lh = inner.height.saturating_sub(1 + u16::from(has_details)) as usize;
            // The selected item's full detail (rows may cut it short).
            if has_details && let Some(d) = vis.get(p.sel).and_then(|i| p.details.get(*i)) {
                let dy = inner.y + inner.height - 1;
                put(
                    buf,
                    inner.x + 1,
                    dy,
                    &truncate(
                        &format!("Selected: {d}"),
                        inner.width.saturating_sub(2) as usize,
                    ),
                    inner.width.saturating_sub(2),
                    theme.accent(),
                );
            }
            if p.sel < p.scroll {
                p.scroll = p.sel;
            } else if p.sel >= p.scroll + lh {
                p.scroll = p.sel + 1 - lh;
            }
            for (row, idx) in vis.iter().skip(p.scroll).take(lh).enumerate() {
                let y = inner.y + 1 + row as u16;
                let s = row + p.scroll == p.sel;
                let rr = Rect::new(inner.x, y, inner.width, 1);
                fill(buf, rr, if s { theme.selected() } else { theme.base() });
                let x = put(
                    buf,
                    inner.x + 1,
                    y,
                    &p.items[*idx],
                    inner.width.saturating_sub(2),
                    if s { theme.selected() } else { theme.base() },
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
                        if s { theme.selected() } else { theme.dim() },
                    );
                }
                push(&mut app.hits, rr, OverlayHit::Item(row + p.scroll));
            }
            if vis.is_empty() {
                put(
                    buf,
                    inner.x + 1,
                    inner.y + 1,
                    "No match",
                    inner.width.saturating_sub(2),
                    theme.dim(),
                );
            }
        }
        Overlay::Env(e) => {
            let r = centered(body, 110, area.height.saturating_sub(4).max(12));
            modal_clear(buf, r, area, theme);
            // Unsaved edits are marked, as on request tabs.
            let blk = block(
                theme,
                if e.dirty {
                    "Environments ● unsaved"
                } else {
                    "Environments"
                },
                true,
            );
            let inner = blk.inner(r);
            blk.render(r, buf);
            let mut x = inner.x;
            for (i, name) in e.envs.iter().enumerate() {
                let label = if name == sendito_store::env::COLLECTION_ENV {
                    " Collection variables ".to_string()
                } else if app.active_env.as_deref() == Some(name.as_str()) {
                    // The highlight shows the env being edited; this marks the one in use.
                    format!(" {name} (active) ")
                } else {
                    format!(" {name} ")
                };
                let w = (label.width() as u16).min((inner.x + inner.width).saturating_sub(x));
                let st = if i == e.current {
                    theme.selected().add_modifier(Modifier::BOLD)
                } else {
                    theme.dim()
                };
                put(buf, x, inner.y, &label, w, st);
                push(
                    &mut app.hits,
                    Rect::new(x, inner.y, w, 1),
                    OverlayHit::EnvTab(i),
                );
                x += w + 1;
            }
            let legend = "Source says where a value lives (Space changes it). For secrets \
                and keychain, select Secret value and type the value. ✕ deletes a row (Ctrl+D). \
                Ctrl+S saves; Esc asks before discarding unsaved changes.";
            let ly = put_wrapped(
                buf,
                inner.x,
                inner.y + 1,
                legend,
                inner.width,
                inner.y + 6,
                theme.dim(),
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
                inner.y + inner.height - 3,
                &truncate(explain, inner.width as usize),
                inner.width,
                theme.fg(theme.info),
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
            );
            for (rect, h) in tmp_hits {
                if let Hit::TableCell { row, col, .. } = h {
                    push(&mut app.hits, rect, OverlayHit::EnvCell { row, col });
                }
            }
            if let Some(err) = &e.error {
                put(
                    buf,
                    inner.x,
                    inner.y + inner.height - 2,
                    &truncate(err, inner.width as usize),
                    inner.width,
                    theme.fg(theme.err),
                );
            } else if e.dirty {
                put(
                    buf,
                    inner.x,
                    inner.y + inner.height - 2,
                    "● unsaved changes",
                    inner.width,
                    theme.fg(theme.warn),
                );
            }
            let by = inner.y + inner.height - 1;
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
        Overlay::Import(i) => {
            let r = centered(body, 100, 16);
            modal_clear(buf, r, area, theme);
            let blk = block(theme, "Import", true);
            let inner = blk.inner(r);
            blk.render(r, buf);
            put(
                buf,
                inner.x + 1,
                inner.y,
                "Paste a curl command, or enter the path of a Postman v2.1 collection (.json).",
                inner.width.saturating_sub(2),
                theme.dim(),
            );
            put(
                buf,
                inner.x + 1,
                inner.y + 1,
                "Relative paths: the folder where you started Sendito, then the collection.",
                inner.width.saturating_sub(2),
                theme.dim(),
            );
            put(
                buf,
                inner.x + 1,
                inner.y + 2,
                "Enter = new line; F5 / Ctrl+S imports.",
                inner.width.saturating_sub(2),
                theme.dim(),
            );
            let er = Rect::new(
                inner.x + 1,
                inner.y + 3,
                inner.width.saturating_sub(2),
                inner.height.saturating_sub(6),
            );
            fill(buf, er, theme.bar());
            cursor = i.input.render(er, buf, theme, true, &is_def);
            push(&mut app.hits, er, OverlayHit::Input);
            if let Some(e) = &i.error {
                put(
                    buf,
                    inner.x + 1,
                    inner.y + inner.height - 2,
                    &truncate(e, inner.width as usize - 2),
                    inner.width - 2,
                    theme.fg(theme.err),
                );
            }
            let by = inner.y + inner.height - 1;
            let x1 = button(buf, inner.x + 1, by, "Import (F5 / Ctrl+S)", false, theme);
            push(
                &mut app.hits,
                Rect::new(inner.x + 1, by, x1 - inner.x - 1, 1),
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
            let w = area.width.min(100).saturating_sub(6).max(10) as usize;
            let lines: usize = (0..rep.viewer.doc.line_count(ViewMode::Raw))
                .map(|i| wrap_text(&rep.viewer.doc.line_ref(ViewMode::Raw, i), w).len())
                .sum();
            let lines = lines.min(u16::MAX as usize) as u16;
            let r = centered(
                body,
                100,
                (lines + 5).clamp(8, area.height.saturating_sub(2)),
            );
            modal_clear(buf, r, area, theme);
            let blk = block(theme, &rep.title, true);
            let inner = blk.inner(r);
            blk.render(r, buf);
            // A blank row separates the text from the buttons.
            // One column of padding on each side of the text.
            let vr = Rect::new(
                inner.x + 1,
                inner.y,
                inner.width.saturating_sub(2),
                inner.height.saturating_sub(2),
            );
            rep.viewer.wrap = true;
            rep.viewer.render(vr, buf, theme, false);
            let by = inner.y + inner.height - 1;
            let copy = if rep.copied {
                "Copy again (y)"
            } else {
                "Copy (y)"
            };
            let x1 = button(buf, inner.x + 1, by, copy, false, theme);
            push(
                &mut app.hits,
                Rect::new(inner.x + 1, by, x1 - inner.x - 1, 1),
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
                        inner.x + inner.width - pw - 1,
                        by,
                        &pos,
                        pw,
                        theme.dim(),
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

fn wrap_text(s: &str, w: usize) -> Vec<String> {
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

/// Renders a single line of styled spans (used by tests and tools).
pub fn line_text(spans: &[(String, Style)]) -> Line<'static> {
    Line::from(
        spans
            .iter()
            .map(|(s, st)| Span::styled(s.clone(), *st))
            .collect::<Vec<_>>(),
    )
}

pub fn paragraph(text: &str) -> Paragraph<'static> {
    Paragraph::new(text.to_string())
}
