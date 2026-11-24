//! Dialogs, pickers, the palette, help and settings. The screen behind a
//! dialog is faded (not cleared) and every dialog casts a shadow.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use crate::app::*;
use crate::config_editor::CfgEditor;
use crate::editor::TextEditor;
use crate::overlay::*;
use crate::theme::Theme;
use crate::viewer::{ViewMode, Viewer};

use super::dialog;
use super::dialog::{buttons, inset, popup, shell, shell_at};
use super::editor::{self, EditorDialog, EditorHits};
use super::kit::*;

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
        .map(|(i, (id, _))| (id.heading(), i))
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

/// Where the environment editor's clicks go.
struct EnvHits;

impl EditorHits for EnvHits {
    fn tab(&self, i: usize) -> OverlayHit {
        OverlayHit::EnvTab(i)
    }
    fn cell(&self, row: usize, col: usize) -> OverlayHit {
        OverlayHit::EnvCell { row, col }
    }
    fn delete(&self, row: usize) -> OverlayHit {
        OverlayHit::EnvDelete(row)
    }
    fn button(&self, i: usize) -> OverlayHit {
        OverlayHit::Button(i)
    }
}

/// Where the MCP servers editor's clicks go.
struct McpHits;

impl EditorHits for McpHits {
    fn tab(&self, i: usize) -> OverlayHit {
        OverlayHit::Item(i)
    }
    fn cell(&self, row: usize, col: usize) -> OverlayHit {
        OverlayHit::McpCell { row, col }
    }
    fn delete(&self, row: usize) -> OverlayHit {
        OverlayHit::McpDelete(row)
    }
    fn button(&self, i: usize) -> OverlayHit {
        OverlayHit::McpButton(i)
    }
}

/// Where a config editor's clicks go.
struct CfgHits;

impl EditorHits for CfgHits {
    fn tab(&self, i: usize) -> OverlayHit {
        OverlayHit::CfgTab(i)
    }
    fn cell(&self, row: usize, col: usize) -> OverlayHit {
        OverlayHit::CfgCell { row, col }
    }
    fn delete(&self, row: usize) -> OverlayHit {
        OverlayHit::CfgDelete(row)
    }
    fn button(&self, i: usize) -> OverlayHit {
        OverlayHit::CfgButton(i)
    }
}

/// The screen a dialog is drawn on: the whole overlay `area`, the `body` the
/// dialog and its shadow stay in (the area without the footer), the theme
/// and the style of a line of dialog text.
#[derive(Clone, Copy)]
struct Frame<'a> {
    area: Rect,
    body: Rect,
    theme: &'a Theme,
    text: Style,
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
    app.hits.push((area, Hit::Overlay(OverlayHit::Outside)));
    // Dialogs leave the footer as it is: its hints describe the dialog. The
    // rest of the screen fades behind it.
    let body = Rect::new(
        area.x,
        area.y,
        area.width,
        area.height.saturating_sub(super::FOOTER_ROWS),
    );
    backdrop(buf, body, theme);
    let frame = Frame {
        area,
        body,
        theme,
        text: theme.overlay(),
    };
    match &mut ov {
        Overlay::Palette(p) => draw_palette(app, buf, frame, &is_def, p, &mut cursor),
        Overlay::Help { viewer, filter } => {
            draw_help(app, buf, frame, &is_def, viewer, filter, &mut cursor)
        }
        Overlay::Settings { sel } => draw_settings(app, buf, frame, sel),
        Overlay::Confirm(c) => draw_confirm(app, buf, frame, c),
        Overlay::Prompt(p) => draw_prompt(app, buf, frame, &is_def, p, &mut cursor),
        Overlay::Picker(p) => draw_picker(app, buf, frame, &is_def, p, &mut cursor),
        Overlay::Env(e) => draw_env(app, buf, frame, &is_def, e, &mut cursor),
        Overlay::Mcp(m) => draw_mcp(app, buf, frame, &is_def, m, &mut cursor),
        Overlay::Cfg(e) => draw_cfg(app, buf, frame, &is_def, e, &mut cursor),
        Overlay::Import(i) => draw_import(app, buf, frame, &is_def, i, &mut cursor),
        Overlay::Report(rep) => draw_report(app, buf, frame, rep),
    }
    app.overlay = Some(ov);
    cursor
}

