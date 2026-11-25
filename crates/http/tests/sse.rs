//! Live SSE transport and opening-timeout regression tests.

#[cfg(test)]
mod tests {
    use sendito_core::{HttpRequest, resolve::ResolvedHttp};
    use sendito_http::{HttpError, SendOptions, sse};
    use std::time::Duration;
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    fn request(url: String) -> ResolvedHttp {
        let mut request = sendito_core::resolve::resolve_http(
            &HttpRequest {
                url,
                ..Default::default()
            },
            &sendito_core::Scope::new(),
            std::path::Path::new("."),
        );
        request.timeout_ms = Some(200);
        request
    }

    async fn server(
        status: u16,
        extra: &str,
        delay_headers: bool,
        delay_body: bool,
        redirect: bool,
    ) -> (String, tokio::task::JoinHandle<Vec<String>>) {
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}/events", listener.local_addr().unwrap());
        let extra = extra.to_string();
        let task = tokio::spawn(async move {
            let mut captures = Vec::new();
            for hop in 0..if redirect { 2 } else { 1 } {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut input = Vec::new();
                loop {
                    let mut buf = [0; 1024];
                    let n = socket.read(&mut buf).await.unwrap();
                    assert!(n > 0);
                    input.extend_from_slice(&buf[..n]);
                    if input.windows(4).any(|w| w == b"\r\n\r\n") {
                        break;
                    }
                }
                captures.push(String::from_utf8(input).unwrap());
                if redirect && hop == 0 {
                    socket
                        .write_all(
                            b"HTTP/1.1 302 Found\r\nLocation: /final\r\nContent-Length: 0\r\n\r\n",
                        )
                        .await
                        .unwrap();
                    continue;
                }
                if delay_headers {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                let body = b"id: 1\r\ndata: first\r\n\r\ndata: second\n\n";
                let headers = format!(
                    "HTTP/1.1 {status} OK\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n",
                    body.len()
                );
                if socket.write_all(headers.as_bytes()).await.is_err() {
                    return captures;
                }
                if delay_body {
                    tokio::time::sleep(Duration::from_millis(400)).await;
                }
                let _ = socket.write_all(body).await;
            }
            captures
        });
        (url, task)
    }

    #[tokio::test]
    async fn sse_body_outlives_opening_timeout_even_after_redirect() {
        for redirect in [false, true] {
            let (url, capture) = server(
                200,
                "Content-Type: Text/Event-Stream; charset=utf-8\r\n",
                false,
                true,
                redirect,
            )
            .await;
            let request = request(url);
            let mut stream = sse::open(&request, &SendOptions::default(), None)
                .await
                .unwrap();
            assert_eq!(stream.next_event().await.unwrap().unwrap().data, "first");
            let second = stream.next_event().await.unwrap().unwrap();
            assert_eq!(second.data, "second");
            assert_eq!(second.id, "1");
            assert!(stream.next_event().await.unwrap().is_none());
            for raw in capture.await.unwrap() {
                assert!(raw.contains("Accept: text/event-stream"));
            }
            assert!(
                !request
                    .headers
                    .iter()
                    .any(|(n, _)| n.eq_ignore_ascii_case("accept"))
            );
        }
    }

    #[tokio::test]
    async fn sse_opening_timeout_and_normal_http_total_timeout() {
        let (url, capture) = server(
            200,
            "Content-Type: text/event-stream\r\n",
            true,
            false,
            false,
        )
        .await;
        assert!(matches!(
            sse::open(&request(url), &SendOptions::default(), None).await,
            Err(HttpError::Timeout(200))
        ));
        capture.await.unwrap();
        let (url, capture) = server(
            200,
            "Content-Type: text/event-stream\r\n",
            false,
            true,
            false,
        )
        .await;
        assert!(matches!(
            sendito_http::send(&request(url), &SendOptions::default()).await,
            Err(HttpError::Timeout(200))
        ));
        capture.await.unwrap();
    }

    #[tokio::test]
    async fn sse_refuses_status_html_and_compression() {
        for (status, headers, expected) in [
            (
                204,
                "Content-Type: text/event-stream\r\n",
                "requires HTTP 200",
            ),
            (200, "Content-Type: text/html\r\n", "requires Content-Type"),
            (
                200,
                "Content-Type: text/event-stream\r\nContent-Encoding: gzip\r\n",
                "compressed streams",
            ),
        ] {
            let (url, capture) = server(status, headers, false, false, false).await;
            let error = match sse::open(&request(url), &SendOptions::default(), None).await {
                Ok(_) => panic!("invalid SSE response accepted"),
                Err(error) => error,
            };
            assert!(error.to_string().contains(expected), "{error}");
            capture.await.unwrap();
        }
    }

    #[tokio::test]
    async fn sse_preserves_explicit_accept_and_accepts_identity_encoding() {
        let (url, capture) = server(
            200,
            "Content-Type: text/event-stream\r\nContent-Encoding: identity\r\n",
            false,
            false,
            false,
        )
        .await;
        let mut request = request(url);
        request
            .headers
            .push(("accept".into(), "custom/type".into()));
        let mut stream = sse::open(&request, &SendOptions::default(), None)
            .await
            .unwrap();
        assert!(stream.next_event().await.unwrap().is_some());
        let raw = capture.await.unwrap().remove(0);
        assert!(raw.contains("Accept: custom/type"));
        assert!(!raw.contains("Accept: text/event-stream"));
    }
}
