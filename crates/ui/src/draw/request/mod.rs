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

mod fields;
mod url;
pub(in crate::draw) use fields::*;
pub(in crate::draw) use url::*;

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
    // A large body is not checked on every frame: its size is known
    // without joining its lines.
    if ed.highlight == crate::editor::Highlight::Json && ed.byte_len() <= 64 * 1024 {
        let text = ed.text();
        if !text.trim().is_empty() {
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
    let names = app.tab_names();
    let is_def = move |n: &str| names.is_defined(n);
    let vim = app.vim;
    // Sub-tabs in the top border, the file in the bottom border.
    let crumb = {
        let t = &app.tabs[app.active];
        match (&t.path, &t.origin) {
            _ if t.folder.is_some() => {
                let rel = t.folder.as_deref().unwrap_or_default();
                if rel.is_empty() {
                    sendito_store::folder::FILE.to_string()
                } else {
                    breadcrumb(&format!("{rel}/{}", sendito_store::folder::FILE))
                }
            }
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
                usize::from(e.auth_kind != sendito_core::AuthKind::Inherit),
                table_count(&e.pre) + table_count(&e.extract) + table_count(&e.asserts),
                0,
            ],
            Kind::Grpc => vec![
                0,
                table_count(&e.metadata),
                usize::from(e.auth_kind != sendito_core::AuthKind::Inherit),
                table_count(&e.pre) + table_count(&e.extract) + table_count(&e.asserts),
                0,
            ],
            Kind::Ws => vec![
                table_count(&e.headers),
                usize::from(!e.message.trimmed_is("[]") && !e.message.trimmed_is("")),
                usize::from(e.auth_kind != sendito_core::AuthKind::Inherit),
                table_count(&e.pre) + table_count(&e.extract) + table_count(&e.asserts),
                0,
            ],
            Kind::Mcp => vec![
                0,
                usize::from(!e.message.trimmed_is("{}") && !e.message.trimmed_is("")),
                table_count(&e.pre) + table_count(&e.extract) + table_count(&e.asserts),
                0,
            ],
            Kind::Folder => vec![
                table_count(&e.headers),
                usize::from(e.auth_kind != sendito_core::AuthKind::Inherit),
                table_count(&e.vars),
                table_count(&e.pre) + table_count(&e.extract) + table_count(&e.asserts),
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