fn draw_palette(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    p: &mut Palette,
    cursor: &mut Option<(u16, u16)>,
) {
    let h = (frame.area.height * 2 / 3)
        .max(10)
        .min(frame.body.height.saturating_sub(2));
    let w = 100.min(frame.body.width.saturating_sub(4));
    // Near the top, where the eye already is.
    let top = frame.body.y + (frame.body.height.saturating_sub(h) / 3).max(1);
    let r = Rect::new(frame.body.x + (frame.body.width - w) / 2, top, w, h);
    let (r, inner) = shell_at(app, buf, r, frame.body, "Command palette", frame.theme);
    let (tr, c) = dialog::input(app, buf, inner, &mut p.input, is_def, frame.theme);
    *cursor = c;
    let vis = p.visible();
    p.sel = p.sel.min(vis.len().saturating_sub(1));
    let lh = inner.height.saturating_sub(2) as usize;
    p.scroll = crate::list::keep_visible(p.sel, p.scroll, lh);
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
                frame.theme.bar().patch(frame.theme.dim()),
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
        let plain = frame.text;
        let base = row_style(
            if sel {
                RowState::Selected
            } else if hovered(app, rr) {
                RowState::Hover
            } else {
                RowState::Plain
            },
            plain,
            frame.theme,
        );
        fill(buf, rr, base);
        if sel {
            selection_bar(buf, rr, base, frame.theme);
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
            let c = frame.theme.method_color(m);
            put(
                buf,
                mx,
                y,
                m,
                m.width() as u16,
                frame.theme.bold(base.patch(frame.theme.fg(if sel {
                    frame.theme.on_selection(c)
                } else {
                    c
                }))),
            );
        }
        // The typed letters, marked in the label.
        for ci in match_positions(&shown, query) {
            let cx = lx + shown.chars().take(ci).collect::<String>().width() as u16;
            if let Some(c) = buf.cell_mut((cx, y)) {
                let fg = if sel {
                    frame.theme.on_selection(frame.theme.accent)
                } else {
                    frame.theme.accent
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
                (false, PaletteItem::Action(_)) => base.patch(frame.theme.accent()),
                (false, _) => base.patch(frame.theme.dim()),
            };
            put(buf, dx, y, detail, dw, st);
        }
        app.hits
            .push((rr, Hit::Overlay(OverlayHit::Item(row + p.scroll))));
    }
    // Rows above and below the visible window, said in the frame.
    let below = vis.len().saturating_sub(p.scroll + lh);
    if below > 0 {
        border_count(buf, r, r.y + r.height - 1, below, false, frame.theme);
    }
    if p.scroll > 0 {
        border_count(buf, r, inner.y + 1, p.scroll, true, frame.theme);
    }
    scrollbar(buf, r, p.scroll, vis.len(), lh, true, frame.theme);
    if vis.is_empty() {
        put(
            buf,
            inner.x + 2,
            inner.y + 2,
            "No match. Commands start with ':' (e.g. :env dev).",
            inner.width.saturating_sub(3),
            frame.theme.dialog_dim(),
        );
    }
}

fn draw_help(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    viewer: &mut Viewer,
    filter: &mut TextEditor,
    cursor: &mut Option<(u16, u16)>,
) {
    // Wide screens read the keys in two columns, like a newspaper.
    let two_cols = frame.area.width >= 180;
    let mw = if two_cols {
        frame.area.width.saturating_sub(8).min(200)
    } else {
        110
    };
    let (r, inner) = shell(
        app,
        buf,
        frame.body,
        mw,
        frame.area.height.saturating_sub(2),
        "Help — keys and actions (Esc closes)",
        frame.theme,
    );
    let ir = Rect::new(inner.x + 1, inner.y, inner.width.saturating_sub(2), 1);
    put(buf, ir.x, ir.y, "Search ", 7, frame.theme.dialog_dim());
    let er = input_box(
        buf,
        Rect::new(ir.x + 7, ir.y, ir.width.saturating_sub(7), 1),
        true,
        frame.theme,
    );
    app.hits.push((er, Hit::Overlay(OverlayHit::Input)));
    *cursor = filter.render(er, buf, frame.theme, true, is_def);
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
            frame.theme.bar().patch(frame.theme.dim()),
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
        let (left, right) = help_columns(buf, vr, viewer, frame.theme);
        app.hits.push((left, Hit::Overlay(OverlayHit::Item(0))));
        app.hits
            .push((right, Hit::Overlay(OverlayHit::Item(left.height as usize))));
        left.height as usize * 2
    } else {
        viewer.render(vr, buf, frame.theme, true);
        app.hits.push((vr, Hit::Overlay(OverlayHit::Item(0))));
        vr.height as usize
    };
    set_bg_unset(buf, vr, frame.theme);
    scrollbar(
        buf,
        r,
        viewer.scroll,
        viewer.row_count(),
        shown,
        true,
        frame.theme,
    );
}

