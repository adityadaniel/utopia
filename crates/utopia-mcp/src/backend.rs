//! Where a tool call runs (PRD §13.8 MC-1, MC-2, MC-4, MC-5; `docs/mcp-design.md` §2.1,
//! §2.3, §5).
//!
//! Two paths for one call, and [`Router`] picks between them:
//!
//! - [`Direct`] is the headless path: no app running. It takes the graph's owner lock, opens
//!   the store for the one call, runs [`utopia_api::handle`] and gives the lock back.
//! - [`Remote`] is the path with the app open: a different process owns the graph, so the op
//!   goes to its loopback API (`POST /v1/op`) and the owner runs it.
//!
//! A tool call that arrives while the app owns the graph gets
//! [`BackendError::OwnedElsewhere`] from `Direct`; the caller then sends the op to the owner.
//! `Remote` reports [`BackendError::NoOwnerApi`] when nothing answers at the owner's
//! discovery file, which is what [`Router`] retries the lock for.

use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

use serde_json::Value;
use utopia_api::{
    access::{access_file, Access},
    discovery_file, handle, Client, ClientError, Op,
};
use utopia_store::{OwnerLock, Store};

/// The refusal a second process gets for a graph another one owns (MC-4). It is the same
/// sentence the write commands print.
pub const ALREADY_OPEN: &str = "This graph is already open in Utopia";

/// What a call answers when the app is closed and `when_closed` is off (MC-5). It names
/// both ways forward, and says that nothing happened.
pub const OPEN_UTOPIA: &str = "Utopia is not running. Open Utopia, or turn on \"Work when \
                              closed\" for assistants in Utopia: Settings → Assistants. \
                              Nothing was read or written.";

/// What a call answers when the graph is owned by a process that runs no automation API
/// (PR 4, the fourth step of [`Router`]). Until Settings can change the level of a running
/// app, a level turned on while the app is open is the usual cause: the app read the access
/// file only when it started.
pub const OWNER_NO_API: &str = "Utopia has this graph open but does not answer calls from \
                               assistants, so nothing was read or written. Try again in a \
                               moment; if it keeps happening, quit Utopia and open it again \
                               (Utopia reads the assistant access setting when it starts).";

/// How `Direct` opens the store: the caller supplies it, so the MCP server cannot be the
/// place that decides about encryption (PR 4's opener never prompts: `stdin` is the
/// protocol channel).
pub type Opener = Box<dyn Fn() -> Result<Store, String> + Send + Sync>;

/// A tool call that did not produce a result.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BackendError {
    /// A different process owns the graph (MC-4). `Direct` opened nothing, so the caller
    /// may send the op to the owner instead.
    OwnedElsewhere { message: String },
    /// A different process owns the graph and nothing answers at its discovery file: no
    /// file, or no server listening at the address in it. Nothing was sent, so nothing ran
    /// (PR 4).
    NoOwnerApi { message: String },
    /// The call failed. `status` is the API status it would answer over HTTP (403 for the
    /// access level, 404 for a page or block that does not exist, …) and `message` is the
    /// text the model reads, unchanged.
    Failed { status: u16, message: String },
}

impl BackendError {
    fn failed(status: u16, message: impl Into<String>) -> Self {
        Self::Failed {
            status,
            message: message.into(),
        }
    }
}

/// One operation in, its JSON result or an error out. Implementations must not print to
/// `stdout` (MC-6) and must not open a store for anything but the call they are given.
///
/// `Send + Sync` because the server runs its calls on a thread of its own (design §3.1),
/// so a caller may hand a backend to it from anywhere.
pub trait Backend: Send + Sync {
    fn call(&self, op: Op) -> Result<Value, BackendError>;

    /// Whether the process that runs a call still holds the undo group when it answers: the
    /// owner that keeps the store ([`Remote`], the app open) does, so `Cmd+Z` in Utopia
    /// reverses an agent's write (MC-9). The headless path ([`Direct`]) opens a store for the
    /// one call and drops it, and the undo stack is that store's memory, so nothing outlives
    /// the answer. A tool that offers a way back reads this rather than guessing (PR 9's
    /// review: a headless `delete_block` promised a Cmd+Z the app could not give). [`Router`]
    /// answers for the path that served its last call.
    fn keeps_undo(&self) -> bool {
        // The safe answer for a backend that does not say: promise no way back.
        false
    }
}

/// The headless path: one call, one store, one owner lock (design §2.3).
///
/// It loads the access file for every call, so a change in Settings applies without
/// restarting the client that started this server (MC-1).
pub struct Direct {
    graph: PathBuf,
    config: PathBuf,
    open: Opener,
}

