//! The import and report dialogs.

use super::*;

pub(super) fn draw_import(
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
        "Paste curl, or a Postman, Insomnia, OpenAPI/Swagger or .http file, or a Bruno directory.",
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

pub(super) fn draw_report(app: &mut App, buf: &mut Buffer, frame: Frame<'_>, rep: &mut Report) {
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
