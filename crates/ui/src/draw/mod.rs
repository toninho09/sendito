//! Rendering. Every clickable region is registered in `app.hits`.

use ratatui::Frame;
use ratatui::buffer::Buffer;
use ratatui::layout::Rect;

use crate::app::*;
use crate::theme::Theme;

mod chrome;
mod complete;
mod dialog;
mod editor;
mod kit;
mod overlay;
mod request;
mod response;
mod run_tab;
mod sidebar;
mod table;
mod welcome;
mod whichkey;

use chrome::*;
use complete::*;

use kit::*;
use overlay::*;
use request::*;
use response::*;

use sidebar::*;
use welcome::*;
use whichkey::*;

/// Rows under the panes: the footer.
const FOOTER_ROWS: u16 = 1;

pub use kit::short_method;

pub fn draw(app: &mut App, f: &mut Frame) {
    app.hits.clear();
    app.areas.caret = None;
    let area = f.area();
    app.size = (area.width, area.height);
    let theme = app.theme.clone();
    let buf = f.buffer_mut();
    fill(buf, area, theme.base());
    if area.height < 8 || area.width < 40 {
        put(
            buf,
            0,
            0,
            "Sendito needs at least 40×8 cells",
            area.width,
            theme.base(),
        );
        return;
    }
    let top = Rect::new(area.x, area.y, area.width, 1);
    let body = Rect::new(
        area.x,
        area.y + 1,
        area.width,
        area.height - 1 - FOOTER_ROWS,
    );
    let footer = Rect::new(area.x, area.y + area.height - 1, area.width, 1);

    draw_tabbar(app, buf, top, &theme);

    let sidebar_visible = app.layout.sidebar_visible;
    let right = app.project.config.ui.sidebar == "right";
    let sw = if sidebar_visible {
        let max = (body.width / 2).max(16);
        let w = app.layout.sidebar_width.clamp(16, max);
        if body.width < 100 {
            w.min((body.width / 3).max(20))
        } else {
            w
        }
    } else {
        0
    };
    let (side, main) = if sw == 0 {
        (Rect::default(), body)
    } else if right {
        (
            Rect::new(body.x + body.width - sw, body.y, sw, body.height),
            Rect::new(body.x, body.y, body.width - sw, body.height),
        )
    } else {
        (
            Rect::new(body.x, body.y, sw, body.height),
            Rect::new(body.x + sw, body.y, body.width - sw, body.height),
        )
    };
    app.areas.sidebar = side;
    app.areas.main = main;
    let zoom_side = app.zoom && matches!(app.focus, Focus::Sidebar | Focus::Filter);
    if sw > 0 {
        let s = if zoom_side { body } else { side };
        draw_sidebar(app, buf, s, &theme);
        // Draggable border column.
        let bx = if right {
            side.x
        } else {
            side.x + side.width - 1
        };
        app.hits.push((
            Rect::new(bx, side.y + 1, 1, side.height.saturating_sub(2)),
            Hit::SidebarBorder,
        ));
    }
    if !zoom_side {
        draw_main(app, buf, main, &theme);
    }
    if app.overlay.is_none() {
        draw_complete(app, buf, area, &theme);
    }
    if app.overlay.is_none() {
        draw_whichkey(app, buf, body, &theme);
    }
    let cursor = draw_footer(app, buf, footer, &theme);
    let mut cursor = cursor.or(app.cursor.take());
    if app.overlay.is_some() {
        cursor = draw_overlay(app, buf, area, &theme);
    }
    if let Some((x, y)) = cursor {
        f.set_cursor_position((x, y));
    }
    app.last_cursor = cursor;
}

fn draw_main(app: &mut App, buf: &mut Buffer, area: Rect, theme: &Theme) {
    if app.tabs.is_empty() {
        app.areas.request = Rect::default();
        app.areas.response = Rect::default();
        draw_welcome(app, buf, area, theme);
        return;
    }
    let req_focus = app.focus.in_request();
    let resp_focus = app.focus.in_response();
    // The URL card governs both panes, so it sits above them (hidden only
    // while the response is zoomed). A low screen gets one row, no border.
    let strip_h = if (app.zoom && resp_focus) || area.height <= 8 {
        0
    } else if area.height < 16 {
        1
    } else {
        3
    };
    let strip = Rect::new(area.x, area.y, area.width, strip_h);
    let panes = Rect::new(
        area.x,
        area.y + strip_h,
        area.width,
        area.height.saturating_sub(strip_h),
    );
    app.areas.main = panes;
    let (req_area, resp_area) = if app.zoom && req_focus {
        (panes, Rect::default())
    } else if app.zoom && resp_focus {
        (Rect::default(), panes)
    } else {
        let rh = ((panes.height as u32 * app.layout.request_pct as u32) / 100) as u16;
        let rh = rh.clamp(5.min(panes.height), panes.height.saturating_sub(5).max(5));
        (
            Rect::new(panes.x, panes.y, panes.width, rh),
            Rect::new(
                panes.x,
                panes.y + rh,
                panes.width,
                panes.height.saturating_sub(rh),
            ),
        )
    };
    app.areas.request = req_area;
    app.areas.response = resp_area;
    let mut cursor = None;
    if strip_h > 0 {
        cursor = draw_url_strip(app, buf, strip, theme);
    }
    if req_area.height > 0
        && let Some(c) = draw_request(app, buf, req_area, theme)
    {
        cursor = Some(c);
    }
    if req_focus {
        app.cursor = cursor;
    }
    if resp_area.height > 0 {
        // Pushed first: the tabs in the same border row win the clicks.
        if req_area.height > 0 {
            app.hits.push((
                Rect::new(
                    resp_area.x + 1,
                    resp_area.y,
                    resp_area.width.saturating_sub(2),
                    1,
                ),
                Hit::SplitBorder,
            ));
        }
        draw_response(app, buf, resp_area, theme);
    }
}
