//! The relay (PRD §12.8): per-page append-only logs of opaque updates plus optional
//! snapshots. It dedupes by change id, assigns monotonic per-page `seq`, enforces auth and
//! quota, generates the manifest, and truncates logs when a client uploads a snapshot.
//! It never parses payloads, so it needs no CRDT and works on ciphertext (§11.7).

pub mod lan;

use std::collections::{BTreeMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex};

use axum::body::Bytes;
use axum::extract::ws::{Message, WebSocket, WebSocketUpgrade};
use axum::extract::{DefaultBodyLimit, Path as UrlPath, Query, State};
use axum::http::{header, HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::routing::{get, post, put};
use axum::Json;
use rusqlite::{params, Connection, OptionalExtension};
use serde::de::DeserializeOwned;
use serde::Deserialize;
use utopia_proto::{
    Manifest, ManifestPage, PullResponse, PushRequest, PushResponse, SnapshotRequest, UpdateOut,
    PROTOCOL_VERSION,
};

const SCHEMA: &str = r#"
PRAGMA journal_mode = WAL;
CREATE TABLE IF NOT EXISTS graph (id TEXT PRIMARY KEY, created_at INTEGER, owner_id TEXT);
CREATE TABLE IF NOT EXISTS page_log (
  graph_id TEXT NOT NULL, page_id TEXT NOT NULL,
  seq INTEGER NOT NULL, change_id TEXT NOT NULL UNIQUE,
  client_id TEXT, client_seq INTEGER,
  frontier TEXT, payload BLOB NOT NULL, received_at INTEGER NOT NULL,
  PRIMARY KEY (graph_id, page_id, seq)
);
CREATE TABLE IF NOT EXISTS page_snapshot (
  graph_id TEXT NOT NULL, page_id TEXT NOT NULL,
  seq INTEGER NOT NULL, frontier TEXT, payload BLOB NOT NULL,
  PRIMARY KEY (graph_id, page_id)
);
CREATE TABLE IF NOT EXISTS graph_rev (graph_id TEXT PRIMARY KEY, rev INTEGER NOT NULL);
CREATE TABLE IF NOT EXISTS relay_meta (key TEXT PRIMARY KEY, value TEXT NOT NULL);
CREATE TABLE IF NOT EXISTS blob (
  graph_id TEXT NOT NULL, hash TEXT NOT NULL,
  payload BLOB NOT NULL, received_at INTEGER NOT NULL,
  PRIMARY KEY (graph_id, hash)
);
"#;

#[derive(Debug)]
pub enum ApiError {
    Unauthorized,
    BadRequest(String),
    QuotaExceeded,
    Internal(String),
}

impl From<rusqlite::Error> for ApiError {
    fn from(e: rusqlite::Error) -> Self {
        ApiError::Internal(e.to_string())
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (code, msg) = match self {
            ApiError::Unauthorized => (StatusCode::UNAUTHORIZED, "unauthorized".to_string()),
            ApiError::BadRequest(m) => (StatusCode::BAD_REQUEST, m),
            ApiError::QuotaExceeded => {
                (StatusCode::PAYLOAD_TOO_LARGE, "quota exceeded".to_string())
            }
            ApiError::Internal(m) => (StatusCode::INTERNAL_SERVER_ERROR, m),
        };
        (code, msg).into_response()
    }
}

pub type ApiResult<T> = Result<T, ApiError>;

fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| d.as_millis() as i64)
}

/// Relay storage (§12.8). One connection behind a mutex: every operation is short.
pub struct Db {
    conn: Mutex<Connection>,
}

impl Db {
    pub fn open(path: &Path) -> ApiResult<Self> {
        Self::init(Connection::open(path)?)
    }

    pub fn open_in_memory() -> ApiResult<Self> {
        Self::init(Connection::open_in_memory()?)
    }

    fn init(conn: Connection) -> ApiResult<Self> {
        conn.execute_batch(SCHEMA)?;
        // One id per relay store, kept across restarts and replaced only by a wiped
        // database, so a client can tell a restarted log from the one it pulled (§12.6).
        conn.execute(
            "INSERT OR IGNORE INTO relay_meta(key, value)
             VALUES ('instance', lower(hex(randomblob(16))))",
            [],
        )?;
        Ok(Self {
            conn: Mutex::new(conn),
        })
    }

