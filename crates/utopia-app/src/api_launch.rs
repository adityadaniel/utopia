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

use utopia_api::{access, Access, ApiCall, ApiServer, Level};

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
}

/// What [`plan`] decided.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Launch {
    pub addr: String,
    pub level: Level,
}

/// The decision, without I/O, so a unit test can call it. `env_api` is `UTOPIA_API`.
pub fn plan(env_api: Option<&str>, access: Access) -> Option<Launch> {
    let level = access.level;
    match env_api.map(str::trim).filter(|v| !v.is_empty()) {
        Some("1") => Some(Launch {
            addr: "127.0.0.1:0".into(),
            level,
        }),
        Some(addr) => Some(Launch {
            addr: addr.to_string(),
            level,
        }),
        None if level != Level::Off => Some(Launch {
            addr: "127.0.0.1:0".into(),
            level,
        }),
        None => None,
    }
}

/// The graph's access setting, with a notice for a bad file logged (MC-1: Off).
pub fn load_access(config: &Path, graph: &Path) -> Access {
    let (access, notice) = Access::load(&access::access_file(config, graph));
    if let Some(n) = notice {
        eprintln!("utopia: {n}");
    }
    access
}

/// Start a server at `level` and write the graph's discovery file
/// (`<config>/api/<graph_key>.json`, mode 0600).
pub fn serve(addr: &str, level: Level, config: &Path, graph: &Path) -> std::io::Result<ApiServer> {
    let server = utopia_api::serve(addr, utopia_api::new_token())?;
    server.set_level(level);
    let file = utopia_api::discovery_file(config, graph);
    if let Err(e) = server.write_discovery(&file) {
        eprintln!("utopia: could not write {}: {e}", file.display());
    }
    Ok(server)
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

    fn access(level: Level) -> Access {
        Access {
            level,
            when_closed: false,
        }
    }

    /// MC-1: Off and no `UTOPIA_API` is start-up as before, with no server; the access file
    /// alone starts it; `UTOPIA_API` picks the address but never the level.
    #[test]
    fn mc_1_the_api_starts_for_utopia_api_or_an_access_level() {
        assert_eq!(plan(None, access(Level::Off)), None);
        assert_eq!(plan(Some(""), access(Level::Off)), None);
        assert_eq!(plan(Some("  "), access(Level::Off)), None);
        for level in [Level::Read, Level::Write] {
            assert_eq!(
                plan(None, access(level)),
                Some(Launch {
                    addr: "127.0.0.1:0".into(),
                    level
                })
            );
        }
        for level in [Level::Off, Level::Read, Level::Write] {
            assert_eq!(
                plan(Some("1"), access(level)),
                Some(Launch {
                    addr: "127.0.0.1:0".into(),
                    level
                }),
                "UTOPIA_API=1 keeps the file's level"
            );
            assert_eq!(
                plan(Some("127.0.0.1:4555"), access(level)),
                Some(Launch {
                    addr: "127.0.0.1:4555".into(),
                    level
                })
            );
        }
        // `when_closed` does not start or stop the app's API.
        assert_eq!(
            plan(
                None,
                Access {
                    level: Level::Off,
                    when_closed: true
                }
            ),
            None
        );
    }

    #[test]
    fn mc_1_a_bad_access_file_starts_nothing() {
        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("g.utopia");
        std::fs::write(&graph, b"").unwrap();
        let config = dir.path().join("config");
        assert_eq!(load_access(&config, &graph), Access::default());
        let f = access::access_file(&config, &graph);
        std::fs::create_dir_all(f.parent().unwrap()).unwrap();
        std::fs::write(&f, r#"{"version": 9, "access": "write"}"#).unwrap();
        assert_eq!(plan(None, load_access(&config, &graph)), None);
        std::fs::write(
            &f,
            r#"{"version": 1, "access": "read", "when_closed": false}"#,
        )
        .unwrap();
        assert_eq!(
            plan(None, load_access(&config, &graph)).map(|l| l.level),
            Some(Level::Read)
        );
    }

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
