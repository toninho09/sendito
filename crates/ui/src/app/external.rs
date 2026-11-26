//! `:edit` and `:pager`: the requests for a program outside the UI, and the
//! result when it ends. The event loop runs the program
//! (`crate::external::run`).

use super::*;
use crate::external::External;

impl App {
    /// `:edit` (Ctrl+W O): edits the focused multi-line text in `$VISUAL` or
    /// `$EDITOR`.
    pub(super) fn edit_externally(&mut self) {
        let field = self.tab().and_then(|t| t.current_field());
        let graphql_query = self
            .tab()
            .is_some_and(|t| t.ed.body_kind == BodyKind::GraphQL && t.ed.graphql_section == 0);
        let Some(e) = self.focused_text() else {
            return self.set_status(
                "focus a text to edit first: a body, a script or a message",
                StatusKind::Info,
            );
        };
        if e.single_line || e.read_only || e.mask {
            return self.set_status("this field is one line: edit it here", StatusKind::Info);
        }
        let ext = match field {
            Some(Field::ScriptCode) => "lua",
            Some(Field::BodyText) if graphql_query => "graphql",
            _ if e.highlight == crate::editor::Highlight::Json => "json",
            _ => "txt",
        };
        self.external = Some(External::Edit {
            text: e.text(),
            ext,
        });
    }

    /// `:pager` (o in the response): shows the response pane's text in
    /// `$PAGER`, as on screen (secrets masked, jq filter applied).
    pub(super) fn open_pager(&mut self) {
        match self.shown_response_text() {
            Some(text) => self.external = Some(External::Pager { text }),
            None => self.set_status("no response to show", StatusKind::Info),
        }
    }

    /// The result of the program the event loop ran: an edited text replaces
    /// the focused text.
    pub fn finish_external(&mut self, result: Result<Option<String>, String>) {
        let text = match result {
            Ok(Some(text)) => text,
            Ok(None) => return,
            Err(e) => return self.set_status(e, StatusKind::Error),
        };
        let Some(e) = self.focused_text() else {
            return self.set_status(
                "the edited text was not used: the field is gone",
                StatusKind::Warn,
            );
        };
        if e.text() == text {
            return self.set_status("no change from the editor", StatusKind::Info);
        }
        e.replace_all(&text);
        self.changed();
        self.set_status("text from the editor", StatusKind::Ok);
    }
}
