//! The top bar (brand, request tabs, environment) and the footer (status
//! segments, messages and key hints).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use sendito_pipeline::human_size;
use unicode_width::UnicodeWidthStr;

use crate::app::*;
use crate::icons::Icons;
use crate::keymap::Action;
use crate::theme::Theme;

use super::kit::*;

pub(super) fn draw_tabbar(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    fill(buf, area, theme.bar());
    let icons = app.icons;
    let maxx = area.x + area.width;
    // Brand.
    let brand = format!(" {}sendito ", Icons::pre(icons.send));
    let mut x = put(
        buf,
        area.x,
        area.y,
        &brand,
        area.width,
        theme.bold(theme.bar().patch(theme.accent())),
    );
    x += 1;
    // Right side: the environment chip and the help key.
    let env = app.active_env.clone();
    let env_name = env.clone().unwrap_or_else(|| "no env".into());
    let env_label = format!(" {} {env_name} ▾ ", icons.env);
    let help_w = " F1 ".width() as u16 + " all keys ".width() as u16;
    let right_w = env_label.width() as u16 + 1 + help_w;
    let tabs_end = maxx.saturating_sub(right_w + 1);
    // Tab labels; the active tab stays visible.
    let labels: Vec<String> = app
        .tabs
        .iter()
        .map(|t| {
            format!(
                "{}{}",
                truncate(&t.title(), 22),
                // A history copy is told apart from the saved request.
                if t.origin.is_some() { " (copy)" } else { "" },
            )
        })
        .collect();
    let tab_w = |i: usize| -> u16 {
        let t = &app.tabs[i];
        // " ◆ BADGE title ● × "
        (short_method(&t.badge()).width() + labels[i].width() + 6) as u16
            + if t.dirty() { 2 } else { 0 }
            + if t.pinned { 2 } else { 0 }
    };
    let mut start = 0;
    while start < app.active {
        let used: u16 = (start..=app.active.min(app.tabs.len().saturating_sub(1)))
            .map(tab_w)
            .sum::<u16>()
            + 4;
        if x + used <= tabs_end {
            break;
        }
        start += 1;
    }
    if start > 0 {
        x = put(buf, x, area.y, "‹ ", 2, theme.bar().patch(theme.dim()));
    }
    for (i, tab) in app.tabs.iter().enumerate().skip(start) {
        let w = tab_w(i);
        if x + w + 4 > tabs_end {
            put(buf, x, area.y, "›", 1, theme.bar().patch(theme.dim()));
            break;
        }
        let active = i == app.active;
        let rect = Rect::new(x, area.y, w, 1);
        let hover = hovered(app, rect);
        let base = if theme.mono {
            if active {
                Style::default().add_modifier(Modifier::REVERSED | Modifier::BOLD)
            } else {
                Style::default()
            }
        } else if active {
            theme.bold(theme.surface())
        } else if hover {
            theme.raised()
        } else {
            theme.bar().patch(theme.dim())
        };
        fill(buf, rect, base);
        let badge = tab.badge();
        let mut cx = put(buf, x, area.y, " ", 1, base);
        if tab.pinned {
            cx = put(buf, cx, area.y, icons.pin, 1, base.patch(theme.accent()));
            cx = put(buf, cx, area.y, " ", 1, base);
        }
        // The method keeps its color over the tab's own background.
        let bst =
            method_style(theme, &badge).patch(Style::default().bg(base.bg.unwrap_or(theme.bar_bg)));
        cx = put(buf, cx, area.y, short_method(&badge), 6, bst);
        cx = put(buf, cx, area.y, " ", 1, base);
        cx = put(buf, cx, area.y, &labels[i], labels[i].width() as u16, base);
        if tab.dirty() {
            cx = put(buf, cx, area.y, " ", 1, base);
            cx = put(
                buf,
                cx,
                area.y,
                icons.dirty,
                1,
                base.patch(theme.fg(theme.warn)),
            );
        }
        cx = put(buf, cx, area.y, " ", 1, base);
        // A spinner while a request runs ("…" only ever means "cut short").
        let close_x = cx;
        let close_hover = hovered(app, Rect::new(close_x, area.y, 1, 1));
        let (close, cst) = if tab.running.is_some() {
            (spinner(app.anim_frame), base.patch(theme.fg(theme.accent)))
        } else if close_hover {
            (icons.close, base.patch(theme.fg(theme.err)))
        } else {
            (
                icons.close,
                base.patch(theme.dim()).remove_modifier(Modifier::BOLD),
            )
        };
        put(buf, close_x, area.y, close, 1, cst);
        app.hits
            .push((Rect::new(x, area.y, close_x - x, 1), Hit::Tab(i)));
        app.hits
            .push((Rect::new(close_x, area.y, 1, 1), Hit::TabClose(i)));
        x += w + 1;
    }
    let px = x;
    let plus_hover = hovered(app, Rect::new(px, area.y, 3, 1));
    x = put(
        buf,
        x,
        area.y,
        " + ",
        3,
        if plus_hover {
            theme.chip(theme.accent)
        } else {
            theme.bold(theme.bar().patch(theme.accent()))
        },
    );
    app.hits
        .push((Rect::new(px, area.y, x - px, 1), Hit::NewTab));
    // Environment chip.
    let ex = maxx - right_w;
    let ew = env_label.width() as u16;
    let env_rect = Rect::new(ex, area.y, ew, 1);
    let est = if hovered(app, env_rect) {
        theme.raised()
    } else {
        theme.bar()
    };
    let dot_color = match &env {
        Some(e) => theme.env_color(e),
        None => theme.dim,
    };
    let mut cx = put(buf, ex, area.y, " ", 1, est);
    cx = put(
        buf,
        cx,
        area.y,
        icons.env,
        1,
        est.patch(theme.fg(dot_color)),
    );
    put(
        buf,
        cx,
        area.y,
        &format!(" {env_name} ▾ "),
        ew.saturating_sub(2),
        if env.is_some() {
            theme.bold(est)
        } else {
            est.patch(theme.dim())
        },
    );
    app.hits.push((env_rect, Hit::EnvSelector));
    let hx = ex + ew + 1;
    let hx2 = keycap(buf, hx, area.y, "F1", maxx, theme);
    put(
        buf,
        hx2,
        area.y,
        " all keys ",
        maxx.saturating_sub(hx2),
        theme.bar().patch(theme.dim()),
    );
    app.hits
        .push((Rect::new(hx, area.y, help_w, 1), Hit::HelpButton));
}

