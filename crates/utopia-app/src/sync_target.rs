//! Where a sync run publishes (§12.7): the one place that turns saved preferences plus
//! environment overrides into a transport. Settings → Sync (`sync_setup`) and the headless
//! `utopia sync` use it, so the window and the CLI cannot disagree about a target.

use std::path::Path;

use utopia_store::Destination;
use utopia_sync::runner::{BoxTransport, SyncConfig, SyncTarget};
use utopia_sync::setup::{Environment, Mode, Preferences};

/// How a Nearby client finds its host (§12.7): mDNS on the local network.
pub type Discover = dyn Fn(&str) -> Option<String> + Send + Sync;

/// A resolved target and, when this run hosts a Nearby relay, the relay.
///
/// The relay stops when it is dropped, so the caller keeps it for as long as it syncs.
pub struct Target {
    pub config: SyncConfig,
    pub lan: Option<utopia_syncd::lan::LanRelay>,
}

/// This run's credentials, already resolved by the caller (environment, a typed field, the
/// Keychain). Never serialized, never printed.
#[derive(Default)]
pub struct Credentials {
    pub token: Option<String>,
    pub passphrase: Option<String>,
}

/// The transport these settings name, or `None` when sync is Off.
///
/// `credentials` is this run's token and passphrase. `relay_file` is the graph's settings
/// file; a hosted Nearby relay keeps its database beside it. `release` is the relay this
/// caller currently hosts, if any: it is dropped only after the new settings are known to be
/// usable, because it holds the listen address a replacement needs.
pub fn resolve(
    preferences: &Preferences,
    env: &Environment,
    graph: &str,
    credentials: Credentials,
    relay_file: &Path,
    release: Option<utopia_syncd::lan::LanRelay>,
    discover: &Discover,
) -> Result<Option<Target>, String> {
    let Credentials { token, passphrase } = credentials;
    let (target, lan) = match preferences.mode {
        Mode::Off => return Ok(None),
        Mode::Relay => (
            SyncTarget::Relay {
                url: preferences.url.clone(),
                token,
            },
            None,
        ),
        Mode::Folder => (
            SyncTarget::Folder(
                preferences
                    .folder
                    .clone()
                    .ok_or("Choose a shared folder.")?,
            ),
            None,
        ),
        Mode::Nearby => match &preferences.host {
            Some(bind) => {
                if env.host.is_none() && token.as_ref().is_none_or(|s| s.is_empty()) {
                    return Err("Hosting needs a relay token.".into());
                }
                if let Some(parent) = relay_file.parent() {
                    std::fs::create_dir_all(parent)
                        .map_err(|_| "Could not create the relay directory.")?;
                }
                let db = relay_file.with_extension("relay.db");
                let cfg = utopia_syncd::Config {
                    tokens: token.clone().into_iter().collect(),
                    ..Default::default()
                };
                // The old host holds the listen address until it is dropped.
                drop(release);
                let relay =
                    utopia_syncd::lan::serve_lan(&db, bind, graph, cfg, true).map_err(|_| {
                        "Could not host the relay. Check the listen address and whether its port \
                         is in use."
                    })?;
                let url = format!("http://127.0.0.1:{}", relay.addr.port());
                (SyncTarget::Relay { url, token }, Some(relay))
            }
            None => {
                // A host that is asleep is normal: the caller looks again on the runner's
                // slow cadence instead of leaving sync off all day.
                let url = discover(graph).ok_or(
                    "No Nearby Mac found for this graph. Enable hosting on the other Mac and \
                     leave both Macs awake.",
                )?;
                (SyncTarget::Relay { url, token }, None)
            }
        },
    };
    Ok(Some(Target {
        config: SyncConfig {
            target,
            graph: graph.to_string(),
            passphrase,
        },
        lan,
    }))
}

/// Build the transport stack for a config: relay or folder, optionally encrypted
/// (§12.7, §11.7). The app and the CLI both call this so the two carry the same stack.
pub fn transport(cfg: &SyncConfig, client_id: &str) -> Result<BoxTransport, String> {
    utopia_sync::runner::build_transport(cfg, client_id)
        .map_err(|_| "Could not open the sync transport.".into())
}

/// The destination this run publishes to (§12.7): the transport's stable address plus the
/// relay store id, when the probe could read one. Sampling it *after* the probe is the
/// point — before that, a relay is only its URL, and a launch without a network must not
/// look like a different destination (`sync-engine-outbox-and-cursor.md`).
pub fn destination(
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
