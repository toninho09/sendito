//! The settings overlay.

use super::*;

/// Settings grouped under headings: (heading before this row, row).
pub(super) fn settings_rows() -> Vec<(Option<&'static str>, usize)> {
    SETTINGS
        .iter()
        .enumerate()
        .map(|(i, (id, _))| (id.heading(), i))
        .collect()
}

/// A line of the settings list: a heading or a setting row (its index in
/// `SETTINGS`).
pub(super) enum SettingsLine {
    Head(&'static str),
    Row(usize),
}

/// Settings grouped under headings, flattened in the order they are drawn.
pub(super) fn settings_lines(rows: &[(Option<&'static str>, usize)]) -> Vec<SettingsLine> {
    let mut out = Vec::new();
    for (heading, i) in rows {
        if let Some(h) = heading {
            out.push(SettingsLine::Head(h));
        }
        out.push(SettingsLine::Row(*i));
    }
    out
}

pub(super) fn draw_settings(app: &mut App, buf: &mut Buffer, frame: Frame<'_>, sel: &mut usize) {
    let rows = settings_rows();
    let lines = settings_lines(&rows);
    let last = lines.len() as u16;
    let h = last + 10;
    let (_, inner) = shell(app, buf, frame.body, 70, h, "Settings", frame.theme);
    // The path is pinned to the bottom and the description block sits
    // above it; the list takes what is left and scrolls to keep the
    // selection visible on a short terminal.
    let footer = if inner.height >= 5 { 2 } else { 0 };
    // The description stays while the list keeps room for a few rows: it is
    // where a setting says what it takes.
    let info = if inner.height >= (last + 1).min(8) + footer + 4 {
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
                    frame.theme.bold(frame.theme.dialog_dim()),
                );
                y += 1;
            }
            continue;
        };
        let (id, label) = SETTINGS[*i];
        let s = *i == *sel;
        let rr = Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), 1);
        let base = if s {
            frame.theme.selected()
        } else if hovered(app, rr) {
            frame.text.bg(frame.theme.surface_alt)
        } else {
            frame.text
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
                frame.theme.bold(base)
            } else {
                base.patch(frame.theme.accent())
            },
        );
        // The current theme's colors at a glance.
        if id == SettingId::Theme && !frame.theme.mono {
            let sw = [
                frame.theme.accent,
                frame.theme.ok,
                frame.theme.warn,
                frame.theme.err,
                frame.theme.info,
                frame.theme.patch,
                frame.theme.grpc,
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
                        .bg(base.bg.unwrap_or(frame.theme.overlay_bg)),
                );
            }
        }
        app.hits.push((rr, Hit::Overlay(OverlayHit::Item(*i))));
        app.hits
            .push((Rect::new(vx, y, vw, 1), Hit::Overlay(OverlayHit::Value(*i))));
        y += 1;
    }
    if info > 0 {
        rule(
            buf,
            inner.x + 1,
            y,
            inner.width.saturating_sub(2),
            "",
            frame.theme,
        );
        set_bg(
            buf,
            Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), 1),
            frame.theme.overlay_bg,
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
            Some(SettingId::StickyFolders) => {
                "Keep the folders of the top tree row at the top while the tree scrolls."
                    .to_string()
            }
            Some(SettingId::ConfirmQuit) => {
                "Ask before quitting while tabs have unsaved changes.".to_string()
            }
            Some(SettingId::History) => {
                "Keep sent requests in History (F7) to reopen or resend.".to_string()
            }
            Some(SettingId::Scope) => {
                "global: every collection (~/.config/sendito/config.toml). project: this collection (.sendito/config.toml, shared through git); it wins over global."
                    .to_string()
            }
            Some(SettingId::CookieJar) => {
                "HTTP requests keep and send cookies when their [options] cookies says nothing."
                    .to_string()
            }
            Some(SettingId::Hints) => {
                "Keys in the footer: full shows all that fit, short the most used, off only Ctrl+W and F1."
                    .to_string()
            }
            Some(id) => crate::app::setting_note(id).to_string(),
            None => String::new(),
        };
        let values = match id {
            Some(SettingId::Theme) => app.themes.names().join(" / "),
            Some(SettingId::Icons) => crate::icons::ICON_SETS.join(" / "),
            Some(SettingId::Sidebar) => "left / right".into(),
            Some(SettingId::Hints) => "full / short / off".into(),
            Some(SettingId::Scope) => "global / project".into(),
            Some(id) if id.typed() || id.opens() => String::new(),
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
            frame.theme.overlay(),
        );
        let label = SETTINGS.get(*sel).map(|(_, l)| *l).unwrap_or("");
        let opens = SETTINGS.get(*sel).is_some_and(|(id, _)| id.opens());
        let vline = if opens {
            format!("{label}: Enter opens the editor")
        } else if values.is_empty() {
            format!("{label}: type a value  (Enter)")
        } else {
            format!("{label}: {values}  (← → / Space)")
        };
        put(
            buf,
            inner.x + 2,
            y + 3,
            &truncate(&vline, tw as usize),
            tw,
            frame.theme.dialog_dim(),
        );
    }
    if footer > 0 {
        let path = SETTINGS
            .get(*sel)
            .and_then(|(id, _)| app.setting_path(*id))
            .map(|p| crate::app::display_path(&p, app.home.as_deref()))
            .unwrap_or_else(|| "(not saved)".into());
        let tw = inner.width.saturating_sub(4);
        put(
            buf,
            inner.x + 2,
            inner.y + inner.height.saturating_sub(2),
            "Changes apply at once and are saved to",
            tw,
            frame.theme.dialog_dim(),
        );
        put(
            buf,
            inner.x + 2,
            inner.y + inner.height.saturating_sub(1),
            &truncate_left(&path, tw as usize),
            tw,
            frame.theme.dialog_dim(),
        );
    }
}
