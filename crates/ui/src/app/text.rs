//! Text builders for reports, timing, headers, events and help. They do not use `App`.

use super::*;

/// What confirming a prompt does, for its button and the hint row.
pub fn prompt_verb(a: &PromptAction) -> &'static str {
    match a {
        PromptAction::SaveAs { .. } | PromptAction::SaveHistory(_) | PromptAction::SaveBody => {
            "save"
        }
        PromptAction::NewFolder { .. } | PromptAction::NewRequest { .. } | PromptAction::NewEnv => {
            "create"
        }
        PromptAction::Rename(_) => "rename",
        PromptAction::CustomMethod => "use",
        PromptAction::McpServer { .. } => "add",
        PromptAction::Setting(_) => "save",
    }
}

/// A short, plain reason for a transport error (the full text follows it).
/// A few words for the status line.
pub(super) fn short_error(e: &str) -> &'static str {
    let l = e.to_lowercase();
    if l.contains("connection refused") || l.contains("os error 111") || l.contains("os error 61") {
        "connection refused"
    } else if l.contains("dns error")
        || l.contains("failed to lookup")
        || l.contains("name or service not known")
    {
        "host name not found (DNS)"
    } else if l.contains("timed out") || l.contains("timeout") {
        "timed out"
    } else if l.contains("certificate") || l.contains("tls") || l.contains("handshake") {
        "TLS failure"
    } else if l.contains("connection reset") || l.contains("broken pipe") {
        "connection closed by the server"
    } else if l.contains("no such file") || l.contains("os error 2") {
        "file not found"
    } else {
        "request failed"
    }
}

pub(super) fn explain_error(e: &str) -> String {
    let l = e.to_lowercase();
    let host = l
        .split("for url (")
        .nth(1)
        .and_then(|r| r.split(')').next())
        .unwrap_or("");
    let target = if host.is_empty() {
        "the server".to_string()
    } else {
        host.to_string()
    };
    if l.contains("connection refused") || l.contains("os error 111") || l.contains("os error 61") {
        format!("nothing is listening at {target} (connection refused).")
    } else if l.contains("dns error")
        || l.contains("failed to lookup")
        || l.contains("name or service not known")
    {
        "the host name could not be resolved (DNS).".into()
    } else if l.contains("timed out") || l.contains("timeout") {
        "the server did not answer in time (timeout).".into()
    } else if l.contains("certificate") || l.contains("tls") || l.contains("handshake") {
        "the TLS connection failed (certificate or handshake); Options can turn off TLS verification.".into()
    } else if l.contains("connection reset") || l.contains("broken pipe") {
        "the server closed the connection.".into()
    } else if l.contains("no such file") || l.contains("os error 2") {
        "a file the request sends was not found.".into()
    } else {
        "see the details below.".into()
    }
}

/// "1 assertion", "3 assertions".
pub(super) fn assertions(n: usize) -> String {
    if n == 1 {
        "1 assertion".into()
    } else {
        format!("{n} assertions")
    }
}

/// `path` with the home directory written as `~`.
pub fn display_path(path: &Path, home: Option<&Path>) -> String {
    match home.and_then(|h| path.strip_prefix(h).ok()) {
        Some(rest) => format!("~/{}", rest.display()),
        None => path.display().to_string(),
    }
}

pub(super) fn ms(d: Option<Duration>) -> String {
    d.map(|d| format!("{:.1} ms", d.as_secs_f64() * 1000.0))
        .unwrap_or_else(|| "—".into())
}

/// Width of the timing waterfall, in cells.
pub(super) const TRACK: usize = 32;