    /// This relay store's instance id (stable for the life of the database).
    fn instance(conn: &Connection) -> ApiResult<String> {
        Ok(conn
            .query_row(
                "SELECT value FROM relay_meta WHERE key='instance'",
                [],
                |r| r.get(0),
            )
            .optional()?
            .unwrap_or_default())
    }

    fn lock(&self) -> ApiResult<std::sync::MutexGuard<'_, Connection>> {
        self.conn
            .lock()
            .map_err(|_| ApiError::Internal("database lock poisoned".into()))
    }

    fn rev(conn: &Connection, gid: &str) -> ApiResult<u64> {
        Ok(conn
            .query_row("SELECT rev FROM graph_rev WHERE graph_id=?1", [gid], |r| {
                r.get::<_, i64>(0)
            })
            .optional()?
            .unwrap_or(0) as u64)
    }

    fn bump(conn: &Connection, gid: &str) -> ApiResult<()> {
        conn.execute(
            "INSERT INTO graph_rev(graph_id, rev) VALUES (?1, 1)
             ON CONFLICT(graph_id) DO UPDATE SET rev = rev + 1",
            [gid],
        )?;
        Ok(())
    }

    fn head(conn: &Connection, gid: &str, pid: &str) -> ApiResult<u64> {
        Ok(conn.query_row(
            "SELECT max(ifnull((SELECT max(seq) FROM page_log WHERE graph_id=?1 AND page_id=?2), 0),
                        ifnull((SELECT seq FROM page_snapshot WHERE graph_id=?1 AND page_id=?2), 0))",
            params![gid, pid],
            |r| r.get::<_, i64>(0),
        )? as u64)
    }

    /// Bytes stored for a graph (logs + snapshots + attachments), for quota.
    pub fn graph_bytes(&self, gid: &str) -> ApiResult<u64> {
        let conn = self.lock()?;
        Self::graph_bytes_conn(&conn, gid)
    }

    fn graph_bytes_conn(conn: &Connection, gid: &str) -> ApiResult<u64> {
        Ok(conn.query_row(
            "SELECT ifnull((SELECT sum(length(payload)) FROM page_log WHERE graph_id=?1), 0)
                  + ifnull((SELECT sum(length(payload)) FROM page_snapshot WHERE graph_id=?1), 0)
                  + ifnull((SELECT sum(length(payload)) FROM blob WHERE graph_id=?1), 0)",
            [gid],
            |r| r.get::<_, i64>(0),
        )? as u64)
    }

    /// Store an attachment (opaque, possibly encrypted). Write-once: an existing hash is
    /// kept. Returns whether it was new.
    pub fn put_blob(
        &self,
        gid: &str,
        hash: &str,
        owner: &str,
        bytes: &[u8],
        quota: u64,
    ) -> ApiResult<bool> {
        if self.get_blob(gid, hash)?.is_some() {
            return Ok(false);
        }
        if self.graph_bytes(gid)? + bytes.len() as u64 > quota {
            return Err(ApiError::QuotaExceeded);
        }
        let conn = self.lock()?;
        conn.execute(
            "INSERT OR IGNORE INTO graph(id, created_at, owner_id) VALUES (?1, ?2, ?3)",
            params![gid, now_ms(), owner],
        )?;
        let n = conn.execute(
            "INSERT OR IGNORE INTO blob(graph_id, hash, payload, received_at) VALUES (?1, ?2, ?3, ?4)",
            params![gid, hash, bytes, now_ms()],
        )?;
        Ok(n > 0)
    }

    pub fn get_blob(&self, gid: &str, hash: &str) -> ApiResult<Option<Vec<u8>>> {
        let conn = self.lock()?;
        Ok(conn
            .query_row(
                "SELECT payload FROM blob WHERE graph_id=?1 AND hash=?2",
                params![gid, hash],
                |r| r.get(0),
            )
            .optional()?)
    }

    pub fn manifest(&self, gid: &str) -> ApiResult<Manifest> {
        let conn = self.lock()?;
        let mut pages: BTreeMap<String, ManifestPage> = BTreeMap::new();
        {
            let mut stmt = conn.prepare(
                "SELECT page_id, max(seq), sum(length(payload)), max(received_at) FROM page_log
                 WHERE graph_id=?1 GROUP BY page_id",
            )?;
            let rows = stmt.query_map([gid], |r| {
                Ok((
                    r.get::<_, String>(0)?,
                    r.get::<_, i64>(1)?,
                    r.get::<_, i64>(2)?,
                    r.get::<_, i64>(3)?,
                ))
            })?;
            for row in rows {
                let (pid, head, bytes, at) = row?;
                pages.insert(
                    pid.clone(),
                    ManifestPage {
                        page_id: pid,
                        head_seq: head as u64,
                        frontier: String::new(),
                        snapshot_seq: 0,
                        tail_bytes: bytes as u64,
                        updated_at: at,
                        deleted: false,
                    },
                );
            }
            let mut stmt =
                conn.prepare("SELECT page_id, seq FROM page_snapshot WHERE graph_id=?1")?;
            let rows =
                stmt.query_map([gid], |r| Ok((r.get::<_, String>(0)?, r.get::<_, i64>(1)?)))?;
            for row in rows {
                let (pid, seq) = row?;
                let p = pages.entry(pid.clone()).or_insert_with(|| ManifestPage {
                    page_id: pid,
                    head_seq: 0,
                    frontier: String::new(),
                    snapshot_seq: 0,
                    tail_bytes: 0,
                    updated_at: 0,
                    deleted: false,
                });
                p.snapshot_seq = seq as u64;
                p.head_seq = p.head_seq.max(seq as u64);
            }
            let mut stmt = conn.prepare(
                "SELECT frontier FROM page_log WHERE graph_id=?1 AND page_id=?2 ORDER BY seq DESC LIMIT 1",
            )?;
            for p in pages.values_mut() {
                p.frontier = stmt
                    .query_row(params![gid, p.page_id], |r| r.get::<_, Option<String>>(0))
                    .optional()?
                    .flatten()
                    .unwrap_or_default();
            }
        }
        Ok(Manifest {
            rev: Self::rev(&conn, gid)?,
            protocol_version: PROTOCOL_VERSION,
            pages: pages.into_values().collect(),
            instance: Self::instance(&conn)?,
        })
    }

    pub fn push(
        &self,
        gid: &str,
        pid: &str,
        owner: &str,
        req: PushRequest,
        quota: u64,
    ) -> ApiResult<PushResponse> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        tx.execute(
            "INSERT OR IGNORE INTO graph(id, created_at, owner_id) VALUES (?1, ?2, ?3)",
            params![gid, now_ms(), owner],
        )?;
        let mut head = Self::head(&tx, gid, pid)?;
        let (mut accepted, mut duplicate) = (0, 0);
        // Dedupe inside the transaction, before quota: a re-pushed batch whose response
        // was lost is entirely duplicates, and the bytes it would add are already stored.
        // Counting them again would refuse it forever (§12.6).
        let mut new_updates = Vec::new();
        let mut new_bytes = 0u64;
        let mut seen: HashSet<String> = HashSet::new();
        for u in req.updates {
            let exists: bool = tx.query_row(
                "SELECT EXISTS(SELECT 1 FROM page_log WHERE change_id=?1)",
                [&u.id],
                |r| r.get(0),
            )?;
            // A change id repeated inside one request is a duplicate as well: the first
            // copy is inserted below, and a second insert would break
            // `page_log.change_id`'s UNIQUE index and roll the whole request back.
            if exists || !seen.insert(u.id.clone()) {
                duplicate += 1;
                continue;
            }
            new_bytes += u.payload.len() as u64;
            new_updates.push(u);
        }
        if Self::graph_bytes_conn(&tx, gid)? + new_bytes > quota {
            return Err(ApiError::QuotaExceeded);
        }
        for u in new_updates {
            head += 1;
            let client = u.id.split(':').next().unwrap_or_default().to_string();
            tx.execute(
                "INSERT INTO page_log(graph_id, page_id, seq, change_id, client_id, client_seq,
                   frontier, payload, received_at) VALUES (?1, ?2, ?3, ?4, ?5, ?6, ?7, ?8, ?9)",
                params![
                    gid,
                    pid,
                    head as i64,
                    u.id,
                    client,
                    u.client_seq,
                    u.frontier,
                    u.payload,
                    now_ms()
                ],
            )?;
            accepted += 1;
        }
        if accepted > 0 {
            Self::bump(&tx, gid)?;
        }
        tx.commit()?;
        Ok(PushResponse {
            accepted,
            head_seq: head,
            duplicate,
        })
    }

    pub fn pull(&self, gid: &str, pid: &str, since: u64) -> ApiResult<PullResponse> {
        let conn = self.lock()?;
        let snap: Option<(i64, Vec<u8>)> = conn
            .query_row(
                "SELECT seq, payload FROM page_snapshot WHERE graph_id=?1 AND page_id=?2",
                params![gid, pid],
                |r| Ok((r.get(0)?, r.get(1)?)),
            )
            .optional()?;
        // When the requested range was truncated, the snapshot replaces it.
        let snapshot = snap.filter(|(seq, _)| since < *seq as u64).map(|(_, p)| p);
        let mut stmt = conn.prepare(
            "SELECT seq, change_id, ifnull(frontier, ''), payload FROM page_log
             WHERE graph_id=?1 AND page_id=?2 AND seq > ?3 ORDER BY seq",
        )?;
        let updates = stmt
            .query_map(params![gid, pid, since as i64], |r| {
                Ok(UpdateOut {
                    seq: r.get::<_, i64>(0)? as u64,
                    id: r.get(1)?,
                    frontier: r.get(2)?,
                    payload: r.get(3)?,
                })
            })?
            .collect::<rusqlite::Result<Vec<_>>>()?;
        Ok(PullResponse {
            snapshot,
            updates,
            head_seq: Self::head(&conn, gid, pid)?,
        })
    }

    pub fn put_snapshot(&self, gid: &str, pid: &str, req: SnapshotRequest) -> ApiResult<()> {
        let mut conn = self.lock()?;
        let tx = conn.transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)?;
        let head = Self::head(&tx, gid, pid)?;
        if req.covers_seq > head {
            return Err(ApiError::BadRequest(
                "snapshot covers unknown updates".into(),
            ));
        }
        let current: Option<i64> = tx
            .query_row(
                "SELECT seq FROM page_snapshot WHERE graph_id=?1 AND page_id=?2",
                params![gid, pid],
                |r| r.get(0),
            )
            .optional()?;
        if current.is_some_and(|s| s as u64 >= req.covers_seq) {
            return Ok(());
        }
        tx.execute(
            "INSERT INTO page_snapshot(graph_id, page_id, seq, frontier, payload) VALUES (?1, ?2, ?3, '', ?4)
             ON CONFLICT(graph_id, page_id) DO UPDATE SET seq=excluded.seq, payload=excluded.payload",
            params![gid, pid, req.covers_seq as i64, req.payload],
        )?;
        tx.execute(
            "DELETE FROM page_log WHERE graph_id=?1 AND page_id=?2 AND seq <= ?3",
            params![gid, pid, req.covers_seq as i64],
        )?;
        Self::bump(&tx, gid)?;
        tx.commit()?;
        Ok(())
    }

    /// Every stored payload (tests: prove the relay only ever holds ciphertext).
    pub fn all_payloads(&self) -> ApiResult<Vec<Vec<u8>>> {
        let conn = self.lock()?;
        let mut stmt = conn.prepare(
            "SELECT payload FROM page_log UNION ALL SELECT payload FROM page_snapshot
                 UNION ALL SELECT payload FROM blob",
        )?;
        let rows = stmt
            .query_map([], |r| r.get(0))?
            .collect::<rusqlite::Result<Vec<Vec<u8>>>>()?;
        Ok(rows)
    }

    /// Log rows stored for a page (diagnostics, tests).
    pub fn log_len(&self, gid: &str, pid: &str) -> ApiResult<usize> {
        let conn = self.lock()?;
        Ok(conn.query_row(
            "SELECT count(*) FROM page_log WHERE graph_id=?1 AND page_id=?2",
            params![gid, pid],
            |r| r.get::<_, i64>(0),
        )? as usize)
    }
}

