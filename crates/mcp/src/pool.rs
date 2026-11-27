//! Sessions kept open between calls: one per key (the caller puts the
//! server, the environment and the transport in it). A kept session skips
//! the `initialize` exchange; a session that broke is opened again once.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use serde_json::Value;
use tokio::sync::mpsc::UnboundedSender;

use crate::{Event, McpError, Op, Options, Session, ToolResult, Transport, finish};

/// Open sessions by key. Clones share the sessions.
#[derive(Clone, Default)]
pub struct Pool(Arc<Mutex<HashMap<String, Slot>>>);

impl std::fmt::Debug for Pool {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "Pool({} sessions)", self.len())
    }
}

struct Slot {
    /// The session; `None` before the first call and after a failure. The
    /// lock also keeps two calls of one key from mixing their messages.
    session: Arc<tokio::sync::Mutex<Option<Session>>>,
    last_used: Instant,
}

impl Pool {
    /// How many sessions are open.
    pub fn len(&self) -> usize {
        self.0.lock().map(|m| m.len()).unwrap_or(0)
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }

    /// Closes every session.
    pub fn clear(&self) {
        if let Ok(mut m) = self.0.lock() {
            m.clear();
        }
    }

    /// Closes the sessions not used for `idle`; returns how many it closed.
    /// A session in use is kept.
    pub fn reap(&self, idle: Duration) -> usize {
        let Ok(mut m) = self.0.lock() else { return 0 };
        let before = m.len();
        m.retain(|_, s| s.last_used.elapsed() < idle || s.session.try_lock().is_err());
        before - m.len()
    }

    fn slot(&self, key: &str) -> Arc<tokio::sync::Mutex<Option<Session>>> {
        let Ok(mut m) = self.0.lock() else {
            return Arc::default();
        };
        let slot = m.entry(key.to_string()).or_insert_with(|| Slot {
            session: Arc::default(),
            last_used: Instant::now(),
        });
        slot.last_used = Instant::now();
        slot.session.clone()
    }

    /// Does `op` in the session of `key`, opening it when needed. When a
    /// kept session fails on the transport, a new one does the call once.
    pub(crate) async fn perform(
        &self,
        key: &str,
        transport: &Transport,
        op: &Op,
        arguments: &Value,
        opts: &Options,
        events: UnboundedSender<Event>,
    ) -> Result<ToolResult, McpError> {
        let slot = self.slot(key);
        let start = Instant::now();
        let mut kept = slot.lock().await;
        let work = async {
            if let Some(s) = kept.as_mut() {
                s.retarget(events.clone(), start);
                match s.perform(op, arguments).await {
                    Err(e) if broken(&e) => {
                        *kept = None;
                        let line = format!("the kept session failed ({e}); opening a new one");
                        let _ = events.send(Event::Log {
                            line,
                            at: start.elapsed(),
                        });
                    }
                    other => return other,
                }
            }
            let mut s = Session::open(transport, events.clone(), start).await?;
            let r = s.perform(op, arguments).await;
            *kept = Some(s);
            r
        };
        let outcome = tokio::time::timeout(opts.timeout, work).await;
        // A timeout or a broken session leaves no session for the next call.
        if matches!(&outcome, Err(_) | Ok(Err(_)))
            && !matches!(
                &outcome,
                Ok(Err(McpError::Rpc { .. } | McpError::Arguments(_)))
            )
        {
            *kept = None;
        }
        // The kept session lets go of this call's channel, so the caller
        // sees the end of the events.
        if let Some(s) = kept.as_ref() {
            let (closed, _) = tokio::sync::mpsc::unbounded_channel();
            s.retarget(closed, start);
        }
        drop(kept);
        if let Ok(mut m) = self.0.lock()
            && let Some(s) = m.get_mut(key)
        {
            s.last_used = Instant::now();
        }
        finish(&events, start, opts, outcome)
    }
}

/// The session cannot be used again.
fn broken(e: &McpError) -> bool {
    matches!(e, McpError::Closed | McpError::Transport(_))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reap_closes_only_idle_sessions() {
        let pool = Pool::default();
        pool.slot("a");
        assert_eq!(pool.len(), 1);
        assert_eq!(pool.reap(Duration::from_secs(60)), 0);
        assert_eq!(pool.reap(Duration::ZERO), 1);
        assert!(pool.is_empty());
        pool.slot("b");
        pool.clear();
        assert!(pool.is_empty());
    }
}