/// One phase on the waterfall: dots for the timeline, blocks from `start`
/// for `len`, with eighth-of-a-cell precision at the end.
pub(super) fn waterfall(start: Duration, len: Duration, total: Duration) -> String {
    let t = total.as_secs_f64();
    if t <= 0.0 {
        return "·".repeat(TRACK);
    }
    let scale = (TRACK * 8) as f64 / t;
    let s8 = ((start.as_secs_f64() * scale).round() as usize).min(TRACK * 8 - 1);
    let mut e8 = (((start + len).as_secs_f64() * scale).round() as usize).min(TRACK * 8);
    if !len.is_zero() && e8 <= s8 {
        e8 = s8 + 1;
    }
    const LEFT: [char; 7] = ['▏', '▎', '▍', '▌', '▋', '▊', '▉'];
    (0..TRACK)
        .map(|cell| {
            let (c0, c1) = (cell * 8, cell * 8 + 8);
            let (from, to) = (s8.max(c0), e8.min(c1));
            if to <= from {
                '·'
            } else if from == c0 && to == c1 {
                '█'
            } else if from == c0 {
                LEFT[to - c0 - 1]
            } else if to == c1 {
                // Starts inside the cell: right-aligned blocks only come in halves.
                if c1 - from >= 6 {
                    '█'
                } else if c1 - from >= 3 {
                    '▐'
                } else {
                    '▕'
                }
            } else {
                '▎'
            }
        })
        .collect()
}

pub fn timing_text(r: &sendito_http::HttpResponse, body_size: u64) -> String {
    let t = r.timing;
    let total = t.total;
    // A phase that did not happen says why, on its own row. Phases follow
    // each other, so each starts where the previous one ended.
    let rows = [
        ("DNS lookup", t.dns, "skipped (IP address)"),
        ("TCP connect", t.connect, "skipped (connection reused)"),
        ("TLS handshake", t.tls, "skipped (not HTTPS)"),
        ("Waiting for first byte", Some(t.ttfb), ""),
        ("Download", Some(t.download), ""),
    ];
    let total_label = ms(Some(total));
    let scale = format!(
        "0{}{}",
        " ".repeat(TRACK.saturating_sub(1 + total_label.trim().len())),
        total_label.trim()
    );
    let mut s = format!("{:<22} {:>9}  {scale}\n", "Phase", "Time");
    let mut at = Duration::ZERO;
    for (label, d, why) in rows {
        match d {
            Some(d) => {
                s.push_str(&format!(
                    "{label:<22} {:>9}  {}\n",
                    ms(Some(d)),
                    waterfall(at, d, total)
                ));
                at += d;
            }
            None => s.push_str(&format!("{label:<22} {:>9}  {why}\n", "—")),
        }
    }
    s.push_str(&format!("{:<22} {:>9}\n\n", "Total", total_label));
    s.push_str(&format!("Protocol        {}\n", r.version));
    if let Some(a) = r.remote_addr {
        s.push_str(&format!("Remote address  {a}\n"));
    }
    s.push_str(&format!("Final URL       {}\n", r.url));
    s.push_str(&format!(
        "Body size       {} ({} bytes)\n",
        human_size(body_size),
        body_size
    ));

    s
}

/// The watch section of the Timing tab: one mark per send while watching
/// (✗ failed, blocks show duration, tallest = slowest).
pub(super) fn watch_text(samples: &[(bool, f64)]) -> String {
    const BLOCKS: [char; 8] = ['▁', '▂', '▃', '▄', '▅', '▆', '▇', '█'];
    let max = samples.iter().map(|(_, ms)| *ms).fold(0.0_f64, f64::max);
    let marks: String = samples
        .iter()
        .map(|&(ok, ms)| {
            if !ok {
                '✗'
            } else if max <= 0.0 {
                BLOCKS[0]
            } else {
                BLOCKS[(((ms / max) * 7.0).clamp(0.0, 7.0)) as usize]
            }
        })
        .collect();
    let fails = samples.iter().filter(|(ok, _)| !ok).count();
    let last = samples
        .last()
        .map(|&(ok, ms)| {
            if ok {
                format!("{:.0} ms", ms)
            } else {
                "failed".into()
            }
        })
        .unwrap_or_else(|| "-".into());
    format!(
        "\nWatch\n\n  {marks}\n\n  {} sends · {fails} failed · last {last}\n  ✗ failed send (error or status 400+) · blocks show duration\n",
        samples.len()
    )
}

pub(super) fn request_headers_text(o: &HttpOutcome) -> String {
    let mut s = format!("Request sent\n{} {}\n", o.resolved.method, o.resolved.url);
    for (n, v) in &o.resolved.headers {
        s.push_str(&format!("{n}: {}\n", mask_header(n, v)));
    }
    s
}