/// The hints the footer shows for the `[ui] hints` level, before they are
/// cut to the width. The main view ends with "menu" and "all keys", which
/// always stay; dialogs keep all their keys whatever the level.
fn hints_for_level(mut hints: Vec<(String, String)>, level: &str) -> Vec<(String, String)> {
    let main_view = hints.last().is_some_and(|(_, l)| l == "all keys");
    if !main_view || hints.len() < 2 {
        return hints;
    }
    let tail = hints.split_off(hints.len() - 2);
    let keep = match level {
        "off" => 0,
        "full" => hints.len(),
        _ => 3,
    };
    hints.truncate(keep);
    hints.extend(tail);
    hints
}

/// Width of one hint ("⏎ send") and of the separator between two.
fn hint_w(k: &str, l: &str) -> u16 {
    (k.width() + 1 + l.width()) as u16
}
const HINT_SEP: &str = " · ";

/// The hints that fit in `room` columns: the last two always stay, the
/// others are dropped from the end.
fn fit_hints(hints: Vec<(String, String)>, room: u16) -> Vec<(String, String)> {
    let total = |h: &[(String, String)]| -> u16 {
        h.iter().map(|(k, l)| hint_w(k, l)).sum::<u16>()
            + (h.len().saturating_sub(1) * HINT_SEP.width()) as u16
    };
    let mut hints = hints;
    while hints.len() > 2 && total(&hints) > room {
        hints.remove(hints.len() - 3);
    }
    hints
}

