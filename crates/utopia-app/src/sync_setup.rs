//! App-side setup coordinator (§12): blocking work stays on the background executor.
use gpui::{App, AppContext, Context, Entity, Subscription, Task};
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use utopia_sync::setup::probe::{test_connection, Connection};
use utopia_sync::setup::{
    save_secrets, Environment, Keychain, Mode, Preferences, Secrets, PASSPHRASE_SERVICE,
    TOKEN_SERVICE,
};
use utopia_ui::{
    sync_runner::{build_transport, BoxTransport, SyncConfig, SyncTarget},
    sync_setup::{Request, SetupHandle, SyncSetup},
    workspace::Workspace,
};

pub struct Coordinator {
    ws: Entity<Workspace>,
    view: Entity<SyncSetup>,
    file: PathBuf,
    graph: String,
    env: Environment,
    secrets: Arc<dyn Secrets>,
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
}

impl Coordinator {
    pub fn install(
        ws: Entity<Workspace>,
        graph_path: &Path,
        config: &Path,
        cx: &mut App,
    ) -> Entity<Self> {
        let env = Environment::read(|k| std::env::var(k).ok());
        let default_graph = crate::paths::graph_id(graph_path);
        let graph = env.graph.clone().unwrap_or_else(|| {
            ws.read(cx).app().clone().update(cx, |app, _| {
                app.store
                    .sync_get("setup_graph")
                    .ok()
                    .flatten()
                    .unwrap_or_else(|| {
                        let _ = app.store.sync_set("setup_graph", &default_graph);
                        default_graph
                    })
            })
        });
        let canonical = graph_path
            .canonicalize()
            .unwrap_or_else(|_| graph_path.to_path_buf());
        let id = utopia_store::sha256_hex(canonical.to_string_lossy().as_bytes());
        cx.set_global(utopia_ui::sync_runner::SyncFacts {
            graph: Some(graph.clone()),
            ..Default::default()
        });
        let file = config.join("sync").join(format!("{id}.json"));
        let view = cx.new(|cx| SyncSetup::new(graph.clone(), cx));
        cx.set_global(SetupHandle(view.clone()));
        let secrets: Arc<dyn Secrets> = Arc::new(Keychain::new(
            std::env::var("UTOPIA_SYNC_KEYCHAIN_PREFIX").ok(),
        ));
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
                    view.finish(
                        "Could not initialize sync bookkeeping. Local editing is still available."
                            .into(),
                        cx,
                    )
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
        let graph = self.graph.clone();
        let file = self.file.clone();
        let secrets = self.secrets.clone();
        let env = self.env.clone();
        let test_only = request.test_only;
        let active_url = self.view.read(cx).active_url.clone();
        let old_listener = if test_only {
            None
        } else {
            self.ws.update(cx, |ws, cx| ws.stop_sync(cx))
        };
        let old_lan = if test_only { None } else { self.lan.take() };
        self.task = Some(cx.spawn(async move |this, cx| {
            let prefs = request.preferences.clone();
            let result = cx.background_executor().spawn(async move {
                if let Some(listener) = old_listener { listener.stop(); }
                drop(old_lan);
                if test_only {
                    return probe(&request, &graph, &env, secrets.as_ref(), active_url).map(|message| (None, message));
                }
                request.preferences.validate()?;
                if request.preferences.mode == Mode::Off {
                    if save { request.preferences.save(&file)?; }
                    return Ok((None, "Sync is Off. Your notes and queued edits are kept.".into()));
                }
                let (token, passphrase) = credentials(&request, &graph, &env, secrets.as_ref())?;
                let key_check = utopia_sync::setup::verify_passphrase(passphrase.as_deref(), &graph, marker.as_deref())?;
                let mut lan = None;
                let target = match request.preferences.mode {
                    Mode::Relay => SyncTarget::Relay { url: request.preferences.url.clone(), token },
                    Mode::Folder => SyncTarget::Folder(request.preferences.folder.clone().ok_or("Choose a shared folder.")?),
                    Mode::Nearby => {
                        let url = if let Some(bind) = &request.preferences.host {
                            if env.host.is_none() && token.as_ref().is_none_or(|s| s.is_empty()) { return Err("Hosting requires a relay token. Enter one, then Apply.".into()); }
                            if let Some(parent) = file.parent() { std::fs::create_dir_all(parent).map_err(|_| "Could not create the relay directory.")?; }
                            let db = file.with_extension("relay.db");
                            let cfg = utopia_syncd::Config { tokens: token.clone().into_iter().collect(), ..Default::default() };
                            let relay = utopia_syncd::lan::serve_lan(&db, bind, &graph, cfg, true).map_err(|_| "Could not host the relay. Check the listen address and whether its port is in use.")?;
                            let url = format!("http://127.0.0.1:{}", relay.addr.port());
                            lan = Some(relay); url
                        } else {
                            utopia_syncd::lan::discover(&graph, Duration::from_secs(5)).ok_or("No Nearby Mac found for this graph. Enable hosting on the other Mac, then Apply again.")?
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
                let connection = test_connection(&mut transport);
                if transport.unreadable { return Err(utopia_sync::setup::verified_target::UNREADABLE.into()); }
                // The first E2E use freezes the key (marker + Keychain), so Apply insists on a
                // target that was reached and read with it. A launch with saved or environment
                // credentials against an unreachable target still starts: `VerifiedTarget`
                // refuses to push until the target has been read, and the marker waits for a
                // run that reaches it.
                let mut key_check = key_check;
                if cfg.passphrase.is_some() && marker.is_none() && connection != Connection::Reachable {
                    if save {
                        return Err(format!("Verify the target before first enabling E2E encryption. {}", connection.message()));
                    }
                    key_check = None;
                }
                // Only explicit setup writes credentials. Environment values override this
                // run without changing Keychain; the graph's marker still prevents re-keying.
                if save {
                    save_secrets(secrets.as_ref(), &graph, &request.token, &request.passphrase)?;
                    request.preferences.save(&file)?;
                }
                let transport: BoxTransport = Box::new(transport);
                Ok((Some(Prepared { cfg, transport, lan, key_check }), "Sync is on. The sidebar shows connection status.".into()))
            }).await;
            let _ = this.update(cx, |this, cx| {
                match result {
                    Ok((prepared, message)) => {
                        if let Some(prepared) = prepared {
                            let identity = prepared.transport.cursor_space().transport;
                            let ready = this.ws.read(cx).app().clone().update(cx, |app, _| {
                                if let Some(marker) = &prepared.key_check { app.store.sync_set("setup_e2e_check", marker)?; }
                                app.store.prepare_sync_destination(&identity)
                            });
                            match ready {
                                Ok(()) => {
                                    this.view.update(cx, |view, _| {
                                        view.active_url = match &prepared.cfg.target { SyncTarget::Relay { url, .. } => Some(url.clone()), _ => None };
                                    });
                                    let started = this.ws.update(cx, |ws, cx| ws.start_prepared_sync(&prepared.cfg, prepared.transport, cx));
                                    if !started {
                                        this.view.update(cx, |view, cx| view.finish("Sync is Off: the runner could not start. Local editing is still available.".into(), cx));
                                        return;
                                    }
                                    this.lan = prepared.lan;
                                }
                                Err(_) => {
                                    this.view.update(cx, |view, cx| view.finish("Sync is Off: could not queue this graph for the new destination. Local data is unchanged.".into(), cx));
                                    return;
                                }
                            }
                        } else if !test_only { this.view.update(cx, |view, _| view.active_url = None); }
                        this.view.update(cx, |view, cx| {
                            if !test_only { view.preferences = prefs; if save { view.notice = None; } }
                            view.finish(message, cx);
                        });
                    }
                    Err(message) => this.view.update(cx, |view, cx| view.finish(if test_only { message } else { format!("Sync is Off: {message}") }, cx)),
                }
            });
        }));
    }
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
            return Err("The E2E passphrase differs from this graph's saved key. Re-keying is not supported; use the original passphrase.".into());
        }
    }
    Ok((token, passphrase))
}
fn probe(
    request: &Request,
    graph: &str,
    env: &Environment,
    secrets: &dyn Secrets,
    active_url: Option<String>,
) -> Result<String, String> {
    let (token, _) = credentials(request, graph, env, secrets)?;
    let url = match request.preferences.mode {
        Mode::Relay => request.preferences.url.clone(),
        Mode::Nearby if request.preferences.host.is_some() => {
            active_url.ok_or("Apply hosting first, then test the connection.")?
        }
        Mode::Nearby => utopia_syncd::lan::discover(graph, Duration::from_secs(5))
            .ok_or("No Nearby Mac found for this graph.")?,
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
            return Ok(test_connection(&mut transport).message().into());
        }
        Mode::Off => return Err("Choose a transport first.".into()),
    };
    let mut transport = utopia_sync::http::HttpTransport::new(&url, graph, token)
        .map_err(|_| Connection::Unreachable.message())?;
    Ok(test_connection(&mut transport).message().into())
}

#[cfg(test)]
mod tests {
    use super::*;
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
    fn request() -> Request {
        Request {
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
        assert!(error.contains("Re-keying is not supported"));
        assert!(!error.contains("different-key") && !error.contains("original-key"));
    }
}