/// The per-graph quota when none is configured (`UTOPIA_SYNCD_QUOTA_MB` unset): 1 GiB.
/// `relay-cloudflare` must default to the same (`tests/worker_constants.rs`).
pub const DEFAULT_QUOTA_BYTES: u64 = 1 << 30;

/// Largest request body: a 64 MiB attachment plus the encryption envelope. Applied to the
/// bytes on the wire (compressed), as `relay-cloudflare` does (`tests/worker_constants.rs`).
pub const MAX_BODY_BYTES: usize = (64 << 20) + (64 << 10);

#[derive(Debug, Clone)]
pub struct Config {
    /// Accepted bearer tokens; empty disables auth (local development only).
    pub tokens: Vec<String>,
    /// Max stored bytes per graph.
    pub quota_bytes: u64,
}

impl Default for Config {
    fn default() -> Self {
        Self {
            tokens: Vec::new(),
            quota_bytes: DEFAULT_QUOTA_BYTES,
        }
    }
}

/// Per-graph broadcast of `{"page_id","head_seq"}` notices (§12.3 realtime).
type Events = Arc<Mutex<std::collections::HashMap<String, tokio::sync::broadcast::Sender<String>>>>;

#[derive(Clone)]
struct Ctx {
    db: Arc<Db>,
    cfg: Arc<Config>,
    events: Events,
}

