//! The record editors (MCP servers, OAuth identities): the entries in a list
//! on the left, the form of the selected entry on the right, the help line
//! of the focused field, the status line and the buttons.

use super::super::table::{TableHit, draw_table};
use super::*;
use crate::keymap::Action;
use crate::records::{Input, Pane, RecordEditor, RecordKind};

/// The width of the entry list, its separator included.
const LIST_W: u16 = 24;
/// The width of a field label.
const LABEL_W: u16 = 15;

pub(super) fn draw_records(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    e: &mut RecordEditor,
    cursor: &mut Option<(u16, u16)>,
) {
    let theme = frame.theme;
    let scope = if e.path == app.project_config_path() {
        "project config"
    } else {
        "global config"
    };
    let what = match e.kind {
        RecordKind::Mcp => "MCP servers",
        RecordKind::OAuth => "OAuth identities",
    };
    let title = if e.dirty {
        format!("{what} · {scope} {} unsaved", app.icons.dirty)
    } else {
        format!("{what} · {scope}")
    };
    let w = frame.area.width.saturating_sub(4).min(118);
    // As tall as the list or the form needs (a table gets 6 rows), inside
    // the screen.
    let form_rows: usize = e
        .record()
        .map(|r| {
            e.visible()
                .iter()
                .map(|i| match r.fields[*i].input {
                    Input::Table(_) => 6,
                    _ => 1,
                })
                .sum()
        })
        .unwrap_or(1);
    let need = form_rows.max(e.records.len() + 1) as u16 + 8;
    let h = need
        .max(16)
        .min(frame.area.height.saturating_sub(4).max(16));
    let (_, inner) = shell(app, buf, frame.body, w, h, &title, theme);
    let inner = inset(inner, 1, 0);
    if inner.height < 8 || inner.width < LIST_W + 20 {
        return;
    }
    // The file on the first row; the help line, the status line and the
    // buttons on the last three.
    let file = crate::app::display_path(&e.path, app.home.as_deref());
    put(
        buf,
        inner.x,
        inner.y,
        &truncate(
            &format!("{} saves to {file}", app.dialog_key(Action::DialogConfirm)),
            inner.width as usize,
        ),
        inner.width,
        theme.dialog_dim(),
    );
    let top = inner.y + 2;
    let bottom = inner.y + inner.height.saturating_sub(4);
    draw_list(
        app,
        buf,
        frame,
        e,
        Rect::new(inner.x, top, LIST_W - 2, bottom - top),
    );
    for y in top..bottom {
        put(
            buf,
            inner.x + LIST_W - 1,
            y,
            "│",
            1,
            theme.on_overlay(theme.border),
        );
    }
    let form = Rect::new(
        inner.x + LIST_W + 1,
        top,
        inner.width.saturating_sub(LIST_W + 1),
        bottom - top,
    );
    *cursor = draw_form(app, buf, frame, is_def, e, form);
    // The help line: a note, the focused field's help, or what the list does.
    let (help, st) = help_line(app, e, theme);
    put(
        buf,
        inner.x,
        inner.y + inner.height.saturating_sub(3),
        &truncate(&help, inner.width as usize),
        inner.width,
        st,
    );
    dialog::status(buf, inner, e.error.as_deref(), e.dirty, &app.icons, theme);
    let labels = crate::records::buttons(e.kind, |a| app.dialog_key(a));
    let labels: Vec<&str> = labels.iter().map(String::as_str).collect();
    buttons(
        app,
        buf,
        inner,
        &labels,
        None,
        &|i| OverlayHit::RecButton(i),
        theme,
    );
}

fn help_line(app: &App, e: &mut RecordEditor, theme: &Theme) -> (String, Style) {
    if let Some(n) = &e.note {
        return (n.clone(), theme.on_overlay(theme.ok));
    }
    if e.pane == Pane::List {
        let new = app.dialog_key(Action::DialogNew);
        let text = match (e.kind, e.records.is_empty()) {
            (RecordKind::Mcp, true) => format!("No servers yet. {new} adds one."),
            (RecordKind::OAuth, true) => format!("No identities yet. {new} adds one."),
            (RecordKind::Mcp, false) => format!(
                "Enter edits the selected server. {} lists its tools to test it.",
                app.dialog_key(Action::DialogTest)
            ),
            (RecordKind::OAuth, false) => {
                "Enter edits the selected identity. A request's OAuth auth names it.".into()
            }
        };
        return (text, theme.on_overlay(theme.info));
    }
    match e.focused() {
        Some(f) => match &f.input {
            Input::Text(ed) if f.secret && crate::tab::plain_secret(&ed.text()) => (
                "Plain text: the config file keeps it as typed. Keep it in an environment secret and write {{name}}.".into(),
                theme.on_overlay(theme.warn),
            ),
            _ => (f.help.to_string(), theme.on_overlay(theme.info)),
        },
        None => (String::new(), theme.on_overlay(theme.info)),
    }
}

