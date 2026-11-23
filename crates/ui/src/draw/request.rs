//! The request pane: URL bar, sub-tabs, fields and tables.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::{Modifier, Style};
use unicode_width::UnicodeWidthStr;

use crate::app::*;
use crate::form::{Cell, ColKind, Table};
use crate::icons::Icons;
use crate::keymap::Action;
use crate::tab::{Field, Kind};
use crate::theme::Theme;

use super::kit::*;

pub(super) fn short_label(f: Field) -> &'static str {
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

/// A note shown under a field: (text, is a warning). `editing`: a text cell
/// of this field is selected, so a path may still be half typed.
pub(super) fn field_note(
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
pub(super) fn auto_value(t: &Table, r: usize, c: usize) -> Option<String> {
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

pub(super) fn table_count(t: &Table) -> usize {
    t.values().len()
}

/// The strip above the panes: method, URL (or gRPC target and method) and
/// the Send button. Returns the cursor when a field there has focus.
pub(super) fn draw_url_strip(
    app: &mut App,
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
) -> Option<(u16, u16)> {
    fill(buf, area, theme.base());
    if area.width < 30 {
        return None;
    }
    let names = app.names.clone();
    let is_def = move |n: &str| names.is_defined(n);
    let focus = app.focus;
    let icons = app.icons;
    let y = area.y;
    let x0 = area.x + 1;
    let right = area.x + area.width - 1;
    let running = app.tabs[app.active].running.is_some();
    let streaming = app.tabs[app.active]
        .running
        .as_ref()
        .is_some_and(|r| r.streaming_input);
    // A gRPC call needs a method first; until then Send is shown disabled.
    let has_method = match &app.tabs[app.active].model {
        sendito_core::Request::Grpc(g) => !g.service.is_empty() && !g.method.is_empty(),
        sendito_core::Request::Ws(w) => !w.url.trim().is_empty(),
        sendito_core::Request::Mcp(m) => !m.server.is_empty() && !m.tool.is_empty(),
        sendito_core::Request::Http(_) => true,
    };
    let send_text = if streaming {
        "Send message"
    } else if running {
        "Sending"
    } else {
        "Send"
    };
    let send_label = format!(" {}{send_text} ", Icons::pre(icons.send));
    let send_w = send_label.width() as u16;
    let send_x = right.saturating_sub(send_w);
    // Cancel sits left of Send: always present, dim when nothing runs.
    let cancel_label = format!(" {}Cancel ", Icons::pre(icons.close));
    let cancel_w = cancel_label.width() as u16;
    let cancel_x = send_x.saturating_sub(cancel_w + 1);
    // The input fields end where Cancel starts.
    let fields_right = cancel_x;
    let mut cursor = None;
    match app.tabs[app.active].ed.kind {
        Kind::Http => {
            let method = app.tabs[app.active].ed.method.clone();
            let ml = format!(" {:<7}▾ ", truncate(&method, 7));
            let mw = ml.width() as u16;
            let mc = theme.method_color(&method);
            let mst = if focus == Focus::Bar(0) {
                theme.chip(mc)
            } else {
                theme.badge(mc)
            };
            put(buf, x0, y, &ml, mw, mst);
            app.hits.push((Rect::new(x0, y, mw, 1), Hit::Bar(0)));
            let ur = Rect::new(x0 + mw + 1, y, fields_right.saturating_sub(x0 + mw + 2), 1);
            let tr = input_box(buf, ur, focus == Focus::Bar(1), theme);
            app.areas.bar_url = tr;
            let tab = &mut app.tabs[app.active];
            if let Some(c) = tab
                .ed
                .url
                .render(tr, buf, theme, focus == Focus::Bar(1), &is_def)
            {
                cursor = Some(c);
            }
            app.hits.push((tr, Hit::Bar(1)));
        }
        Kind::Grpc => {
            // The target input leads the strip; the method chip sits at the
            // right end, by Cancel/Send. Same parts as every kind: 0=chip,
            // 1=input — only the visual order differs.
            let avail = fields_right.saturating_sub(x0 + 1);
            let cw = if avail >= 36 {
                (avail / 3).clamp(16, 44)
            } else {
                16
            };
            let cx = fields_right.saturating_sub(cw);
            let tr = input_box(
                buf,
                Rect::new(x0, y, cx.saturating_sub(x0 + 1), 1),
                focus == Focus::Bar(1),
                theme,
            );
            app.areas.bar_url = tr;
            let tab = &mut app.tabs[app.active];
            if let Some(c) = tab
                .ed
                .target
                .render(tr, buf, theme, focus == Focus::Bar(1), &is_def)
            {
                cursor = Some(c);
            }
            app.hits.push((tr, Hit::Bar(1)));
            // The method chip is editable inline; ▾ opens the schema picker.
            let mr = Rect::new(cx, y, cw.saturating_sub(3), 1);
            let focused = focus == Focus::Bar(0);
            input_box(buf, mr, focused, theme);
            let mut shown = None;
            if focused {
                if let Some(tab) = app.tabs.get_mut(app.active) {
                    shown = tab
                        .ed
                        .grpc_method_text
                        .render(mr, buf, theme, true, &is_def);
                }
            } else {
                let t = &app.tabs[app.active];
                let (svc, m) = (t.ed.service(), t.ed.method());
                let short = if svc.is_empty() {
                    "none".to_string()
                } else {
                    format!("{}/{}", svc.rsplit('.').next().unwrap_or(&svc), m)
                };
                put(
                    buf,
                    mr.x,
                    y,
                    &truncate(&short, mr.width as usize),
                    mr.width,
                    theme.badge(theme.grpc),
                );
            }
            put(buf, cx + cw - 3, y, " ▾", 2, theme.badge(theme.grpc));
            if let Some(c) = shown {
                app.areas.caret = Some(c);
                cursor = Some(c);
            }
            app.hits.push((Rect::new(cx, y, cw, 1), Hit::Bar(0)));
        }
        Kind::Ws => {
            // A static badge where the other kinds have their picker chip.
            let ml = format!(" {:<8}", "WS");
            let mw = ml.width() as u16;
            put(buf, x0, y, &ml, mw, theme.badge(theme.grpc));
            let tr = input_box(
                buf,
                Rect::new(x0 + mw + 1, y, fields_right.saturating_sub(x0 + mw + 2), 1),
                focus == Focus::Bar(1),
                theme,
            );
            app.areas.bar_url = tr;
            let tab = &mut app.tabs[app.active];
            if let Some(c) = tab
                .ed
                .url
                .render(tr, buf, theme, focus == Focus::Bar(1), &is_def)
            {
                cursor = Some(c);
            }
            app.hits.push((tr, Hit::Bar(1)));
        }
        Kind::Mcp => {
            // Like the gRPC strip: the server input leads (a name from the
            // config or a direct http(s):// URL), the tool chip sits at the
            // right end by Cancel/Send. Parts as every kind: 0=chip, 1=input.
            let strip = fields_right.saturating_sub(x0 + 1);
            let cw = if strip >= 36 {
                (strip / 3).clamp(16, 44)
            } else {
                16
            };
            let cx = fields_right.saturating_sub(cw);
            // Where the call goes when the input holds a config name: the HTTP
            // server's URL or the stdio command, dim before the chip. Space is
            // reserved for it first, so the input never swallows it.
            let server = app.tabs[app.active].ed.mcp_server.text();
            let addr = app
                .project
                .config
                .mcp
                .servers
                .get(&server)
                .map(|s| match s {
                    sendito_store::config::McpServer::Stdio(s) => {
                        format!("{} {}", s.command, s.args.join(" "))
                    }
                    sendito_store::config::McpServer::Http(h) => h.url.clone(),
                })
                .unwrap_or_default();
            let avail = cx.saturating_sub(x0 + 1);
            // The address is dropped when it would leave less than 18 columns
            // for the server input.
            let aw = if addr.is_empty() || avail < 40 {
                0
            } else {
                (addr.width() as u16).min(40).min(avail - 20)
            };
            if aw > 0 {
                put(
                    buf,
                    cx - aw - 2,
                    y,
                    &truncate(&addr, aw as usize),
                    aw,
                    theme.dim(),
                );
            }
            let iw = avail.saturating_sub(if aw > 0 { aw + 2 } else { 0 });
            let tr = input_box(buf, Rect::new(x0, y, iw, 1), focus == Focus::Bar(1), theme);
            app.areas.bar_url = tr;
            let tab = &mut app.tabs[app.active];
            if let Some(c) =
                tab.ed
                    .mcp_server
                    .render(tr, buf, theme, focus == Focus::Bar(1), &is_def)
            {
                cursor = Some(c);
            }
            app.hits.push((tr, Hit::Bar(1)));
            // The tool chip is editable inline; the arrow discovers tools.
            let mr = Rect::new(cx, y, cw.saturating_sub(3), 1);
            let focused = focus == Focus::Bar(0);
            input_box(buf, mr, focused, theme);
            let mut shown = None;
            if focused {
                if let Some(tab) = app.tabs.get_mut(app.active) {
                    shown = tab.ed.mcp_tool.render(mr, buf, theme, true, &is_def);
                }
            } else {
                let tool = app.tabs[app.active].ed.mcp_tool.text();
                let short = if tool.is_empty() {
                    "none".to_string()
                } else {
                    tool
                };
                put(
                    buf,
                    mr.x,
                    y,
                    &truncate(&short, mr.width as usize),
                    mr.width,
                    theme.badge(theme.grpc),
                );
            }
            put(buf, cx + cw - 3, y, " ▾", 2, theme.badge(theme.grpc));
            if let Some(c) = shown {
                app.areas.caret = Some(c);
                cursor = Some(c);
            }
            app.hits.push((Rect::new(cx, y, cw, 1), Hit::Bar(0)));
        }
    }
    let send_st = if !has_method {
        theme.bar().patch(theme.dim())
    } else if running && !streaming {
        if focus == Focus::Bar(2) {
            theme.chip(theme.warn)
        } else {
            theme.badge(theme.warn)
        }
    } else {
        button_style(Btn::Primary, focus == Focus::Bar(2), theme)
    };
    let cancel_st = if running {
        theme.badge(theme.warn)
    } else {
        theme.bar().patch(theme.dim())
    };
    put(buf, cancel_x, y, &cancel_label, cancel_w, cancel_st);
    app.hits
        .push((Rect::new(cancel_x, y, cancel_w, 1), Hit::Bar(3)));
    put(buf, send_x, y, &send_label, send_w, send_st);
    app.hits
        .push((Rect::new(send_x, y, send_w, 1), Hit::Bar(2)));
    cursor
}

/// "Folder › name" for a request path ("Folder/name.toml").
fn breadcrumb(path: &str) -> String {
    path.strip_suffix(".toml")
        .unwrap_or(path)
        .split('/')
        .collect::<Vec<_>>()
        .join(" › ")
}

/// Line, column and JSON validity of the selected tab's text editor.
fn editor_status(app: &mut App, theme: &Theme) -> Spans {
    let focused = app.focus == Focus::ReqBody;
    let tab = &mut app.tabs[app.active];
    let current = tab.current_field();
    let Some(f) = tab.fields().into_iter().find(|f| f.is_multiline()) else {
        return Vec::new();
    };
    let Some(ed) = tab.ed.text_mut(f) else {
        return Vec::new();
    };
    let mut out: Spans = Vec::new();
    if ed.highlight == crate::editor::Highlight::Json {
        let text = ed.text();
        if !text.trim().is_empty() && text.len() <= 64 * 1024 {
            // {{variables}} stand for values: check the JSON around them.
            let probe = sendito_core::template::spans(&text).into_iter().rev().fold(
                text.clone(),
                |mut t, (span, _)| {
                    t.replace_range(span, "0");
                    t
                },
            );
            match serde_json::from_str::<serde::de::IgnoredAny>(&probe) {
                Ok(_) => out.push(("✓ JSON".into(), theme.fg(theme.ok))),
                Err(e) => out.push((
                    format!("✗ JSON: line {}, col {}", e.line(), e.column()),
                    theme.fg(theme.err),
                )),
            }
        }
    }
    if focused && current == Some(f) {
        if !out.is_empty() {
            out.push((" · ".into(), theme.dim()));
        }
        out.push((
            format!("Ln {}, Col {}", ed.cursor.row + 1, ed.cursor.col + 1),
            theme.dim(),
        ));
    }
    out
}

pub(super) fn draw_request(
    app: &mut App,
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
) -> Option<(u16, u16)> {
    let focused = matches!(app.focus, Focus::ReqTabs | Focus::ReqBody);
    let inner = frame(buf, area, theme, focused);
    if inner.height < 1 || inner.width < 10 {
        return None;
    }
    let names = app.names.clone();
    let is_def = move |n: &str| names.is_defined(n);
    let vim = app.vim;
    // Sub-tabs in the top border, the file on the right.
    let crumb = {
        let t = &app.tabs[app.active];
        match (&t.path, &t.origin) {
            (Some(p), _) => breadcrumb(p),
            (None, Some(o)) => format!("{o} · {}", t.name),
            (None, None) => format!(
                "{} · no file yet ({} saves it)",
                t.title(),
                app.keymap.hint(Action::Save, app.vim)
            ),
        }
    };
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
            Kind::Ws => vec![
                table_count(&e.headers),
                usize::from(e.message.text().trim() != "[]" && !e.message.text().trim().is_empty()),
                usize::from(e.auth_kind != sendito_core::AuthKind::None),
                table_count(&e.pre) + table_count(&e.extract) + table_count(&e.asserts),
                0,
            ],
            Kind::Mcp => vec![
                0,
                0,
                table_count(&e.pre) + table_count(&e.extract) + table_count(&e.asserts),
                0,
            ],
        };
        (n, c)
    };
    // Tables show their row count ("Headers 2"); Body and Auth show "•"
    // when set. Narrow panes get shorter labels.
    let style_labels = |style: usize| -> Vec<String> {
        names_list
            .iter()
            .enumerate()
            .map(|(i, name)| {
                let set = counts[i] > 0;
                let counted = set && !matches!(*name, "Body" | "Auth");
                match style {
                    0 if counted => format!("{name} {}", counts[i]),
                    1 if counted => format!("{name}{}", counts[i]),
                    _ if set && style < 3 => format!("{name}•"),
                    _ => name.to_string(),
                }
            })
            .collect()
    };
    let max_x = area.x + area.width - 1;
    let tabs_room = area.width.saturating_sub(4);
    let labels = fit_labels((0..4).map(style_labels).collect(), tabs_room);
    let labels_w: u16 = labels.iter().map(|l| l.width() as u16 + 2).sum();
    let look = if app.focus == Focus::ReqTabs {
        TabLook::Strong
    } else if focused {
        TabLook::Soft
    } else {
        TabLook::Quiet
    };
    let req_tab = app.tabs[app.active].req_tab;
    let rects = tab_strip(
        buf,
        area.x + 2,
        area.y,
        max_x,
        &labels,
        req_tab,
        look,
        theme,
    );
    for (i, r) in rects.into_iter().enumerate() {
        app.hits.push((r, Hit::ReqTab(i)));
    }
    // A file path ends with the request's name; a tab without a file leads
    // with its name (or where it came from). Cut each from the other side.
    let no_file = app.tabs[app.active].path.is_none();
    let cut = |s: &str, w: usize| {
        if no_file {
            truncate(s, w)
        } else {
            truncate_left(s, w)
        }
    };
    let crumb_room = area.width.saturating_sub(labels_w + 8);
    let crumb_bottom = crumb_room < 16.min(crumb.width() as u16 + 2);
    if !crumb_bottom {
        let c = format!(" {} ", cut(&crumb, crumb_room as usize - 2));
        let cw = c.width() as u16;
        put(
            buf,
            max_x.saturating_sub(cw + 1),
            area.y,
            &c,
            cw,
            theme.dim(),
        );
    }
    // Content, one column in from the border.
    let content = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    );
    app.areas.req_body = content;
    let cursor = draw_fields(app, buf, content, theme, &is_def, vim);
    // Editor position and JSON check in the bottom border (and the file
    // when the top border had no room for it).
    let status = editor_status(app, theme);
    let by = area.y + area.height - 1;
    let mut sx = max_x;
    if !status.is_empty() {
        let w = spans_width(&status) + 2;
        sx = max_x.saturating_sub(w + 1);
        if sx > area.x + 2 {
            let x = put(buf, sx, by, " ", 1, theme.surface());
            let x = put_spans(buf, x, by, max_x, &status);
            put(buf, x, by, " ", 1, theme.surface());
        }
    }
    if crumb_bottom && area.height > 2 {
        let room = sx.saturating_sub(area.x + 4) as usize;
        if room >= 10 {
            let c = format!(" {} ", cut(&crumb, room - 2));
            put(buf, area.x + 2, by, &c, c.width() as u16, theme.dim());
        }
    }
    if focused { cursor } else { None }
}

