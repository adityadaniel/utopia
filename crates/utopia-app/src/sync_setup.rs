//! App-side setup coordinator (§12): blocking work stays on the background executor.
use gpui::{App, AppContext, Context, Entity, Subscription, Task};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use utopia_store::Destination;
use utopia_sync::setup::probe::{probe, Connection};
use utopia_sync::setup::{
    save_secrets, Environment, Keychain, Mode, Preferences, Secrets, PASSPHRASE_SERVICE,
    TOKEN_SERVICE,
};
use utopia_sync::Transport;
use utopia_ui::{
    sync_runner::{build_transport, BoxTransport, SyncConfig, SyncTarget},
    sync_setup::{Request, SetupHandle, SyncSetup},
    workspace::Workspace,
};

/// How this device finds a Mac hosting this graph (§12.7 Nearby): mDNS on the local
/// network. Injected so the discovery retry can be driven in a test without multicast.
type Discover = Arc<dyn Fn(&str) -> Option<String> + Send + Sync>;

/// How often a Nearby client looks for its host again (§12.7): the runner's slow cadence,
/// because a host that is asleep now may wake at any point during the day.
const DISCOVERY_RETRY: Duration = utopia_ui::sync_runner::SLOW;

fn lan_discovery() -> Discover {
    Arc::new(|graph: &str| utopia_syncd::lan::discover(graph, Duration::from_secs(5)))
}

pub struct Coordinator {
    ws: Entity<Workspace>,
    view: Entity<SyncSetup>,
    file: PathBuf,
    graph: String,
    env: Environment,
    secrets: Arc<dyn Secrets>,
    discover: Discover,
    lan: Option<utopia_syncd::lan::LanRelay>,
    /// The initial load; its own slot because it ends by calling [`Self::run`], which
    /// replaces `task` while this one is still being polled.
    load: Option<Task<()>>,
    /// The setup or test in flight; replaced (cancelled), never accumulated.
    task: Option<Task<()>>,
    _sub: Subscription,
}
struct Prepared {
    cfg: SyncConfig,
    transport: BoxTransport,
    key_check: Option<String>,
    lan: Option<utopia_syncd::lan::LanRelay>,
    /// Where this run publishes (§12.7), as the probe observed it.
    destination: Destination,
}

impl Coordinator {
    pub fn install(
        ws: Entity<Workspace>,
        graph_path: &Path,
        config: &Path,
        cx: &mut App,
    ) -> Entity<Self> {
        let env = Environment::read(|k| std::env::var(k).ok());
        let secrets: Arc<dyn Secrets> = Arc::new(Keychain::new(
            std::env::var("UTOPIA_SYNC_KEYCHAIN_PREFIX").ok(),
        ));
        Self::install_with(ws, graph_path, config, env, secrets, lan_discovery(), cx)
    }