impl Direct {
    pub fn new(graph: impl Into<PathBuf>, config: impl Into<PathBuf>, open: Opener) -> Self {
        Self {
            graph: graph.into(),
            config: config.into(),
            open,
        }
    }
}

impl Direct {
    /// The same call, but waiting up to `wait` for the owner lock. [`Router`] uses this for
    /// its last step, because a short write command (`utopia capture`, `import-roam`) can
    /// hold the lock for a moment (PR 4). A zero wait is [`Backend::call`].
    pub fn call_with(&self, op: Op, wait: Duration) -> Result<Value, BackendError> {
        // 1. The level of this graph, read fresh (MC-1, MC-2). Off and read-only refusals
        //    never touch the graph: no lock, no store.
        let (access, notice) = Access::load(&access_file(&self.config, &self.graph));
        if let Some(notice) = notice {
            stderr(&format!("utopia mcp: {notice}"));
        }
        if let Some(refusal) = access.level.refusal(op.kind()) {
            return Err(BackendError::failed(403, refusal));
        }

        // 2. The owner lock (MC-4). The app holds it for its whole life, so a free lock
        //    means the app is closed. The router does not wait; its last step does.
        let taken = if wait.is_zero() {
            OwnerLock::try_acquire(&self.graph)
        } else {
            OwnerLock::acquire_within(&self.graph, wait)
        };
        let lock = match taken {
            Ok(Some(lock)) => lock,
            Ok(None) => {
                return Err(BackendError::OwnedElsewhere {
                    message: ALREADY_OPEN.to_string(),
                })
            }
            Err(e) => {
                return Err(BackendError::failed(
                    500,
                    format!("could not take the owner lock of the graph: {e}"),
                ))
            }
        };

        // 3. With the app closed, the owner's second switch decides (MC-5, decision D1).
        if !access.when_closed {
            return Err(BackendError::failed(503, OPEN_UTOPIA));
        }

        // 4. One store for this call. It is dropped (and with it the store's shared lock
        //    and its buffered writes flushed) before the owner lock goes back.
        let mut store = match (self.open)() {
            Ok(store) => store,
            Err(e) => return Err(BackendError::failed(500, e)),
        };
        let body = match handle(&mut store, op) {
            Ok(outcome) => outcome.body,
            Err(e) => {
                return Err(BackendError::failed(e.status, e.message));
            }
        };
        if let Err(e) = store.flush() {
            return Err(BackendError::failed(500, e.to_string()));
        }
        drop(store);
        drop(lock);
        Ok(body)
    }
}

impl Backend for Direct {
    fn call(&self, op: Op) -> Result<Value, BackendError> {
        self.call_with(op, Duration::ZERO)
    }
}

/// The owner's loopback API (design §2.1, §2.4): the app or `serve-api` holds the graph and
/// runs `POST /v1/op` for the process that starts `utopia mcp`.
///
/// It reads the graph's discovery file for each call, because the owner can start or stop
/// while this server runs. The owner enforces the access level (MC-2); its status and text
/// go to the agent unchanged, so a 403, 429, 503 or 504 says what happened.
pub struct Remote {
    graph: PathBuf,
    config: PathBuf,
}

impl Remote {
    pub fn new(graph: impl Into<PathBuf>, config: impl Into<PathBuf>) -> Self {
        Self {
            graph: graph.into(),
            config: config.into(),
        }
    }

    /// `<config>/api/<graph_key>.json`: an address, not the truth (design §2.2).
    pub fn discovery(&self) -> PathBuf {
        discovery_file(&self.config, &self.graph)
    }

    pub fn graph(&self) -> &Path {
        &self.graph
    }

    pub fn config(&self) -> &Path {
        &self.config
    }
}

impl Backend for Remote {
    fn call(&self, op: Op) -> Result<Value, BackendError> {
        let file = self.discovery();
        let Ok(client) = Client::from_discovery(&file) else {
            return Err(BackendError::NoOwnerApi {
                message: format!("no discovery file at {}", file.display()),
            });
        };
        match client.op(&op) {
            Ok(body) => Ok(body),
            // Nothing listens at the address in the file: a stale file, or an owner that
            // started before it wrote one and has stopped. Nothing was sent.
            Err(ClientError::NotListening(e)) => Err(BackendError::NoOwnerApi {
                message: format!("nothing listens at {}: {e}", client.url()),
            }),
            // The owner ran the checks and refused or failed the call: its text is the
            // agent's text, unchanged.
            Err(ClientError::Status { status, message }) => {
                Err(BackendError::failed(status, message))
            }
            // The request may have run: never turn this into "try again" advice.
            Err(ClientError::Transport(e)) => Err(BackendError::failed(
                502,
                format!(
                    "The open Utopia app did not answer the call ({e}). The call may have run, \
                     so read again before you write."
                ),
            )),
        }
    }