fn draw_settings(app: &mut App, buf: &mut Buffer, frame: Frame<'_>, sel: &mut usize) {
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

fn draw_confirm(app: &mut App, buf: &mut Buffer, frame: Frame<'_>, c: &mut Confirm) {
    let lines = wrap_text(&c.message, 56);
    let (_, inner) = shell(
        app,
        buf,
        frame.body,
        64,
        lines.len() as u16 + 6,
        &c.title,
        frame.theme,
    );
    // Leave the button row for the buttons on a short terminal.
    let room = inner.height.saturating_sub(2) as usize;
    for (i, l) in lines.iter().take(room).enumerate() {
        put(
            buf,
            inner.x + 2,
            inner.y + 1 + i as u16,
            l,
            inner.width.saturating_sub(3),
            frame.text,
        );
    }
    // Every button names its key, as in the other dialogs.
    let keys: &[&str] = if c.buttons.len() == 3 {
        &["s", "d", "Esc"]
    } else {
        &["y", "Esc"]
    };
    let labels: Vec<String> = c
        .buttons
        .iter()
        .enumerate()
        .map(|(i, b)| match keys.get(i) {
            Some(k) => format!("{b} ({k})"),
            None => b.clone(),
        })
        .collect();
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    buttons(
        app,
        buf,
        inset(inner, 2, 0),
        &labels,
        Some(c.sel),
        &|i| OverlayHit::Button(i),
        frame.theme,
    );
}

fn draw_prompt(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    p: &mut Prompt,
    cursor: &mut Option<(u16, u16)>,
) {
    let has_msg = p.error.is_some() || p.note.is_some();
    let h = 7 + 2 * p.choices.len() as u16 + if has_msg { 2 } else { 0 };
    let (_, inner) = shell(app, buf, frame.body, 66, h, &p.title, frame.theme);
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
            frame.theme.dialog_dim(),
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
            frame.theme,
        );
        let c = p.input.render(er, buf, frame.theme, p.focus == 0, is_def);
        if p.focus == 0 {
            *cursor = c;
        }
        app.hits.push((er, Hit::Overlay(OverlayHit::Input)));
    }
    let mut choices_end = inner.y + 1;
    for (k, (label, opts, i)) in p.choices.iter().enumerate() {
        let y = first_choice + step * k as u16;
        if y >= by {
            break;
        }
        put(buf, inner.x + 2, y, label, lw, frame.theme.dialog_dim());
        let v = format!(" ‹ {} › ", opts.get(*i).cloned().unwrap_or_default());
        put(
            buf,
            inner.x + 2 + lw,
            y,
            &v,
            inner.width.saturating_sub(lw + 4),
            if p.focus == k + 1 {
                frame.theme.chip(frame.theme.accent)
            } else {
                frame.theme.bar()
            },
        );
        app.hits.push((
            Rect::new(inner.x + 2 + lw, y, v.width() as u16, 1),
            Hit::Overlay(OverlayHit::Choice(k)),
        ));
        let hx = inner.x + 2 + lw + v.width() as u16 + 1;
        put(
            buf,
            hx,
            y,
            "← → change · Space list",
            (inner.x + inner.width).saturating_sub(hx),
            frame.theme.dialog_dim(),
        );
        choices_end = y;
    }
    let msg = p
        .error
        .as_ref()
        .map(|e| (e, frame.theme.fg(frame.theme.err)))
        .or_else(|| {
            p.note
                .as_ref()
                .map(|n| (n, frame.theme.fg(frame.theme.warn)))
        });
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
                st.bg(frame.theme.overlay_bg),
            );
        }
    }
    // The confirm button says what it does.
    let verb = crate::app::prompt_verb(&p.action);
    let ok = format!("{}{} (Enter)", verb[..1].to_uppercase(), &verb[1..]);
    buttons(
        app,
        buf,
        inset(inner, 2, 0),
        &[&ok, "Cancel (Esc)"],
        None,
        &|i| OverlayHit::Button(i),
        frame.theme,
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
            .min(frame.area.width.saturating_sub(inner.x + 2 + lw));
        let room = (frame.area.y + frame.area.height).saturating_sub(y0 + 2);
        let shown = (opts.len() as u16).min(room);
        if shown > 0 && w >= 4 && y0 < frame.area.y + frame.area.height {
            let lr = Rect::new(inner.x + 2 + lw, y0, w, shown + 2);
            let li = popup(buf, lr, frame.body, frame.text, frame.theme);
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
                    frame.theme.selected()
                } else {
                    frame.text
                };
                fill(buf, Rect::new(li.x, y, li.width, 1), st);
                let mark = if j == *cur { "✓ " } else { "  " };
                put(buf, li.x, y, &format!("{mark}{o}"), li.width, st);
                app.hits.push((
                    Rect::new(li.x, y, li.width, 1),
                    Hit::Overlay(OverlayHit::PromptOption(j)),
                ));
            }
        }
    }
}