    /// The real app passes the Keychain and mDNS; tests pass the in-memory `Secrets` double
    /// and a discovery that can be made to succeed on a later attempt, so no test touches a
    /// real credential or the network.
    fn install_with(
        ws: Entity<Workspace>,
        graph_path: &Path,
        config: &Path,
        env: Environment,
        secrets: Arc<dyn Secrets>,
        discover: Discover,
        cx: &mut App,
    ) -> Entity<Self> {
        let default_graph = crate::paths::graph_id(graph_path);
        // Opening a graph does not freeze its id: a copy of the file is a new notebook and
        // may pick its own before it first syncs (§12.7). Only a successful Apply writes it.
        let stored = ws.read(cx).app().clone().update(cx, |app, _| {
            app.store.sync_get("setup_graph").ok().flatten()
        });
        // An environment variable owns this run, so its id is not the user's to edit here.
        let graph_editable = env.graph.is_none() && stored.is_none();
        let graph = env.graph.clone().or(stored).unwrap_or(default_graph);
        let canonical = graph_path
            .canonicalize()
            .unwrap_or_else(|_| graph_path.to_path_buf());
        let id = utopia_store::sha256_hex(canonical.to_string_lossy().as_bytes());
        cx.set_global(utopia_ui::sync_runner::SyncFacts {
            graph: Some(graph.clone()),
            ..Default::default()
        });
        let file = config.join("sync").join(format!("{id}.json"));
        let view = cx.new(|cx| SyncSetup::new(graph.clone(), graph_editable, cx));
        cx.set_global(SetupHandle(view.clone()));
        cx.new(|cx| {
            let sub = cx.subscribe(&view, |this: &mut Self, _, request: &Request, cx| {
                this.run(request.clone(), true, cx)
            });
            let mut this = Self {
                ws,
                view,
                file,
                graph,
                env,
                secrets,
                discover,
                lan: None,
                load: None,
                task: None,
                _sub: sub,
            };
            this.load(cx);
            this
        })
    }
    fn load(&mut self, cx: &mut Context<Self>) {
        let file = self.file.clone();
        self.load = Some(cx.spawn(async move |this, cx| {
            let (saved, notice) = cx
                .background_executor()
                .spawn(async move { Preferences::load(&file) })
                .await;
            let _ = this.update(cx, |this, cx| {
                let preferences = this.env.apply(&saved);
                this.view.update(cx, |view, cx| {
                    view.loaded(preferences.clone(), this.env.names.clone(), notice, cx)
                });
                this.run(
                    Request {
                        graph: this.graph.clone(),
                        preferences,
                        token: String::new(),
                        passphrase: String::new(),
                        test_only: false,
                    },
                    false,
                    cx,
                );
            });
        }));
    }
    fn run(&mut self, request: Request, save: bool, cx: &mut Context<Self>) {
        let client = self.ws.read(cx).app().clone().update(cx, |app, _| {
            utopia_sync::SyncEngine::new(&app.store).map(|engine| engine.client_id().to_string())
        });
        let client = match client {
            Ok(client) => client,
            Err(_) => {
                self.view.update(cx, |view, cx| {
                    view.finish("Could not set up sync for this graph.".into(), cx)
                });
                return;
            }
        };
        let marker = self
            .ws
            .read(cx)
            .app()
            .read(cx)
            .store
            .sync_get("setup_e2e_check")
            .ok()
            .flatten();
        let graph = request.graph.clone();
        let file = self.file.clone();
        let secrets = self.secrets.clone();
        let env = self.env.clone();
        let discover = self.discover.clone();
        let test_only = request.test_only;
        let active_url = self.view.read(cx).active_url.clone();
        // Nothing that already works is torn down before the new target is ready: a failed
        // Apply (a bad URL, an unreachable host, a key mismatch) leaves the running sync
        // alone. Hosting is the exception, because the port has to be free before a new
        // relay can bind it; it is released below, where the bind happens.
        let hosting = !test_only
            && request.preferences.mode == Mode::Nearby
            && request.preferences.host.is_some();
        let old_lan = if hosting { self.lan.take() } else { None };
        self.task = Some(cx.spawn(async move |this, cx| {
            let prefs = request.preferences.clone();
            let mut old_lan = old_lan;
            loop {
            // A retry runs the same setup again; the background task gets its own copies
            // (the relay database sits next to the settings file the UI thread keeps).
            let request = request.clone();
            let graph = graph.clone();
            let env = env.clone();
            let secrets = secrets.clone();
            let discover = discover.clone();
            let active_url = active_url.clone();
            let client = client.clone();
            let relay_file = file.clone();
            let marker = marker.clone();
            let old_lan = old_lan.take();
            let result = cx.background_executor().spawn(async move {
                if test_only {
                    return test_target(&request, &graph, &env, secrets.as_ref(), active_url, discover.as_ref()).map(|message| (None, message));
                }
                request.preferences.validate()?;
                if request.preferences.mode == Mode::Off {
                    return Ok((None, "Sync is Off.".into()));
                }
                let (token, passphrase) = credentials(&request, &graph, &env, secrets.as_ref())?;
                let key_check = utopia_sync::setup::verify_passphrase(passphrase.as_deref(), &graph, marker.as_deref())?;
                let mut lan = None;
                let target = match request.preferences.mode {
                    Mode::Relay => SyncTarget::Relay { url: request.preferences.url.clone(), token },
                    Mode::Folder => SyncTarget::Folder(request.preferences.folder.clone().ok_or("Choose a shared folder.")?),
                    Mode::Nearby => {
                        let url = if let Some(bind) = &request.preferences.host {
                            if env.host.is_none() && token.as_ref().is_none_or(|s| s.is_empty()) { return Err("Hosting needs a relay token.".into()); }
                            if let Some(parent) = relay_file.parent() { std::fs::create_dir_all(parent).map_err(|_| "Could not create the relay directory.")?; }
                            let db = relay_file.with_extension("relay.db");
                            let cfg = utopia_syncd::Config { tokens: token.clone().into_iter().collect(), ..Default::default() };
                            // The old host holds the listen address until it is dropped.
                            drop(old_lan);
                            let relay = utopia_syncd::lan::serve_lan(&db, bind, &graph, cfg, true).map_err(|_| "Could not host the relay. Check the listen address and whether its port is in use.")?;
                            let url = format!("http://127.0.0.1:{}", relay.addr.port());
                            lan = Some(relay); url
                        } else {
                            // A host that is asleep is normal: the caller looks again on the
                            // runner's slow cadence instead of leaving sync off all day.
                            discover(&graph).ok_or("No Nearby Mac found for this graph. Enable hosting on the other Mac and leave both Macs awake.")?
                        };
                        SyncTarget::Relay { url, token }
                    }
                    Mode::Off => unreachable!("handled before credentials"),
                };
                let cfg = SyncConfig { target, graph: graph.clone(), passphrase };
                let transport = build_transport(&cfg, &client).map_err(|_| "Could not open the sync transport.")?;
                let mut transport = utopia_sync::setup::verified_target::VerifiedTarget::new(transport);
                // Learn relay identity before choosing a destination; an unavailable relay
                // still starts the normal retry loop, preserving offline-first editing.
                let connected = probe(&mut transport);
                let connection = connected.connection();
                if transport.unreadable { return Err(utopia_sync::setup::verified_target::UNREADABLE.into()); }
                // The first E2E use freezes the key (marker + Keychain), so Apply insists on a
                // target that was reached and read with it. A launch with saved or environment
                // credentials against an unreachable target still starts: `VerifiedTarget`
                // refuses to push until the target has been read, and the marker waits for a
                // run that reaches it. An empty target is not proof either — nothing can be
                // read back — so a wrong passphrase is never frozen into the graph file
                // (§12.6, `ac_12_6_an_empty_target_never_proves_a_key`).
                let mut key_check = key_check;
                if cfg.passphrase.is_some() && marker.is_none() {
                    if connection != Connection::Reachable {
                        if save {
                            return Err(format!("Verify the target before first enabling E2E encryption. {}", connection.message()));
                        }
                        key_check = None;
                    } else if !utopia_sync::setup::may_freeze_key(&connected) {
                        key_check = None;
                    }
                }
                // Only explicit setup writes credentials. Environment values override this
                // run without changing Keychain; the graph's marker still prevents re-keying.
                if save {
                    save_secrets(secrets.as_ref(), &graph, &request.token, &request.passphrase)?;
                }
                let transport: BoxTransport = Box::new(transport);
                let destination = destination(&cfg, transport.cursor_space(), connected.instance);
                Ok((Some(Prepared { cfg, transport, lan, key_check, destination }), "Sync is on.".into()))
            }).await;
            let update = this.update(cx, |this, cx| -> (Option<utopia_sync::realtime::Listener>, bool) {
                match result {
                    Ok((None, message)) => {
                        // Off (or a connection test): only stopping sync touches the runner.
                        let old = if test_only { None } else { this.ws.update(cx, |ws, cx| ws.stop_sync(cx)) };
                        if !test_only {
                            this.lan = None;
                            this.view.update(cx, |view, _| view.active_url = None);
                            if save {
                                if let Err(error) = prefs.save(&file) {
                                    this.view.update(cx, |view, cx| view.finish(format!("Sync is Off: {error}"), cx));
                                    return (old, false);
                                }
                            }
                        }
                        this.view.update(cx, |view, cx| {
                            if !test_only { view.preferences = prefs.clone(); if save { view.notice = None; } }
                            view.finish(message, cx);
                        });
                        (old, false)
                    }
                    Ok((Some(prepared), message)) => {
                        // The old runner is stopped only now, and before the destination is
                        // queued: a round still in flight must not push the republished rows
                        // to the target being replaced.
                        let old = if test_only { None } else { this.ws.update(cx, |ws, cx| ws.stop_sync(cx)) };
                        let graph = prepared.cfg.graph.clone();
                        let ready = this.ws.read(cx).app().clone().update(cx, |app, _| {
                            if let Some(marker) = &prepared.key_check { app.store.sync_set("setup_e2e_check", marker)?; }
                            // From here on the id names data: it is frozen into the graph.
                            app.store.sync_set("setup_graph", &graph)?;
                            app.store.prepare_sync_destination(&prepared.destination)
                        });
                        // Every failure from here on leaves no runner, so a relay this
                        // device was hosting must stop with it.
                        if ready.is_err() {
                            this.lan = None;
                            this.view.update(cx, |view, cx| view.finish("Sync is Off: could not queue this graph for the new destination.".into(), cx));
                            return (old, false);
                        }
                        // The settings describe what is running, so they are written after
                        // the destination record, never before a target is known to work.
                        if save {
                            if let Err(error) = prefs.save(&file) {
                                this.lan = None;
                                this.view.update(cx, |view, cx| view.finish(format!("Sync is Off: {error}"), cx));
                                return (old, false);
                            }
                        }
                        this.view.update(cx, |view, _| {
                            view.active_url = match &prepared.cfg.target { SyncTarget::Relay { url, .. } => Some(url.clone()), _ => None };
                        });
                        let started = this.ws.update(cx, |ws, cx| ws.start_prepared_sync(&prepared.cfg, prepared.transport, cx));
                        if !started {
                            this.lan = None;
                            this.view.update(cx, |view, cx| view.finish("Sync is Off: it could not start.".into(), cx));
                            return (old, false);
                        }
                        this.lan = prepared.lan;
                        this.graph = graph.clone();
                        this.view.update(cx, |view, cx| {
                            view.freeze_graph(graph, cx);
                            view.preferences = prefs.clone();
                            if save { view.notice = None; }
                            view.finish(message, cx);
                        });
                        (old, false)
                    }
                    Err(message) => {
                        // A Nearby Mac that is asleep is the one failure worth waiting out:
                        // look again on the slow cadence (§12.7). The previous target, if
                        // any, keeps running meanwhile: nothing was torn down.
                        let retry = !test_only && prefs.mode == Mode::Nearby && prefs.host.is_none();
                        let message = if retry {
                            format!("Sync is Off: {message} Looking again every minute.")
                        } else {
                            format!("Sync is Off: {message}")
                        };
                        this.view.update(cx, |view, cx| view.finish(message, cx));
                        let old = if hosting {
                            // Its listen socket is gone with the old host, so it cannot work.
                            this.ws.update(cx, |ws, cx| ws.stop_sync(cx))
                        } else {
                            None
                        };
                        (old, retry)
                    }
                }
            });
            // Joining a listener blocks, so it never runs on the UI thread.
            let (listener, retry) = match update {
                Ok(update) => update,
                Err(_) => break,
            };
            if let Some(listener) = listener {
                cx.background_executor().spawn(async move { listener.stop() }).await;
            }
            if !retry {
                break;
            }
            cx.background_executor().timer(DISCOVERY_RETRY).await;
            }
        }));
    }
}

