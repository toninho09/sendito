//! The response pane: tabs and the result in the top border, the view mode
//! and position in the bottom border, a scrollbar on the right.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use sendito_pipeline::human_size;
use unicode_width::UnicodeWidthStr;

use crate::app::*;
use crate::keymap::Action;
use crate::tab::{Kind, QueryMode};
use crate::theme::Theme;
use crate::viewer::ViewMode;

use super::kit::*;

pub(super) fn draw_response(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    let focused = app.focus.in_response();
    let frame_no = app.anim_frame;
    let tab = &app.tabs[app.active];
    let running = tab
        .running
        .as_ref()
        .map(|r| (r.started.elapsed(), r.streaming_input));
    let summary = tab
        .response
        .as_ref()
        .map(|r| r.summary.clone())
        .unwrap_or_default();
    let inner = frame(buf, area, theme, focused);
    let max_x = area.x + area.width - 1;
    // The result, right-aligned in the top border.
    let mut spans: Spans = Vec::new();
    if let Some((el, streaming)) = running {
        let what = match tab.response.as_ref().and_then(|r| r.grpc_kind) {
            Some(k) if streaming => format!("{} open", k.label()),
            Some(k) => format!("{} {} running", spinner(frame_no), k.label()),
            None => format!("{} sending", spinner(frame_no)),
        };
        spans.push((
            format!(" {what} {:.1}s ", el.as_secs_f64()),
            theme.bold(theme.fg(theme.accent)),
        ));
    }
    // A saved example, not a response: named before its status.
    if let Some(name) = tab.response.as_ref().and_then(|r| r.example.as_ref()) {
        spans.push((
            format!(" example: {} ", truncate(name, 30)),
            theme.bold(theme.fg(theme.accent)),
        ));
    }
    if !summary.status.is_empty() {
        let color = result_color(
            theme,
            summary.code,
            summary.ok,
            summary.error.as_deref() == Some("cancelled"),
            running.is_some(),
        );
        spans.push((
            format!(" {} ", truncate(&summary.status, 40)),
            theme.chip(color),
        ));
        if let Some(t) = summary.time_ms {
            // Slow responses read as such.
            let st = if t >= 1000.0 {
                theme.fg(theme.warn)
            } else {
                theme.fg(theme.fg)
            };
            spans.push((format!(" {} ", crate::tab::fmt_ms(t)), st));
        }
        if let Some(s) = summary.size {
            // gRPC: the size of the response messages, said explicitly.
            let what = if tab.ed.kind == Kind::Grpc {
                " received"
            } else {
                ""
            };
            spans.push((
                format!(
                    "{}{}{what} ",
                    if summary.time_ms.is_some() { "" } else { " " },
                    human_size(s)
                ),
                theme.dim(),
            ));
            // A decoded body shows the encoding and the size it had on the wire.
            if let Some(d) = &summary.decoded {
                spans.push((format!("← {d} "), theme.dim()));
            }
        }
    }
    // Tabs in the top border.
    let resp_tab = tab.resp_tab;
    let kind = tab.ed.kind;
    let names = crate::tab::resp_tab_names(kind);
    let badge = match (kind, tab.response.as_ref()) {
        (Kind::Http, Some(r)) => r.results_badge.clone(),
        _ => None,
    };
    let labels: Vec<String> = names
        .iter()
        .enumerate()
        .map(|(i, n)| match (&badge, i) {
            (Some(b), 3) => format!("{n} {b}"),
            _ => n.to_string(),
        })
        .collect();
    let look = if app.focus == Focus::RespTabs {
        TabLook::Strong
    } else if focused {
        TabLook::Soft
    } else {
        TabLook::Quiet
    };
    let total: u16 = spans_width(&spans);
    let tabs_end = max_x.saturating_sub(total + 1);
    let rects = tab_strip(
        buf,
        area.x + 2,
        area.y,
        tabs_end,
        &labels,
        resp_tab,
        look,
        theme,
    );
    let tabs_w = rects.last().map(|r| r.x + r.width).unwrap_or(area.x + 2);
    for (i, r) in rects.into_iter().enumerate() {
        app.hits.push((r, Hit::RespTab(i)));
    }
    let sx = max_x.saturating_sub(total + 1).max(tabs_w + 1);
    put_spans(buf, sx, area.y, max_x, &spans);
    if inner.height < 1 {
        return;
    }
    let bottom = area.y + area.height - 1;
    // The view modes only matter once there is a body to view.
    let has_resp = tab
        .response
        .as_ref()
        .is_some_and(|r| !r.body.doc.is_empty() || !r.body.doc.raw.is_empty());
    // Pretty / Raw / Hex in the bottom border.
    let mut modes_end = area.x + 2;
    if has_resp && resp_tab == 0 {
        let mode = tab
            .response
            .as_ref()
            .map(|r| r.body.mode)
            .unwrap_or(ViewMode::Raw);
        // "Pretty" only for bodies that have a pretty form (JSON).
        let has_pretty = tab
            .response
            .as_ref()
            .is_some_and(|r| r.body.doc.pretty.is_some());
        let modes: Vec<ViewMode> = [ViewMode::Pretty, ViewMode::Raw, ViewMode::Hex]
            .into_iter()
            .filter(|m| has_pretty || *m != ViewMode::Pretty)
            .collect();
        let labels: Vec<String> = modes.iter().map(|m| m.label().to_string()).collect();
        let active = modes.iter().position(|m| *m == mode).unwrap_or(0);
        let rects = tab_strip(
            buf,
            area.x + 2,
            bottom,
            max_x,
            &labels,
            active,
            TabLook::Soft,
            theme,
        );
        if let Some(r) = rects.last() {
            modes_end = r.x + r.width;
        }
        for (r, m) in rects.into_iter().zip(modes) {
            app.hits.push((r, Hit::ViewMode(m)));
        }
        // A jq filter applied to the body, in the bottom border after the
        // modes, while the query bar does not show it.
        let bar_jq = tab
            .response
            .as_ref()
            .and_then(|r| r.query.as_ref())
            .is_some_and(|q| q.mode == QueryMode::Jq);
        if let Some(expr) = tab
            .response
            .as_ref()
            .and_then(|r| r.jq.as_deref())
            .filter(|_| !bar_jq)
        {
            let t = format!(" jq: {} ", truncate(expr, 24));
            let tw = t.width() as u16;
            let x = modes_end + 1;
            if x + tw < max_x {
                put(buf, x, bottom, &t, tw, theme.dim().bg(theme.surface));
            }
        }
    }
    let view = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    );
    let view = draw_query_bar(app, buf, view, theme);
    let (view, stream_pos) = draw_stream_split(app, buf, view, theme);
    let mut resp_hit = view;
    app.areas.resp_body = view;
    let k = app.send_hint();
    let verb = app.send_verb();
    let cancel = app.keymap.hint(Action::Cancel, app.vim);
    let tab = &mut app.tabs[app.active];
    match &mut tab.response {
        None => {
            // Nothing sent yet: the key that sends, centered.
            let mut line: Spans = vec![("Press ".into(), theme.dim())];
            for (i, key) in k.split('/').enumerate() {
                if i > 0 {
                    line.push((" or ".into(), theme.dim()));
                }
                line.push((format!(" {key} "), theme.keycap()));
            }
            line.push((format!(" to {verb}."), theme.dim()));
            centered_lines(
                buf,
                view,
                &[
                    line,
                    Vec::new(),
                    vec![("The response appears here.".into(), theme.dim())],
                ],
            );
        }
        Some(r) => {
            let empty =
                r.viewer_mut(resp_tab).doc.is_empty() && r.viewer_mut(resp_tab).doc.raw.is_empty();
            if let (Some((el, _)), true) = (running, r.grpc_events.is_empty() && empty) {
                // A bar that moves while the request is on its way.
                let w = 24usize;
                let pos = (frame_no as usize) % (w + 6);
                let bar: String = (0..w)
                    .map(|i| {
                        if i + 6 > pos && i <= pos {
                            '▰'
                        } else {
                            '▱'
                        }
                    })
                    .collect();
                centered_lines(
                    buf,
                    view,
                    &[
                        vec![(
                            format!(
                                "{} Waiting for the response… {:.1}s",
                                spinner(frame_no),
                                el.as_secs_f64()
                            ),
                            theme.bold(theme.fg(theme.accent)),
                        )],
                        vec![(bar, theme.fg(theme.accent))],
                        vec![
                            (format!(" {cancel} "), theme.keycap()),
                            (" cancels".into(), theme.dim()),
                        ],
                    ],
                );
            } else if empty && resp_tab == 0 {
                let msg = if summary.code.is_some() {
                    "(empty body)"
                } else if summary.error.as_deref() == Some("cancelled") {
                    "Cancelled before a response arrived."
                } else if kind == Kind::Grpc {
                    "(no message yet)"
                } else {
                    "(no response)"
                };
                centered_lines(buf, view, &[vec![(msg.into(), theme.dim())]]);
            } else {
                // The gRPC Log keeps its legend pinned above the scrolling log.
                let legend = kind == Kind::Grpc && resp_tab == 3 && view.height > 3;
                if legend {
                    let l: Spans = vec![
                        ("→".into(), theme.bold(theme.fg(theme.accent))),
                        (" sent  ".into(), theme.dim()),
                        ("←".into(), theme.bold(theme.fg(theme.ok))),
                        (" received  ".into(), theme.dim()),
                        ("■".into(), theme.bold(theme.fg(theme.warn))),
                        (" final status  ".into(), theme.dim()),
                        ("+s".into(), theme.fg(theme.fg)),
                        (" since start".into(), theme.dim()),
                    ];
                    put_spans(buf, view.x, view.y, view.x + view.width, &l);
                }
                let top = u16::from(legend);
                let v = r.viewer_mut(resp_tab);
                // Line numbers for a body the server sent, not for an error text.
                v.numbers = resp_tab == 0 && (summary.code.is_some() || summary.ok);
                v.syntax = match resp_tab {
                    1 if kind == Kind::Mcp => crate::viewer::Syntax::Json,
                    1 => crate::viewer::Syntax::Headers,
                    2 => crate::viewer::Syntax::Timing,
                    3 => crate::viewer::Syntax::Messages,
                    _ => crate::viewer::Syntax::Plain,
                };
                let vr = Rect::new(
                    view.x,
                    view.y + top,
                    view.width,
                    view.height.saturating_sub(top).max(1),
                );
                resp_hit = vr;
                v.render(vr, buf, theme, app.focus == Focus::RespBody);
                scrollbar(
                    buf,
                    area,
                    v.scroll,
                    v.row_count(),
                    vr.height as usize,
                    focused,
                    theme,
                );
                // Where the view is, in the bottom border.
                let more = v.scroll + (vr.height as usize) < v.row_count();
                let st = format!(
                    " {}{}{}{} ",
                    stream_pos
                        .as_deref()
                        .map(|p| format!("{p} · "))
                        .unwrap_or_default(),
                    v.status().replacen("line", "Ln", 1),
                    if v.wrap && resp_tab != 2 {
                        " · wrap"
                    } else {
                        ""
                    },
                    if more { " · ↓ more" } else { "" }
                );
                let sw = st.width() as u16;
                let px = max_x.saturating_sub(sw + 1);
                if px > modes_end + 1 {
                    put(buf, px, bottom, &st, sw, theme.dim().bg(theme.surface));
                }
                // The JSON path at the cursor, left of the position; a long
                // path keeps its end.
                let room = px.saturating_sub(modes_end + 3) as usize;
                if let Some(path) = (resp_tab == 0 && room > 8)
                    .then(|| v.json_path())
                    .flatten()
                    .filter(|p| p != ".")
                {
                    let t = format!(" {} ", truncate_left(&path, room - 2));
                    let tw = t.width() as u16;
                    put(
                        buf,
                        px - tw,
                        bottom,
                        &t,
                        tw,
                        theme.fg(theme.accent).bg(theme.surface),
                    );
                }
            }
        }
    }
    app.hits.push((resp_hit, Hit::RespView));
}