fn draw_picker(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    p: &mut Picker,
    cursor: &mut Option<(u16, u16)>,
) {
    let vis = p.visible();
    let has_details =
        p.details.iter().any(|d| !d.is_empty()) && !matches!(p.action, PickAction::MoveTo(_));
    let h = (vis.len() as u16 + 6 + u16::from(has_details))
        .min(frame.area.height.saturating_sub(4))
        .max(4);
    let w = p
        .items
        .iter()
        .zip(p.details.iter().chain(std::iter::repeat(&String::new())))
        .map(|(a, b)| a.width() + b.width() + 8)
        .max()
        .unwrap_or(30)
        .clamp(38, 130) as u16;
    let (r, inner) = shell(app, buf, frame.body, w, h, &p.title, frame.theme);
    let (_tr, c) = dialog::input(app, buf, inner, &mut p.filter, is_def, frame.theme);
    *cursor = c;
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
            frame.theme.dialog_accent(),
        );
    }
    p.scroll = crate::list::keep_visible(p.sel, p.scroll, lh);
    let methods = matches!(p.action, PickAction::Method);
    let envs = matches!(p.action, PickAction::Env);
    for (row, idx) in vis.iter().skip(p.scroll).take(lh).enumerate() {
        let y = inner.y + 2 + row as u16;
        let s = row + p.scroll == p.sel;
        let rr = Rect::new(inner.x, y, inner.width, 1);
        let base = row_style(
            if s {
                RowState::Selected
            } else if hovered(app, rr) {
                RowState::Hover
            } else {
                RowState::Plain
            },
            frame.text,
            frame.theme,
        );
        fill(buf, rr, base);
        if s {
            selection_bar(buf, rr, base, frame.theme);
        }
        let item = &p.items[*idx];
        let mut x = inner.x + 2;
        // Methods in their colors; environments with their marker.
        let item_st = if methods {
            let c = frame.theme.method_color(item);
            frame.theme.bold(base.patch(frame.theme.fg(if s {
                frame.theme.on_selection(c)
            } else {
                c
            })))
        } else {
            base
        };
        if envs {
            let real = !item.starts_with('(') && !item.is_empty();
            let c = if real {
                frame.theme.env_color(item)
            } else {
                frame.theme.dim
            };
            x = put(
                buf,
                x,
                y,
                &format!("{} ", app.icons.env),
                2,
                base.patch(
                    frame
                        .theme
                        .fg(if s { frame.theme.on_selection(c) } else { c }),
                ),
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
                if s {
                    base
                } else {
                    base.patch(frame.theme.dim())
                },
            );
        }
        app.hits
            .push((rr, Hit::Overlay(OverlayHit::Item(row + p.scroll))));
    }
    scrollbar(buf, r, p.scroll, vis.len(), lh, true, frame.theme);
    if vis.is_empty() {
        put(
            buf,
            inner.x + 2,
            inner.y + 2,
            "No match",
            inner.width.saturating_sub(3),
            frame.theme.dialog_dim(),
        );
    }
}