/// The footer: status segments on the left, the message and the key hints
/// on the right.
pub(super) fn draw_footer(
    app: &mut App,
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
) -> Option<(u16, u16)> {
    let bar = theme.bar();
    fill(buf, area, bar);
    let icons = app.icons;
    let maxx = area.x + area.width;
    let message = app.status.clone();
    // The status segments: (text, style). The vim mode has its own color.
    let mut segs: Vec<(String, Style)> = Vec::new();
    if app.vim {
        let mode = vim_mode_label(app);
        let color = match mode {
            "INSERT" => theme.ok,
            "VISUAL" | "V-LINE" => theme.patch,
            _ => theme.accent,
        };
        segs.push((mode.to_string(), theme.chip(color)));
    }
    let seg_bg = theme.raised();
    // The collection, as an anchor for the fields after it.
    segs.push((
        truncate(&app.project.name, 24),
        seg_bg.patch(theme.bold(theme.dim())),
    ));
    let dirty = app.tabs.iter().filter(|t| t.dirty()).count();
    if dirty > 0 {
        segs.push((
            format!("{} {dirty} unsaved", icons.dirty),
            seg_bg.patch(theme.fg(theme.warn)),
        ));
    }
    // Variables the current request uses but nothing defines.
    if let Some(t) = app.tab() {
        let (req, names) = app.effective(t);
        let un = crate::runner::unresolved(&req, &names);
        if !un.is_empty() {
            let text = match un.as_slice() {
                [one] => format!("{{{{{one}}}}} undefined"),
                many => format!("{} undefined variables", many.len()),
            };
            segs.push((text, seg_bg.patch(theme.fg(theme.err))));
        }
    }
    // While a request runs, say so; a watched tab also shows its interval.
    if let Some(t) = app.tab()
        && let Some(w) = &t.watch
    {
        segs.push((
            format!("\u{21bb} {} s", w.interval.as_secs()),
            seg_bg.patch(theme.bold(theme.fg(theme.accent))),
        ));
    }
    if let Some(t) = app.tab()
        && let Some(r) = &t.running
    {
        let text = if r.streaming_input {
            "stream open".to_string()
        } else {
            format!(
                "{} sending… ({} cancels)",
                spinner(app.anim_frame),
                crate::keymap::short_keys(&app.keymap.hint(Action::Cancel, app.vim))
            )
        };
        segs.push((text, seg_bg.patch(theme.fg(theme.accent))));
    }
    // The last request's result, when the response pane is hidden.
    let response_hidden = app.areas.response.height == 0;
    if response_hidden
        && let Some(t) = app.tab()
        && let Some(r) = &t.response
    {
        let s = &r.summary;
        if !s.status.is_empty() {
            let color = result_color(
                theme,
                s.code,
                s.ok,
                s.error.as_deref() == Some("cancelled"),
                t.running.is_some(),
            );
            let mut text = format!("last request {}", truncate(&s.status, 28));
            if let Some(ms) = s.time_ms {
                text.push_str(&format!(" · {}", crate::tab::fmt_ms(ms)));
            }
            if let Some(sz) = s.size {
                text.push_str(&format!(" · {}", human_size(sz)));
            }
            segs.push((text, seg_bg.patch(theme.bold(theme.fg(color)))));
        }
    }
    // A chip is its text, a cell on each side and a gap after it.
    let segs_w: u16 = segs.iter().map(|(t, _)| t.width() as u16 + 3).sum();
    // The hints get the room the segments leave.
    let mut hints: Vec<(String, String)> =
        hints_for_level(app.hints(), &app.project.config.ui.hints)
            .into_iter()
            .map(|(k, l)| (crate::keymap::short_keys(&k), l))
            .collect();
    let free = area.width.saturating_sub(segs_w + 3);
    // Room kept for the message: its width, at most half of the free room.
    let mut reserve = 0;
    if let Some((msg, kind, _)) = &message
        && app.overlay.is_none()
        && app.complete.is_none()
    {
        // A note takes the place of the hints in the main view; a warning or
        // an error keeps the hints that fit beside it. Dialogs and the
        // completion popup keep all their hints.
        if matches!(kind, StatusKind::Info | StatusKind::Ok) {
            hints.clear();
        } else {
            reserve = (msg.width() as u16 + 3).min(free / 2);
        }
    }
    let hints = fit_hints(hints, free.saturating_sub(reserve));
    let hints_w = hints.iter().map(|(k, l)| hint_w(k, l)).sum::<u16>()
        + (hints.len().saturating_sub(1) * HINT_SEP.width()) as u16;
    let hints_x = maxx.saturating_sub(hints_w + 1);
    let mut hx = hints_x;
    for (i, (k, l)) in hints.iter().enumerate() {
        if i > 0 {
            hx = put(
                buf,
                hx,
                area.y,
                HINT_SEP,
                maxx.saturating_sub(hx),
                bar.patch(theme.dim()),
            );
        }
        hx = put(buf, hx, area.y, k, maxx.saturating_sub(hx), theme.bold(bar));
        hx = put(buf, hx, area.y, " ", 1, bar);
        hx = put(
            buf,
            hx,
            area.y,
            l,
            maxx.saturating_sub(hx),
            bar.patch(theme.dim()),
        );
    }
    let segx = hints_x.saturating_sub(2);
    let mut x = area.x;
    for (text, st) in &segs {
        if x >= segx {
            break;
        }
        x = chip(buf, x, area.y, text, *st, segx, &icons, theme);
        x = put(buf, x, area.y, " ", 1, bar);
    }
    // The message, right-aligned before the hints, with an icon for its kind.
    if let Some((msg, kind, _)) = message {
        let (color, icon) = match kind {
            StatusKind::Info => (theme.info, icons.info),
            StatusKind::Ok => (theme.ok, icons.ok),
            StatusKind::Warn => (theme.warn, icons.warn),
            StatusKind::Error => (theme.err, icons.fail),
        };
        let end = if hints.is_empty() { maxx } else { segx };
        let room = end.saturating_sub(x + 3) as usize;
        let text = truncate(&msg, room);
        let w = text.width() as u16 + 3;
        let mx = end.saturating_sub(w).max(x);
        let mut cx = put(
            buf,
            mx,
            area.y,
            icon,
            1,
            theme.bold(bar.patch(theme.fg(color))),
        );
        cx = put(buf, cx, area.y, " ", 1, bar);
        put(
            buf,
            cx,
            area.y,
            &text,
            end.saturating_sub(cx),
            bar.patch(theme.fg(color)),
        );
    }
    None
}

pub(super) fn vim_mode_label(app: &mut App) -> &'static str {
    use crate::editor::VimMode;
    app.focused_vim_mode().unwrap_or(VimMode::Normal).label()
}
