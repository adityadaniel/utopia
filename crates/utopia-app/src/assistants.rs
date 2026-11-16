//! Settings → Assistants, the app side (PRD §13.8 MC-12; `docs/mcp-design.md` §5, §8.3 2.1).
//!
//! The tab is a view that emits a [`Request`], the same split as Settings → Sync: `utopia-ui`
//! cannot reach the loopback API or the config directory. This coordinator saves the access
//! file (atomic, local only, rule 9) and then starts, stops or re-levels the API with no
//! restart.
//!
//! The launch rule is [`crate::api_launch::plan`]'s, not a second copy of it (MC-1): the
//! only thing added here is what to do with a server that already runs.

use std::path::{Path, PathBuf};

use gpui::{App, AppContext as _, Context, Entity, Subscription};
use utopia_api::{access, Access, Level};
use utopia_ui::assistants::{AssistantsHandle, AssistantsSetup, Bridge, Request};

use crate::api_launch::{self, ApiSlot};

/// What a saved setting asks of the loopback API (MC-12). A value, so [`action`] can be
/// called with no window and no server.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// No server runs and none should: the level is Off and `UTOPIA_API` is not set.
    Nothing,
    /// Start a server at this address, at this level.
    Start { addr: String, level: Level },
    /// A server runs and stays: `/v1/op` follows the new level from now on.
    SetLevel(Level),
    /// Drop the server and remove the graph's discovery file.
    Stop,
}

/// The apply rule. `env_api` is `UTOPIA_API` and `running` says whether this graph's server
/// lives now. `api_launch::plan` decides whether a server should run and at which level;
/// this only compares that with what is running (MC-1: Off with no `UTOPIA_API` means no
/// socket; Off with `UTOPIA_API` keeps the socket for scripts).
pub fn action(env_api: Option<&str>, running: bool, access: Access) -> Action {
    match (api_launch::plan(env_api, access), running) {
        (None, false) => Action::Nothing,
        (None, true) => Action::Stop,
        (Some(launch), true) => Action::SetLevel(launch.level),
        (Some(launch), false) => Action::Start {
            addr: launch.addr,
            level: launch.level,
        },
    }
}

/// The bridge as the view says it: running at an address, or off.
fn bridge(api: &ApiSlot) -> Bridge {
    match api.url() {
        Some(url) => Bridge::Running(url),
        None => Bridge::Off,
    }
}

pub struct Coordinator {
    view: Entity<AssistantsSetup>,
    config: PathBuf,
    graph: PathBuf,
    api: ApiSlot,
    /// The setting the access file and the server agree on. The view is optimistic: it holds
    /// the *requested* value by the time the request arrives here, so it cannot say what to
    /// restore when a save fails.
    confirmed: Access,
    /// `UTOPIA_API` as this run found it. It is a launch override, read once, so an apply
    /// cannot see a different environment from the one the API started under.
    env_api: Option<String>,
    _sub: Subscription,
}

impl Coordinator {
    /// Install the tab for the graph that just opened. `api` is the same slot the inbox
    /// loop reads, so a server started here is drained by the running app (MC-12).
    pub fn install(graph: &Path, config: &Path, api: ApiSlot, cx: &mut App) -> Entity<Self> {
        Self::install_with(graph, config, api, std::env::var("UTOPIA_API").ok(), cx)
    }

    /// [`Self::install`] with the launch override given, so a test does not depend on the
    /// environment of the machine that runs it.
    fn install_with(
        graph: &Path,
        config: &Path,
        api: ApiSlot,
        env_api: Option<String>,
        cx: &mut App,
    ) -> Entity<Self> {
        // The client entries pin `--graph`, and a client starts the server from its own
        // directory: a relative path shown here would point somewhere else (MC-13).
        let graph = crate::mcp::absolute(graph).unwrap_or_else(|_| graph.to_path_buf());
        let file = access::access_file(config, &graph);
        let (setting, notice) = Access::load(&file);
        if let Some(notice) = &notice {
            eprintln!("utopia: {notice}");
        }
        let bridge = bridge(&api);
        let bin = std::env::current_exe().unwrap_or_default();
        let view = cx
            .new(|cx| AssistantsSetup::new(setting, bridge, notice, graph.to_path_buf(), bin, cx));
        cx.set_global(AssistantsHandle(view.clone()));
        cx.new(|cx| {
            let sub = cx.subscribe(&view, |this: &mut Self, _, request: &Request, cx| {
                this.apply(request.clone(), cx)
            });
            Self {
                view,
                config: config.to_path_buf(),
                graph: graph.to_path_buf(),
                api,
                confirmed: setting,
                env_api,
                _sub: sub,
            }
        })
    }

