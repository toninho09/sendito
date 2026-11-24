//! The field list of a request sub-tab and its notes and hints.

use super::*;

pub(in crate::draw) fn short_label(f: Field) -> &'static str {
    match f {
        Field::BodyKind => "Body type",
        Field::RawContentType | Field::FileContentType => "Content-Type",
        Field::FilePath => "File",
        Field::AuthKind => "Auth type",
        Field::AuthUser => "Username",
        Field::AuthPass => "Password",
        Field::AuthToken => "Token",
        Field::AuthOAuthName => "OAuth identity",
        Field::Cookies => "Cookie jar",
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
        Field::McpServer => "Server",
        Field::McpTool => "Tool",
        Field::WsSubprotocols => "Subprotocols",
        Field::WsMessages => "Messages sent after connecting",
        Field::ScriptPre | Field::ScriptExtract | Field::ScriptAssert => "",
        _ => "",
    }
}

/// The size of the file at `path`, `None` when it is not a file. A result is
/// kept for two seconds: the note is drawn on every frame, and a file on a
/// slow mount must not hold up each one.
pub(super) fn file_size(path: &std::path::Path) -> Option<u64> {
    use std::collections::HashMap;
    use std::time::{Duration, Instant};
    thread_local! {
        static SEEN: std::cell::RefCell<HashMap<std::path::PathBuf, (Instant, Option<u64>)>> =
            std::cell::RefCell::new(HashMap::new());
    }
    SEEN.with(|seen| {
        let mut seen = seen.borrow_mut();
        if let Some((at, size)) = seen.get(path)
            && at.elapsed() < Duration::from_secs(2)
        {
            return *size;
        }
        let size = std::fs::metadata(path)
            .ok()
            .filter(|m| m.is_file())
            .map(|m| m.len());
        if seen.len() > 256 {
            seen.clear();
        }
        seen.insert(path.to_path_buf(), (Instant::now(), size));
        size
    })
}

/// A note shown under a field: (text, is a warning). `editing`: a text cell
/// of this field is selected, so a path may still be half typed.
pub(in crate::draw) fn field_note(
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
                                && file_size(&sendito_store::env::expand(root, path)).is_none() =>
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
                            file_size(&sendito_store::env::expand(root, path))
                                .map(|len| (path.clone(), len))
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
pub(in crate::draw) fn auto_value(t: &Table, r: usize, c: usize) -> Option<String> {
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
            sendito_pipeline::http::guess_mime(&filename)
        }
    ))
}

pub(in crate::draw) fn table_count(t: &Table) -> usize {
    t.values().len()
}

