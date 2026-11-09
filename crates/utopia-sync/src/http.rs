//! HTTP transport for the hosted or self-hosted relay (`utopia-syncd`, §12.7 variants 1–2).
//! Blocking by design: the app runs it on a background thread (see [`crate::exchange`]).

use std::io::Read;
use std::time::Duration;

use serde::de::DeserializeOwned;
use serde::Serialize;
use utopia_core::PageId;

use crate::wire::{Manifest, PullResponse, PushRequest, PushResponse, SnapshotRequest};
use crate::{CursorKind, CursorSpace, Result, SyncError, Transport};

/// Largest response body accepted (a page snapshot plus its tail).
const MAX_BODY: u64 = 256 << 20;

#[derive(Clone)]
pub struct HttpTransport {
    agent: ureq::Agent,
    base: String,
    graph: String,
    token: Option<String>,
    /// The relay store's id, learned from its manifest (§12.6). `None` until the first
    /// manifest, and forever with a relay that predates the field.
    instance: Option<String>,
}

/// A stable spelling of a relay origin for the cursor space: scheme and authority
/// lowercased, a default port dropped, no trailing slash. `http://Relay.Example:80/` and
/// `http://relay.example` are the same relay; a different scheme or port is not.
pub fn normalize_origin(base: &str) -> String {
    let base = base.trim_end_matches('/');
    let Some((scheme, rest)) = base.split_once("://") else {
        return base.to_ascii_lowercase();
    };
    let scheme = scheme.to_ascii_lowercase();
    let (authority, path) = match rest.find('/') {
        Some(i) => (&rest[..i], &rest[i..]),
        None => (rest, ""),
    };
    let authority = authority.to_ascii_lowercase();
    let authority = match authority.rsplit_once(':') {
        Some((host, "80")) if scheme == "http" => host.to_string(),
        Some((host, "443")) if scheme == "https" => host.to_string(),
        _ => authority,
    };
    format!("{scheme}://{authority}{}", path.trim_end_matches('/'))
}

/// The destination origin of a relay (§12.7): its normalized URL plus the graph id. This
/// is what a launch can know before a manifest has been read; the relay's store id is the
/// finer part of the same destination (`utopia_store::sync_destination::Destination`).
pub fn destination_origin(base: &str, graph: &str) -> String {
    format!("relay:{}#{graph}", normalize_origin(base))
}

/// Graph ids travel in URLs; keep them to a safe alphabet.
pub fn valid_graph_id(id: &str) -> bool {
    !id.is_empty()
        && id.len() <= 128
        && id
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_')
}

fn map_err(e: ureq::Error) -> SyncError {
    match e {
        ureq::Error::Status(401 | 403, _) => SyncError::Unauthorized,
        ureq::Error::Status(code, resp) => {
            let body = resp.into_string().unwrap_or_default();
            SyncError::Transport(format!("HTTP {code}: {}", body.trim()))
        }
        ureq::Error::Transport(t) => SyncError::Transport(t.to_string()),
    }
}

fn read_json<T: DeserializeOwned>(resp: ureq::Response) -> Result<T> {
    let zstd = resp.header("Content-Encoding") == Some("zstd");
    let mut buf = Vec::new();
    resp.into_reader()
        .take(MAX_BODY)
        .read_to_end(&mut buf)
        .map_err(|e| SyncError::Transport(e.to_string()))?;
    if zstd {
        buf = zstd::decode_all(&buf[..]).map_err(|e| SyncError::Transport(e.to_string()))?;
    }
    serde_json::from_slice(&buf).map_err(|e| SyncError::Transport(format!("bad response: {e}")))
}

impl HttpTransport {
    /// `base` is the relay origin, e.g. `https://sync.example.com`.
    pub fn new(base: &str, graph: &str, token: Option<String>) -> Result<Self> {
        if !valid_graph_id(graph) {
            return Err(SyncError::Transport(format!("invalid graph id {graph:?}")));
        }
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(5))
            .timeout(Duration::from_secs(30))
            .build();
        Ok(Self {
            agent,
            base: base.trim_end_matches('/').to_string(),
            graph: graph.to_string(),
            token,
            instance: None,
        })
    }

    fn url(&self, rest: &str) -> String {
        format!("{}/v1/graphs/{}/{rest}", self.base, self.graph)
    }

    fn request(&self, method: &str, url: &str) -> ureq::Request {
        let r = self.agent.request(method, url);
        match &self.token {
            Some(t) => r.set("Authorization", &format!("Bearer {t}")),
            None => r,
        }
    }

    fn post_json<B: Serialize>(&self, url: &str, body: &B) -> Result<ureq::Response> {
        let json = serde_json::to_vec(body).map_err(|e| SyncError::Transport(e.to_string()))?;
        let compressed =
            zstd::encode_all(&json[..], 3).map_err(|e| SyncError::Transport(e.to_string()))?;
        self.request("POST", url)
            .set("Content-Type", "application/json")
            .set("Content-Encoding", "zstd")
            .send_bytes(&compressed)
            .map_err(map_err)
    }
}