fn draw_list(app: &mut App, buf: &mut Buffer, frame: Frame<'_>, e: &RecordEditor, r: Rect) {
    let theme = frame.theme;
    let n = e.records.len() + 1;
    let first = (e.sel + 1).saturating_sub(r.height as usize);
    for (row, i) in (first..n).take(r.height as usize).enumerate() {
        let y = r.y + row as u16;
        let rr = Rect::new(r.x, y, r.width, 1);
        let sel = i == e.sel;
        let state = match (sel, e.pane) {
            (true, Pane::List) => RowState::Selected,
            (true, Pane::Form) => RowState::SelectedQuiet,
            _ if hovered(app, rr) => RowState::Hover,
            _ => RowState::Plain,
        };
        let base = row_style(state, frame.text, theme);
        fill(buf, rr, base);
        if sel {
            selection_bar(buf, rr, base, theme);
        }
        match e.records.get(i) {
            Some(rec) => {
                let name = rec.name();
                let (label, st) = if name.is_empty() {
                    ("(no name)".to_string(), base.patch(theme.dim()))
                } else {
                    (name, base)
                };
                let kind = match e.kind {
                    RecordKind::Mcp => rec_kind(rec, "kind"),
                    RecordKind::OAuth => String::new(),
                };
                let kw = kind.width() as u16;
                put(
                    buf,
                    r.x + 2,
                    y,
                    &truncate(&label, r.width.saturating_sub(kw + 4) as usize),
                    r.width.saturating_sub(kw + 3),
                    st,
                );
                if kw > 0 {
                    put(
                        buf,
                        r.x + r.width.saturating_sub(kw),
                        y,
                        &kind,
                        kw,
                        base.patch(theme.dim()),
                    );
                }
            }
            None => {
                put(
                    buf,
                    r.x + 2,
                    y,
                    &format!("+ New ({})", app.dialog_key(Action::DialogNew)),
                    r.width.saturating_sub(2),
                    base.patch(theme.fg(theme.accent)),
                );
            }
        }
        app.hits.push((rr, Hit::Overlay(OverlayHit::RecList(i))));
    }
}

/// The value of a choice field, for the list's kind column.
fn rec_kind(rec: &crate::records::Record, id: &str) -> String {
    rec.fields
        .iter()
        .find(|f| f.id == id)
        .and_then(|f| match &f.input {
            Input::Choice { options, sel } => options.get(*sel).map(|s| s.to_string()),
            _ => None,
        })
        .unwrap_or_default()
}

