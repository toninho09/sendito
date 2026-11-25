//! Server-sent events: UTF-8 lines, persistent IDs and no automatic reconnect.

use crate::{HttpError, HttpStream, RedirectCookies, SendOptions};
use sendito_core::resolve::ResolvedHttp;

pub const MAX_EVENT_BYTES: usize = 1024 * 1024;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SseEvent {
    pub event: String,
    pub data: String,
    pub id: String,
    /// The latest server retry value. Sendito does not reconnect.
    pub retry_ms: Option<u64>,
}

#[derive(Default)]
pub struct Parser {
    line: Vec<u8>,
    seen_line: bool,
    skip_lf: bool,
    data: String,
    event: String,
    id: String,
    retry_ms: Option<u64>,
    has_data: bool,
}

impl Parser {
    pub fn new() -> Self {
        Self::default()
    }

    /// Accepts one wire byte. EOF does not dispatch an incomplete event.
    pub fn push(&mut self, byte: u8) -> Result<Option<SseEvent>, HttpError> {
        if self.skip_lf {
            self.skip_lf = false;
            if byte == b'\n' {
                return Ok(None);
            }
        }
        if byte == b'\r' || byte == b'\n' {
            self.skip_lf = byte == b'\r';
            return self.finish_line();
        }
        if self.line.len() >= MAX_EVENT_BYTES {
            return Err(limit());
        }
        self.line.push(byte);
        Ok(None)
    }

    fn finish_line(&mut self) -> Result<Option<SseEvent>, HttpError> {
        let line = std::mem::take(&mut self.line);
        let line = if !self.seen_line {
            self.seen_line = true;
            line.strip_prefix(b"\xef\xbb\xbf").unwrap_or(&line)
        } else {
            &line
        };
        let line = String::from_utf8_lossy(line);
        if line.is_empty() {
            let event = if self.has_data {
                self.data.pop();
                Some(SseEvent {
                    event: if self.event.is_empty() {
                        "message".into()
                    } else {
                        std::mem::take(&mut self.event)
                    },
                    data: std::mem::take(&mut self.data),
                    id: self.id.clone(),
                    retry_ms: self.retry_ms,
                })
            } else {
                None
            };
            self.event.clear();
            self.data.clear();
            self.has_data = false;
            return Ok(event);
        }
        if line.starts_with(':') {
            return Ok(None);
        }
        let (field, value) = line.split_once(':').unwrap_or((&line, ""));
        let value = value.strip_prefix(' ').unwrap_or(value);
        match field {
            "data" => {
                self.data.push_str(value);
                self.data.push('\n');
                self.has_data = true;
            }
            "event" => self.event = value.into(),
            "id" if !value.contains('\0') => self.id = value.into(),
            "retry" if !value.is_empty() && value.bytes().all(|b| b.is_ascii_digit()) => {
                if let Ok(ms) = value.parse() {
                    self.retry_ms = Some(ms);
                }
            }
            _ => {}
        }
        if self.data.len() + self.event.len() + self.id.len() > MAX_EVENT_BYTES {
            return Err(limit());
        }
        Ok(None)
    }
}

fn limit() -> HttpError {
    HttpError::Send("SSE event exceeds 1 MiB".into())
}

pub struct SseStream {
    pub transport: HttpStream,
    parser: Parser,
    chunk: bytes::Bytes,
    offset: usize,
}

impl SseStream {
    pub async fn next_event(&mut self) -> Result<Option<SseEvent>, HttpError> {
        loop {
            while self.offset < self.chunk.len() {
                let byte = self.chunk[self.offset];
                self.offset += 1;
                if let Some(event) = self.parser.push(byte)? {
                    return Ok(Some(event));
                }
            }
            match self.transport.chunk().await? {
                Some(chunk) => {
                    self.chunk = chunk;
                    self.offset = 0;
                }
                None => return Ok(None),
            }
        }
    }
}

