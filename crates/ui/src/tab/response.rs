//! The response state of a tab: body viewer, stream log and query bar.

use super::*;

/// What the query bar above the response view does with its text.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum QueryMode {
    /// A jq expression filters the body.
    Jq,
    /// Plain text is searched in the view.
    Find,
}

/// The query bar drawn above the response view while a jq filter or a
/// search is typed or active.
#[derive(Debug, Clone)]
pub struct QueryBar {
    pub mode: QueryMode,
    pub input: TextEditor,
    /// The response tab the bar belongs to (jq: always the body).
    pub tab: usize,
    /// Why the typed text does not apply; the view keeps the last valid result.
    pub error: Option<String>,
    /// The jq history entry shown while Up/Down walk it (`None`: typed text).
    pub history_pos: Option<usize>,
    /// The text kept by the last Enter (or the bar's first text): Esc goes
    /// back to it.
    pub committed: String,
}

impl QueryBar {
    pub fn new(mode: QueryMode, text: &str, tab: usize) -> Self {
        let placeholder = match mode {
            QueryMode::Jq => ".items[] | select(.active)",
            QueryMode::Find => "text to find",
        };
        let mut input = TextEditor::single(text).with_placeholder(placeholder);
        input.move_to_end();
        Self {
            mode,
            input,
            tab,
            error: None,
            history_pos: None,
            committed: text.to_string(),
        }
    }
}

/// One message of a stream: a gRPC response message or a WebSocket data
/// frame (text, or binary as base64).
#[derive(Debug, Clone, Copy)]
pub struct StreamMsg<'a> {
    /// Time since the call started.
    pub at: std::time::Duration,
    pub size: usize,
    /// The whole message.
    pub text: &'a str,
    /// The message on one line, for the list.
    pub preview: &'a str,
    pub json: bool,
}

pub(super) fn grpc_message(e: &sendito_pipeline::grpc::Event) -> bool {
    matches!(e, sendito_pipeline::grpc::Event::Received { .. })
}

pub(super) fn ws_message(e: &sendito_pipeline::ws::Event) -> bool {
    matches!(e, sendito_pipeline::ws::Event::Received { kind, .. } if kind.has_payload())
}

/// Whether an event is kept for the message list or the headers and status
/// views. `Sent` events and control frames are shown in the event log only:
/// keeping them here would grow the list for the whole session.
pub(super) fn grpc_stored(e: &sendito_pipeline::grpc::Event) -> bool {
    matches!(
        e,
        sendito_pipeline::grpc::Event::Received { .. }
            | sendito_pipeline::grpc::Event::Headers { .. }
            | sendito_pipeline::grpc::Event::Finished { .. }
    )
}

pub(super) fn ws_stored(e: &sendito_pipeline::ws::Event) -> bool {
    match e {
        sendito_pipeline::ws::Event::Received { kind, .. } => kind.has_payload(),
        sendito_pipeline::ws::Event::Handshake { .. }
        | sendito_pipeline::ws::Event::Finished { .. } => true,
        _ => false,
    }
}

impl StreamMsg<'_> {
    pub fn doc(&self) -> Doc {
        Doc::from_bytes(
            self.text.as_bytes().to_vec(),
            self.json.then_some("application/json"),
        )
    }
}

/// Counts kept as stream events arrive, so each event costs the same however
/// long the stream is.
#[derive(Debug, Clone, Default)]
pub struct StreamStats {
    /// Messages received (gRPC messages, WebSocket text and binary frames),
    /// the dropped ones included.
    pub received: usize,
    pub sent: usize,
    /// Bytes received in messages.
    pub bytes: u64,
    /// When the first message arrived.
    pub first_at: Option<std::time::Duration>,
    /// When the headers (gRPC) or the handshake answer (WebSocket) arrived.
    pub opened_at: Option<std::time::Duration>,
    /// When the call or the session finished.
    pub closed_at: Option<std::time::Duration>,
    /// The oldest messages no longer kept, because of `[ui] stream_messages_kept`.
    pub dropped: usize,
}

impl StreamStats {
    /// Messages still in the list.
    pub fn kept(&self) -> usize {
        self.received - self.dropped
    }

    fn received(&mut self, size: usize, at: std::time::Duration) {
        self.received += 1;
        self.bytes += size as u64;
        self.first_at.get_or_insert(at);
    }
}

