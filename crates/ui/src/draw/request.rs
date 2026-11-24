//! The request pane: URL bar, sub-tabs, fields and tables.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use sendito_pipeline::human_size;
use unicode_width::UnicodeWidthStr;

use crate::app::*;
use crate::form::Table;
use crate::keymap::Action;
use crate::tab::{Field, Kind, Tab};
use crate::theme::Theme;
use sendito_core::BodyKind;

use super::kit::*;
use super::table::{TableHit, draw_table};

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

/// The card's border and title: the protocol in the colour of the kind (the
/// method for HTTP), `●` while unsaved, the watch interval on the right.
/// Returns the content row.
fn draw_url_card(app: &App, buf: &mut Buffer, area: Rect, theme: &Theme) -> Rect {
    let tab = &app.tabs[app.active];
    let (kind, color) = match tab.ed.kind {
        // The chip already names the method: the title names the protocol,
        // the border keeps the method's colour.
        Kind::Http => ("HTTP".to_string(), theme.method_color(&tab.ed.method)),
        Kind::Grpc => ("GRPC".to_string(), theme.grpc),
        Kind::Ws => ("WS".to_string(), theme.grpc),
        Kind::Mcp => ("MCP".to_string(), theme.grpc),
    };
    let focused = matches!(app.focus, Focus::Bar(_));
    let border = if focused || theme.mono {
        theme.border(focused)
    } else {
        theme.fg(color)
    };
    let inner = frame_styled(buf, area, theme, focused, theme.surface(), border);
    let bg = theme.surface();
    let mut x = put(buf, area.x + 1, area.y, " ", 1, bg);
    x = put(
        buf,
        x,
        area.y,
        &kind,
        kind.width() as u16,
        bg.patch(theme.bold(theme.fg(color))),
    );
    if tab.dirty() {
        x = put(buf, x, area.y, " ", 1, bg);
        x = put(
            buf,
            x,
            area.y,
            app.icons.dirty,
            1,
            bg.patch(theme.fg(theme.warn)),
        );
    }
    put(buf, x, area.y, " ", 1, bg);
    if let Some(w) = &tab.watch {
        let s = format!(" \u{21bb} {} s ", w.interval.as_secs());
        let sw = s.width() as u16;
        put(
            buf,
            (area.x + area.width - 2).saturating_sub(sw),
            area.y,
            &s,
            sw,
            bg.patch(theme.bold(theme.fg(theme.accent))),
        );
    }
    inner
}

/// The endpoint with its `{{variables}}` filled in, for the dim preview after
/// the input; `None` when there is nothing to fill in. Secrets and `$dynamic`
/// values stay as written. The flag is true when a variable is not defined:
/// the text then names those variables.
fn resolved_preview(scope: &sendito_core::Scope, s: &str) -> Option<(String, bool)> {
    if !s.contains("{{") {
        return None;
    }
    let mut out = String::new();
    let mut undefined = Vec::new();
    for seg in sendito_core::template::parse(s) {
        match seg {
            sendito_core::template::Segment::Text(t) => out.push_str(t),
            sendito_core::template::Segment::Var { name, span } => {
                let found = if name.starts_with('$') {
                    None
                } else {
                    scope.lookup(name)
                };
                match found {
                    Some(r) if !r.secret => out.push_str(&r.text),
                    None if !name.starts_with('$') && !scope.is_defined(name) => {
                        undefined.push(name);
                        out.push_str(&s[span]);
                    }
                    _ => out.push_str(&s[span]),
                }
            }
        }
    }
    if !undefined.is_empty() {
        Some((format!("undefined: {}", undefined.join(", ")), true))
    } else if out == s {
        None
    } else {
        Some((out, false))
    }
}

/// Width of the gRPC method or MCP tool chip: its text, the caret and " ▾",
/// between 16 columns and a third of the free strip.
fn chip_width(text_w: u16, avail: u16) -> u16 {
    let max = if avail >= 36 {
        (avail / 3).clamp(16, 44)
    } else {
        16
    };
    (text_w + 5).clamp(16.min(max), max)
}

