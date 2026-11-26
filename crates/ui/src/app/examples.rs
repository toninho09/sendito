//! Saved example responses: `:saveexample [name]` and `:examples`.

use sendito_store::examples::{self, Example};

use super::*;
use crate::tab::Stored;

impl App {
    /// The saved HTTP request of the active tab, or why there is none.
    fn example_request(&self) -> Result<String, String> {
        let t = self.tab().ok_or("open a request first")?;
        if t.ed.kind != Kind::Http || t.folder.is_some() {
            return Err("examples are for HTTP requests".into());
        }
        t.path.clone().ok_or_else(|| {
            "save the request first (Ctrl+S): its examples are kept next to its file".into()
        })
    }

    /// `:saveexample [name]`: saves the response on screen as an example; a
    /// history response too, not an example. Credential headers are masked;
    /// the body is saved as shown before a jq filter (secrets the server
    /// echoed are already masked there).
    pub(super) fn save_example(&mut self, name: &str) {
        let rel = match self.example_request() {
            Ok(rel) => rel,
            Err(e) => return self.set_status(e, StatusKind::Warn),
        };
        let Some((head, body)) = self
            .tab()
            .and_then(|t| t.response.as_ref())
            .filter(|r| !matches!(r.stored, Some(Stored::Example(_))))
            .and_then(|r| r.http_head.clone().map(|h| (h, r.body_bytes())))
        else {
            return self.set_status(
                "send the request first: no response to save",
                StatusKind::Warn,
            );
        };
        let ex = Example {
            name: if name.trim().is_empty() {
                Example::default_name(head.status, &head.reason)
            } else {
                name.trim().to_string()
            },
            status: head.status,
            reason: head.reason,
            headers: examples::mask_credential_headers(head.headers),
            body,
        };
        match examples::save(&self.project.coll, &rel, &ex) {
            Ok(saved) => self.set_status(
                format!("saved example {saved} (:examples lists them)"),
                StatusKind::Ok,
            ),
            Err(e) => self.set_status(format!("example not saved: {e}"), StatusKind::Error),
        }
    }

    /// `:examples` lists the active request's examples; `:examples delete
    /// <name>` deletes one.
    pub(super) fn examples_command(&mut self, arg: &str) {
        let rel = match self.example_request() {
            Ok(rel) => rel,
            Err(e) => return self.set_status(e, StatusKind::Warn),
        };
        if let Some(name) = arg.strip_prefix("delete ") {
            let name = name.trim();
            return match examples::delete(&self.project.coll, &rel, name) {
                Ok(()) => self.set_status(format!("deleted example {name}"), StatusKind::Ok),
                Err(e) => self.set_status(format!("example not deleted: {e}"), StatusKind::Error),
            };
        }
        if !arg.is_empty() {
            return self.set_status("usage: :examples [delete <name>]", StatusKind::Error);
        }
        let names = match examples::list(&self.project.coll, &rel) {
            Ok(n) => n,
            Err(e) => return self.set_status(e.to_string(), StatusKind::Error),
        };
        if names.is_empty() {
            return self.set_status(
                "no saved examples: send the request, then :saveexample",
                StatusKind::Info,
            );
        }
        let details = names
            .iter()
            .map(|n| {
                examples::load(&self.project.coll, &rel, n)
                    .map(|e| format!("{} {}", e.status, e.reason))
                    .unwrap_or_else(|e| e.to_string())
            })
            .collect();
        self.overlay = Some(Overlay::Picker(Picker::new(
            "Saved examples (Enter shows one; :examples delete <name> deletes)",
            names,
            details,
            PickAction::Example,
            None,
        )));
    }

    /// Shows a saved example in the response pane, marked as an example.
    pub(super) fn show_example(&mut self, name: &str) {
        let rel = match self.example_request() {
            Ok(rel) => rel,
            Err(e) => return self.set_status(e, StatusKind::Warn),
        };
        let ex = match examples::load(&self.project.coll, &rel, name) {
            Ok(ex) => ex,
            Err(e) => return self.set_status(e.to_string(), StatusKind::Error),
        };
        let name = ex.name.clone();
        let resp = Response::from_stored(ex, Stored::Example(name));
        if let Some(t) = self.tab_mut() {
            t.response = Some(resp);
            t.resp_tab = 0;
        }
        self.focus = Focus::RespBody;
    }
}
