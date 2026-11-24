//! The URL card and the URL strip with its chips and resolved preview.

use super::*;

/// The card's border and title: the protocol in the colour of the kind (the
/// method for HTTP), `●` while unsaved, the watch interval on the right.
/// Returns the content row.
pub(super) fn draw_url_card(app: &App, buf: &mut Buffer, area: Rect, theme: &Theme) -> Rect {
    let tab = &app.tabs[app.active];
    let (kind, color) = match tab.ed.kind {
        // The chip already names the method: the title names the protocol,
        // the border keeps the method's colour.
        Kind::Http => ("HTTP".to_string(), theme.method_color(&tab.ed.method)),
        Kind::Grpc => ("GRPC".to_string(), theme.grpc),
        Kind::Ws => ("WS".to_string(), theme.grpc),
        Kind::Mcp => ("MCP".to_string(), theme.grpc),
        Kind::Folder => ("FOLDER".to_string(), theme.accent),
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
pub(super) fn resolved_preview(scope: &sendito_core::Scope, s: &str) -> Option<(String, bool)> {
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
pub(super) fn chip_width(text_w: u16, avail: u16) -> u16 {
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
pub(in crate::draw) fn draw_url_strip(
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
    if let Some(rel) = app.tabs[app.active].folder.clone() {
        draw_folder_strip(app, buf, row, theme, &rel);
        return None;
    }
    let names = app.tab_names();
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
        // Drawn by `draw_folder_strip`.
        Kind::Folder => {}
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
        if let Some((preview, undefined)) = resolved_preview(&app.tab_names(), &text) {
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

/// The strip of a folder tab: which folder, how many requests its settings
/// reach, and the file they are saved to.
fn draw_folder_strip(app: &App, buf: &mut Buffer, row: Rect, theme: &Theme, rel: &str) {
    let requests = app
        .tree
        .root
        .find_folder(rel)
        .map(|f| f.count_requests())
        .unwrap_or(0);
    let file = if rel.is_empty() {
        sendito_store::folder::FILE.to_string()
    } else {
        format!("{rel}/{}", sendito_store::folder::FILE)
    };
    let name = crate::app::folder_label(rel);
    let save = app.keymap.hint(Action::Save, app.vim);
    let text = format!(
        "{name} · applies to {requests} request{} below · {save} saves {file}",
        if requests == 1 { "" } else { "s" }
    );
    let x = row.x + 1;
    let w = row.width.saturating_sub(2);
    put(buf, x, row.y, &truncate(&text, w as usize), w, theme.dim());
}
