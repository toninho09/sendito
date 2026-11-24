//! The DNS / connect / TLS / TTFB / download timing of a send.

use super::*;

/// Phase durations. `None` when the phase did not happen (e.g. no DNS for an IP
/// literal, no TLS for `http://`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Timing {
    pub dns: Option<Duration>,
    pub connect: Option<Duration>,
    pub tls: Option<Duration>,
    /// From connection ready (or request start) to response headers.
    pub ttfb: Duration,
    pub download: Duration,
    pub total: Duration,
}

#[derive(Debug, Default)]
pub(super) struct Marks {
    pub(super) dns_start: Option<Instant>,
    pub(super) dns_end: Option<Instant>,
    pub(super) connect_start: Option<Instant>,
    pub(super) tls_start: Option<Instant>,
    pub(super) connect_end: Option<Instant>,
}

pub(super) type Shared = Arc<Mutex<Marks>>;

pub(super) fn mark(m: &Shared, f: impl FnOnce(&mut Marks) -> &mut Option<Instant>) {
    if let Ok(mut g) = m.lock() {
        let slot = f(&mut g);
        if slot.is_none() {
            *slot = Some(Instant::now());
        }
    }
}

/// DNS resolver that records when resolution starts and ends.
pub(super) struct TimedResolver(pub(super) Shared);

impl reqwest::dns::Resolve for TimedResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        let marks = self.0.clone();
        let host = name.as_str().to_string();
        Box::pin(async move {
            mark(&marks, |m| &mut m.dns_start);
            let addrs: Vec<SocketAddr> = tokio::net::lookup_host((host.as_str(), 0))
                .await
                .map_err(|e| -> Box<dyn std::error::Error + Send + Sync> {
                    Box::new(std::io::Error::new(
                        e.kind(),
                        format!("DNS lookup for {host} failed: {e}"),
                    ))
                })?
                .collect();
            mark(&marks, |m| &mut m.dns_end);
            let it: reqwest::dns::Addrs = Box::new(addrs.into_iter());
            Ok(it)
        })
    }
}

/// Connector layer recording when connection establishment finishes.
#[derive(Clone)]
pub(super) struct TimingLayer(pub(super) Shared);

impl<S> tower::Layer<S> for TimingLayer {
    type Service = TimingService<S>;
    fn layer(&self, inner: S) -> Self::Service {
        TimingService {
            inner,
            marks: self.0.clone(),
        }
    }
}

#[derive(Clone)]
pub(super) struct TimingService<S> {
    pub(super) inner: S,
    pub(super) marks: Shared,
}

impl<S, R> tower_service::Service<R> for TimingService<S>
where
    S: tower_service::Service<R>,
    S::Future: Send + 'static,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = Pin<Box<dyn Future<Output = Result<S::Response, S::Error>> + Send>>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, req: R) -> Self::Future {
        mark(&self.marks, |m| &mut m.connect_start);
        let fut = self.inner.call(req);
        let marks = self.marks.clone();
        Box::pin(async move {
            let r = fut.await;
            mark(&marks, |m| &mut m.connect_end);
            r
        })
    }
}

/// Session store wrapper: rustls consults it while building the ClientHello,
/// right after the TCP connection is up, which marks the start of the handshake.
#[derive(Debug)]
pub(super) struct TimedSessions {
    pub(super) inner: ClientSessionMemoryCache,
    pub(super) marks: Shared,
}

impl ClientSessionStore for TimedSessions {
    fn set_kx_hint(&self, server_name: ServerName<'static>, group: NamedGroup) {
        self.inner.set_kx_hint(server_name, group)
    }
    fn kx_hint(&self, server_name: &ServerName<'_>) -> Option<NamedGroup> {
        mark(&self.marks, |m| &mut m.tls_start);
        self.inner.kx_hint(server_name)
    }
    fn set_tls12_session(
        &self,
        server_name: ServerName<'static>,
        value: rustls::client::Tls12ClientSessionValue,
    ) {
        self.inner.set_tls12_session(server_name, value)
    }
    fn tls12_session(
        &self,
        server_name: &ServerName<'_>,
    ) -> Option<rustls::client::Tls12ClientSessionValue> {
        mark(&self.marks, |m| &mut m.tls_start);
        self.inner.tls12_session(server_name)
    }
    fn remove_tls12_session(&self, server_name: &ServerName<'static>) {
        self.inner.remove_tls12_session(server_name)
    }
    fn insert_tls13_ticket(
        &self,
        server_name: ServerName<'static>,
        value: rustls::client::Tls13ClientSessionValue,
    ) {
        self.inner.insert_tls13_ticket(server_name, value)
    }
    fn take_tls13_ticket(
        &self,
        server_name: &ServerName<'static>,
    ) -> Option<rustls::client::Tls13ClientSessionValue> {
        mark(&self.marks, |m| &mut m.tls_start);
        self.inner.take_tls13_ticket(server_name)
    }
}

impl fmt::Display for Timing {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let ms = |d: Option<Duration>| {
            d.map(|d| format!("{:.1} ms", d.as_secs_f64() * 1000.0))
                .unwrap_or_else(|| "-".into())
        };
        write!(
            f,
            "dns {} · connect {} · tls {} · ttfb {} · download {} · total {}",
            ms(self.dns),
            ms(self.connect),
            ms(self.tls),
            ms(Some(self.ttfb)),
            ms(Some(self.download)),
            ms(Some(self.total))
        )
    }
}
