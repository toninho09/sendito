//! The screen shown when no request tab is open: the wordmark, what the
//! collection holds, how to start, and the most recent requests.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Style;
use unicode_width::UnicodeWidthStr;

use crate::app::*;
use crate::icons::Icons;
use crate::keymap::Action;
use crate::theme::Theme;

use super::kit::*;

/// "sendito" in a 4×5 pixel font, one string per pixel row.
const WORDMARK: [&str; 5] = [
    ".###.####.#..#.###..###.###..##.",
    "#....#....##.#.#..#..#...#..#..#",
    ".##..###..#.##.#..#..#...#..#..#",
    "...#.#....#..#.#..#..#...#..#..#",
    "###..####.#..#.###..###..#...##.",
];

/// The wordmark as three rows of half blocks.
fn wordmark_rows() -> Vec<String> {
    let px = |r: usize, c: usize| WORDMARK.get(r).and_then(|l| l.as_bytes().get(c)) == Some(&b'#');
    let w = WORDMARK[0].len();
    (0..3)
        .map(|row| {
            (0..w)
                .map(|c| match (px(row * 2, c), px(row * 2 + 1, c)) {
                    (true, true) => '█',
                    (true, false) => '▀',
                    (false, true) => '▄',
                    (false, false) => ' ',
                })
                .collect()
        })
        .collect()
}