impl Ctx {
    fn channel(&self, gid: &str) -> tokio::sync::broadcast::Sender<String> {
        let mut map = self.events.lock().unwrap_or_else(|p| p.into_inner());
        map.entry(gid.to_string())
            .or_insert_with(|| tokio::sync::broadcast::channel(256).0)
            .clone()
    }

    fn notify(&self, gid: &str, pid: &str, head: u64) {
        let msg = serde_json::json!({ "page_id": pid, "head_seq": head }).to_string();
        let _ = self.channel(gid).send(msg);
    }
}

impl Ctx {
    /// Returns the caller identity (the token) or 401.
    fn auth(&self, headers: &HeaderMap) -> ApiResult<String> {
        if self.cfg.tokens.is_empty() {
            return Ok("anonymous".into());
        }
        let token = headers
            .get(header::AUTHORIZATION)
            .and_then(|v| v.to_str().ok())
            .and_then(|v| v.strip_prefix("Bearer "))
            .ok_or(ApiError::Unauthorized)?;
        if self.cfg.tokens.iter().any(|t| t == token) {
            Ok(token.to_string())
        } else {
            Err(ApiError::Unauthorized)
        }
    }
}

fn valid_id(s: &str) -> ApiResult<()> {
    let ok = !s.is_empty()
        && s.len() <= 128
        && s.bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-' || b == b'_');
    if ok {
        Ok(())
    } else {
        Err(ApiError::BadRequest(format!("invalid id {s:?}")))
    }
}