impl Transport for HttpTransport {
    fn cursor_space(&self) -> CursorSpace {
        // The relay store id once its manifest has been seen: the URL can change (a DHCP
        // lease, a LAN port, a host rename) without the data changing, and a wiped store
        // keeps its URL while its sequence restarts. A relay that predates the field is
        // identified by its URL and graph.
        let store = self
            .instance
            .clone()
            .unwrap_or_else(|| normalize_origin(&self.base));
        CursorSpace::new(format!("relay:{store}#{}", self.graph), CursorKind::Seq)
    }

    fn manifest(&mut self, if_none_match: Option<u64>) -> Result<Option<Manifest>> {
        let mut req = self.request("GET", &self.url("manifest"));
        if let Some(rev) = if_none_match {
            req = req.set("If-None-Match", &format!("\"{rev}\""));
        }
        let resp = req.call().map_err(map_err)?;
        if resp.status() == 304 {
            return Ok(None);
        }
        let m: Manifest = read_json(resp)?;
        if !m.instance.is_empty() {
            self.instance = Some(m.instance.clone());
        }
        Ok(Some(m))
    }

    fn push(&mut self, page: &PageId, req: PushRequest) -> Result<PushResponse> {
        let url = self.url(&format!("pages/{page}/updates"));
        read_json(self.post_json(&url, &req)?)
    }

    fn pull(&mut self, page: &PageId, since_seq: u64) -> Result<PullResponse> {
        let url = self.url(&format!("pages/{page}/updates?since={since_seq}"));
        read_json(self.request("GET", &url).call().map_err(map_err)?)
    }

    fn put_snapshot(&mut self, page: &PageId, covers_seq: u64, payload: Vec<u8>) -> Result<()> {
        let url = self.url(&format!("pages/{page}/snapshot"));
        self.post_json(
            &url,
            &SnapshotRequest {
                covers_seq,
                payload,
            },
        )?;
        Ok(())
    }

    fn put_blob(&mut self, hash: &str, bytes: Vec<u8>) -> Result<bool> {
        if !utopia_core::attach::is_hash(hash) {
            return Err(SyncError::Transport(format!("invalid blob hash {hash:?}")));
        }
        self.request("PUT", &self.url(&format!("blobs/{hash}")))
            .set("Content-Type", "application/octet-stream")
            .send_bytes(&bytes)
            .map_err(map_err)?;
        Ok(true)
    }

    fn get_blob(&mut self, hash: &str) -> Result<Option<Vec<u8>>> {
        if !utopia_core::attach::is_hash(hash) {
            return Ok(None);
        }
        match self
            .request("GET", &self.url(&format!("blobs/{hash}")))
            .call()
        {
            Ok(resp) => {
                let mut buf = Vec::new();
                std::io::Read::read_to_end(
                    &mut resp.into_reader().take(BLOB_LIMIT as u64),
                    &mut buf,
                )
                .map_err(|e| SyncError::Transport(e.to_string()))?;
                Ok(Some(buf))
            }
            Err(ureq::Error::Status(404, _)) => Ok(None),
            Err(e) => Err(map_err(e)),
        }
    }
}

/// The store accepts attachments up to 64 MB; encrypted ones carry a small envelope.
const BLOB_LIMIT: usize = 64 * 1024 * 1024 + 4096;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn graph_ids_are_url_safe() {
        assert!(valid_graph_id("my-graph_1"));
        assert!(!valid_graph_id("../etc"));
        assert!(!valid_graph_id(""));
        assert!(HttpTransport::new("http://x", "a/b", None).is_err());
    }

    #[test]
    fn the_fallback_cursor_space_ignores_url_spelling() {
        let a = HttpTransport::new("http://Relay.Example:80/", "g", None).unwrap();
        let b = HttpTransport::new("HTTP://relay.example", "g", None).unwrap();
        assert_eq!(a.cursor_space(), b.cursor_space());
        let secure = HttpTransport::new("https://relay.example", "g", None).unwrap();
        assert_ne!(
            a.cursor_space(),
            secure.cursor_space(),
            "another scheme is another relay"
        );
        let other_graph = HttpTransport::new("http://relay.example", "h", None).unwrap();
        assert_ne!(a.cursor_space(), other_graph.cursor_space());
    }

    #[test]
    fn unreachable_relay_is_a_transport_error() {
        let mut t = HttpTransport::new("http://127.0.0.1:9", "g", None).unwrap();
        assert!(matches!(t.manifest(None), Err(SyncError::Transport(_))));
    }
}
