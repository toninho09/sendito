//! The top bar (brand, request tabs, environment), the hint row and the
//! status line.

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
        // " BADGE title ● × "
        (short_method(&t.badge()).width() + labels[i].width() + 6) as u16
            + if t.dirty() { 2 } else { 0 }
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

pub(super) fn draw_hints(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    fill(buf, area, theme.base());
    let hints = app.hints();
    let mut x = area.x + 1;
    let maxx = area.x + area.width;
    let need = |k: &str, l: &str| (k.width() + l.width() + 5) as u16;
    let mut skipped = false;
    for (i, (k, label)) in hints.iter().enumerate() {
        // Always keep the last two hints (palette, help) visible, plus room
        // for the "…" that says some hints did not fit.
        let reserve: u16 = if i + 2 < hints.len() {
            hints[hints.len() - 2..]
                .iter()
                .map(|(k, l)| need(k, l))
                .sum::<u16>()
                + 3
        } else {
            0
        };
        if x + need(k, label) + reserve > maxx && i + 2 < hints.len() {
            skipped = true;
            continue;
        }
        // More keys exist than fit: "F1 all keys" (always last) covers them;
        // in a dialog without it, an ellipsis says so.
        if skipped && i + 2 == hints.len() && hints[i + 1].1 != "all keys" {
            x = put(buf, x, area.y, "…  ", 3, theme.dim());
        }
        x = keycap(buf, x, area.y, k, maxx, theme);
        x = put(buf, x, area.y, " ", 1, theme.base());
        x = put(buf, x, area.y, label, maxx.saturating_sub(x), theme.dim());
        x = put(buf, x, area.y, "  ", 2, theme.base());
    }
}

pub(super) fn draw_status(
    app: &mut App,
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
) -> Option<(u16, u16)> {
    fill(buf, area, theme.bar());
    let icons = app.icons;
    let maxx = area.x + area.width;
    let bar = theme.bar();
    let mut x = area.x;
    let gap = |buf: &mut Buffer, x: u16| put(buf, x, area.y, "  ", 2, bar);
    // Vim mode, as a chip.
    if app.vim {
        let mode = vim_mode_label(app);
        let color = match mode {
            "INSERT" => theme.ok,
            "VISUAL" | "V-LINE" => theme.patch,
            _ => theme.accent,
        };
        x = chip(buf, x, area.y, mode, theme.chip(color), maxx, &icons, theme);
        x = gap(buf, x);
    } else {
        x = put(buf, x, area.y, " ", 1, bar);
    }
    // The collection, as an anchor for the fields after it.
    x = put(
        buf,
        x,
        area.y,
        &truncate(&app.project.name, 24),
        maxx.saturating_sub(x),
        theme.bold(bar.patch(theme.dim())),
    );
    x = gap(buf, x);
    let dirty = app.tabs.iter().filter(|t| t.dirty()).count();
    if dirty > 0 {
        x = put(
            buf,
            x,
            area.y,
            &format!("{} {dirty} unsaved", icons.dirty),
            maxx.saturating_sub(x),
            bar.patch(theme.fg(theme.warn)),
        );
        x = gap(buf, x);
    }
    // Variables the current request uses but nothing defines.
    if let Some(t) = app.tab() {
        let un = crate::runner::unresolved(&t.model, &app.names);
        if !un.is_empty() {
            let text = match un.as_slice() {
                [one] => format!("{{{{{one}}}}} undefined"),
                many => format!("{} undefined variables", many.len()),
            };
            x = put(
                buf,
                x,
                area.y,
                &text,
                maxx.saturating_sub(x),
                bar.patch(theme.fg(theme.err)),
            );
            x = gap(buf, x);
        }
    }
    // While a request runs, say so; a watched tab also shows its interval.
    if let Some(t) = app.tab()
        && let Some(w) = &t.watch
    {
        x = put(
            buf,
            x,
            area.y,
            &format!("\u{21bb} {} s", w.interval.as_secs()),
            maxx.saturating_sub(x),
            theme.bold(bar.patch(theme.fg(theme.accent))),
        );
        x = gap(buf, x);
    }
    if let Some(t) = app.tab()
        && t.running.is_some()
    {
        let streaming = t.running.as_ref().is_some_and(|r| r.streaming_input);
        let text = if streaming {
            "stream open".to_string()
        } else {
            format!(
                "{} sending… ({} cancels)",
                spinner(app.anim_frame),
                app.keymap.hint(Action::Cancel, app.vim)
            )
        };
        x = put(
            buf,
            x,
            area.y,
            &text,
            maxx.saturating_sub(x),
            bar.patch(theme.fg(theme.accent)),
        );
        x = gap(buf, x);
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
            x = put(buf, x, area.y, "last request ", 13, bar.patch(theme.dim()));
            x = put(
                buf,
                x,
                area.y,
                &truncate(&s.status, 28),
                maxx.saturating_sub(x),
                theme.bold(bar.patch(theme.fg(color))),
            );
            let mut extra = String::new();
            if let Some(ms) = s.time_ms {
                extra.push_str(&format!(" · {}", crate::tab::fmt_ms(ms)));
            }
            if let Some(sz) = s.size {
                extra.push_str(&format!(" · {}", human_size(sz)));
            }
            x = put(buf, x, area.y, &extra, maxx.saturating_sub(x), bar);
            x = gap(buf, x);
        }
    }
    // Confirmations and notes expire; warnings and errors stay until replaced.
    if let Some((_, StatusKind::Info | StatusKind::Ok, at)) = &app.status
        && at.elapsed() > std::time::Duration::from_secs(10)
    {
        app.status = None;
    }
    // The message, right-aligned, with an icon for its kind.
    if let Some((msg, kind, _)) = app.status.clone() {
        let (color, icon) = match kind {
            StatusKind::Info => (theme.info, icons.info),
            StatusKind::Ok => (theme.ok, icons.ok),
            StatusKind::Warn => (theme.warn, icons.warn),
            StatusKind::Error => (theme.err, icons.fail),
        };
        let room = maxx.saturating_sub(x + 4) as usize;
        let text = truncate(&msg, room.saturating_sub(3));
        let w = text.width() as u16 + 4;
        let mx = maxx.saturating_sub(w).max(x);
        let mut cx = put(buf, mx, area.y, " ", 1, bar);
        cx = put(
            buf,
            cx,
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
            maxx.saturating_sub(cx),
            bar.patch(theme.fg(color)),
        );
    }
    None
}

pub(super) fn vim_mode_label(app: &mut App) -> &'static str {
    use crate::editor::VimMode;
    app.focused_vim_mode().unwrap_or(VimMode::Normal).label()
}