/// The destination this run publishes to (§12.7): the transport's stable address plus the
/// relay store id, when the probe could read one. Sampling it *after* the probe is the
/// point — before that, a relay is only its URL, and a launch without a network must not
/// look like a different destination (`sync-engine-outbox-and-cursor.md`).
fn destination(
    cfg: &SyncConfig,
    space: utopia_sync::CursorSpace,
    instance: Option<String>,
) -> Destination {
    let origin = match &cfg.target {
        // The cursor space of a relay is `relay:<store-or-url>#<graph>`; the origin is the
        // same string spelled with the URL, which is what an offline launch knows.
        SyncTarget::Relay { url, .. } => utopia_sync::http::destination_origin(url, &cfg.graph),
        // A folder is identified by its root, and has no store id to learn.
        SyncTarget::Folder(_) => space.transport,
    };
    Destination::new(origin, instance)
}

fn credentials(
    request: &Request,
    graph: &str,
    env: &Environment,
    secrets: &dyn Secrets,
) -> Result<(Option<String>, Option<String>), String> {
    let token = match env
        .token
        .clone()
        .or_else(|| (!request.token.is_empty()).then(|| request.token.clone()))
    {
        Some(token) => Some(token),
        None => secrets.get(TOKEN_SERVICE, graph)?,
    };
    let stored = secrets.get(PASSPHRASE_SERVICE, graph)?;
    let passphrase = env
        .passphrase
        .clone()
        .or_else(|| (!request.passphrase.is_empty()).then(|| request.passphrase.clone()))
        .or_else(|| stored.clone());
    if let (Some(old), Some(new)) = (&stored, &passphrase) {
        if old != new {
            return Err("This passphrase is not the one the graph was first synced with. Use the original passphrase.".into());
        }
    }
    Ok((token, passphrase))
}
fn test_target(
    request: &Request,
    graph: &str,
    env: &Environment,
    secrets: &dyn Secrets,
    active_url: Option<String>,
    discover: &(dyn Fn(&str) -> Option<String> + Send + Sync),
) -> Result<String, String> {
    let (token, _) = credentials(request, graph, env, secrets)?;
    let url = match request.preferences.mode {
        Mode::Relay => request.preferences.url.clone(),
        Mode::Nearby if request.preferences.host.is_some() => {
            active_url.ok_or("Apply hosting first, then test the connection.")?
        }
        Mode::Nearby => discover(graph).ok_or("No Nearby Mac found for this graph.")?,
        Mode::Folder => {
            let path = request
                .preferences
                .folder
                .as_ref()
                .ok_or("Choose a shared folder.")?;
            if !path.is_dir() {
                return Err("The shared folder is not available.".into());
            }
            let mut transport = utopia_sync::folder::FolderTransport::new(path, "connection-test")
                .map_err(|_| "Could not open the shared folder.")?;
            return Ok(probe(&mut transport).connection().message().into());
        }
        Mode::Off => return Err("Choose a transport first.".into()),
    };
    let mut transport = utopia_sync::http::HttpTransport::new(&url, graph, token)
        .map_err(|_| Connection::Unreachable.message())?;
    Ok(probe(&mut transport).connection().message().into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;
    use std::sync::Mutex;
    use utopia_store::Store;
    use utopia_ui::app_state::AppState;
    use utopia_ui::theme::Theme;
    use utopia_ui::workspace::Workspace;

    /// The `Secrets` double: no test touches a real Keychain item (§15.6).
    #[derive(Default)]
    struct MemorySecrets(Mutex<Vec<(String, String, String)>>);
    impl Secrets for MemorySecrets {
        fn get(&self, service: &str, account: &str) -> Result<Option<String>, String> {
            Ok(self
                .0
                .lock()
                .unwrap()
                .iter()
                .find(|(s, a, _)| s == service && a == account)
                .map(|(_, _, value)| value.clone()))
        }
        fn set(&self, service: &str, account: &str, value: &str) -> Result<(), String> {
            let mut all = self.0.lock().unwrap();
            all.retain(|(s, a, _)| !(s == service && a == account));
            all.push((service.into(), account.into(), value.into()));
            Ok(())
        }
    }

    /// The settings file the coordinator reads for `graph` (`sync/<sha256>.json`).
    fn settings_file(config: &Path, graph: &Path) -> PathBuf {
        let canonical = graph.canonicalize().unwrap_or_else(|_| graph.to_path_buf());
        config.join("sync").join(format!(
            "{}.json",
            utopia_store::sha256_hex(canonical.to_string_lossy().as_bytes())
        ))
    }

    struct Harness {
        app: Entity<AppState>,
        ws: Entity<Workspace>,
        setup: Entity<SyncSetup>,
        /// Held, as the app's `Ready` holds it: an entity nobody owns is released and its
        /// background tasks can no longer update it.
        _coordinator: Entity<Coordinator>,
        graph: PathBuf,
        config: PathBuf,
    }

    /// A workspace over a scratch graph, with the coordinator installed the way the app
    /// installs it at startup (`saved` is what the settings file holds).
    fn harness(
        cx: &mut TestAppContext,
        dir: &Path,
        saved: Option<&Preferences>,
        discover: Discover,
    ) -> Harness {
        let graph = dir.join("Demo.utopia");
        std::fs::write(&graph, b"").unwrap();
        let config = dir.join("config");
        if let Some(prefs) = saved {
            let file = settings_file(&config, &graph);
            std::fs::create_dir_all(file.parent().unwrap()).unwrap();
            prefs.save(&file).unwrap();
        }
        cx.update(|cx| {
            cx.set_global(Theme::light());
            utopia_ui::actions::install_keymap(cx, None);
        });
        let store = Store::open_in_memory().unwrap();
        let app = cx.new(|_| AppState::new(store));
        let (ws, cx) = cx.add_window_view(|window, cx| Workspace::new(app.clone(), window, cx));
        let secrets: Arc<dyn Secrets> = Arc::new(MemorySecrets::default());
        let coordinator = cx.update(|_, cx| {
            Coordinator::install_with(
                ws.clone(),
                &graph,
                &config,
                Environment::default(),
                secrets,
                discover,
                cx,
            )
        });
        let setup = coordinator.read_with(cx, |coordinator, _| coordinator.view.clone());
        Harness {
            app,
            ws,
            setup,
            _coordinator: coordinator,
            graph,
            config,
        }
    }

    /// §12.7: a failed Apply must not stop the sync that already works. The saved settings
    /// keep naming the old target, so the pane and the next launch agree with the runner.
    #[gpui::test]
    fn ac_12_7_a_failed_apply_keeps_the_running_sync(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        let saved = Preferences {
            mode: Mode::Folder,
            folder: Some(shared.clone()),
            ..Default::default()
        };
        let h = harness(cx, dir.path(), Some(&saved), lan_discovery());
        cx.run_until_parked();
        assert!(
            h.ws.read_with(cx, |ws, _| ws.sync_phase().is_some()),
            "the saved folder starts sync: {}",
            h.setup.read_with(cx, |view, _| view.status.clone())
        );
        // A folder under a file cannot be opened, so the Apply fails after the transport
        // is built — one of the failures that used to stop the working sync.
        let blocker = dir.path().join("blocker");
        std::fs::write(&blocker, b"not a directory").unwrap();
        let unreachable = blocker.join("shared");
        h.setup.update(cx, |view, cx| {
            view.choose(Mode::Folder, cx);
            view.folder.update(cx, |input, cx| {
                input.fill(&unreachable.display().to_string(), cx)
            });
            view.request(false, cx);
        });
        cx.run_until_parked();
        let status = h.setup.read_with(cx, |view, _| view.status.clone());
        assert!(status.starts_with("Sync is Off:"), "{status}");
        assert!(
            h.ws.read_with(cx, |ws, _| ws.sync_phase().is_some()),
            "a failed Apply stopped the working sync: {status}"
        );
        assert_eq!(
            Preferences::load(&settings_file(&h.config, &h.graph)).0,
            saved,
            "the saved settings still name the target that is running"
        );
    }

    struct Stored;
    impl Secrets for Stored {
        fn get(&self, service: &str, _: &str) -> Result<Option<String>, String> {
            Ok(Some(
                if service == TOKEN_SERVICE {
                    "saved-token"
                } else {
                    "original-key"
                }
                .into(),
            ))
        }
        fn set(&self, _: &str, _: &str, _: &str) -> Result<(), String> {
            panic!("resolving an environment override must not write credentials")
        }
    }
    /// §12.7: the graph id names the graph on the relay. Opening a graph must not freeze it:
    /// a copy of `Notes.utopia` used as a second notebook would otherwise publish into the
    /// first one's relay graph and merge both. The first Apply records the id and freezes it.
    #[gpui::test]
    fn ac_12_7_an_unsynced_graph_id_stays_editable(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let shared = dir.path().join("shared");
        std::fs::create_dir_all(&shared).unwrap();
        let h = harness(cx, dir.path(), None, lan_discovery());
        cx.run_until_parked();
        assert!(
            h.app
                .read_with(cx, |app, _| app.store.sync_get("setup_graph").unwrap())
                .is_none(),
            "opening a graph wrote its id into the file"
        );
        assert!(h.setup.read_with(cx, |view, _| view.graph_editable));
        h.setup.update(cx, |view, cx| {
            view.graph_input
                .update(cx, |input, cx| input.fill("notes-second", cx));
            view.choose(Mode::Folder, cx);
            view.folder.update(cx, |input, cx| {
                input.fill(&shared.display().to_string(), cx)
            });
            view.request(false, cx);
        });
        cx.run_until_parked();
        assert_eq!(
            h.app
                .read_with(cx, |app, _| app.store.sync_get("setup_graph").unwrap())
                .as_deref(),
            Some("notes-second"),
            "the chosen id is the one the graph syncs under"
        );
        assert!(
            !h.setup.read_with(cx, |view, _| view.graph_editable),
            "the id is frozen once it names synced data"
        );
        assert!(h.ws.read_with(cx, |ws, _| ws.sync_phase().is_some()));
    }

    /// §12.7: a Nearby Mac can be asleep when the laptop starts. Discovery is retried on the
    /// runner's slow cadence instead of leaving sync off until someone opens Settings again.
    #[gpui::test]
    fn ac_12_7_a_nearby_client_keeps_looking_for_its_host(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let saved = Preferences {
            mode: Mode::Nearby,
            ..Default::default()
        };
        // The host is up, but only visible to the second look: exactly the laptop that
        // wakes before the Mac in the study.
        let relay = utopia_syncd::lan::serve_lan(
            &dir.path().join("lan.db"),
            "127.0.0.1:0",
            "Demo",
            utopia_syncd::Config::default(),
            false,
        )
        .unwrap();
        let url = format!("http://{}", relay.addr);
        let looks = Arc::new(Mutex::new(0usize));
        let discover: Discover = {
            let looks = looks.clone();
            Arc::new(move |_graph: &str| {
                let mut n = looks.lock().unwrap();
                *n += 1;
                (*n > 1).then(|| url.clone())
            })
        };
        let h = harness(cx, dir.path(), Some(&saved), discover);
        cx.run_until_parked();
        assert!(
            h.ws.read_with(cx, |ws, _| ws.sync_phase().is_none()),
            "the first look finds nothing, so sync is off for now"
        );
        // The host appears; a minute later the app looks again by itself.
        cx.background_executor.advance_clock(DISCOVERY_RETRY);
        cx.run_until_parked();
        assert_eq!(
            *looks.lock().unwrap(),
            2,
            "one retry after the slow cadence"
        );
        assert!(
            h.ws.read_with(cx, |ws, _| ws.sync_phase().is_some()),
            "the client never looked again: {}",
            h.setup.read_with(cx, |view, _| view.status.clone())
        );
    }

    fn request() -> Request {
        Request {
            graph: "g".into(),
            preferences: Preferences::default(),
            token: String::new(),
            passphrase: String::new(),
            test_only: false,
        }
    }
    #[test]
    fn ac_15_6_environment_credentials_override_without_writing_keychain() {
        let env = Environment {
            token: Some("run-token".into()),
            ..Default::default()
        };
        let (token, pass) = credentials(&request(), "g", &env, &Stored).unwrap();
        assert_eq!(token.as_deref(), Some("run-token"));
        assert_eq!(pass.as_deref(), Some("original-key"));
    }
    #[test]
    fn ac_12_6_environment_cannot_silently_rekey_an_encrypted_graph() {
        let env = Environment {
            passphrase: Some("different-key".into()),
            ..Default::default()
        };
        let error = credentials(&request(), "g", &env, &Stored).unwrap_err();
        assert!(error.contains("original passphrase"), "{error}");
        assert!(!error.contains("different-key") && !error.contains("original-key"));
    }
}
