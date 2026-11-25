//! The settings overlay.

use super::*;

/// A line of the settings list: a heading or a setting row (its index in
/// `SETTINGS`).
pub(super) enum SettingsLine {
    Head(&'static str),
    Row(usize),
}

/// The visible settings under their headings, in the order they are drawn.
/// A heading shows when one of its settings matches the filter.
pub(super) fn settings_lines(visible: &[usize]) -> Vec<SettingsLine> {
    let mut out = Vec::new();
    let mut group = None;
    let mut shown = None;
    for (i, (id, _)) in SETTINGS.iter().enumerate() {
        if let Some(h) = id.heading() {
            group = Some(h);
        }
        if !visible.contains(&i) {
            continue;
        }
        if let Some(h) = group
            && shown != Some(h)
        {
            out.push(SettingsLine::Head(h));
            shown = Some(h);
        }
        out.push(SettingsLine::Row(i));
    }
    out
}

/// The keys a config file sets, read once per frame.
fn file_table(path: Option<std::path::PathBuf>) -> Option<toml::Table> {
    let text = std::fs::read_to_string(path?).ok()?;
    toml::from_str(&text).ok()
}

/// Whether `table` sets `section.key`.
fn sets(table: &Option<toml::Table>, section: &str, key: &str) -> bool {
    let Some(mut cur) = table.as_ref() else {
        return false;
    };
    for name in section.split('.') {
        match cur.get(name).and_then(toml::Value::as_table) {
            Some(next) => cur = next,
            None => return false,
        }
    }
    cur.contains_key(key)
}

pub(super) fn draw_settings(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    v: &mut SettingsView,
    cursor: &mut Option<(u16, u16)>,
) {
    let theme = frame.theme;
    let visible = v.visible();
    let lines = settings_lines(&visible);
    let all_lines = settings_lines(&(0..SETTINGS.len()).collect::<Vec<_>>()).len() as u16;
    // The height stays the same while the filter narrows the list.
    let h = all_lines + 12;
    let (_, inner) = shell(app, buf, frame.body, 74, h, "Settings", theme);
    // The file tabs and the filter on top; the path pinned to the bottom
    // and the description above it; the list takes what is left.
    let global = app.config_scope == ConfigScope::Global;
    let tabs = vec!["Global".to_string(), "Project".to_string()];
    let x = put(buf, inner.x + 2, inner.y, "Save to ", 8, theme.dialog_dim());
    let rects = tab_strip(
        buf,
        x,
        inner.y,
        inner.x + inner.width,
        &tabs,
        usize::from(!global),
        TabLook::Strong,
        theme,
    );
    for (i, r) in rects.into_iter().enumerate() {
        app.hits.push((r, Hit::Overlay(OverlayHit::Choice(i))));
    }
    let fr = Rect::new(inner.x + 2, inner.y + 1, inner.width.saturating_sub(4), 1);
    let ftr = input_box(buf, fr, v.edit.is_none(), theme);
    let fc = v
        .filter
        .render(ftr, buf, theme, v.edit.is_none(), &|_: &str| true);
    if v.edit.is_none() {
        *cursor = fc;
    }
    let footer = if inner.height >= 7 { 2 } else { 0 };
    let info = if inner.height >= 14 { 4 } else { 0 };
    let top = inner.y + 3;
    let bottom = (inner.y + inner.height).saturating_sub(footer + info);
    let list_h = bottom.saturating_sub(top) as usize;
    let sel_line = lines
        .iter()
        .position(|l| matches!(l, SettingsLine::Row(i) if *i == v.sel))
        .unwrap_or(0);
    let scroll = sel_line.saturating_sub(list_h.saturating_sub(1));
    // What each file sets: the dot marks a value of the chosen file, and a
    // global value the project file overrides says so.
    let here = file_table(app.scope_path(app.config_scope));
    let project = (global)
        .then(|| file_table(app.scope_path(ConfigScope::Project)))
        .flatten();
    let mut y = top;
    if lines.is_empty() {
        put(
            buf,
            inner.x + 4,
            y,
            "No setting matches the filter.",
            inner.width.saturating_sub(4),
            theme.dialog_dim(),
        );
    }
    for line in lines.iter().skip(scroll).take(list_h) {
        let i = match line {
            SettingsLine::Head(hd) => {
                put(
                    buf,
                    inner.x + 2,
                    y,
                    hd,
                    inner.width.saturating_sub(2),
                    theme.bold(theme.dialog_dim()),
                );
                y += 1;
                continue;
            }
            SettingsLine::Row(i) => *i,
        };
        let (id, label) = SETTINGS[i];
        let s = i == v.sel;
        let rr = Rect::new(inner.x + 1, y, inner.width.saturating_sub(2), 1);
        let base = if s {
            theme.selected()
        } else if hovered(app, rr) {
            frame.text.bg(theme.surface_alt)
        } else {
            frame.text
        };
        fill(buf, rr, base);
        let set_here = id.key().is_some_and(|(sec, k)| sets(&here, sec, k));
        let overridden = id.key().is_some_and(|(sec, k)| sets(&project, sec, k));
        if set_here {
            put(
                buf,
                inner.x + 2,
                y,
                "•",
                1,
                base.patch(theme.fg(theme.accent)),
            );
        }
        // A many-valued setting shows where it is among its values.
        let val = app.setting_value(id);
        let mut chip = match id {
            SettingId::Theme => {
                let names = app.themes.names();
                let n = names.iter().position(|n| *n == val).unwrap_or(0);
                format!("‹ {val} › ({} of {})", n + 1, names.len())
            }
            _ => format!("‹ {val} ›"),
        };
        if overridden {
            chip = format!("project wins  {chip}");
        }
        let editing = s && v.edit.is_some();
        let vw = if editing {
            (inner.width / 2).max(16)
        } else {
            chip.width() as u16
        };
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
        if let (true, Some(ed)) = (editing, v.edit.as_mut()) {
            let er = input_box(buf, Rect::new(vx, y, vw, 1), true, theme);
            *cursor = ed.render(er, buf, theme, true, &|_: &str| true);
        } else {
            put(
                buf,
                vx,
                y,
                &chip,
                vw,
                if s {
                    theme.bold(base)
                } else if overridden {
                    base.patch(theme.fg(theme.warn))
                } else {
                    base.patch(theme.accent())
                },
            );
        }
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
        app.hits.push((rr, Hit::Overlay(OverlayHit::Item(i))));
        app.hits
            .push((Rect::new(vx, y, vw, 1), Hit::Overlay(OverlayHit::Value(i))));
        y += 1;
    }
    let sel = &v.sel;
    let error = v.error.clone();
    // The description sits at the same place whatever the filter shows.
    let y = bottom;
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
            Some(SettingId::HidePlainSecrets) => {
                "Show •••••• in place of a password, token or key typed as plain text in a request (Ctrl+W * shows it). {{variables}} stay visible."
                    .to_string()
            }
            Some(SettingId::ConfirmQuit) => {
                "Ask before quitting while tabs have unsaved changes.".to_string()
            }
            Some(SettingId::History) => {
                "Keep sent requests in History (F7) to reopen or resend.".to_string()
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
            format!("{label}: Enter edits it in place  (• = set in this file)")
        } else {
            format!("{label}: {values}  (← → / Space)")
        };
        let (vline, st) = match &error {
            Some(e) => (e.clone(), frame.theme.on_overlay(frame.theme.err)),
            None => (vline, frame.theme.dialog_dim()),
        };
        put(
            buf,
            inner.x + 2,
            y + 3,
            &truncate(&vline, tw as usize),
            tw,
            st,
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