#[derive(Debug, Clone)]
pub struct Response {
    pub summary: Summary,
    pub body: Viewer,
    pub headers: Viewer,
    pub timing: Viewer,
    pub messages: Viewer,
    pub grpc_events: VecDeque<sendito_pipeline::grpc::Event>,
    pub grpc_kind: Option<sendito_pipeline::grpc::CallKind>,
    pub ws_events: VecDeque<sendito_pipeline::ws::Event>,
    pub stream: StreamStats,
    /// Short outcome of the assertions, shown on the Results tab: "2/3 ✗", "3/3 ✓".
    pub results_badge: Option<String>,
    /// The jq expression filtering the body for display, if any.
    pub jq: Option<String>,
    /// The unfiltered body bytes, captured when the first filter is applied.
    pub jq_src: Option<Vec<u8>>,
    /// How many values the jq filter gave.
    pub jq_outputs: usize,
    /// The query bar above the view, if open.
    pub query: Option<QueryBar>,
    /// The stream message the body shows; `None` follows the last one (live).
    pub stream_sel: Option<usize>,
}

impl Response {
    pub fn new() -> Self {
        Self {
            summary: Summary::default(),
            body: Viewer::empty(),
            headers: wrapped(Doc::text("")),
            timing: wrapped(Doc::text("")),
            messages: wrapped(Doc::text("")),
            grpc_events: VecDeque::new(),
            grpc_kind: None,
            ws_events: VecDeque::new(),
            stream: StreamStats::default(),
            results_badge: None,
            jq: None,
            jq_src: None,
            jq_outputs: 0,
            query: None,
            stream_sel: None,
        }
    }

    /// Replaces the body document (a response, message or call result); any
    /// jq filter no longer applies to the new document.
    pub fn set_body(&mut self, doc: Doc) {
        self.jq = None;
        self.jq_src = None;
        self.query = None;
        self.stream_sel = None;
        self.body = Viewer::new(doc);
    }

    /// Replaces the body with wrapped text (a failure report); any jq filter
    /// no longer applies to it.
    pub fn set_body_wrapped(&mut self, text: String) {
        self.set_body(Doc::text(&text));
        self.body.wrap = true;
    }

    /// Applies (or, with an empty expression, removes) the jq filter over the
    /// body and rebuilds the viewer. The original bytes are captured when the
    /// first filter succeeds, so a new expression re-filters the original
    /// body and clearing restores it.
    ///
    /// Returns whether a filter is active afterwards.
    pub fn apply_jq(&mut self, expr: &str) -> Result<bool, String> {
        let expr = expr.trim();
        if expr.is_empty() {
            self.jq = None;
            if let Some(src) = self.jq_src.take() {
                self.swap_body(Doc::from_bytes(src, Some("application/json")));
            }
            return Ok(false);
        }
        let outs = match self.jq_src.as_deref() {
            Some(src) => crate::filter::apply(expr, src),
            None => {
                // Capture the original bytes only when the filter succeeds,
                // so a failed first filter leaves no half-state behind.
                let src = self.body.doc.bytes().to_vec();
                let outs = crate::filter::apply(expr, &src);
                if outs.is_ok() {
                    self.jq_src = Some(src);
                }
                outs
            }
        };
        let outs = outs?;
        self.jq = Some(expr.to_string());
        self.jq_outputs = outs.len();
        let doc = match outs.as_slice() {
            [] => Doc::text("(no output)\n"),
            [one] => Doc::from_bytes(one.clone(), Some("application/json")),
            many => Doc::json_stream(many),
        };
        self.swap_body(doc);
        Ok(true)
    }

    /// Adds a gRPC event. At most `keep` messages stay; the oldest go first.
    pub fn push_grpc(&mut self, ev: sendito_pipeline::grpc::Event, keep: usize) {
        let message = match &ev {
            sendito_pipeline::grpc::Event::Received { size, at, .. } => {
                self.stream.received(*size, *at);
                true
            }
            sendito_pipeline::grpc::Event::Sent { .. } => {
                self.stream.sent += 1;
                false
            }
            sendito_pipeline::grpc::Event::Headers { at, .. } => {
                self.stream.opened_at = Some(*at);
                false
            }
            sendito_pipeline::grpc::Event::Finished { at, .. } => {
                self.stream.closed_at = Some(*at);
                false
            }
            _ => false,
        };
        if !grpc_stored(&ev) {
            return;
        }
        self.grpc_events.push_back(ev);
        if message {
            while self.stream.kept() > keep.max(1) {
                let Some(i) = self.grpc_events.iter().position(grpc_message) else {
                    break;
                };
                self.grpc_events.remove(i);
                self.dropped_one();
            }
            self.message_arrived();
        }
    }