/// Authorization values are shown masked in the UI.
pub(super) fn mask_header(name: &str, value: &str) -> String {
    let n = name.to_ascii_lowercase();
    if n == "authorization"
        || n == "proxy-authorization"
        || n.contains("api-key")
        || n.contains("token")
        || n == "cookie"
    {
        let scheme = value
            .split_whitespace()
            .next()
            .filter(|_| value.contains(' '))
            .unwrap_or("");
        if scheme.is_empty() {
            crate::MASK.to_string()
        } else {
            format!("{scheme} {}", crate::MASK)
        }
    } else {
        value.to_string()
    }
}

pub(super) fn headers_text(r: &sendito_http::HttpResponse, o: &HttpOutcome) -> String {
    let mut s = format!("{} {} {}\n", r.version, r.status, r.reason);
    for (n, v) in &r.headers {
        s.push_str(&format!("{n}: {v}\n"));
    }
    s.push('\n');
    s.push_str(&request_headers_text(o));
    s
}

pub(super) fn messages_text(
    sc: &runner::ScriptResults,
    scope_errors: &[String],
    notes: &[String],
) -> String {
    let mut s = String::new();
    if !sc.asserts.is_empty() {
        let pass = sc.asserts.iter().filter(|a| a.pass).count();
        s.push_str(&format!("Assertions: {pass}/{} passed\n", sc.asserts.len()));
        for a in &sc.asserts {
            if a.pass {
                s.push_str(&format!("  ✓ PASS  {}\n", a.label));
            } else {
                s.push_str(&format!("  ✗ FAIL  {}   (actual: {})\n", a.label, a.actual));
            }
        }
        s.push('\n');
    }
    if !sc.pre.is_empty() {
        s.push_str("Pre-request variables:\n");
        for (k, v) in &sc.pre {
            s.push_str(&format!(
                "  {k} = {}\n",
                if v.secret {
                    crate::MASK.to_string()
                } else {
                    v.value.clone()
                }
            ));
        }
        s.push('\n');
    }
    if !sc.extracts.is_empty() {
        s.push_str("Extracted variables:\n");
        for e in &sc.extracts {
            match &e.value {
                Ok(v) => s.push_str(&format!(
                    "  {} = {}\n",
                    e.var,
                    if v.secret {
                        crate::MASK.to_string()
                    } else {
                        v.value.clone()
                    }
                )),
                Err(err) => s.push_str(&format!("  {}: {err}\n", e.var)),
            }
        }
        s.push('\n');
    }
    for e in scope_errors {
        s.push_str(&format!("warning: {e}\n"));
    }
    for n in notes {
        s.push_str(&format!("note: {n}\n"));
    }
    if s.is_empty() {
        s.push_str("No scripts or assertions for this request. Add them in the Scripts tab.\n");
    }
    s
}

pub(super) fn clock(t: std::time::SystemTime) -> String {
    let dt: time::OffsetDateTime = t.into();
    format!(
        "{:02}:{:02}:{:02}.{:03}",
        dt.hour(),
        dt.minute(),
        dt.second(),
        dt.millisecond()
    )
}

pub(super) fn mcp_argument_template(schema: &serde_json::Value) -> String {
    fn sample(schema: &serde_json::Value, depth: usize) -> serde_json::Value {
        if let Some(value) = schema.get("default").or_else(|| {
            schema
                .get("enum")
                .and_then(|v| v.as_array())
                .and_then(|v| v.first())
        }) {
            return value.clone();
        }
        if depth >= 8 {
            return serde_json::Value::Null;
        }
        match schema.get("type").and_then(|v| v.as_str()) {
            Some("object") => {
                let mut out = serde_json::Map::new();
                if let Some(properties) = schema.get("properties").and_then(|v| v.as_object()) {
                    for (name, value) in properties {
                        out.insert(name.clone(), sample(value, depth + 1));
                    }
                }
                serde_json::Value::Object(out)
            }
            Some("array") => serde_json::json!([]),
            Some("string") => serde_json::json!(""),
            Some("integer" | "number") => serde_json::json!(0),
            Some("boolean") => serde_json::json!(false),
            _ => serde_json::Value::Null,
        }
    }
    let value = sample(schema, 0);
    if value.is_object() {
        serde_json::to_string_pretty(&value).unwrap_or_else(|_| "{}".into())
    } else {
        "{}".into()
    }
}

