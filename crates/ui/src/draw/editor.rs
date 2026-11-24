//! The configuration table editor: the shape the environment and MCP-server
//! dialogs share (title, optional env tabs, legend, table, explanation, status
//! and a row of buttons).

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::app::{App, Hit, OverlayHit};
use crate::form::Table;
use crate::theme::Theme;

use super::dialog;
use super::dialog::inset;
use super::kit::{TabLook, put, put_wrapped, tab_strip, truncate};
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
}

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
        true,
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
    cursor
}