/// Opens SSE with the HTTP redirect, TLS, proxy and cookie rules.
/// Only opening the response has a timeout; the stream has no total timeout.
pub async fn open(
    request: &ResolvedHttp,
    options: &SendOptions,
    cookies: Option<&dyn RedirectCookies>,
) -> Result<SseStream, HttpError> {
    let mut request = request.clone();
    if !request
        .headers
        .iter()
        .any(|(n, _)| n.eq_ignore_ascii_case("accept"))
    {
        request
            .headers
            .push(("Accept".into(), "text/event-stream".into()));
    }
    let transport = crate::open_stream(&request, options, cookies)
        .await?
        .into_stream();
    SseStream::from_transport(transport)
}

impl SseStream {
    pub fn from_transport(transport: HttpStream) -> Result<Self, HttpError> {
        if transport.status() != 200 {
            return Err(HttpError::Send(format!(
                "SSE requires HTTP 200, received {}",
                transport.status()
            )));
        }
        let headers = transport.headers();
        if !headers.iter().any(|(n, v)| {
            n.eq_ignore_ascii_case("content-type")
                && v.split(';')
                    .next()
                    .is_some_and(|v| v.trim().eq_ignore_ascii_case("text/event-stream"))
        }) {
            return Err(HttpError::Send(
                "SSE requires Content-Type: text/event-stream".into(),
            ));
        }
        if headers.iter().any(|(n, v)| {
            n.eq_ignore_ascii_case("content-encoding")
                && v.split(',')
                    .any(|encoding| !encoding.trim().eq_ignore_ascii_case("identity"))
        }) {
            return Err(HttpError::Send(
                "SSE does not support compressed streams".into(),
            ));
        }
        Ok(SseStream {
            transport,
            parser: Parser::new(),
            chunk: bytes::Bytes::new(),
            offset: 0,
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    fn parse(bytes: &[u8]) -> Vec<SseEvent> {
        let mut parser = Parser::new();
        bytes
            .iter()
            .filter_map(|b| parser.push(*b).unwrap())
            .collect()
    }

    #[test]
    fn sse_lines_bom_utf8_and_comments() {
        let events = parse(
            "\u{feff}: comment\r\ndata: héllo\rdata: world\n\nevent: update\ndata:\n\n".as_bytes(),
        );
        assert_eq!(events.len(), 2);
        assert_eq!(events[0].data, "héllo\nworld");
        assert_eq!(events[0].event, "message");
        assert_eq!(events[1].event, "update");
        assert_eq!(events[1].data, "");
        assert_eq!(parse(b"data:  two spaces\n\n")[0].data, " two spaces");
        assert_eq!(parse(b"data\n\n")[0].data, "");
    }

    #[test]
    fn sse_persistent_ids_retry_and_empty_dispatch() {
        let events = parse(b"id: 42\nretry: 123\n\ndata: one\n\nid: invalid\0id\nretry: -1\ndata: two\n\nid:\nretry: 1.2\ndata: three\n\n");
        assert_eq!(
            events.iter().map(|e| e.id.as_str()).collect::<Vec<_>>(),
            ["42", "42", ""]
        );
        assert!(events.iter().all(|e| e.retry_ms == Some(123)));
        assert!(parse(b"event: ignored\nid: saved\n\n").is_empty());
        assert!(parse(b"data: no blank line\n").is_empty());
        assert!(parse(b"data: no newline").is_empty());
        assert_eq!(
            parse(b"event: ignored\n\ndata: next\n\n")[0].event,
            "message"
        );
    }

    #[test]
    fn sse_invalid_utf8_unknown_fields_and_limits() {
        assert_eq!(
            parse(b"unknown: ignore\ndata: \xff\n\n")[0].data,
            "\u{fffd}"
        );
        let mut p = Parser::new();
        for _ in 0..MAX_EVENT_BYTES {
            p.push(b'x').unwrap();
        }
        assert!(p.push(b'x').is_err());
        let mut p = Parser::new();
        let mut failed = false;
        for _ in 0..200_000 {
            for b in b"data: abcdef\n" {
                if p.push(*b).is_err() {
                    failed = true;
                    break;
                }
            }
            if failed {
                break;
            }
        }
        assert!(failed);
    }
}
