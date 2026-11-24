//! Response body decoding and file request bodies.

use super::*;

/// Decompresses a response body according to its `Content-Encoding` header.
/// Only whole-body encodings Sendito can decode are touched: `gzip`,
/// `deflate` (zlib, with a raw-deflate fallback) and `br`. Anything else
/// (multiple codings, `zstd`, unknown names) and a stream that does not
/// decompress cleanly is returned untouched, so what is shown is what was
/// received. Sendito never advertises `Accept-Encoding`, so this only decodes
/// what the server chose to send.
pub(super) fn decode_body(
    headers: &[(String, String)],
    body: Vec<u8>,
    max_body: usize,
) -> (Vec<u8>, Option<Decoded>, bool) {
    let mut encodings = headers
        .iter()
        .filter(|(n, _)| n.eq_ignore_ascii_case("content-encoding"));
    let enc = encodings
        .next()
        .map(|(_, v)| v.trim().to_ascii_lowercase())
        .unwrap_or_default();
    // Multiple header fields form a list of codings; do not partially decode.
    if body.is_empty() || encodings.next().is_some() {
        return (body, None, false);
    }
    let raw_len = body.len() as u64;
    let decoded = match enc.as_str() {
        "gzip" => read_limited(flate2::read::MultiGzDecoder::new(&body[..]), max_body),
        "deflate" => read_limited(flate2::read::ZlibDecoder::new(&body[..]), max_body)
            .or_else(|| read_limited(flate2::read::DeflateDecoder::new(&body[..]), max_body)),
        "br" => read_limited(brotli::Decompressor::new(&body[..], 4096), max_body),
        _ => None,
    };
    match decoded {
        Some((b, truncated)) => (
            b,
            Some(Decoded {
                encoding: enc,
                raw_len,
            }),
            truncated,
        ),
        None => (body, None, false),
    }
}

pub(super) fn read_limited(r: impl std::io::Read, max_body: usize) -> Option<(Vec<u8>, bool)> {
    use std::io::Read;
    let limit = (max_body as u64).saturating_add(1);
    let mut out = Vec::new();
    r.take(limit).read_to_end(&mut out).ok()?;
    let truncated = out.len() > max_body;
    out.truncate(max_body);
    Some((out, truncated))
}

/// Guesses a MIME type from a file name.
pub fn guess_mime(filename: &str) -> String {
    mime_guess::from_path(filename)
        .first_raw()
        .unwrap_or("application/octet-stream")
        .to_string()
}

/// A file as a body that is read while it is sent, and its size. The file is
/// opened again for each send, so a redirect hop sends it again.
pub(super) async fn file_body(path: &std::path::Path) -> Result<(reqwest::Body, u64), HttpError> {
    let err = |e: std::io::Error| HttpError::File {
        path: path.display().to_string(),
        msg: e.to_string(),
    };
    let file = tokio::fs::File::open(path).await.map_err(err)?;
    let len = file.metadata().await.map_err(err)?.len();
    let body = FileBody {
        stream: tokio_util::io::ReaderStream::new(file),
        len,
    };
    Ok((reqwest::Body::wrap(body), len))
}

/// A file read in chunks while it is sent. Its exact size goes to the HTTP
/// layer, which frames it with `Content-Length` as it frames bytes.
pub(super) struct FileBody {
    pub(super) stream: tokio_util::io::ReaderStream<tokio::fs::File>,
    pub(super) len: u64,
}

impl http_body::Body for FileBody {
    type Data = bytes::Bytes;
    type Error = std::io::Error;

    fn poll_frame(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
    ) -> Poll<Option<Result<http_body::Frame<Self::Data>, Self::Error>>> {
        use tokio_stream::Stream;
        Pin::new(&mut self.get_mut().stream)
            .poll_next(cx)
            .map(|chunk| chunk.map(|r| r.map(http_body::Frame::data)))
    }

    fn size_hint(&self) -> http_body::SizeHint {
        http_body::SizeHint::with_exact(self.len)
    }
}