/// One MCP event's log line (the Response "Log" tab).
pub fn mcp_event_text(e: &runner::McpEvent) -> String {
    use runner::McpEvent::*;
    match e {
        Sent { json, at, wall } => format!(
            "{} +{:>8.3}s  \u{2192} sent      {json}\n",
            clock(*wall),
            at.as_secs_f64()
        ),
        Received { json, at, wall } => format!(
            "{} +{:>8.3}s  \u{2190} received  {json}\n",
            clock(*wall),
            at.as_secs_f64()
        ),
        Log { line, at } => format!("{:12} +{:>8.3}s  \u{2502} {line}\n", "", at.as_secs_f64()),
        Finished { ok, message, at } => {
            let tail = if *ok {
                String::new()
            } else if message.is_empty() {
                "failed".into()
            } else {
                format!("failed: {message}")
            };
            format!("{:12} +{:>8.3}s  \u{25a0} {tail}\n", "", at.as_secs_f64())
        }
    }
}

/// The JSON-RPC line `on_mcp` keeps on the "Request" tab, if this event sent one.
pub fn mcp_sent_line(e: &runner::McpEvent) -> Option<&str> {
    match e {
        runner::McpEvent::Sent { json, .. } => Some(json),
        _ => None,
    }
}

pub fn event_text(e: &sendito_grpc::Event) -> String {
    use sendito_grpc::Event::*;
    match e {
        Sent { json, at, wall } => format!(
            "{} +{:>8.3}s  → sent      {json}\n",
            clock(*wall),
            at.as_secs_f64()
        ),
        SendClosed { at } => format!(
            "{:12} +{:>8.3}s  → end of request stream\n",
            "",
            at.as_secs_f64()
        ),
        Headers { metadata, at } => {
            let md: Vec<String> = metadata.iter().map(|(k, v)| format!("{k}={v}")).collect();
            format!(
                "{:12} +{:>8.3}s  ← headers   {}\n",
                "",
                at.as_secs_f64(),
                md.join(" ")
            )
        }
        Received {
            compact: c,
            size,
            at,
            wall,
            ..
        } => format!(
            "{} +{:>8.3}s  ← received  ({size} B) {c}\n",
            clock(*wall),
            at.as_secs_f64()
        ),
        Finished {
            code, message, at, ..
        } => {
            let name = sendito_grpc::code_name(*code);
            format!(
                "{:12} +{:>8.3}s  ■ status    {name} ({code}){}{message}\n",
                "",
                at.as_secs_f64(),
                if message.is_empty() { "" } else { ": " }
            )
        }
    }
}

pub(super) fn grpc_headers_text<'a>(
    events: impl IntoIterator<Item = &'a sendito_grpc::Event>,
) -> String {
    let mut s = String::new();
    for e in events {
        match e {
            sendito_grpc::Event::Headers { metadata, .. } => {
                s.push_str("Response headers\n");
                for (k, v) in metadata {
                    s.push_str(&format!("{k}: {v}\n"));
                }
                s.push('\n');
            }
            sendito_grpc::Event::Finished {
                code,
                message,
                metadata,
                ..
            } => {
                s.push_str("Trailers\n");
                s.push_str(&format!(
                    "grpc-status: {code} ({})\n",
                    sendito_grpc::code_name(*code)
                ));
                if !message.is_empty() {
                    s.push_str(&format!("grpc-message: {message}\n"));
                }
                for (k, v) in metadata {
                    s.push_str(&format!("{k}: {v}\n"));
                }
            }
            _ => {}
        }
    }
    s
}

pub(super) fn grpc_timing_text(st: &crate::tab::StreamStats) -> String {
    stream_timing_text(
        st,
        [
            ("Headers received  ", 10),
            ("Finished          ", 10),
            ("First message     ", 10),
        ],
        &format!(
            "Messages sent     {}\nMessages received {}",
            st.sent, st.received
        ),
    )
}

