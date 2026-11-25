//! The environment editor's dialog: title, env tabs, an optional one-line
//! input, legend, table, explanation, status and a row of buttons.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Hit, OverlayHit};
use crate::editor::TextEditor;
use crate::form::Table;
use crate::theme::Theme;

use super::dialog;
use super::dialog::inset;
use super::kit::{TabLook, input_box, put, put_wrapped, tab_strip, truncate};
use super::table::draw_table;

/// Everything one of these dialogs shows around its table.
pub(super) struct EditorDialog<'a> {
    pub title: &'a str,
    pub width: u16,
    pub height: u16,
    /// Env names, shown as tabs above the table; empty means no tab row.
    pub tabs: Vec<String>,
    pub tab_sel: usize,
    /// Rows kept below the table: a blank, the explanation, the status line
    /// and the buttons. Three when the table may sit against the status.
    pub reserved: u16,
    pub legend: &'a str,
    /// What the selected row means, on the line above the status.
    pub explain: &'a str,
    pub error: Option<&'a str>,
    pub dirty: bool,
    pub buttons: &'a [&'a str],
    /// Whether a `{{name}}` is defined (the table's cells show the answer).
    pub is_def: &'a dyn Fn(&str) -> bool,
    /// A one-line input above the legend; when it has the focus, the table
    /// does not.
    pub head: Option<Head<'a>>,
}

/// A labelled one-line input with a status after it.
pub(super) struct Head<'a> {
    pub label: &'a str,
    pub input: &'a mut TextEditor,
    pub focused: bool,
    pub status: &'a str,
    /// The status tells of a problem (`Some(true)`), of a good state
    /// (`Some(false)`) or is neutral (`None`).
    pub problem: Option<bool>,
    pub hit: OverlayHit,
}

/// The width of the label of [`Head`].
const HEAD_LABEL_W: u16 = 12;

/// Where the clicks of one of these dialogs go (the two differ only in the
/// variants they use).
pub(super) trait EditorHits {
    fn tab(&self, i: usize) -> OverlayHit;
    fn cell(&self, row: usize, col: usize) -> OverlayHit;
    fn delete(&self, row: usize) -> OverlayHit;
    fn button(&self, i: usize) -> OverlayHit;
}

pub(super) fn render(
    app: &mut App,
    buf: &mut Buffer,
    body: Rect,
    theme: &Theme,
    d: EditorDialog<'_>,
    table: &mut Table,
    hits: &dyn EditorHits,
) -> Option<(u16, u16)> {
    let title = if d.dirty {
        format!("{} {} unsaved", d.title, app.icons.dirty)
    } else {
        d.title.to_string()
    };
    let (_, inner) = dialog::shell(app, buf, body, d.width, d.height, &title, theme);
    let inner = inset(inner, 1, 0);
    // Tabs (when there are any) then the legend, all above the table.
    let mut top = inner.y;
    if !d.tabs.is_empty() {
        let rects = tab_strip(
            buf,
            inner.x,
            top,
            inner.x + inner.width,
            &d.tabs,
            d.tab_sel,
            TabLook::Strong,
            theme,
        );
        for (i, r) in rects.into_iter().enumerate() {
            app.hits.push((r, Hit::Overlay(hits.tab(i))));
        }
        top += 1;
    }
    let mut head_cursor = None;
    let table_focused = !d.head.as_ref().is_some_and(|h| h.focused);
    if let Some(h) = d.head {
        top += 1;
        let label_st = if h.focused {
            theme.bold(theme.on_overlay(theme.accent))
        } else {
            theme.dialog_dim()
        };
        put(buf, inner.x, top, h.label, HEAD_LABEL_W, label_st);
        let sw = (h.status.width() as u16 + 1).min(inner.width / 2);
        let w = inner.width.saturating_sub(HEAD_LABEL_W + sw);
        let r = input_box(
            buf,
            Rect::new(inner.x + HEAD_LABEL_W, top, w, 1),
            h.focused,
            theme,
        );
        let c = h.input.render(r, buf, theme, h.focused, d.is_def);
        if h.focused {
            head_cursor = c;
        }
        app.hits.push((r, Hit::Overlay(h.hit)));
        let st = match h.problem {
            Some(true) => theme.on_overlay(theme.warn),
            Some(false) => theme.on_overlay(theme.ok),
            None => theme.dialog_dim(),
        };
        put(
            buf,
            inner.x + HEAD_LABEL_W + w + 1,
            top,
            &truncate(h.status, sw.saturating_sub(1) as usize),
            sw.saturating_sub(1),
            st,
        );
        top += 1;
    }
    let ly = put_wrapped(
        buf,
        inner.x,
        top + 1,
        d.legend,
        inner.width,
        (top + 6).min(inner.y + inner.height.saturating_sub(4)),
        theme.dialog_dim(),
    );
    // The explanation, the status line and the buttons take the last rows.
    let tr = Rect::new(
        inner.x,
        ly + 1,
        inner.width,
        (inner.y + inner.height).saturating_sub(ly + 1 + d.reserved),
    );
    put(
        buf,
        inner.x,
        inner.y + inner.height.saturating_sub(3),
        &truncate(d.explain, inner.width as usize),
        inner.width,
        theme.on_overlay(theme.info),
    );
    let cursor = draw_table(
        buf,
        tr,
        theme,
        table,
        table_focused,
        "",
        d.is_def,
        &mut |r, h| {
            let hit = match h {
                super::table::TableHit::Cell { row, col } => hits.cell(row, col),
                super::table::TableHit::Delete { row } => hits.delete(row),
                // The editors' tables have no enable column.
                super::table::TableHit::Toggle { .. } => return,
            };
            app.hits.push((r, Hit::Overlay(hit)));
        },
        app.hover,
    );
    dialog::status(buf, inner, d.error, d.dirty, &app.icons, theme);
    dialog::buttons(app, buf, inner, d.buttons, None, &|i| hits.button(i), theme);
    head_cursor.or(cursor)
}