    /// The owner ran the call and still holds the store, so its undo stack has the group.
    fn keeps_undo(&self) -> bool {
        true
    }
}

/// The rule that picks the path for each call (PR 4):
///
/// 1. The access file decides first. Off needs no lock and no connection (MC-1).
/// 2. The owner lock, with no wait. A free lock is the headless call — or "Open Utopia"
///    when `when_closed` is off (MC-5).
/// 3. The lock is held: another process owns the graph, so the op goes to its API.
/// 4. Nothing answers at the owner's address. It may be a short CLI write that holds the
///    lock, so try the lock again for [`Router::RETRY`]; if it stays held, [`OWNER_NO_API`].
pub struct Router {
    direct: Direct,
    remote: Remote,
    retry: Duration,
    /// Whether the path that served the last call keeps its undo group
    /// ([`Backend::keeps_undo`]). The protocol runs one call at a time (design §3.1), so a
    /// tool asks right after its own call. `Direct` answers `false` and `Remote` `true`, and
    /// the router is the only place that knows which of the two ran.
    last_kept_undo: AtomicBool,
}

impl Router {
    /// How long step 4 waits for the owner lock: the same 2 s the app gives a headless call
    /// (`cli::OWNER_WAIT`, design §2.2).
    pub const RETRY: Duration = Duration::from_secs(2);

    pub fn new(direct: Direct, remote: Remote) -> Self {
        Self {
            direct,
            remote,
            retry: Self::RETRY,
            last_kept_undo: AtomicBool::new(false),
        }
    }

    /// The router for one graph, with the store opener `Direct` uses.
    pub fn for_graph(graph: impl Into<PathBuf>, config: impl Into<PathBuf>, open: Opener) -> Self {
        let graph = graph.into();
        let config = config.into();
        Self::new(
            Direct::new(graph.clone(), config.clone(), open),
            Remote::new(graph, config),
        )
    }

    /// Wait this long in step 4 instead of [`Router::RETRY`]. Tests use a short wait; the
    /// command keeps the default.
    pub fn retry_within(mut self, wait: Duration) -> Self {
        self.retry = wait;
        self
    }

    /// Record which path ran, so a tool that finishes after [`Backend::call`] gets the fact
    /// right: the headless call dropped its store, and the owner still holds it.
    fn answer(
        &self,
        answer: Result<Value, BackendError>,
        kept_undo: bool,
    ) -> Result<Value, BackendError> {
        self.last_kept_undo.store(kept_undo, Ordering::Relaxed);
        answer
    }
}

impl Backend for Router {
    fn call(&self, op: Op) -> Result<Value, BackendError> {
        // 1. The level comes before the lock and before any connection (MC-1, MC-2). The
        //    notice of a bad access file is logged by the `Direct` call below.
        let (access, _) = Access::load(&access_file(self.remote.config(), self.remote.graph()));
        if let Some(refusal) = access.level.refusal(op.kind()) {
            return Err(BackendError::failed(403, refusal));
        }

        // 2. A free lock is the headless call, or "Open Utopia" (MC-5).
        match self.direct.call(op.clone()) {
            Err(BackendError::OwnedElsewhere { .. }) => {}
            other => return self.answer(other, false),
        }

        // 3. Another process owns the graph: it runs the op over its API, at its level.
        match self.remote.call(op.clone()) {
            Err(BackendError::NoOwnerApi { .. }) => {}
            other => return self.answer(other, true),
        }

        // 4. The owner runs no API. A short write command may hold the lock, so try it
        //    again; a lock that stays held gives the text that says what to do (PR 4).
        let answer = match self.direct.call_with(op, self.retry) {
            Err(BackendError::OwnedElsewhere { .. }) => {
                Err(BackendError::failed(503, OWNER_NO_API))
            }
            other => other,
        };
        self.answer(answer, false)
    }

    /// The path that served the last call: only the owner keeps the undo group.
    fn keeps_undo(&self) -> bool {
        self.last_kept_undo.load(Ordering::Relaxed)
    }
}

/// One line on `stderr` (never `stdout`: MC-6).
fn stderr(line: &str) {
    use std::io::Write as _;
    let _ = writeln!(std::io::stderr(), "{line}");
}

#[cfg(test)]
mod tests {
    use super::*;
    use utopia_api::access::Level;

