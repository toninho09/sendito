//! The response pane: tabs and the result in the top border, the view mode
//! and position in the bottom border, a scrollbar on the right.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use unicode_width::UnicodeWidthStr;

use crate::app::*;
use crate::keymap::Action;
use crate::tab::Kind;
use crate::theme::Theme;
use crate::viewer::ViewMode;

use super::kit::*;

pub(super) fn draw_response(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    let focused = matches!(app.focus, Focus::RespTabs | Focus::RespBody);
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
            theme.fg(theme.accent).add_modifier(Modifier::BOLD),
        ));
    }
    if !summary.status.is_empty() {
        let color = match summary.code {
            Some(c) => theme.status_color(c),
            // A call in progress is not an error.
            None if running.is_some() => theme.accent,
            None if summary.error.as_deref() == Some("cancelled") => theme.warn,
            None if summary.ok => theme.ok,
            None => theme.err,
        };
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
    }
    let view = Rect::new(
        inner.x + 1,
        inner.y,
        inner.width.saturating_sub(2),
        inner.height,
    );
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
                            theme.fg(theme.accent).add_modifier(Modifier::BOLD),
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
                        (
                            "→".into(),
                            theme.fg(theme.accent).add_modifier(Modifier::BOLD),
                        ),
                        (" sent  ".into(), theme.dim()),
                        ("←".into(), theme.fg(theme.ok).add_modifier(Modifier::BOLD)),
                        (" received  ".into(), theme.dim()),
                        (
                            "■".into(),
                            theme.fg(theme.warn).add_modifier(Modifier::BOLD),
                        ),
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
                    " {}{}{} ",
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
            }
        }
    }
    app.hits.push((resp_hit, Hit::RespView));
}
