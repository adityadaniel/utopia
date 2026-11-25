//! Whether the loopback automation API starts with a graph, on which address and at which
//! assistant access level (PRD §13.8 MC-1, MC-2; docs/mcp-design.md §2.4, §5).
//!
//! The API starts when `UTOPIA_API` is set (as before) or when the graph's access file says
//! `read` or `write`. The level always comes from the access file: `UTOPIA_API` opens the
//! socket for scripts but cannot raise what `/v1/op` permits (MC-1). With the level Off and
//! no `UTOPIA_API`, there is no server and no socket.

use std::cell::RefCell;
use std::path::Path;
use std::rc::Rc;

use gpui::BackgroundExecutor;
use utopia_api::{ApiCall, ApiServer, Level};
pub use utopia_cli::api_launch::{load_access, plan, serve};

/// The loopback API of the open graph, in one shared slot.
///
/// The inbox loop reads the slot every tick instead of capturing the `Option` at load:
/// Settings → Assistants starts, stops and re-levels the API while the app runs (MC-12), so
/// a server that starts later must still have its calls drained.
#[derive(Clone, Default)]
pub struct ApiSlot(Rc<RefCell<Option<ApiServer>>>);

impl ApiSlot {
    pub fn new(server: Option<ApiServer>) -> Self {
        Self(Rc::new(RefCell::new(server)))
    }

    /// The next queued call, when a server runs.
    pub fn try_recv(&self) -> Option<ApiCall> {
        self.0.borrow().as_ref()?.try_recv()
    }

    pub fn is_running(&self) -> bool {
        self.0.borrow().is_some()
    }

    pub fn url(&self) -> Option<String> {
        self.0.borrow().as_ref().map(ApiServer::url)
    }

    /// Change what `/v1/op` permits from now on, on the server that runs.
    pub fn set_level(&self, level: Level) {
        if let Some(server) = self.0.borrow().as_ref() {
            server.set_level(level);
        }
    }

    /// Put a server in the slot, returning the one it replaced. The replaced server stops as
    /// it drops, and its port is free again when this returns.
    pub fn replace(&self, server: Option<ApiServer>) -> Option<ApiServer> {
        self.0.replace(server)
    }

    /// Stop the server: dropping it shuts the listener down.
    pub fn take(&self) -> Option<ApiServer> {
        self.0.take()
    }

    /// Stop the server without blocking the caller (MC-12, rule 10).
    ///
    /// Queued calls are dropped first: each carries the oneshot its HTTP handler waits on,
    /// so the handler answers "the app is not running" at once instead of holding the
    /// server's graceful shutdown. The destructor then joins the server thread, which waits
    /// for calls already in flight, so that join runs on `executor`, never on the caller's UI
    /// thread — a request the UI has not drained yet would otherwise block the loop that
    /// would drain it, until the server's wait expires.
    pub fn stop(&self, executor: &BackgroundExecutor) {
        let Some(server) = self.take() else {
            return;
        };
        // A call that reaches the handler between here and the shutdown is refused rather
        // than queueing for an owner that no longer drains (nothing would run it, but the
        // refusal is the answer the level now says).
        server.set_level(Level::Off);
        while let Some(call) = server.try_recv() {
            drop(call);
        }
        executor.spawn(async move { drop(server) }).detach();
    }
}

/// The app's start-up: read `UTOPIA_API` and the access file, then start the server if
/// [`plan`] says so. A failure is logged; the app runs without the API.
pub fn start(config: &Path, graph: &Path) -> Option<ApiServer> {
    let env = std::env::var("UTOPIA_API").ok();
    let launch = plan(env.as_deref(), load_access(config, graph))?;
    match serve(&launch.addr, launch.level, config, graph) {
        Ok(server) => {
            eprintln!(
                "utopia: automation API on {} (assistant access: {:?})",
                server.url(),
                launch.level
            );
            Some(server)
        }
        Err(e) => {
            eprintln!("utopia: automation API failed: {e}");
            None
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// MC-12: the slot is what the inbox loop reads, so a server that Settings starts after
    /// the graph loaded is drained too. Stopping it gives the port back.
    #[test]
    fn mc_12_the_inbox_sees_a_server_started_after_load() {
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("g.utopia");
        std::fs::write(&graph, b"").unwrap();
        let config = dir.path().join("config");
        let slot = ApiSlot::new(None);
        assert!(!slot.is_running());
        assert_eq!(slot.url(), None);
        assert!(slot.try_recv().is_none());

        let server = serve("127.0.0.1:0", Level::Read, &config, &graph).unwrap();
        let url = server.url();
        assert!(slot.replace(Some(server)).is_none());
        assert!(slot.is_running());
        assert_eq!(slot.url(), Some(url.clone()));
        assert!(url.starts_with("http://127.0.0.1:"), "{url}");
        assert!(slot.try_recv().is_none(), "nothing was sent yet");
        assert!(utopia_api::discovery_file(&config, &graph).is_file());

        assert!(slot.take().is_some());
        assert!(!slot.is_running());
    }
}
