//! The discovery file of a graph's API and a small blocking client for it
//! (docs/mcp-design.md §2.2; decision D10).
//!
//! The owner of a graph (the app or `serve-api`) writes `{url, token}` to
//! `<config>/api/<graph_key>.json`, readable by the user only. A second process that finds
//! the owner lock held reads it and sends its work to the owner. The file is only an
//! address: the lock is the truth, and the token makes a stale address (another server on
//! the same port) refuse the call with 401 instead of running it on another graph.

use std::path::{Path, PathBuf};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::Value;

use crate::Op;

/// `<config>/api/<graph_key>.json`.
pub fn discovery_file(config: &Path, graph: &Path) -> PathBuf {
    config
        .join("api")
        .join(format!("{}.json", utopia_store::graph_key(graph)))
}

/// The body of a discovery file.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Discovery {
    pub url: String,
    pub token: String,
}

/// Read a discovery file back.
pub fn read_discovery(path: &Path) -> std::io::Result<Discovery> {
    let bytes = std::fs::read(path)?;
    serde_json::from_slice(&bytes)
        .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e))
}

/// Why a call to the owner failed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientError {
    /// The request never reached a server: nothing listens at the address (a stale
    /// discovery file). Nothing was sent, so nothing ran.
    NotListening(String),
    /// The server answered with an error. `message` is its `error` text unchanged (or the
    /// whole body when it is not the API's JSON), so an agent sees what the owner said.
    Status { status: u16, message: String },
    /// The connection failed after it was made, or the answer could not be read. The call
    /// may have run.
    Transport(String),
}

impl std::fmt::Display for ClientError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ClientError::NotListening(e) => write!(f, "no server at the address: {e}"),
            ClientError::Status { status, message } => write!(f, "{status}: {message}"),
            ClientError::Transport(e) => write!(f, "{e}"),
        }
    }
}

impl std::error::Error for ClientError {}

/// A blocking client for an owner's API.
#[derive(Debug, Clone)]
pub struct Client {
    url: String,
    token: String,
    agent: ureq::Agent,
}

impl Client {
    /// How long a call waits for its answer: past the owner's own wait
    /// ([`crate::Limits::wait`], 30 s), so the owner's 504 text arrives instead of a client
    /// timeout that cannot say whether the call ran.
    pub const TIMEOUT: Duration = Duration::from_secs(40);

    pub fn new(d: Discovery) -> Self {
        let agent = ureq::AgentBuilder::new()
            .timeout_connect(Duration::from_secs(2))
            .timeout(Self::TIMEOUT)
            .build();
        Self {
            url: d.url.trim_end_matches('/').to_string(),
            token: d.token,
            agent,
        }
    }

    /// A client for the owner named in a discovery file.
    pub fn from_discovery(path: &Path) -> std::io::Result<Self> {
        read_discovery(path).map(Self::new)
    }

    pub fn url(&self) -> &str {
        &self.url
    }

    /// `POST /v1/capture`: append `text` to today's page.
    pub fn capture(&self, text: &str) -> Result<Value, ClientError> {
        self.post("/v1/capture", &serde_json::json!({ "text": text }))
    }

    /// `POST /v1/op`: run one [`Op`] at the owner's access level (MC-2).
    pub fn op(&self, op: &Op) -> Result<Value, ClientError> {
        let body = serde_json::to_value(op).map_err(|e| ClientError::Transport(e.to_string()))?;
        self.post("/v1/op", &body)
    }

    fn post(&self, path: &str, body: &Value) -> Result<Value, ClientError> {
        let resp = self
            .agent
            .post(&format!("{}{path}", self.url))
            .set("Authorization", &format!("Bearer {}", self.token))
            .set("Content-Type", "application/json")
            .send_string(&body.to_string());
        match resp {
            Ok(r) => {
                let text = r
                    .into_string()
                    .map_err(|e| ClientError::Transport(e.to_string()))?;
                serde_json::from_str(&text).map_err(|e| ClientError::Transport(e.to_string()))
            }
            Err(ureq::Error::Status(status, r)) => {
                let text = r.into_string().unwrap_or_default();
                let message = serde_json::from_str::<Value>(&text)
                    .ok()
                    .and_then(|v| v.get("error").and_then(Value::as_str).map(str::to_string))
                    .unwrap_or(text);
                Err(ClientError::Status { status, message })
            }
            Err(ureq::Error::Transport(t)) => {
                if t.kind() == ureq::ErrorKind::ConnectionFailed {
                    Err(ClientError::NotListening(t.to_string()))
                } else {
                    Err(ClientError::Transport(t.to_string()))
                }
            }
        }
    }
}