    /// Adds a WebSocket event. At most `keep` messages stay; the oldest go first.
    pub fn push_ws(&mut self, ev: sendito_pipeline::ws::Event, keep: usize) {
        let message = match &ev {
            sendito_pipeline::ws::Event::Received { kind, size, at, .. } if kind.has_payload() => {
                self.stream.received(*size, *at);
                true
            }
            sendito_pipeline::ws::Event::Sent { kind, .. } if kind.has_payload() => {
                self.stream.sent += 1;
                false
            }
            sendito_pipeline::ws::Event::Handshake { at, .. } => {
                self.stream.opened_at = Some(*at);
                false
            }
            sendito_pipeline::ws::Event::Finished { at, .. } => {
                self.stream.closed_at = Some(*at);
                false
            }
            _ => false,
        };
        if !ws_stored(&ev) {
            return;
        }
        self.ws_events.push_back(ev);
        if message {
            while self.stream.kept() > keep.max(1) {
                let Some(i) = self.ws_events.iter().position(ws_message) else {
                    break;
                };
                self.ws_events.remove(i);
                self.dropped_one();
            }
            self.message_arrived();
        }
    }

    /// The oldest message left the list: a paused selection keeps pointing
    /// at the same message.
    fn dropped_one(&mut self) {
        self.stream.dropped += 1;
        if let Some(sel) = self.stream_sel.as_mut() {
            *sel = sel.saturating_sub(1);
        }
    }

