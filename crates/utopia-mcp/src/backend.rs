//! Where a tool call runs: the [`Backend`] trait and [`Direct`], the headless path
//! (PRD §13.8 MC-1, MC-2, MC-4, MC-5; `docs/mcp-design.md` §2.3, §5).
//!
//! `Direct` is the path with no app running. It takes the graph's owner lock, opens the
//! store for the one call, runs [`utopia_api::handle`] and gives the lock back. A tool call
//! that arrives while the app owns the graph gets [`BackendError::OwnedElsewhere`]; the
//! caller then sends the op to the owner over the loopback API instead (`Remote`, PR 4).

use std::path::PathBuf;

use serde_json::Value;
use utopia_api::{
    access::{access_file, Access},
    handle, Op,
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

impl Backend for Direct {
    fn call(&self, op: Op) -> Result<Value, BackendError> {
        // 1. The level of this graph, read fresh (MC-1, MC-2). Off and read-only refusals
        //    never touch the graph: no lock, no store.
        let (access, notice) = Access::load(&access_file(&self.config, &self.graph));
        if let Some(notice) = notice {
            stderr(&format!("utopia mcp: {notice}"));
        }
        if let Some(refusal) = access.level.refusal(op.kind()) {
            return Err(BackendError::failed(403, refusal));
        }

        // 2. The owner lock, with no wait (MC-4). The app holds it for its whole life, so a
        //    free lock means the app is closed.
        let lock = match OwnerLock::try_acquire(&self.graph) {
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
}