/// The query bar (jq filter or search) on the first row of the view, with a
/// rule below it. Returns the rest of the view.
fn draw_query_bar(app: &mut App, buf: &mut Buffer, view: Rect, theme: &Theme) -> Rect {
    if !app.query_shown() || view.height < 4 {
        return view;
    }
    let focused = app.focus == Focus::RespQuery;
    let names = app.names.clone();
    let tab = &mut app.tabs[app.active];
    let Some(r) = tab.response.as_mut() else {
        return view;
    };
    let live = r.live_jq();
    let (jq, outputs) = (r.jq.clone(), r.jq_outputs);
    let search = r
        .viewer_mut(tab.resp_tab)
        .search
        .as_ref()
        .map(|s| (s.matches.is_empty(), s.position_label()));
    let Some(q) = r.query.as_mut() else {
        return view;
    };
    let text = q.input.text();
    // What the typed text gives, on the right of the bar.
    let state: Option<(String, ratatui::style::Style)> = match q.mode {
        _ if text.trim().is_empty() => None,
        QueryMode::Jq => match &q.error {
            Some(e) => Some((format!("✗ {e}"), theme.fg(theme.err))),
            None if jq.as_deref() != Some(text.trim()) && !live => {
                Some(("Enter applies".into(), theme.dim()))
            }
            None => Some((
                format!("{outputs} result{}", if outputs == 1 { "" } else { "s" }),
                theme.fg(theme.ok),
            )),
        },
        QueryMode::Find => search.map(|(empty, label)| {
            if empty {
                ("no match".into(), theme.fg(theme.warn))
            } else {
                (label, theme.fg(theme.ok))
            }
        }),
    };
    let fr = Rect::new(view.x, view.y, view.width, 1);
    let ir = input_box(buf, fr, focused, theme);
    let label = match q.mode {
        QueryMode::Jq => "jq ▸ ",
        QueryMode::Find => "find ▸ ",
    };
    let lw = label.width() as u16;
    let x = put(
        buf,
        ir.x,
        ir.y,
        label,
        lw,
        theme.field().patch(theme.accent()),
    );
    let max_x = ir.x + ir.width;
    // The state takes at most half of the bar; a long error is cut.
    let mut end = max_x;
    if let Some((t, st)) = state {
        let t = format!(" {} ", truncate(&t, (ir.width / 2) as usize));
        let tw = t.width() as u16;
        end = max_x.saturating_sub(tw);
        put(buf, end, ir.y, &t, tw, theme.field().patch(st));
    }
    let er = Rect::new(x, ir.y, end.saturating_sub(x + 1), 1);
    let cursor = q
        .input
        .render(er, buf, theme, focused, &|n| names.is_defined(n));
    if focused && let Some(c) = cursor {
        app.cursor = Some(c);
        // The completion popup opens under the caret.
        app.areas.caret = Some(c);
    }
    put(
        buf,
        view.x,
        view.y + 1,
        &"─".repeat(view.width as usize),
        view.width,
        theme.dim(),
    );
    app.hits.push((fr, Hit::RespQuery));
    Rect::new(view.x, view.y + 2, view.width, view.height - 2)
}