fn draw_env(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    e: &mut EnvEditor,
    cursor: &mut Option<(u16, u16)>,
) {
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
    let legend = "Source says where a value lives (Space changes it). For secrets \
            and keychain, select Secret value and type the value. ✕ deletes a row (Ctrl+D). \
            Ctrl+S saves; Esc asks before discarding unsaved changes.";
    *cursor = editor::render(
        app,
        buf,
        frame.body,
        frame.theme,
        EditorDialog {
            title: "Environments",
            width: 112,
            height: frame.area.height.saturating_sub(4).max(12),
            tabs: labels,
            tab_sel: e.current,
            reserved: 4,
            legend,
            explain,
            error: e.error.as_deref(),
            dirty: e.dirty,
            buttons: &["Save (Ctrl+S)", "New env (Ctrl+N)", "Close (Esc)"],
            is_def,
        },
        &mut e.table,
        &EnvHits,
    );
}

fn draw_mcp(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    m: &mut McpEditor,
    cursor: &mut Option<(u16, u16)>,
) {
    let explain = match m.table.rows.get(m.table.sel_row) {
        Some(r) => match (r.cells.get(1).map(|c| c.choice()), m.table.sel_col) {
            (Some(1), _) => "stdio: started as a child process; the value is the command.",
            (_, 3) => {
                "Auth: none, bearer (the Token column) or oauth (sign in when the server asks)."
            }
            (_, 4) => {
                "Token: the bearer token; write {{name}} and keep the value in an environment secret."
            }
            _ => "http: a URL the tool calls go to (variables allowed).",
        },
        None => "Type a name in the last row to add a server.",
    };
    let legend = "Each row is a [mcp.servers] entry: a name, the kind and the auth (Space \
            changes them), the URL or command, and a bearer token. Typing in the last row adds \
            one; ✕ deletes a row (Ctrl+D). Ctrl+O opens the selected server's env (stdio) or \
            headers, TLS check and OAuth fields (http). Ctrl+S saves to .sendito/config.toml; \
            Esc asks before discarding unsaved changes.";
    *cursor = editor::render(
        app,
        buf,
        frame.body,
        frame.theme,
        EditorDialog {
            title: "MCP servers",
            width: 108,
            // The frame grows with the list: a few servers get a short
            // dialog, a long list scrolls inside the screen height.
            height: (m.table.rows.len() as u16 + 13)
                .min(frame.area.height.saturating_sub(4).max(12)),
            tabs: Vec::new(),
            tab_sel: 0,
            reserved: 3,
            legend,
            explain,
            error: m.error.as_deref(),
            dirty: m.dirty,
            buttons: &[
                "Save (Ctrl+S)",
                "Add row (Ctrl+N)",
                "Details (Ctrl+O)",
                "Close (Esc)",
            ],
            is_def,
        },
        &mut m.table,
        &McpHits,
    );
}

fn draw_cfg(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    e: &mut CfgEditor,
    cursor: &mut Option<(u16, u16)>,
) {
    use crate::config_editor::CfgKind;
    let file = crate::app::display_path(&e.path, app.home.as_deref());
    let (title, legend) = match &e.kind {
        CfgKind::McpServer { name, http: true } => (
            format!("MCP server {name}"),
            format!(
                "Options: the TLS check and, for auth = oauth, the OAuth overrides (empty = \
                 discovery). Headers: sent with every POST. Ctrl+PgUp/PgDn switches tabs. \
                 Ctrl+S saves to {file}; Esc returns to the servers."
            ),
        ),
        CfgKind::McpServer { name, .. } => (
            format!("MCP server {name}"),
            format!(
                "Variables added to the child process's environment. Typing in the last row \
                 adds one; ✕ deletes a row (Ctrl+D). Ctrl+S saves to {file}; Esc returns to \
                 the servers."
            ),
        ),
        CfgKind::OAuth => (
            "OAuth identities".to_string(),
            format!(
                "Each row is an [oauth.<name>] identity (authorization code + PKCE) that a \
                 request's auth names. Typing in the last row adds one; ✕ deletes a row \
                 (Ctrl+D). Ctrl+S saves to {file} (Settings chooses the file); Esc asks \
                 before discarding unsaved changes."
            ),
        ),
        CfgKind::Keymap => (
            "Key bindings".to_string(),
            format!(
                "Each row replaces the keys of one action: all applies in both modes, \
                 conventional and vim in one. Keys look like ctrl+s, f5 or \"ctrl+w x\" (a \
                 sequence). Ctrl+PgUp/PgDn switches tabs. Ctrl+S saves to {file} (Settings \
                 chooses the file)."
            ),
        ),
    };
    let explain = crate::config_editor::cfg_explain(e);
    let rows = e.tables.get(e.current).map_or(0, |t| t.rows.len()) as u16;
    let current = e.current;
    let tabs = e.tabs.clone();
    let Some(table) = e.tables.get_mut(current) else {
        return;
    };
    *cursor = editor::render(
        app,
        buf,
        frame.body,
        frame.theme,
        EditorDialog {
            title: &title,
            width: 112,
            height: (rows + 14).min(frame.area.height.saturating_sub(4).max(12)),
            tabs,
            tab_sel: current,
            reserved: 3,
            legend: &legend,
            explain: &explain,
            error: e.error.as_deref(),
            dirty: e.dirty,
            buttons: &["Save (Ctrl+S)", "Add row (Ctrl+N)", "Close (Esc)"],
            is_def,
        },
        table,
        &CfgHits,
    );
}