/// The timing lines of a stream: when it opened, when it closed, when the
/// first message came, then the counts.
fn stream_timing_text(
    st: &crate::tab::StreamStats,
    labels: [(&str, usize); 3],
    counts: &str,
) -> String {
    let mut s = String::new();
    for ((label, w), at) in labels.iter().zip([st.opened_at, st.closed_at, st.first_at]) {
        if let Some(at) = at {
            s.push_str(&format!("{label}{:>w$.1} ms\n", at.as_secs_f64() * 1000.0));
        }
    }
    s.push_str(&format!("\n{counts}\n"));
    s
}

pub(super) fn ws_event_text(e: &sendito_ws::Event) -> String {
    use sendito_ws::Event::*;
    match e {
        Handshake {
            status,
            subprotocol,
            at,
            ..
        } => format!(
            "{:12} +{:>8.3}s  ← handshake  {status}{}\n",
            "",
            at.as_secs_f64(),
            subprotocol
                .as_ref()
                .map(|s| format!(" ({s})"))
                .unwrap_or_default()
        ),
        Sent {
            kind,
            preview,
            size,
            at,
            wall,
        } => format!(
            "{} +{:>8.3}s  → sent {}     ({size} B) {}\n",
            clock(*wall),
            at.as_secs_f64(),
            kind.as_str(),
            preview
        ),
        Received {
            kind,
            preview,
            size,
            at,
            wall,
        } => format!(
            "{} +{:>8.3}s  ← received {} ({size} B) {}\n",
            clock(*wall),
            at.as_secs_f64(),
            kind.as_str(),
            preview
        ),
        Finished { code, reason, at } => format!(
            "{:12} +{:>8.3}s  ■ close      {code}{}{reason}\n",
            "",
            at.as_secs_f64(),
            if reason.is_empty() { "" } else { ": " }
        ),
    }
}

pub(super) fn ws_headers_text(headers: &[(String, String)]) -> String {
    let mut s = String::from("Handshake response\n");
    for (k, v) in headers {
        s.push_str(&format!("{k}: {v}\n"));
    }
    s
}

pub(super) fn ws_timing_text(st: &crate::tab::StreamStats) -> String {
    stream_timing_text(
        st,
        [
            ("Handshake answered ", 7),
            ("Closed            ", 10),
            ("First frame       ", 10),
        ],
        &format!(
            "Frames sent       {}\nFrames received  {}",
            st.sent, st.received
        ),
    )
}

pub(super) fn help_text(km: &Keymap, vim: bool) -> String {
    let mut s = String::from("All keys, including any you changed in the config.\n");
    s.push_str(&format!(
        "Current mode: {} (switch in Settings, {}).\n",
        if vim { "vim" } else { "conventional" },
        km.hint(Action::Settings, vim)
    ));
    let mut cur = None;
    for (ctx, a, conv, vimk) in km.rows() {
        if cur != Some(ctx) {
            cur = Some(ctx);
            s.push_str(&format!("\n── {} ──\n", ctx.title()));
        }
        let keys = if vim { vimk } else { conv };
        if keys.is_empty() {
            continue;
        }
        s.push_str(&format!("  {:<28} {}\n", keys, a.description()));
    }
    // Fixed keys, one section per place they work in.
    let mut place = "";
    let kw = crate::keymap::FIXED_KEYS
        .iter()
        .map(|(_, k, _)| k.chars().count())
        .max()
        .unwrap_or(28)
        .max(28);
    for (w, k, d) in crate::keymap::FIXED_KEYS {
        if *w != place {
            place = w;
            s.push_str(&format!("\n── Fixed keys · {w} ──\n"));
        }
        s.push_str(&format!("  {k:<kw$}  {d}\n"));
    }
    s.push_str("\n── Symbols ──\n");
    for (sym, what) in crate::keymap::SYMBOLS {
        s.push_str(&format!("  {sym:<28} {what}\n"));
    }
    s.push_str("\n── Commands (command palette) ──\n");
    for (c, d) in crate::overlay::COMMANDS {
        s.push_str(&format!("  {c:<28} {d}\n"));
    }
    s
}