    /// The messages a stream kept, in order, without collecting them.
    pub fn messages(&self) -> impl DoubleEndedIterator<Item = StreamMsg<'_>> {
        let grpc = self.grpc_events.iter().filter_map(|e| match e {
            sendito_pipeline::grpc::Event::Received {
                json,
                compact,
                size,
                at,
                ..
            } => Some(StreamMsg {
                at: *at,
                size: *size,
                text: json,
                preview: compact,
                json: true,
            }),
            _ => None,
        });
        let ws = self.ws_events.iter().filter_map(|e| match e {
            sendito_pipeline::ws::Event::Received {
                kind,
                preview,
                size,
                at,
                ..
            } if kind.has_payload() => Some(StreamMsg {
                at: *at,
                size: *size,
                text: preview,
                preview,
                json: false,
            }),
            _ => None,
        });
        grpc.chain(ws)
    }

    /// The messages a stream kept, in order.
    pub fn stream_messages(&self) -> Vec<StreamMsg<'_>> {
        self.messages().collect()
    }

    /// A new stream message arrived: the body shows it in live mode.
    pub fn message_arrived(&mut self) {
        if self.stream_sel.is_none() {
            self.set_message(usize::MAX);
        }
    }

    /// Shows stream message `i` (the last one for a larger index); the last
    /// message turns live mode on.
    pub fn set_message(&mut self, i: usize) {
        let Some(last) = self.stream.kept().checked_sub(1) else {
            return;
        };
        let i = i.min(last);
        // The last message is found from the end: live mode stays cheap.
        let msg = if i == last {
            self.messages().next_back()
        } else {
            self.messages().nth(i)
        };
        let Some(doc) = msg.map(|m| m.doc()) else {
            return;
        };
        self.stream_sel = (i < last).then_some(i);
        self.show_message(doc);
    }

    /// Moves to the previous (`-1`) or next (`1`) stream message. Returns
    /// false when there is none.
    pub fn select_message(&mut self, step: isize) -> bool {
        let n = self.stream.kept();
        let cur = self.stream_sel.unwrap_or(n.saturating_sub(1)) as isize;
        let next = cur + step;
        if n < 2 || next < 0 || next >= n as isize {
            return false;
        }
        self.set_message(next as usize);
        true
    }

    /// Puts a stream message in the body. The view mode, the jq filter, the
    /// search and the query bar stay; a jq error goes to the bar and the body
    /// shows the message unfiltered.
    fn show_message(&mut self, doc: Doc) {
        let expr = self.jq.take();
        self.jq_src = None;
        // The first message opens in its own default view (pretty for JSON).
        if self.body.doc.is_empty() {
            self.body = Viewer::new(doc);
        } else {
            self.swap_body(doc);
        }
        if let Some(e) = expr {
            let err = self.apply_jq(&e).err();
            if err.is_some() {
                self.jq = Some(e);
            }
            if let Some(q) = self.query.as_mut().filter(|q| q.mode == QueryMode::Jq) {
                q.error = err;
            }
        }
        if let Some(q) = self
            .query
            .as_ref()
            .filter(|q| q.mode == QueryMode::Find && q.tab == 0)
        {
            let text = q.input.text();
            self.body.search(&text);
        }
    }

    /// Whether the body is small enough to filter on every key press; a
    /// larger body is filtered when Enter is pressed.
    pub fn live_jq(&self) -> bool {
        self.jq_src.as_ref().map_or(self.body.doc.len(), Vec::len) <= LIVE_JQ_MAX
    }

    /// Runs the query bar's text: a jq filter over the body, or a search in
    /// the view. A jq error is kept on the bar and the body keeps the last
    /// valid result.
    pub fn run_query(&mut self) {
        let Some(q) = &self.query else {
            return;
        };
        let (text, mode, tab) = (q.input.text(), q.mode, q.tab);
        let error = match mode {
            QueryMode::Jq => self.apply_jq(&text).err(),
            QueryMode::Find => {
                self.viewer_mut(tab).search(&text);
                None
            }
        };
        if let Some(q) = &mut self.query {
            q.error = error;
        }
    }

    /// Takes over how `old` showed its body: view mode, wrap, line numbers,
    /// the jq filter and the position. A watch repeat uses it so the view
    /// does not jump back to the raw body at line 1 on every send.
    pub fn keep_view_of(&mut self, old: &Response) {
        // A blank response (a send in progress) has no view to keep.
        if old.summary.status.is_empty() {
            return;
        }
        self.body.mode = old.body.mode;
        self.body.wrap = old.body.wrap;
        self.body.numbers = old.body.numbers;
        if let Some(expr) = &old.jq {
            let _ = self.apply_jq(expr);
        }
        self.query = old.query.clone();
        self.stream_sel = old.stream_sel;
        // Rows differ when the old view wraps: carry the line. The new viewer
        // has no rows yet, so its cursor is a line until it renders.
        let line = old.body.cursor_line();
        self.body.cursor = line;
        self.body.scroll = line.saturating_sub(old.body.cursor.saturating_sub(old.body.scroll));
        self.body.hscroll = old.body.hscroll;
    }

    /// Swaps the body viewer's document, keeping the view mode, wrap and line
    /// numbers.
    fn swap_body(&mut self, doc: Doc) {
        let mut v = Viewer::new(doc);
        v.mode = self.body.mode;
        v.wrap = self.body.wrap;
        v.numbers = self.body.numbers;
        self.body = v;
    }

    pub fn viewer_mut(&mut self, tab: usize) -> &mut Viewer {
        match tab {
            0 => &mut self.body,
            1 => &mut self.headers,
            2 => &mut self.timing,
            _ => &mut self.messages,
        }
    }
}

/// A viewer that wraps long lines (used for text panes).
pub fn wrapped(doc: Doc) -> Viewer {
    let mut v = Viewer::new(doc);
    v.wrap = true;
    v
}

/// The path of a URL (without scheme, host or a leading `{{base}}` variable, and
/// without query); the whole URL when it has no path.
pub fn url_path(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, r)| r);
    let rest = if rest.starts_with("{{") {
        rest.split_once("}}").map_or(rest, |(_, r)| r)
    } else {
        rest.find('/').map_or("", |i| &rest[i..])
    };
    let path = rest.split(['?', '#']).next().unwrap_or("");
    if path.is_empty() || path == "/" {
        url.to_string()
    } else {
        path.to_string()
    }
}

/// Formats milliseconds: one decimal below 10 ms.
pub fn fmt_ms(ms: f64) -> String {
    if ms < 10.0 {
        format!("{ms:.1} ms")
    } else {
        format!("{} ms", ms.round() as u64)
    }
}

impl Default for Response {
    fn default() -> Self {
        Self::new()
    }
}