    fn scratch() -> (tempfile::TempDir, PathBuf, PathBuf) {
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("g.utopia");
        let config = dir.path().join("config");
        (dir, graph, config)
    }

    fn direct(graph: &std::path::Path, config: &std::path::Path) -> Direct {
        let g = graph.to_path_buf();
        Direct::new(
            graph,
            config,
            Box::new(move || Store::open(&g).map_err(|e| e.to_string())),
        )
    }

    /// MC-9: only the process that holds the store can undo a write after it answers. The
    /// headless path opens one store per call and drops it, so it must not claim otherwise,
    /// and the owner (`Remote`) must.
    #[test]
    fn mc_9_only_the_owner_keeps_the_undo_group() {
        let (_dir, graph, config) = scratch();
        assert!(
            !direct(&graph, &config).keeps_undo(),
            "a headless call drops its store, and the undo group with it"
        );
        assert!(
            Remote::new(graph.clone(), config.clone()).keeps_undo(),
            "the owner holds the store, so its history has the group"
        );

        // The router is what `utopia mcp` runs, and it is the only place that knows which
        // path served a call: after a headless one it must answer for `Direct` (PR 9's
        // review was that it answered the trait's default for every path).
        std::fs::write(&graph, b"").unwrap();
        set_access(&config, &graph, Level::Read, true);
        let router = router(&graph, &config, Duration::ZERO);
        router.call(Op::ListPages { daily: false }).unwrap();
        assert!(
            !router.keeps_undo(),
            "the free lock means the headless path ran"
        );
    }

    fn set_access(config: &std::path::Path, graph: &std::path::Path, level: Level, closed: bool) {
        Access {
            level,
            when_closed: closed,
        }
        .save(&access_file(config, graph))
        .unwrap();
    }

    /// MC-1: with access missing (Off) nothing is opened, so a graph file that does not
    /// exist is not created by a call.
    #[test]
    fn mc_1_off_opens_nothing() {
        let (_dir, graph, config) = scratch();
        let d = direct(&graph, &config);
        let err = d.call(Op::ListPages { daily: false }).unwrap_err();
        assert_eq!(
            err,
            BackendError::Failed {
                status: 403,
                message: Level::Off
                    .refusal(utopia_api::OpKind::Read)
                    .unwrap()
                    .to_string()
            }
        );
        assert!(!graph.exists(), "the graph was not created");
    }

    /// MC-5: the app is closed and `when_closed` is off: the call stops before opening.
    #[test]
    fn mc_5_a_closed_app_needs_the_second_switch() {
        let (_dir, graph, config) = scratch();
        std::fs::write(&graph, b"").unwrap();
        set_access(&config, &graph, Level::Read, false);
        let d = direct(&graph, &config);
        let err = d.call(Op::ListPages { daily: false }).unwrap_err();
        match err {
            BackendError::Failed { status, message } => {
                assert_eq!(status, 503);
                assert!(message.contains("Open Utopia"), "{message}");
            }
            other => panic!("expected the closed-app error, got {other:?}"),
        }
    }

    /// MC-4: while another process owns the graph the call is the distinct error, so PR 4
    /// can select the `Remote` backend.
    #[test]
    fn mc_4_an_owned_graph_is_the_distinct_error() {
        let (_dir, graph, config) = scratch();
        std::fs::write(&graph, b"").unwrap();
        set_access(&config, &graph, Level::Read, true);
        let _owner = OwnerLock::try_acquire(&graph).unwrap().unwrap();
        let d = direct(&graph, &config);
        assert_eq!(
            d.call(Op::ListPages { daily: false }).unwrap_err(),
            BackendError::OwnedElsewhere {
                message: ALREADY_OPEN.to_string()
            }
        );
    }

    /// MC-1, MC-2: every call re-reads the access file, so a change applies at once.
    #[test]
    fn mc_2_the_access_file_is_read_for_each_call() {
        let (_dir, graph, config) = scratch();
        std::fs::write(&graph, b"").unwrap();
        let d = direct(&graph, &config);
        assert!(matches!(
            d.call(Op::ListPages { daily: false }),
            Err(BackendError::Failed { status: 403, .. })
        ));
        set_access(&config, &graph, Level::Read, true);
        let body = d.call(Op::ListPages { daily: false }).unwrap();
        assert_eq!(body["pages"], serde_json::json!([]));
        set_access(&config, &graph, Level::Off, true);
        assert!(matches!(
            d.call(Op::ListPages { daily: false }),
            Err(BackendError::Failed { status: 403, .. })
        ));
    }

