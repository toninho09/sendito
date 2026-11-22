//! The inline completion popup: a small list anchored under the cursor. It is
//! not a dialog: nothing is dimmed and the hint and status rows stay live.

use ratatui::buffer::Buffer;
use ratatui::layout::Rect;
use ratatui::style::Modifier;
use ratatui::widgets::{Clear, Widget};
use unicode_width::UnicodeWidthStr;

use crate::app::{App, Hit};
use crate::complete::Completion;
use crate::theme::Theme;

use super::kit::*;

pub(super) fn draw_complete(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    let Some(c) = app.complete.take() else {
        return;
    };
    let anchor = app
        .areas
        .caret
        .or(app.last_cursor)
        .unwrap_or((area.x, area.y));
    // Room above the hint and status rows.
    let bottom = area.y + area.height.saturating_sub(2);
    let rows = c.items.len().min(Completion::ROWS);
    let footer = u16::from(c.loading.is_some() && !c.items.is_empty());
    let h = (rows as u16).max(1) + footer + 2;
    let above = anchor.1.saturating_sub(area.y + 1);
    let below = bottom.saturating_sub(anchor.1 + 1);
    let y = if below < h && above > below {
        anchor.1.saturating_sub(h)
    } else {
        anchor.1 + 1
    };
    let y = y.min(bottom.saturating_sub(h)).max(area.y + 1);
    let visible: Vec<&crate::complete::Candidate> = c
        .items
        .iter()
        .skip(c.scroll)
        .take(Completion::ROWS)
        .collect();
    let name_w = visible
        .iter()
        .map(|i| i.name.width())
        .max()
        .unwrap_or(10)
        .clamp(8, 26);
    let widest = visible
        .iter()
        .map(|i| name_w + i.detail.width() + 5)
        .max()
        .unwrap_or(20);
    let w = (widest as u16)
        .clamp(24, 60)
        .min(area.width.saturating_sub(4));
    let x = anchor
        .0
        .min(area.x + area.width.saturating_sub(w + 2))
        .max(area.x + 1);
    let rect = Rect::new(x, y, w, h);
    shadow(buf, rect, area, theme);
    Clear.render(rect, buf);
    let inner = frame_with(buf, rect, theme, false, theme.surface());
    let mut dy = 0u16;
    for (i, item) in c.items.iter().enumerate().skip(c.scroll).take(rows) {
        let y = inner.y + dy;
        let selected = i == c.sel;
        let row = Rect::new(inner.x, y, inner.width, 1);
        if selected {
            fill(buf, row, theme.selected());
        }
        let st = if selected {
            theme.selected()
        } else {
            theme.surface()
        };
        let label = format!("{:<width$}", truncate(&item.name, name_w), width = name_w);
        let x = put(buf, inner.x, y, &label, name_w as u16, st);
        let room = inner.width.saturating_sub(x - inner.x + 1);
        put(
            buf,
            x + 1,
            y,
            &truncate(&item.detail, room as usize),
            room,
            theme.dim(),
        );
        app.hits.push((row, Hit::CompleteItem(i)));
        dy += 1;
    }
    if rows == 0 {
        let msg = match (c.loading, c.failed) {
            (Some(what), _) => format!("loading {what}…"),
            (None, Some(what)) => format!("{what} failed"),
            (None, None) => "no suggestions".to_string(),
        };
        put(
            buf,
            inner.x + 1,
            inner.y,
            &msg,
            inner.width.saturating_sub(2),
            theme.dim(),
        );
    } else if footer > 0
        && let Some(what) = c.loading
    {
        put(
            buf,
            inner.x + 1,
            inner.y + dy,
            &format!("loading {what}…"),
            inner.width.saturating_sub(2),
            theme.dim().add_modifier(Modifier::ITALIC),
        );
    }
    app.complete = Some(c);
}