    /// Save first, then move the socket: the file is what a headless call reads (MC-1), so
    /// the setting must not depend on the server starting here.
    fn apply(&mut self, request: Request, cx: &mut Context<Self>) {
        // What is in force if the write fails: the controls go back to it, so the same click
        // can retry (the view holds the requested value, not this one).
        let previous = self.confirmed;
        let file = access::access_file(&self.config, &self.graph);
        if let Err(error) = request.access.save(&file) {
            let bridge = bridge(&self.api);
            self.view.update(cx, |view, cx| {
                view.applied(
                    previous,
                    bridge,
                    Some(format!("Could not save the access setting: {error}")),
                    cx,
                )
            });
            return;
        }
        // The file is the setting now, whatever happens to the socket next.
        self.confirmed = request.access;
        let failure = match action(
            self.env_api.as_deref(),
            self.api.is_running(),
            request.access,
        ) {
            Action::Nothing => None,
            Action::SetLevel(level) => {
                self.api.set_level(level);
                None
            }
            Action::Stop => {
                // Queued calls are cancelled and the join runs off this thread (MC-12): a
                // request the UI has not drained yet would otherwise hold the shutdown until
                // its timeout, with the loop that drains it blocked here. The discovery file
                // must not outlive the server, or a client would dial a dead address.
                self.api.stop(cx.background_executor());
                let _ = std::fs::remove_file(utopia_api::discovery_file(&self.config, &self.graph));
                None
            }
            Action::Start { addr, level } => {
                match api_launch::serve(&addr, level, &self.config, &self.graph) {
                    Ok(server) => {
                        self.api.replace(Some(server));
                        None
                    }
                    Err(error) => Some(format!(
                        "The setting is saved, but the bridge could not start: {error}"
                    )),
                }
            }
        };
        let bridge = bridge(&self.api);
        self.view.update(cx, |view, cx| {
            view.applied(request.access, bridge, failure, cx)
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    fn access(level: Level) -> Access {
        Access {
            level,
            when_closed: false,
        }
    }

    /// MC-12: a level written in Settings moves the loopback API with no restart. It reuses
    /// the launch decision of MC-1, so the three cases are the ones `plan` already defines.
    #[test]
    fn mc_12_a_level_starts_stops_or_relevels_the_bridge() {
        // Off and no `UTOPIA_API`: no socket, and one that runs is dropped.
        assert_eq!(action(None, false, access(Level::Off)), Action::Nothing);
        assert_eq!(action(None, true, access(Level::Off)), Action::Stop);
        // Off with `UTOPIA_API`: the socket stays for scripts, `/v1/op` is Off.
        assert_eq!(
            action(Some("1"), true, access(Level::Off)),
            Action::SetLevel(Level::Off)
        );
        assert_eq!(
            action(Some("1"), false, access(Level::Off)),
            Action::Start {
                addr: "127.0.0.1:0".into(),
                level: Level::Off
            }
        );
        // Read and Write start a server when none runs, and re-level it when one does.
        for level in [Level::Read, Level::Write] {
            assert_eq!(
                action(None, false, access(level)),
                Action::Start {
                    addr: "127.0.0.1:0".into(),
                    level
                }
            );
            assert_eq!(action(None, true, access(level)), Action::SetLevel(level));
        }
        // `UTOPIA_API` chooses the address, never the level (MC-1).
        assert_eq!(
            action(Some("127.0.0.1:4555"), false, access(Level::Read)),
            Action::Start {
                addr: "127.0.0.1:4555".into(),
                level: Level::Read
            }
        );
        // `when_closed` does not own the socket (MC-5).
        assert_eq!(
            action(
                None,
                false,
                Access {
                    level: Level::Read,
                    when_closed: true
                }
            ),
            Action::Start {
                addr: "127.0.0.1:0".into(),
                level: Level::Read
            }
        );
    }

    /// MC-12: the view is optimistic, so the coordinator cannot read "what was in force"
    /// back from it after a request arrives. A save that fails must put the confirmed level
    /// back, or the control says Off while the server still permits writes (and clicking Off
    /// again would be a no-op, so there is no retry).
    #[gpui::test]
    fn mc_12_a_failed_save_puts_the_confirmed_level_back(cx: &mut TestAppContext) {
        use std::os::unix::fs::PermissionsExt;

        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("g.utopia");
        std::fs::write(&graph, b"").unwrap();
        let config = dir.path().join("config");
        let file = access::access_file(&config, &graph);
        Access {
            level: Level::Write,
            when_closed: false,
        }
        .save(&file)
        .unwrap();

        let (coordinator, view) = cx.update(|cx| {
            let coordinator = Coordinator::install(&graph, &config, ApiSlot::new(None), cx);
            let view = cx.global::<AssistantsHandle>().0.clone();
            (coordinator, view)
        });
        let _coordinator = coordinator;
        assert_eq!(view.read_with(cx, |v, _| v.access.level), Level::Write);

        // Make the save fail but keep the file: a directory this process cannot write in.
        let mcp = config.join("mcp");
        std::fs::set_permissions(&mcp, std::fs::Permissions::from_mode(0o500)).unwrap();
        view.update(cx, |v, cx| v.choose_level(Level::Off, cx));
        cx.run_until_parked();
        std::fs::set_permissions(&mcp, std::fs::Permissions::from_mode(0o700)).unwrap();

        assert_eq!(
            view.read_with(cx, |v, _| v.access.level),
            Level::Write,
            "a save that failed leaves the control on the level still in force"
        );
        assert_eq!(
            Access::load(&file).0.level,
            Level::Write,
            "the file was not changed"
        );
    }

    /// MC-12 (rule 10): turning access off must not join the server thread on the UI thread.
    /// A call the app has not drained yet keeps its HTTP handler waiting, so a synchronous
    /// drop would block the very loop that would drain it, until the server's wait expires.
    #[gpui::test]
    fn mc_12_turning_access_off_does_not_wait_for_a_queued_call(cx: &mut TestAppContext) {
        use std::time::{Duration, Instant};

        let dir = tempfile::tempdir().unwrap();
        let graph = dir.path().join("g.utopia");
        std::fs::write(&graph, b"").unwrap();
        let config = dir.path().join("config");
        let file = access::access_file(&config, &graph);
        Access {
            level: Level::Write,
            when_closed: false,
        }
        .save(&file)
        .unwrap();

        // A real server at Write. Its wait is short (2 s instead of the default 30 s), so a
        // regression fails this test in a second instead of hanging the suite.
        let server = utopia_api::serve_with(
            "127.0.0.1:0",
            "token".into(),
            utopia_api::Limits {
                wait: Duration::from_secs(2),
                ..Default::default()
            },
        )
        .unwrap();
        // `/v1/op` checks the level when the request arrives; the server starts at Off.
        server.set_level(Level::Write);
        let discovery = utopia_api::Discovery {
            url: server.url(),
            token: server.token.clone(),
        };

        // One request that the app does not answer. Taking its call out of the queue and
        // holding it keeps the handler waiting, which is the state a synchronous drop has to
        // wait for.
        let request = std::thread::spawn(move || {
            utopia_api::Client::new(discovery).op(&utopia_api::Op::ListPages { daily: false })
        });
        let deadline = Instant::now() + Duration::from_secs(5);
        let call = loop {
            if let Some(call) = server.try_recv() {
                break call;
            }
            assert!(
                Instant::now() < deadline,
                "the request never reached the queue"
            );
            std::thread::sleep(Duration::from_millis(10));
        };

        let (coordinator, view) = cx.update(|cx| {
            let coordinator = Coordinator::install(&graph, &config, ApiSlot::new(Some(server)), cx);
            let view = cx.global::<AssistantsHandle>().0.clone();
            (coordinator, view)
        });
        let _coordinator = coordinator;

        // The request is applied as an effect at the end of this update, on this thread.
        let started = Instant::now();
        view.update(cx, |v, cx| v.choose_level(Level::Off, cx));
        let elapsed = started.elapsed();

        // Let the waiting handler answer, then let the background shutdown finish joining.
        drop(call);
        cx.run_until_parked();
        let result = request.join().unwrap();
        assert!(
            result.is_err(),
            "the queued call was cancelled without running: {result:?}"
        );
        assert!(
            elapsed < Duration::from_millis(500),
            "turning access off blocked the UI thread for {elapsed:?} on a queued call"
        );
        assert_eq!(Access::load(&file).0.level, Level::Off);
        assert_eq!(view.read_with(cx, |v, _| v.access.level), Level::Off);
    }
}