fn draw_form(
    app: &mut App,
    buf: &mut Buffer,
    frame: Frame<'_>,
    is_def: &impl Fn(&str) -> bool,
    e: &mut RecordEditor,
    area: Rect,
) -> Option<(u16, u16)> {
    let theme = frame.theme;
    let vis = e.visible();
    if vis.is_empty() {
        let what = match e.kind {
            RecordKind::Mcp => "a server",
            RecordKind::OAuth => "an identity",
        };
        let new = app.dialog_key(Action::DialogNew);
        let text = format!("Select {what}, or press {new} to add one.");
        put(buf, area.x, area.y, &text, area.width, theme.dialog_dim());
        return None;
    }
    let focus = (e.pane == Pane::Form).then_some(e.field);
    let rec = e.records.get_mut(e.sel)?;
    // One row per field; the tables share what is left.
    let tables = vis
        .iter()
        .filter(|i| matches!(rec.fields[**i].input, Input::Table(_)))
        .count() as u16;
    let fixed = vis.len() as u16 - tables;
    let per_table = area
        .height
        .saturating_sub(fixed)
        .checked_div(tables)
        .unwrap_or(0)
        .max(3);
    let mut cursor = None;
    let mut y = area.y;
    let end = area.y + area.height;
    for (k, &i) in vis.iter().enumerate() {
        if y >= end {
            break;
        }
        let f = &mut rec.fields[i];
        let focused = focus == Some(k);
        let lst = if focused {
            theme.bold(theme.on_overlay(theme.accent))
        } else {
            theme.dialog_dim()
        };
        let vr = Rect::new(area.x + LABEL_W, y, area.width.saturating_sub(LABEL_W), 1);
        match &mut f.input {
            Input::Heading => {
                rule(buf, area.x, y, area.width, f.label, theme);
                set_bg(buf, Rect::new(area.x, y, area.width, 1), theme.overlay_bg);
                y += 1;
            }
            Input::Table(t) => {
                let h = per_table.min(end - y);
                let tr = Rect::new(area.x, y, area.width, h);
                let field = k;
                if let Some(c) = draw_table(
                    buf,
                    tr,
                    theme,
                    t,
                    focused,
                    "",
                    is_def,
                    &mut |r, hit| {
                        let hit = match hit {
                            TableHit::Cell { row, col } => OverlayHit::RecCell { field, row, col },
                            TableHit::Delete { row } => OverlayHit::RecDelete { field, row },
                            TableHit::Toggle { .. } => return,
                        };
                        app.hits.push((r, Hit::Overlay(hit)));
                    },
                    app.hover,
                ) && focused
                {
                    cursor = Some(c);
                }
                y += h;
            }
            Input::Text(ed) => {
                put(buf, area.x, y, f.label, LABEL_W, lst);
                let mut tr = input_box(buf, vr, focused, theme);
                if f.secret && crate::tab::plain_secret(&ed.text()) {
                    let tag = " ⚠ plain text ";
                    let tw = tag.width() as u16;
                    if tr.width > tw + 12 {
                        tr.width -= tw;
                        put(
                            buf,
                            tr.x + tr.width,
                            y,
                            tag,
                            tw,
                            theme.field().patch(theme.fg(theme.warn)),
                        );
                    }
                }
                if let Some(c) = ed.render(tr, buf, theme, focused, is_def)
                    && focused
                {
                    cursor = Some(c);
                }
                app.hits.push((tr, Hit::Overlay(OverlayHit::RecField(k))));
                y += 1;
            }
            Input::Choice { options, sel } => {
                put(buf, area.x, y, f.label, LABEL_W, lst);
                let chosen = |focused: bool| {
                    if focused {
                        theme.bold(theme.chip(theme.accent))
                    } else {
                        theme.bold(theme.bar().patch(theme.fg(theme.accent)))
                    }
                };
                let all: u16 = options.iter().map(|o| o.width() as u16 + 3).sum();
                if all <= vr.width {
                    // Every option as a chip: the chosen one stands out.
                    let mut x = vr.x;
                    for (n, o) in options.iter().enumerate() {
                        let label = format!(" {o} ");
                        let w = label.width() as u16;
                        let st = if n == *sel {
                            chosen(focused)
                        } else {
                            theme.dialog_dim()
                        };
                        put(buf, x, y, &label, w, st);
                        x += w + 1;
                    }
                } else {
                    let label = format!(" ‹ {} › ", options.get(*sel).copied().unwrap_or(""));
                    let x = put(buf, vr.x, y, &label, vr.width, chosen(focused));
                    put(
                        buf,
                        x + 1,
                        y,
                        "← → change",
                        (vr.x + vr.width).saturating_sub(x + 1),
                        theme.dialog_dim(),
                    );
                }
                app.hits.push((vr, Hit::Overlay(OverlayHit::RecField(k))));
                y += 1;
            }
            Input::Toggle(on) => {
                put(buf, area.x, y, f.label, LABEL_W, lst);
                let label = format!(
                    " [{}] {} ",
                    if *on { "✓" } else { " " },
                    if *on { "On" } else { "Off" }
                );
                let st = if focused {
                    theme.chip(theme.accent)
                } else if *on {
                    theme.bar().patch(theme.fg(theme.ok))
                } else {
                    theme.bar()
                };
                put(buf, vr.x, y, &label, vr.width, theme.bold(st));
                app.hits.push((vr, Hit::Overlay(OverlayHit::RecField(k))));
                y += 1;
            }
        }
    }
    cursor
}