fn decode_body<T: DeserializeOwned>(headers: &HeaderMap, body: &Bytes) -> ApiResult<T> {
    let zstd = headers
        .get(header::CONTENT_ENCODING)
        .is_some_and(|v| v.as_bytes() == b"zstd");
    let raw = if zstd {
        zstd::decode_all(&body[..]).map_err(|e| ApiError::BadRequest(e.to_string()))?
    } else {
        body.to_vec()
    };
    serde_json::from_slice(&raw).map_err(|e| ApiError::BadRequest(e.to_string()))
}

async fn manifest(
    State(cx): State<Ctx>,
    UrlPath(gid): UrlPath<String>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    cx.auth(&headers)?;
    valid_id(&gid)?;
    let m = cx.db.manifest(&gid)?;
    let etag = format!("\"{}\"", m.rev);
    if headers
        .get(header::IF_NONE_MATCH)
        .is_some_and(|v| v.as_bytes() == etag.as_bytes())
    {
        return Ok(StatusCode::NOT_MODIFIED.into_response());
    }
    let mut resp = Json(m).into_response();
    if let Ok(v) = HeaderValue::from_str(&etag) {
        resp.headers_mut().insert(header::ETAG, v);
    }
    Ok(resp)
}

async fn push(
    State(cx): State<Ctx>,
    UrlPath((gid, pid)): UrlPath<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<Json<PushResponse>> {
    let owner = cx.auth(&headers)?;
    valid_id(&gid)?;
    valid_id(&pid)?;
    let req: PushRequest = decode_body(&headers, &body)?;
    let resp = cx.db.push(&gid, &pid, &owner, req, cx.cfg.quota_bytes)?;
    if resp.accepted > 0 {
        cx.notify(&gid, &pid, resp.head_seq);
    }
    Ok(Json(resp))
}

async fn events(
    State(cx): State<Ctx>,
    UrlPath(gid): UrlPath<String>,
    headers: HeaderMap,
    ws: WebSocketUpgrade,
) -> ApiResult<Response> {
    cx.auth(&headers)?;
    valid_id(&gid)?;
    let rx = cx.channel(&gid).subscribe();
    Ok(ws.on_upgrade(move |socket| forward(socket, rx)))
}

async fn forward(mut socket: WebSocket, mut rx: tokio::sync::broadcast::Receiver<String>) {
    loop {
        match rx.recv().await {
            Ok(msg) => {
                if socket.send(Message::Text(msg.into())).await.is_err() {
                    return;
                }
            }
            // Missed notices are harmless: the client's next pull catches up.
            Err(tokio::sync::broadcast::error::RecvError::Lagged(_)) => continue,
            Err(_) => return,
        }
    }
}

#[derive(Deserialize)]
struct Since {
    #[serde(default)]
    since: u64,
}

async fn pull(
    State(cx): State<Ctx>,
    UrlPath((gid, pid)): UrlPath<(String, String)>,
    Query(q): Query<Since>,
    headers: HeaderMap,
) -> ApiResult<Json<PullResponse>> {
    cx.auth(&headers)?;
    valid_id(&gid)?;
    valid_id(&pid)?;
    Ok(Json(cx.db.pull(&gid, &pid, q.since)?))
}

async fn snapshot(
    State(cx): State<Ctx>,
    UrlPath((gid, pid)): UrlPath<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    cx.auth(&headers)?;
    valid_id(&gid)?;
    valid_id(&pid)?;
    let req: SnapshotRequest = decode_body(&headers, &body)?;
    cx.db.put_snapshot(&gid, &pid, req)?;
    Ok(StatusCode::NO_CONTENT)
}

fn valid_hash(h: &str) -> ApiResult<()> {
    if h.len() == 64 && h.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
        Ok(())
    } else {
        Err(ApiError::BadRequest(format!("invalid blob hash {h:?}")))
    }
}

