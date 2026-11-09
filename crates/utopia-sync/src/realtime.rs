//! Realtime invalidation (§12.3 "Realtime"): a WebSocket that only carries
//! `{"page_id", "head_seq"}` notices. A notice triggers the ordinary HTTP pull, so the
//! relay stays stateless-ish and correctness never depends on the socket.

use serde::{Deserialize, Serialize};
use tungstenite::client::IntoClientRequest;
use tungstenite::Message;

use crate::{Result, SyncError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Notice {
    pub page_id: String,
    pub head_seq: u64,
}

/// `http(s)://host` → `ws(s)://host/v1/graphs/{gid}/events`.
pub fn events_url(base: &str, graph: &str) -> String {
    let base = base.trim_end_matches('/');
    let ws = if let Some(rest) = base.strip_prefix("https://") {
        format!("wss://{rest}")
    } else if let Some(rest) = base.strip_prefix("http://") {
        format!("ws://{rest}")
    } else {
        base.to_string()
    };
    format!("{ws}/v1/graphs/{graph}/events")
}

/// Block on the event stream, calling `on_notice` per notice until it returns `false` or
/// the connection ends. Run it on a background thread and reconnect with backoff.
pub fn listen(
    base: &str,
    graph: &str,
    token: Option<&str>,
    mut on_notice: impl FnMut(Notice) -> bool,
) -> Result<()> {
    let mut req = events_url(base, graph)
        .into_client_request()
        .map_err(|e| SyncError::Transport(e.to_string()))?;
    if let Some(t) = token {
        let v = format!("Bearer {t}")
            .parse()
            .map_err(|_| SyncError::Transport("bad token".into()))?;
        req.headers_mut().insert("Authorization", v);
    }
    let (mut socket, _) = tungstenite::connect(req).map_err(|e| match e {
        tungstenite::Error::Http(r) if r.status() == 401 || r.status() == 403 => {
            SyncError::Unauthorized
        }
        e => SyncError::Transport(e.to_string()),
    })?;
    loop {
        let msg = socket
            .read()
            .map_err(|e| SyncError::Transport(e.to_string()))?;
        match msg {
            Message::Text(_) | Message::Binary(_) => {
                let text = msg.to_text().unwrap_or_default();
                if let Ok(n) = serde_json::from_str::<Notice>(text) {
                    if !on_notice(n) {
                        let _ = socket.close(None);
                        return Ok(());
                    }
                }
            }
            Message::Close(_) => return Ok(()),
            _ => {}
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn urls() {
        assert_eq!(
            events_url("https://s.example/", "g"),
            "wss://s.example/v1/graphs/g/events"
        );
        assert_eq!(
            events_url("http://127.0.0.1:1", "g"),
            "ws://127.0.0.1:1/v1/graphs/g/events"
        );
    }
}

/// An owned listener: dropping it cancels DNS, connect, reads and backoff, not just the
/// next reconnect (§12.3). No thread or socket outlives this handle indefinitely.
pub struct Listener {
    stop: Option<tokio::sync::oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}
impl Drop for Listener {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
    }
}
impl Listener {
    /// Wait for shutdown on a background thread when replacing a target.
    pub fn stop(mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
    pub fn start(
        base: String,
        graph: String,
        token: Option<String>,
        nudge: std::sync::Arc<std::sync::atomic::AtomicBool>,
    ) -> std::io::Result<Self> {
        let (stop, stopped) = tokio::sync::oneshot::channel();
        let thread = std::thread::Builder::new()
            .name("utopia-realtime".into())
            .spawn(move || {
                let Ok(rt) = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                else {
                    return;
                };
                rt.block_on(async move {
                    tokio::select! {
                        _ = stopped => {},
                        _ = reconnect(base, graph, token, nudge) => {},
                    }
                });
            })?;
        Ok(Self {
            stop: Some(stop),
            thread: Some(thread),
        })
    }
}

async fn reconnect(
    base: String,
    graph: String,
    token: Option<String>,
    nudge: std::sync::Arc<std::sync::atomic::AtomicBool>,
) {
    use futures_util::StreamExt;
    use std::time::Duration;
    use tokio_tungstenite::tungstenite::client::IntoClientRequest;
    let mut backoff = Duration::from_secs(2);
    loop {
        let Ok(mut req) = IntoClientRequest::into_client_request(events_url(&base, &graph)) else {
            return;
        };
        if let Some(token) = &token {
            let Ok(value) = format!("Bearer {token}").parse() else {
                return;
            };
            req.headers_mut().insert("Authorization", value);
        }
        let connection = tokio::time::timeout(
            Duration::from_secs(10),
            tokio_tungstenite::connect_async(req),
        )
        .await;
        match connection {
            Ok(Ok((mut socket, _))) => {
                backoff = Duration::from_secs(2);
                while let Some(Ok(msg)) = socket.next().await {
                    if msg.is_close() {
                        break;
                    }
                    if msg
                        .to_text()
                        .ok()
                        .and_then(|s| serde_json::from_str::<Notice>(s).ok())
                        .is_some()
                    {
                        nudge.store(true, std::sync::atomic::Ordering::SeqCst);
                    }
                }
            }
            Ok(Err(tokio_tungstenite::tungstenite::Error::Http(r)))
                if r.status() == 401 || r.status() == 403 =>
            {
                backoff = Duration::from_secs(60)
            }
            _ => {}
        }
        tokio::time::sleep(backoff).await;
        backoff = (backoff * 2).min(Duration::from_secs(60));
    }
}

#[cfg(test)]
mod lifecycle_tests {
    use super::*;
    use std::net::TcpListener;
    use std::sync::{mpsc, Arc};
    use std::time::Duration;

    #[test]
    fn ac_12_3_stop_cancels_a_quiet_socket_and_replacement_has_one_listener() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let url = format!("http://{}", server.local_addr().unwrap());
        let (tx, rx) = mpsc::channel();
        let host = std::thread::spawn(move || {
            for _ in 0..2 {
                let (tcp, _) = server.accept().unwrap();
                tcp.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                let mut ws = tungstenite::accept(tcp).unwrap();
                tx.send("connected").unwrap();
                assert!(ws.read().is_err(), "stop must close even a quiet socket");
                tx.send("closed").unwrap();
            }
        });
        for _ in 0..2 {
            let listener = Listener::start(url.clone(), "g".into(), None, Arc::default()).unwrap();
            assert_eq!(
                rx.recv_timeout(Duration::from_secs(3)).unwrap(),
                "connected"
            );
            listener.stop();
            assert_eq!(rx.recv_timeout(Duration::from_secs(3)).unwrap(), "closed");
        }
        host.join().unwrap();
    }

    #[test]
    fn ac_12_3_stop_cancels_a_stalled_handshake() {
        let server = TcpListener::bind("127.0.0.1:0").unwrap();
        let listener = Listener::start(
            format!("http://{}", server.local_addr().unwrap()),
            "g".into(),
            None,
            Arc::default(),
        )
        .unwrap();
        let (_tcp, _) = server.accept().unwrap();
        let now = std::time::Instant::now();
        listener.stop();
        assert!(now.elapsed() < Duration::from_secs(2));
    }
}