pub(super) fn draw_welcome(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    let inner = frame(buf, area, theme, false);
    if inner.width < 20 || inner.height < 4 {
        return;
    }
    let k = |a: Action| app.keymap.hint(a, app.vim);
    let start: Vec<(String, &str)> = vec![
        (k(Action::NewRequest), "New HTTP request tab"),
        ("n / N".into(), "New request / folder in the sidebar"),
        ("Enter".into(), "Open the selected request"),
        (
            k(Action::Palette),
            "Command palette (actions, requests, :commands)",
        ),
        (
            k(Action::Import),
            "Import a Postman collection or a curl command",
        ),
        (k(Action::SelectEnv), "Choose the environment"),
        (
            "m".into(),
            "Move the selected request (or drag it with the mouse)",
        ),
        (
            k(Action::Settings),
            "Settings: vim mode, theme, icons, sidebar side",
        ),
        (k(Action::Help), "All keys"),
        (k(Action::Quit), "Quit"),
    ];
    // What the collection holds.
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
    let mut summary: Spans = vec![
        (app.project.name.clone(), theme.bold(theme.fg(theme.fg))),
        (format!("  ·  {}", plural(reqs, "request")), theme.dim()),
    ];
    if dirs > 0 {
        summary.push((format!(" · {}", plural(dirs, "folder")), theme.dim()));
    }
    summary.push(("  ·  ".into(), theme.dim()));
    match &app.active_env {
        Some(e) => {
            summary.push((format!("{} ", app.icons.env), theme.fg(theme.env_color(e))));
            summary.push((e.clone(), theme.fg(theme.fg)));
        }
        None => summary.push(("no environment".into(), theme.dim())),
    }
    let recent: Vec<(usize, String, String, String)> = app
        .history
        .as_ref()
        .map(|h| {
            h.iter()
                .enumerate()
                .take(6)
                .map(|(i, e)| {
                    (
                        i,
                        e.badge.clone(),
                        e.name.clone(),
                        e.ts.get(11..16).unwrap_or("").to_string(),
                    )
                })
                .collect()
        })
        .unwrap_or_default();
    // Favorites go above Recent, in the same column.
    let favorites: Vec<(String, String, String)> = app
        .tree
        .favorites
        .iter()
        .filter_map(|rel| app.tree.root.find_request(rel))
        .take(6)
        .map(|r| (r.rel.clone(), r.badge.clone(), r.name.clone()))
        .collect();
    // Layout: Recent beside Start on wide screens, below it when there is
    // height for it, and the wordmark only when there is room for everything.
    let col_w_wide: u16 = 68;
    let recent_w_wide: u16 = 40;
    let two_cols = inner.width >= col_w_wide + recent_w_wide + 4 + 4;
    let list_h = start.len() as u16;
    // Title, rows (or the empty line), a blank line and the "all history" line.
    // Favorites: the title, the rows and a blank line.
    let fav_h = if favorites.is_empty() {
        0
    } else {
        favorites.len() as u16 + 2
    };
    let recent_h = fav_h + 1 + (recent.len() as u16).max(1) + 2;
    let small_head: u16 = 1 + 1 + 2;
    let big_head: u16 = 3 + 1 + 1 + 1 + 2;
    let stacked = !two_cols && inner.height >= small_head + 1 + list_h + 1 + recent_h;
    let below_h = if stacked { 1 + recent_h } else { 0 };
    let big = inner.height >= big_head + 1 + list_h + below_h + 4 && inner.width >= 60;
    let head_h = if big { big_head } else { small_head };
    let body_h = head_h + 1 + list_h + below_h;
    let mut y = inner.y + inner.height.saturating_sub(body_h) / 2;
    let center = |w: u16| inner.x + inner.width.saturating_sub(w) / 2;
    if big {
        let rows = wordmark_rows();
        let w = rows[0].width() as u16;
        let x0 = center(w);
        for row in &rows {
            for (i, ch) in row.chars().enumerate() {
                if ch == ' ' {
                    continue;
                }
                let t = i as f32 / w.max(1) as f32;
                let c = theme
                    .mix(theme.accent, theme.grpc, t)
                    .unwrap_or(theme.accent);
                put(
                    buf,
                    x0 + i as u16,
                    y,
                    &ch.to_string(),
                    1,
                    theme.bold(theme.fg(c)),
                );
            }
            y += 1;
        }
        y += 1;
        let tag = "HTTP and gRPC, from the terminal";
        put(
            buf,
            center(tag.width() as u16),
            y,
            tag,
            inner.width,
            theme.dim(),
        );
        y += 1;
    } else {
        let t = "sendito";
        put(
            buf,
            center(t.width() as u16),
            y,
            t,
            inner.width,
            theme.bold(theme.accent()),
        );
        y += 1;
    }
    let sw = spans_width(&summary);
    put_spans(buf, center(sw), y, inner.x + inner.width, &summary);
    y += 2;
    // Start (and Recent beside it on wide screens).
    let key_w: u16 = 14;
    let col_w: u16 = if two_cols {
        col_w_wide
    } else {
        inner.width.saturating_sub(4).min(72)
    };
    let recent_w: u16 = if two_cols { recent_w_wide } else { col_w };
    let total_w = col_w + if two_cols { recent_w + 4 } else { 0 };
    let x0 = center(total_w);
    let bottom = inner.y + inner.height;
    let heading = |buf: &mut Buffer, x: u16, y: u16, text: &str, w: u16| {
        put(buf, x, y, text, w, theme.title_style());
    };
    heading(buf, x0, y, "Start", col_w);
    y += 1;
    let ys = y;
    let kw = start
        .iter()
        .map(|(k, _)| k.width() as u16 + 2)
        .max()
        .unwrap_or(8);
    for (key, desc) in &start {
        if y >= bottom {
            break;
        }
        keycap_w(buf, x0, y, key, kw, theme);
        put(
            buf,
            x0 + key_w,
            y,
            &truncate(desc, col_w.saturating_sub(key_w) as usize),
            col_w.saturating_sub(key_w),
            theme.fg(theme.fg),
        );
        y += 1;
    }
    // The column's first row: its title.
    let (rx, mut y) = if two_cols {
        (x0 + col_w + 4, ys - 1)
    } else if stacked {
        (x0, y + 1)
    } else {
        return;
    };
    if !favorites.is_empty() {
        heading(
            buf,
            rx,
            y,
            &format!("{}Favorites", Icons::pre(app.icons.favorite)),
            recent_w,
        );
        y += 1;
        for (rel, badge, name) in &favorites {
            if y >= bottom {
                break;
            }
            let r = Rect::new(rx, y, recent_w, 1);
            let base = list_row(app, buf, r, theme);
            let mut x = method_cell(buf, rx, y, badge, base, false, theme);
            x = put(buf, x, y, " ", 1, base);
            let w = recent_w.saturating_sub(8);
            put(buf, x, y, &truncate(name, w as usize), w, base);
            app.hits.push((r, Hit::FavoriteRow(rel.clone())));
            y += 1;
        }
        y += 1;
    }
    if y < bottom {
        heading(buf, rx, y, "Recent", recent_w);
    }
    y += 1;
    if recent.is_empty() && y < bottom {
        put(
            buf,
            rx,
            y,
            "Requests you send appear here.",
            recent_w,
            theme.dim(),
        );
        y += 1;
    }
    for (i, badge, name, time) in &recent {
        if y >= bottom {
            break;
        }
        let r = Rect::new(rx, y, recent_w, 1);
        let base = list_row(app, buf, r, theme);
        let mut x = method_cell(buf, rx, y, badge, base, false, theme);
        x = put(buf, x, y, " ", 1, base);
        let tw = time.width() as u16;
        put(
            buf,
            x,
            y,
            &truncate(name, recent_w.saturating_sub(8 + tw) as usize),
            recent_w.saturating_sub(8 + tw),
            base,
        );
        put(
            buf,
            rx + recent_w.saturating_sub(tw),
            y,
            time,
            tw,
            base.patch(theme.dim()),
        );
        app.hits.push((r, Hit::RecentRow(*i)));
        y += 1;
    }
    if y + 1 < bottom {
        y += 1;
        let hk = k(Action::History);
        let x = keycap(buf, rx, y, &hk, rx + recent_w, theme);
        put(buf, x, y, " all history", recent_w, theme.dim());
    }
}

/// Fills a start-screen list row (hover aware) and returns its style.
fn list_row(app: &App, buf: &mut Buffer, r: Rect, theme: &Theme) -> Style {
    let state = if hovered(app, r) {
        RowState::Hover
    } else {
        RowState::Plain
    };
    let base = row_style(state, theme.surface(), theme);
    fill(buf, r, base);
    base
}