fn draw_import(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    i: &mut Import,
    cursor: &mut Option<(u16, u16)>,
) {
    let (_, inner) = shell(app, buf, frame.body, 100, 17, "Import", frame.theme);
    let tw = inner.width.saturating_sub(4);
    let note = frame.theme.dialog_dim();
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
    let tr = input_box(buf, er, true, frame.theme);
    *cursor = i.input.render(tr, buf, frame.theme, true, is_def);
    app.hits.push((tr, Hit::Overlay(OverlayHit::Input)));
    if let Some(e) = &i.error {
        put(
            buf,
            inner.x + 2,
            inner.y + inner.height.saturating_sub(2),
            &truncate(e, tw as usize),
            tw,
            frame.theme.on_overlay(frame.theme.err),
        );
    }
    buttons(
        app,
        buf,
        inset(inner, 2, 0),
        &["Import (F5 / Ctrl+S)", "Cancel (Esc)"],
        None,
        &|i| OverlayHit::Button(i),
        frame.theme,
    );
}

fn draw_report(app: &mut App, buf: &mut Buffer, frame: Frame<'_>, rep: &mut Report) {
    let w = frame.area.width.min(100).saturating_sub(8).max(10) as usize;
    // Rows as the wrapped viewer draws them (it breaks long words too).
    let lines: usize = (0..rep.viewer.doc.line_count(ViewMode::Raw))
        .map(|i| {
            let mut rows = 1;
            sendito_pipeline::doc::wrap_breaks(
                &rep.viewer.doc.line_ref(ViewMode::Raw, i),
                w,
                |_| rows += 1,
            );
            rows
        })
        .sum();
    let lines = lines.min(u16::MAX as usize) as u16;
    let (r, inner) = shell(
        app,
        buf,
        frame.body,
        100,
        (lines.saturating_add(6))
            .min(frame.area.height.saturating_sub(2))
            .max(4),
        &rep.title,
        frame.theme,
    );
    // Padding around the text; a blank row separates it from the buttons.
    let vr = Rect::new(
        inner.x + 2,
        inner.y + 1,
        inner.width.saturating_sub(4),
        inner.height.saturating_sub(3),
    );
    rep.viewer.wrap = true;
    rep.viewer.render(vr, buf, frame.theme, false);
    set_bg_unset(buf, vr, frame.theme);
    scrollbar(
        buf,
        r,
        rep.viewer.scroll,
        rep.viewer.row_count(),
        vr.height as usize,
        true,
        frame.theme,
    );
    let by = inner.y + inner.height.saturating_sub(1);
    let copy = if rep.copied {
        "Copy again (y)"
    } else {
        "Copy (y)"
    };
    let bar = inset(inner, 2, 0);
    let x1 = button(buf, bar.x, by, copy, false, frame.theme);
    app.hits.push((
        Rect::new(bar.x, by, x1 - bar.x, 1),
        Hit::Overlay(OverlayHit::Button(0)),
    ));
    let x2 = button(buf, x1 + 2, by, "Close (Esc)", false, frame.theme);
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
                frame.theme.dialog_dim(),
            );
        }
    }
    app.hits.push((
        Rect::new(x1 + 2, by, x2 - x1 - 2, 1),
        Hit::Overlay(OverlayHit::Button(1)),
    ));
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
        put(buf, rx, y, "│", 1, theme.on_overlay(theme.border));
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