pub(super) fn draw_fields(
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
                    app.hover,
                )
            {
                cursor = Some(c);
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
                    theme.chip(theme.accent)
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

pub(super) fn choice_hint(f: Field) -> &'static str {
    match f {
        Field::BodyKind => "Enter or Space to change",
        Field::AuthKind => "Enter or Space to change · values may use {{variables}}",
        Field::AuthIn => "header or query parameter",
        Field::SchemaSource => "reflection asks the server; proto files are compiled locally",
        Field::ScriptSection => "Enter to switch",
        _ => "",
    }
}

pub(super) fn toggle_hint(f: Field) -> &'static str {
    match f {
        Field::FollowRedirects => "follow 3xx responses (up to 10)",
        Field::VerifyTls | Field::GrpcVerify => "turn off for self-signed certificates",
        Field::Tls => "or prefix the target with https://",
        _ => "",
    }
}

#[allow(clippy::too_many_arguments)]
pub(super) fn draw_table(
    buf: &mut Buffer,
    area: Rect,
    theme: &Theme,
    t: &mut Table,
    focused: bool,
    label: &str,
    is_def: &dyn Fn(&str) -> bool,
    hits: &mut Vec<(Rect, Hit)>,
    field: usize,
    hover: Option<(u16, u16)>,
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
    let sep_st = theme.fg(theme.border);
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
        let row_rect = Rect::new(area.x, y, area.width, 1);
        let row_sel = focused && r == t.sel_row;
        let is_new = r == t.rows.len();
        let row_hover =
            hover.is_some_and(|(hx, hy)| hy == y && hx >= area.x && hx < area.x + area.width);
        // Alternate rows are shaded so wide rows stay readable.
        let zebra = r % 2 == 1 && !theme.mono && !is_new;
        let row_bg = if zebra {
            theme.surface_alt
        } else {
            theme.surface
        };
        let base = Style::default().fg(theme.fg).bg(row_bg);
        fill(buf, row_rect, base);
        let mut x = area.x;
        if t.has_enable {
            if is_new {
                put(buf, x, y, " +  ", check_w, theme.accent().bg(row_bg));
            } else {
                let on = t.rows[r].enabled;
                let x2 = put(buf, x, y, "[", 1, base.patch(theme.dim()));
                let x2 = put(
                    buf,
                    x2,
                    y,
                    if on { "✓" } else { " " },
                    1,
                    base.patch(theme.fg(theme.ok)).add_modifier(Modifier::BOLD),
                );
                put(buf, x2, y, "] ", 2, base.patch(theme.dim()));
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
                    theme.dim().add_modifier(Modifier::ITALIC).bg(row_bg)
                };
                if cell_sel {
                    fill(buf, cell_rect, theme.selected());
                }
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
                        // The row's background, then the selected cell on top.
                        set_bg(
                            buf,
                            cell_rect,
                            if cell_sel && !theme.mono {
                                theme.sel_bg
                            } else {
                                row_bg
                            },
                        );
                        if !enabled {
                            for xx in cell_rect.x..cell_rect.x + cell_rect.width {
                                if let Some(cell) = buf.cell_mut((xx, y)) {
                                    cell.set_style(
                                        theme.dim().add_modifier(Modifier::CROSSED_OUT).bg(row_bg),
                                    );
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
                                theme.fg(theme.info).bg(row_bg)
                            },
                        );
                    }
                    (Cell::Toggle(b), _) => {
                        put(
                            buf,
                            x,
                            y,
                            if *b { "[✓]" } else { "[ ]" },
                            *w,
                            if cell_sel { theme.selected() } else { base },
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
                put(buf, x, y, "│", 1, sep_st.bg(row_bg));
                x += 1;
            }
        }
        if !is_new {
            let dx = area.x + area.width - del_w;
            put(buf, dx, y, "│", 1, sep_st.bg(row_bg));
            // The delete button is quiet until its row is selected or pointed at.
            let del_st = if row_sel || row_hover {
                theme.fg(theme.err).add_modifier(Modifier::BOLD)
            } else {
                theme.dim()
            };
            put(buf, dx + 1, y, "✕", 1, del_st.bg(row_bg));
            hits.push((
                Rect::new(dx, y, del_w, 1),
                Hit::TableDelete { field, row: r },
            ));
        }
    }
    if focused { cursor } else { None }
}