/// The card above the panes: the kind in its top border, then method,
/// endpoint (or gRPC target and method) and the action button. A low screen
/// gets one row without the border. Returns the cursor when a field there
/// has focus.
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
    let card = area.height >= 3;
    let row = if card {
        draw_url_card(app, buf, area, theme)
    } else {
        Rect::new(area.x, area.y, area.width, 1)
    };
    let names = app.names.clone();
    let is_def = move |n: &str| names.is_defined(n);
    let focus = app.focus;
    let icons = app.icons;
    let y = row.y;
    let x0 = row.x + 1;
    let right = row.x + row.width - 1;
    let (running, streaming, elapsed) = match &app.tabs[app.active].running {
        Some(r) => (true, r.streaming_input, r.started.elapsed().as_secs_f64()),
        None => (false, false, 0.0),
    };
    // A gRPC call needs a method first; until then Send is shown disabled.
    let has_method = match &app.tabs[app.active].model {
        sendito_core::Request::Grpc(g) => !g.service.is_empty() && !g.method.is_empty(),
        sendito_core::Request::Ws(w) => !w.url.trim().is_empty(),
        sendito_core::Request::Mcp(m) => !m.server.is_empty() && !m.tool.is_empty(),
        sendito_core::Request::Http(_) => true,
    };
    // One action button: Send, or Cancel while a call runs. An open stream
    // takes more messages, so it keeps Send and adds a small stop button.
    let send_icon = if icons.send.is_empty() {
        "▶"
    } else {
        icons.send
    };
    let stop_icon = if icons.cancel.is_empty() {
        "■"
    } else {
        icons.cancel
    };
    let send_label = if running && !streaming {
        format!(
            " {stop_icon} Cancel {} {elapsed:.1}s ",
            spinner(app.anim_frame)
        )
    } else if streaming {
        format!(" {send_icon} Send message ")
    } else if row.width >= 80 {
        let key = crate::keymap::short_keys(&app.keymap.hint(Action::Send, app.vim));
        format!(" {send_icon} Send  {key} ")
    } else {
        format!(" {send_icon} Send ")
    };
    let stop_label = format!(" {stop_icon} ");
    let stop_w = if streaming {
        stop_label.width() as u16 + 1
    } else {
        0
    };
    let send_w = send_label.width() as u16;
    let send_x = right.saturating_sub(send_w + stop_w);
    // The input fields end where the button starts.
    let fields_right = send_x;
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
            // right end, by the button. Same parts as every kind: 0=chip,
            // 1=input — only the visual order differs.
            let focused = focus == Focus::Bar(0);
            let short = {
                let t = &app.tabs[app.active];
                let (svc, m) = (t.ed.service(), t.ed.method());
                if svc.is_empty() {
                    "none".to_string()
                } else {
                    format!("{}/{}", svc.rsplit('.').next().unwrap_or(&svc), m)
                }
            };
            let text_w = if focused {
                app.tabs[app.active].ed.grpc_method_text.text().width()
            } else {
                short.width()
            } as u16;
            let cw = chip_width(text_w, fields_right.saturating_sub(x0 + 1));
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
            // The card's title names the kind; a single row keeps a static
            // badge where the other kinds have their picker chip.
            let mw = if card {
                0
            } else {
                let ml = format!(" {:<8}", "WS");
                let mw = ml.width() as u16 + 1;
                put(buf, x0, y, &ml, mw - 1, theme.badge(theme.grpc));
                mw
            };
            let tr = input_box(
                buf,
                Rect::new(x0 + mw, y, fields_right.saturating_sub(x0 + mw + 1), 1),
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
            // right end by the button. Parts as every kind: 0=chip, 1=input.
            let focused = focus == Focus::Bar(0);
            let tool = app.tabs[app.active].ed.mcp_tool.text();
            let short = if tool.is_empty() {
                "none".to_string()
            } else {
                tool
            };
            let cw = chip_width(short.width() as u16, fields_right.saturating_sub(x0 + 1));
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
            input_box(buf, mr, focused, theme);
            let mut shown = None;
            if focused {
                if let Some(tab) = app.tabs.get_mut(app.active) {
                    shown = tab.ed.mcp_tool.render(mr, buf, theme, true, &is_def);
                }
            } else {
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
    // What the endpoint becomes, dim after its text while it has no focus.
    if focus != Focus::Bar(1) {
        let tr = app.areas.bar_url;
        let text = app.tabs[app.active]
            .ed
            .bar_text(1)
            .map(|e| e.text())
            .unwrap_or_default();
        if let Some((preview, undefined)) = resolved_preview(&app.names, &text) {
            let x = tr.x + text.width() as u16 + 2;
            let room = (tr.x + tr.width).saturating_sub(x);
            if room >= 10 {
                let st = if undefined {
                    theme.field().patch(theme.fg(theme.err))
                } else {
                    theme.field().patch(theme.dim())
                };
                let s = truncate(&format!("→ {preview}"), room as usize);
                put(buf, x, y, &s, room, st);
            }
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
    put(buf, send_x, y, &send_label, send_w, send_st);
    app.hits
        .push((Rect::new(send_x, y, send_w, 1), Hit::Bar(2)));
    if streaming {
        let sx = send_x + send_w + 1;
        let sw = stop_w - 1;
        put(buf, sx, y, &stop_label, sw, theme.badge(theme.warn));
        app.hits.push((Rect::new(sx, y, sw, 1), Hit::Bar(3)));
    }
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
/// The JSON check and the cursor position of the body editor, and where the
/// JSON error is (0-based line and column), if any.
fn editor_status(app: &mut App, theme: &Theme) -> (Spans, Option<(usize, usize)>) {
    let focused = app.focus == Focus::ReqBody;
    let tab = &mut app.tabs[app.active];
    let current = tab.current_field();
    let Some(f) = tab.fields().into_iter().find(|f| f.is_multiline()) else {
        return (Vec::new(), None);
    };
    let Some(ed) = tab.ed.text_mut(f) else {
        return (Vec::new(), None);
    };
    let mut out: Spans = Vec::new();
    let mut error = None;
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
                Err(e) => {
                    error = Some((e.line().saturating_sub(1), e.column().saturating_sub(1)));
                    out.push((
                        format!("✗ JSON: line {}, col {}", e.line(), e.column()),
                        theme.fg(theme.err),
                    ));
                }
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
    (out, error)
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
    // Sub-tabs in the top border, the file in the bottom border.
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
                usize::from(e.message.text().trim() != "{}" && !e.message.text().trim().is_empty()),
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
    // Content, one column in from the border.
    let content = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    );
    app.areas.req_body = content;
    let cursor = draw_fields(app, buf, content, theme, &is_def, vim);
    // The file on the left of the bottom border, the editor position and
    // JSON check on the right.
    let (status, error) = editor_status(app, theme);
    let by = area.y + area.height - 1;
    let mut sx = max_x;
    if !status.is_empty() {
        let w = spans_width(&status) + 2;
        sx = max_x.saturating_sub(w + 1);
        if sx > area.x + 2 {
            let x = put(buf, sx, by, " ", 1, theme.surface());
            // A click on the JSON error puts the cursor there.
            if let (Some((line, col)), Some(first)) = (error, status.first()) {
                let ew = first.0.width() as u16;
                app.hits
                    .push((Rect::new(x, by, ew, 1), Hit::JsonError { line, col }));
            }
            let x = put_spans(buf, x, by, max_x, &status);
            put(buf, x, by, " ", 1, theme.surface());
        }
    }
    if area.height > 2 {
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
fn body_content_type(tab: &Tab) -> Option<(String, &'static str)> {
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

#[cfg(test)]
mod tests {
    use super::*;
    use sendito_core::{Scope, VarValue};

    fn scope() -> Scope {
        let mut s = Scope::new();
        s.push_layer(
            "env",
            [
                ("host".to_string(), VarValue::plain("localhost:8080")),
                ("token".to_string(), VarValue::secret("")),
            ]
            .into(),
        );
        s
    }

    #[test]
    fn preview_fills_plain_values_only() {
        let s = scope();
        assert_eq!(resolved_preview(&s, "localhost"), None);
        assert_eq!(
            resolved_preview(&s, "http://{{host}}/a"),
            Some(("http://localhost:8080/a".into(), false))
        );
        // Secrets and generated values stay as written.
        assert_eq!(resolved_preview(&s, "{{token}}/{{$uuid}}"), None);
        assert_eq!(
            resolved_preview(&s, "{{host}}/{{$uuid}}"),
            Some(("localhost:8080/{{$uuid}}".into(), false))
        );
        assert_eq!(
            resolved_preview(&s, "{{a}}{{host}}{{b}}"),
            Some(("undefined: a, b".into(), true))
        );
    }

    #[test]
    fn chip_fits_its_text() {
        assert_eq!(chip_width(10, 120), 16);
        assert_eq!(chip_width(20, 120), 25);
        assert_eq!(chip_width(80, 120), 40);
        assert_eq!(chip_width(20, 30), 16);
    }
}