    fn router(graph: &std::path::Path, config: &std::path::Path, retry: Duration) -> Router {
        let g = graph.to_path_buf();
        Router::for_graph(
            graph,
            config,
            Box::new(move || Store::open(&g).map_err(|e| e.to_string())),
        )
        .retry_within(retry)
    }

    fn off_refusal() -> String {
        Level::Off
            .refusal(utopia_api::OpKind::Read)
            .unwrap()
            .to_string()
    }

    /// MC-1, step 1 of the rule: Off needs no lock and no connection. The discovery file
    /// names a port that nothing listens on, and it is never opened.
    #[test]
    fn mc_1_off_makes_no_connection() {
        let (_dir, graph, config) = scratch();
        std::fs::write(&graph, b"").unwrap();
        set_access(&config, &graph, Level::Off, true);
        let dead = std::net::TcpListener::bind("127.0.0.1:0")
            .unwrap()
            .local_addr()
            .unwrap();
        let file = discovery_file(&config, &graph);
        std::fs::create_dir_all(file.parent().unwrap()).unwrap();
        std::fs::write(
            &file,
            format!(r#"{{"url": "http://{dead}", "token": "t"}}"#),
        )
        .unwrap();
        assert_eq!(
            router(&graph, &config, Duration::from_millis(10))
                .call(Op::ListPages { daily: false })
                .unwrap_err(),
            BackendError::Failed {
                status: 403,
                message: off_refusal()
            }
        );
    }

    /// MC-2, step 3: the owner runs the op and its own answer — including a refusal — goes
    /// to the agent unchanged (429, 503 and 504 travel the same way).
    #[test]
    fn mc_2_the_owner_status_and_text_go_through_unchanged() {
        let (_dir, graph, config) = scratch();
        std::fs::write(&graph, b"").unwrap();
        let server = utopia_api::serve("127.0.0.1:0", utopia_api::new_token()).unwrap();
        server.set_level(Level::Off);
        let file = discovery_file(&config, &graph);
        server.write_discovery(&file).unwrap();
        let err = Remote::new(&graph, &config)
            .call(Op::ListPages { daily: false })
            .unwrap_err();
        match err {
            BackendError::Failed { status, message } => {
                assert_eq!(status, 403, "the owner's status");
                assert_eq!(message, off_refusal(), "the owner's text, unchanged");
            }
            other => panic!("expected the owner's 403, got {other:?}"),
        }
    }

    /// MC-5, step 4: the owner holds the graph and runs no API (a level turned on while
    /// the app runs, until Settings can change it live). The retry waits, then says what to
    /// do; nothing is opened and nothing is written.
    #[test]
    fn mc_5_an_owner_without_an_api_gets_the_open_utopia_text() {
        let (_dir, graph, config) = scratch();
        std::fs::write(&graph, b"").unwrap();
        set_access(&config, &graph, Level::Read, true);
        let owner = OwnerLock::try_acquire(&graph).unwrap().unwrap();

        let started = std::time::Instant::now();
        let err = router(&graph, &config, Duration::from_millis(150))
            .call(Op::ListPages { daily: false })
            .unwrap_err();
        assert!(
            started.elapsed() >= Duration::from_millis(150),
            "it tried the lock again first"
        );
        assert_eq!(
            err,
            BackendError::Failed {
                status: 503,
                message: OWNER_NO_API.to_string()
            }
        );

        // The owner is gone: the same call is the headless one now.
        drop(owner);
        assert!(router(&graph, &config, Duration::from_millis(10))
            .call(Op::ListPages { daily: false })
            .is_ok());
    }

    /// MC-4, step 4: a short write command holds the lock for a moment. The retry gets it
    /// and the call runs headless, so a capture does not make an assistant call fail.
    #[test]
    fn mc_4_the_retry_runs_the_call_after_a_short_write() {
        let (_dir, graph, config) = scratch();
        std::fs::write(&graph, b"").unwrap();
        set_access(&config, &graph, Level::Read, true);

        let (tx, rx) = std::sync::mpsc::channel();
        let held = graph.clone();
        let writer = std::thread::spawn(move || {
            let lock = OwnerLock::try_acquire(&held).unwrap().unwrap();
            tx.send(()).unwrap();
            std::thread::sleep(Duration::from_millis(150));
            drop(lock);
        });
        rx.recv().unwrap();
        let body = router(&graph, &config, Duration::from_secs(2))
            .call(Op::ListPages { daily: false })
            .unwrap();
        writer.join().unwrap();
        assert_eq!(body["pages"], serde_json::json!([]));
    }
}