/// A stream with several messages, on the body tab: the message list on the
/// left and the shown message on the right. Returns the rest of the view and,
/// when the pane is too narrow for the list, the position for the border
/// (`◀ 3/12 ▶`, `12/12 live`).
fn draw_stream_split(
    app: &mut App,
    buf: &mut Buffer,
    view: Rect,
    theme: &Theme,
) -> (Rect, Option<String>) {
    let tab = &app.tabs[app.active];
    let Some(r) = tab.response.as_ref().filter(|_| tab.resp_tab == 0) else {
        return (view, None);
    };
    let n = r.stream.kept();
    if n < 2 {
        return (view, None);
    }
    let cur = r.stream_sel.unwrap_or(n - 1);
    let live = r.stream_sel.is_none();
    // Numbers count every message, the dropped ones too.
    let (first, total) = (r.stream.dropped, r.stream.received);
    if view.width < 70 || view.height < 3 {
        let pos = if live {
            format!("{total}/{total} live")
        } else {
            format!("◀ {}/{total} ▶", first + cur + 1)
        };
        return (view, Some(pos));
    }
    let lw = (view.width * 2 / 5).min(38);
    let kept = if first > 0 {
        format!(" ({first} older dropped)")
    } else {
        String::new()
    };
    let unit = if tab.ed.kind == Kind::Sse {
        "events"
    } else {
        "messages"
    };
    let head = if live {
        format!("{total} {unit}{kept} · live")
    } else {
        format!("{total} {unit}{kept} · {}/{total}", first + cur + 1)
    };
    put(
        buf,
        view.x,
        view.y,
        &truncate(&head, lw as usize),
        lw,
        theme.dim(),
    );
    // The selected row stays in sight; live mode shows the newest rows.
    let rows = (view.height - 1) as usize;
    let top = (cur + 1).saturating_sub(rows);
    let num_w = total.to_string().len();
    let mut hits = Vec::new();
    for (k, m) in r.messages().enumerate().skip(top).take(rows) {
        let y = view.y + 1 + (k - top) as u16;
        let line = format!(
            "#{:<num_w$} +{:.3}s {:>6} {}",
            first + k + 1,
            m.at.as_secs_f64(),
            human_size(m.size as u64),
            m.preview.replace(['\n', '\r'], " ")
        );
        let row = Rect::new(view.x, y, lw, 1);
        let st = if k == cur {
            theme.selected()
        } else {
            theme.fg(theme.fg)
        };
        fill(buf, row, st);
        put(buf, view.x, y, &truncate(&line, lw as usize), lw, st);
        hits.push((row, Hit::StreamMsg(k)));
    }
    for y in view.y..view.y + view.height {
        put(buf, view.x + lw, y, "│", 1, theme.dim());
    }
    app.hits.extend(hits);
    let x = view.x + lw + 2;
    (
        Rect::new(x, view.y, view.width.saturating_sub(lw + 2), view.height),
        None,
    )
}