pub(in crate::draw) fn draw_fields(
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
            if let Some(tbl) = tbl {
                let field = i;
                if let Some(c) = draw_table(
                    buf,
                    rect,
                    theme,
                    tbl,
                    is_sel,
                    label,
                    is_def,
                    &mut |r, h| {
                        let hit = match h {
                            TableHit::Cell { row, col } => Hit::TableCell { field, row, col },
                            TableHit::Toggle { row } => Hit::TableToggle { field, row },
                            TableHit::Delete { row } => Hit::TableDelete { field, row },
                        };
                        app.hits.push((r, hit));
                    },
                    app.hover,
                ) {
                    cursor = Some(c);
                }
            }
        } else if f.is_multiline() {
            let ed = tab.ed.text_mut(*f);
            if let Some(ed) = ed {
                ed.gutter = true;
                if let Some(c) = ed.render(rect, buf, theme, is_sel, is_def) {
                    app.areas.caret = Some(c);
                    cursor = Some(c);
                }
            }
            app.hits.push((rect, Hit::TextArea { field: i }));
        } else {
            let lst = if is_sel {
                theme.bold(theme.accent())
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
            let chips_w: u16 = BodyKind::ALL
                .iter()
                .map(|k| k.short_label().width() as u16 + 3)
                .sum();
            if *f == Field::BodyKind && chips_w <= vr.width {
                // Every body type as a chip: the current one stands out.
                let mut x = vr.x;
                for (n, k) in BodyKind::ALL.iter().enumerate() {
                    let label = format!(" {} ", k.short_label());
                    let w = label.width() as u16;
                    let st = match (*k == tab.ed.body_kind, is_sel) {
                        (true, true) => theme.bold(theme.chip(theme.accent)),
                        (true, false) => theme.bold(theme.bar().patch(theme.accent())),
                        _ => theme.dim(),
                    };
                    put(buf, x, y, &label, w, st);
                    app.hits.push((
                        Rect::new(x, y, w, 1),
                        Hit::ChoiceOption { field: i, index: n },
                    ));
                    x += w + 1;
                }
                // The Content-Type that goes on the wire, on the right.
                // A narrow pane drops the source, then the "application/".
                if let Some((ct, from)) = body_content_type(tab) {
                    let short = ct.strip_prefix("application/").unwrap_or(&ct);
                    let end = vr.x + vr.width;
                    let fit = [
                        format!("→ {ct} ({from})"),
                        format!("→ {ct}"),
                        format!("→ {short}"),
                    ]
                    .into_iter()
                    .find(|t| end >= x + t.width() as u16);
                    if let Some(t) = fit {
                        let tw = t.width() as u16;
                        put(buf, end - tw, y, &t, tw, theme.dim());
                    }
                }
            } else if f.is_choice() {
                let label = format!(" {} ▾ ", tab.ed.choice_label(*f));
                let st = if is_sel {
                    theme.chip(theme.accent)
                } else {
                    theme.bar()
                };
                put(buf, vr.x, y, &label, vr.width, theme.bold(st));
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
                    if on { "✓" } else { " " },
                    if on { "On" } else { "Off" }
                );
                let st = if is_sel {
                    theme.chip(theme.accent)
                } else if on {
                    theme.bar().patch(theme.fg(theme.ok))
                } else {
                    theme.bar()
                };
                put(buf, vr.x, y, &label, vr.width, theme.bold(st));
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
                let tr = input_box(buf, vr, is_sel, theme);
                if let Some(c) = ed.render(tr, buf, theme, is_sel, is_def) {
                    app.areas.caret = Some(c);
                    cursor = Some(c);
                }
                app.hits.push((tr, Hit::TextArea { field: i }));
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
                    rule(buf, r.x, ay, r.width, "Added automatically", theme);
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

/// The Content-Type the body adds on the wire and where it comes from: the
/// body type itself ("auto") or a Content-Type row of the Headers tab.
pub(super) fn body_content_type(tab: &Tab) -> Option<(String, &'static str)> {
    let kind = tab.ed.body_kind;
    let auto = kind.content_type()?;
    if kind != BodyKind::Multipart
        && let sendito_core::Request::Http(r) = &tab.model
        && let Some(h) = r
            .headers
            .iter()
            .find(|h| h.enabled && h.name.trim().eq_ignore_ascii_case("content-type"))
    {
        return Some((h.value.clone(), "Headers"));
    }
    Some((auto.to_string(), "auto"))
}

pub(in crate::draw) fn choice_hint(f: Field) -> &'static str {
    match f {
        Field::BodyKind => "Enter or Space to change",
        Field::AuthKind => "Enter or Space to change · values may use {{variables}}",
        Field::AuthIn => "header or query parameter",
        Field::SchemaSource => "reflection asks the server; proto files are compiled locally",
        Field::ScriptSection => "Enter to switch",
        _ => "",
    }
}

pub(in crate::draw) fn toggle_hint(f: Field) -> &'static str {
    match f {
        Field::FollowRedirects => "follow 3xx responses (up to 10)",
        Field::VerifyTls | Field::GrpcVerify => "turn off for self-signed certificates",
        Field::Tls => "or prefix the target with https://",
        _ => "",
    }
}
