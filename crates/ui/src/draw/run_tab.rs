//! The run tab: the summary in the URL card and one row per finished
//! request in the request pane. The response pane shows the selected row.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use super::*;
use crate::keymap::Action;

/// The summary line of the run in the URL card.
pub(super) fn draw_run_strip(app: &App, buf: &mut Buffer, row: Rect, theme: &Theme) {
    let t = &app.tabs[app.active];
    let Some(view) = &t.run else { return };
    let folder = crate::app::folder_label(&view.folder);
    let env = app.active_env.as_deref().unwrap_or("no environment");
    let data = view
        .data_file
        .as_ref()
        .map(|f| format!(" · {f} ({} rows)", view.data.len()))
        .unwrap_or_default();
    let done = view.rows.len();
    let counts = format!(
        "{} passed, {} failed, {} error(s)",
        view.count("pass"),
        view.count("fail"),
        view.count("error")
    );
    let tail = match (&t.running, view.took) {
        (Some(r), _) => format!(
            "{} {done}/{} · {:.1} s · {} stops",
            spinner(app.anim_frame),
            view.planned(),
            r.started.elapsed().as_secs_f64(),
            app.keymap.hint(Action::Cancel, app.vim)
        ),
        (None, took) => format!(
            "{done} sent in {:.1} s · {} runs again",
            took.unwrap_or_default().as_secs_f64(),
            app.keymap.hint(Action::Send, app.vim)
        ),
    };
    let text = format!("Run {folder} · {env}{data} · {counts} · {tail}");
    let x = row.x + 1;
    let w = row.width.saturating_sub(2);
    put(buf, x, row.y, &truncate(&text, w as usize), w, theme.dim());
}

/// The rows of the run in the request pane.
pub(super) fn draw_run(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    let focused = matches!(app.focus, Focus::ReqTabs | Focus::ReqBody);
    let inner = frame(buf, area, theme, focused);
    title(buf, area, "Requests", focused, theme);
    if inner.height < 2 || inner.width < 30 {
        return;
    }
    let anim = app.anim_frame;
    let t = &mut app.tabs[app.active];
    let running = t.running.is_some();
    let Some(view) = t.run.as_mut() else { return };
    let w = inner.width as usize;
    let (status_w, time_w, asserts_w) = (24usize, 9usize, 8usize);
    let path_w = w.saturating_sub(3 + status_w + time_w + asserts_w + 3);
    let line = |path: &str, status: &str, time: &str, asserts: &str| {
        format!(
            "{:<path_w$} {:<status_w$} {:>time_w$} {:>asserts_w$}",
            truncate(path, path_w),
            truncate(status, status_w),
            time,
            asserts
        )
    };
    put(
        buf,
        inner.x + 3,
        inner.y,
        &line("Request", "Status", "Time", "Asserts"),
        inner.width.saturating_sub(3),
        theme.title_style(),
    );
    let rows_h = inner.height as usize - 1;
    // The running line takes one row under the finished ones.
    let total = view.rows.len() + usize::from(running);
    if view.sel < view.scroll {
        view.scroll = view.sel;
    } else if view.sel >= view.scroll + rows_h {
        view.scroll = view.sel + 1 - rows_h;
    }
    view.scroll = view.scroll.min(total.saturating_sub(rows_h));
    for (i, y) in (view.scroll..total).zip(inner.y + 1..inner.y + inner.height) {
        let r = Rect::new(inner.x, y, inner.width, 1);
        let Some(row) = view.rows.get(i) else {
            let text = format!(
                "{} sending {} of {}",
                spinner(anim),
                view.rows.len() + 1,
                view.planned()
            );
            put(
                buf,
                inner.x + 2,
                y,
                &text,
                inner.width.saturating_sub(2),
                theme.dim(),
            );
            continue;
        };
        let e = &row.entry;
        let state = match (i == view.sel, focused) {
            (true, true) => RowState::Selected,
            (true, false) => RowState::SelectedQuiet,
            _ => RowState::Plain,
        };
        let base = fill_row(buf, r, state, theme.surface(), theme);
        let (mark, color) = match e.outcome {
            "pass" => ("✓", theme.ok),
            "fail" => ("✗", theme.err),
            _ => ("!", theme.err),
        };
        put(buf, inner.x + 1, y, mark, 1, base.patch(theme.fg(color)));
        let status = e
            .status
            .clone()
            .or_else(|| {
                e.error
                    .as_deref()
                    .map(|s| s.lines().next().unwrap_or_default().to_string())
            })
            .unwrap_or_default();
        let time = e
            .duration_ms
            .map(|ms| format!("{ms} ms"))
            .unwrap_or_default();
        let asserts = if e.asserts.is_empty() {
            String::new()
        } else {
            let pass = e.asserts.iter().filter(|a| a.pass).count();
            format!("{pass}/{}", e.asserts.len())
        };
        put(
            buf,
            inner.x + 3,
            y,
            &line(&e.path, &status, &time, &asserts),
            inner.width.saturating_sub(3),
            base,
        );
    }
    if total == 0 {
        put(
            buf,
            inner.x + 2,
            inner.y + 1,
            "no request was sent",
            inner.width.saturating_sub(2),
            theme.dim(),
        );
    }
}