async fn put_blob(
    State(cx): State<Ctx>,
    UrlPath((gid, hash)): UrlPath<(String, String)>,
    headers: HeaderMap,
    body: Bytes,
) -> ApiResult<StatusCode> {
    let owner = cx.auth(&headers)?;
    valid_id(&gid)?;
    valid_hash(&hash)?;
    let new = cx
        .db
        .put_blob(&gid, &hash, &owner, &body, cx.cfg.quota_bytes)?;
    Ok(if new {
        StatusCode::CREATED
    } else {
        StatusCode::OK
    })
}

async fn get_blob(
    State(cx): State<Ctx>,
    UrlPath((gid, hash)): UrlPath<(String, String)>,
    headers: HeaderMap,
) -> ApiResult<Response> {
    cx.auth(&headers)?;
    valid_id(&gid)?;
    valid_hash(&hash)?;
    Ok(match cx.db.get_blob(&gid, &hash)? {
        Some(b) => ([(header::CONTENT_TYPE, "application/octet-stream")], b).into_response(),
        None => StatusCode::NOT_FOUND.into_response(),
    })
}

pub fn router(db: Arc<Db>, cfg: Config) -> Router {
    Router::new()
        .route("/v1/graphs/{gid}/manifest", get(manifest))
        .route("/v1/graphs/{gid}/pages/{pid}/updates", post(push).get(pull))
        .route("/v1/graphs/{gid}/pages/{pid}/snapshot", post(snapshot))
        .route("/v1/graphs/{gid}/events", get(events))
        .route("/v1/graphs/{gid}/blobs/{hash}", put(put_blob).get(get_blob))
        .route("/healthz", get(|| async { "ok" }))
        .layer(DefaultBodyLimit::max(MAX_BODY_BYTES))
        .with_state(Ctx {
            db,
            cfg: Arc::new(cfg),
            events: Arc::default(),
        })
}

pub use axum::Router;

/// Serve the relay on an already-bound listener.
pub async fn serve(listener: tokio::net::TcpListener, app: Router) -> std::io::Result<()> {
    axum::serve(listener, app).await
}
