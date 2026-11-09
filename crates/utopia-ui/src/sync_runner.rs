//! Background sync for the desktop app (§12.3, §13.3): store work (prepare/apply) runs on
//! the UI thread in short steps; network exchange runs on a background thread. Polls every
//! 5 s while there is activity, backing off to 60 s when idle or failing.

use std::time::Duration;

use utopia_sync::{Incoming, Outgoing, SyncEngine, SyncError, SyncReport, SyncStatus, Transport};

pub const FAST: Duration = Duration::from_secs(5);
pub const SLOW: Duration = Duration::from_secs(60);

/// Where sync goes (§12.7 transports).
#[derive(Clone)]
pub enum SyncTarget {
    /// Hosted or self-hosted relay (`utopia-syncd`), with an optional bearer token.
    Relay { url: String, token: Option<String> },
    /// A shared folder (iCloud Drive, Dropbox, Syncthing): no server.
    Folder(std::path::PathBuf),
}

/// Where and how to sync; resolved by the app from env/keychain, never stored in the graph.
#[derive(Clone)]
pub struct SyncConfig {
    pub target: SyncTarget,
    pub graph: String,
    /// End-to-end encryption passphrase (§11.7); `None` = plaintext payloads.
    pub passphrase: Option<String>,
}

/// What Settings → Sync can say about the transport in force (§12, ST-5): where it goes and
/// whether the payloads are encrypted. Built from [`SyncConfig`], with the bearer token left
/// out on purpose — secrets never reach the UI, `settings.json` or a screenshot (§15.6).
/// The app installs it once the config is resolved; with no global installed (tests, a
/// headless run) sync reads as off.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct SyncFacts {
    /// Human-readable transport, e.g. "Relay · https://relay.example" or "Folder · /…".
    pub target: Option<String>,
    /// The graph's id, which the relay uses to find its pages.
    pub graph: Option<String>,
    pub encrypted: bool,
}

impl gpui::Global for SyncFacts {}

impl SyncFacts {
    pub fn from_config(cfg: &SyncConfig) -> Self {
        let target = match &cfg.target {
            SyncTarget::Relay { url, .. } if url == "lan" => {
                "Relay · LAN, discovered with mDNS".to_string()
            }
            SyncTarget::Relay { url, .. } => format!("Relay · {url}"),
            SyncTarget::Folder(dir) => format!("Folder · {}", dir.display()),
        };
        Self {
            target: Some(target),
            graph: Some(cfg.graph.clone()),
            encrypted: cfg.passphrase.is_some(),
        }
    }

    pub fn is_on(&self) -> bool {
        self.target.is_some()
    }
}

/// The sync facts in force; sync off when the app has installed none.
pub fn facts(cx: &gpui::App) -> SyncFacts {
    cx.try_global::<SyncFacts>().cloned().unwrap_or_default()
}

pub type BoxTransport = Box<dyn Transport + Send>;

/// Build the transport stack for a config: relay or folder, optionally encrypted.
pub fn build_transport(cfg: &SyncConfig, client_id: &str) -> Result<BoxTransport, SyncError> {
    let inner: BoxTransport = match &cfg.target {
        SyncTarget::Relay { url, token } => Box::new(utopia_sync::http::HttpTransport::new(
            url,
            &cfg.graph,
            token.clone(),
        )?),
        SyncTarget::Folder(dir) => {
            Box::new(utopia_sync::folder::FolderTransport::new(dir, client_id)?)
        }
    };
    Ok(match &cfg.passphrase {
        Some(p) => {
            let key = utopia_sync::e2e::GraphKey::derive(p, &cfg.graph)?;
            Box::new(utopia_sync::e2e::Encrypted::new(
                inner,
                key,
                cfg.graph.clone(),
            ))
        }
        None => inner,
    })
}

/// Which half of a sync round is running, and therefore where the transport is (§12.3,
/// §13.3). The transport's home is [`SyncPhase::Idle`]; no phase ever means "no transport
/// exists". `interval` and `last` stay fields of [`SyncRunner`] on purpose: they are the
/// cadence and the history, not a phase, and both survive every transition.
pub enum SyncPhase {
    /// Between rounds: the transport is home and the next poll waits `interval`.
    Idle { transport: BoxTransport },
    /// The UI thread holds the transport and gathers pushes (short store reads, no network).
    Preparing,
    /// The background thread holds the transport and is talking to the relay.
    Exchanging,
    /// The exchange returned; the UI thread holds the transport and imports the result.
    Applying,
    /// The relay refused this device (401/403) or speaks a protocol this client does not
    /// (§12.6). The transport is still home: rounds continue at [`SLOW`] only, and realtime
    /// notices are ignored, so a transient refusal (a proxy hiccup, a relay starting before
    /// it has loaded its token file) heals by itself while a revoked token stops the 5 s
    /// polling. There is no in-app re-auth prompt yet, so this is the prompt's stand-in.
    Refused {
        reason: RefusalReason,
        transport: BoxTransport,
    },
}

/// Why the relay refused us: the two answers that retrying quickly cannot fix (§12.6).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RefusalReason {
    /// 401/403: the token is wrong or revoked.
    NeedsAuth,
    /// The relay's `protocol_version` is not the one this client speaks.
    Unsupported,
}

impl RefusalReason {
    /// The refusal in `error`, if it has one.
    pub fn of(error: &SyncError) -> Option<Self> {
        match error {
            SyncError::Unauthorized => Some(Self::NeedsAuth),
            SyncError::ProtocolVersion(_) => Some(Self::Unsupported),
            _ => None,
        }
    }

    /// The outcome to show while refused.
    pub fn status(self) -> SyncStatus {
        match self {
            Self::NeedsAuth => SyncStatus::NeedsAuth,
            Self::Unsupported => SyncStatus::Unsupported,
        }
    }
}

pub struct SyncRunner {
    pub engine: SyncEngine,
    /// The round's phase; the only place that says where the transport is.
    pub phase: SyncPhase,
    /// The last round's outcome; `None` = no round has finished yet ("connecting…").
    pub last: Option<SyncStatus>,
    /// Cadence between rounds: [`FAST`] while there is activity, backing off to [`SLOW`].
    pub interval: Duration,
    /// "Merged N changes" notice (§9.3) with the time it was raised.
    pub notice: Option<(String, i64)>,
    /// Raised by the realtime listener to sync now instead of at the next poll.
    pub nudge: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// This runner's identity. A loop may only drive the runner it was started for, so a
    /// superseded loop cannot write into its replacement (nothing else has this number).
    pub generation: u64,
    /// The loop driving this runner. Kept here so that replacing the runner drops (and so
    /// cancels) the previous loop: it is replaced, never accumulated (`_tasks` would only
    /// release it with the workspace).
    pub loop_task: Option<gpui::Task<()>>,
    pub realtime: Option<utopia_sync::realtime::Listener>,
}

impl SyncRunner {
    pub fn new(engine: SyncEngine, transport: BoxTransport) -> Self {
        static NEXT: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(1);
        Self {
            engine,
            phase: SyncPhase::Idle { transport },
            last: None,
            interval: FAST,
            notice: None,
            nudge: Default::default(),
            generation: NEXT.fetch_add(1, std::sync::atomic::Ordering::Relaxed),
            loop_task: None,
            realtime: None,
        }
    }

    /// Whether `generation` is this runner's token; false once sync was restarted.
    pub fn is_generation(&self, generation: u64) -> bool {
        self.generation == generation
    }

    /// Start a round: take the transport out of [`SyncPhase::Idle`], or out of
    /// [`SyncPhase::Refused`] for the slow retry. `None` when a round is already running,
    /// which the loop treats as someone else's round and leaves alone.
    pub fn begin_round(&mut self) -> Option<BoxTransport> {
        let transport = match std::mem::replace(&mut self.phase, SyncPhase::Exchanging) {
            SyncPhase::Idle { transport } | SyncPhase::Refused { transport, .. } => transport,
            other => {
                self.phase = other;
                return None;
            }
        };
        self.phase = SyncPhase::Preparing;
        Some(transport)
    }

    /// The round's store work failed, so nothing left the device: the transport comes home
    /// and the next poll backs off like any other failed round (§12.6). A local failure is
    /// never terminal, so this cannot halt. Only a `Preparing` round can fail this way.
    pub fn round_failed(&mut self, transport: BoxTransport, status: SyncStatus) {
        if !matches!(self.phase, SyncPhase::Preparing) {
            return;
        }
        self.interval = back_off(self.interval);
        self.last = Some(status);
        self.phase = SyncPhase::Idle { transport };
    }

    /// The prepared pushes leave for the background thread. Only `Preparing → Exchanging`;
    /// the transport is not in the runner at this point, so an out-of-order call must not
    /// clobber `Idle { transport }`.
    pub fn exchange(&mut self) {
        if matches!(self.phase, SyncPhase::Preparing) {
            self.phase = SyncPhase::Exchanging;
        }
    }

    /// The exchange returned; the UI thread imports it next. Only `Exchanging → Applying`.
    pub fn exchange_returned(&mut self) {
        if matches!(self.phase, SyncPhase::Exchanging) {
            self.phase = SyncPhase::Applying;
        }
    }

    /// The round is over: remember what to show and how soon to poll again. A refusal or a
    /// protocol mismatch pins the cadence to [`SLOW`] (§12.6): no fast polling, one retry
    /// per slow interval, and the transport stays so the retry can happen.
    pub fn end_round(
        &mut self,
        transport: BoxTransport,
        report: &Result<SyncReport, SyncError>,
        status: SyncStatus,
    ) {
        if !matches!(self.phase, SyncPhase::Applying) {
            return;
        }
        if let Err(e) = report {
            if let Some(reason) = RefusalReason::of(e) {
                self.interval = SLOW;
                self.last = Some(reason.status());
                self.phase = SyncPhase::Refused { reason, transport };
                return;
            }
        }
        self.interval = self.next_interval(report);
        self.last = Some(status);
        self.phase = SyncPhase::Idle { transport };
    }

    /// The cadence that would follow `result`, from the interval in force. `end_round` uses
    /// it; it is public for the tests that pin the backoff policy.
    pub fn next_interval(&self, result: &Result<SyncReport, SyncError>) -> Duration {
        next_interval(self.interval, result)
    }
}

/// Next poll interval after a round: fast again when anything moved, else back off. A round
/// that moved something but left a page failed backs off like a failure: the failed page is
/// retried, not spun on.
fn next_interval(current: Duration, result: &Result<SyncReport, SyncError>) -> Duration {
    match result {
        Ok(r) if !r.is_quiet() && r.failed.is_empty() => FAST,
        _ => back_off(current),
    }
}

/// One step of exponential backoff, capped at [`SLOW`].
fn back_off(current: Duration) -> Duration {
    (current * 2).min(SLOW)
}

/// The background half of a round.
pub fn exchange_owned(
    mut t: BoxTransport,
    out: Outgoing,
) -> (Result<Incoming, SyncError>, BoxTransport) {
    let r = utopia_sync::exchange(t.as_mut(), out);
    (r, t)
}

/// What the sidebar's single sync dot says (SB-4): grey when sync is off, green when the
/// last round finished, amber while a round is in flight or still connecting, red when the
/// relay refused, was unreachable or speaks another protocol, or when the round left a page
/// that could not sync (`Partial`).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SyncDot {
    Off,
    Synced,
    Syncing,
    Error,
}

impl SyncDot {
    /// Spelled into the element's debug selector, so a test can tell the states apart.
    pub fn key(self) -> &'static str {
        match self {
            SyncDot::Off => "off",
            SyncDot::Synced => "synced",
            SyncDot::Syncing => "syncing",
            SyncDot::Error => "error",
        }
    }
}

pub fn sync_dot(runner: Option<&SyncRunner>) -> SyncDot {
    let Some(runner) = runner else {
        return SyncDot::Off;
    };
    match &runner.phase {
        // Refused is not a round: the dot shows the refusal, not "syncing".
        SyncPhase::Refused { .. } => SyncDot::Error,
        // A round in flight wins over the last round's outcome.
        SyncPhase::Preparing | SyncPhase::Exchanging | SyncPhase::Applying => SyncDot::Syncing,
        SyncPhase::Idle { .. } => match runner.last {
            None => SyncDot::Syncing,
            Some(SyncStatus::Synced { .. }) => SyncDot::Synced,
            Some(SyncStatus::Partial { .. })
            | Some(SyncStatus::Offline { .. })
            | Some(SyncStatus::LocalError { .. })
            | Some(SyncStatus::NeedsAuth)
            | Some(SyncStatus::Unsupported) => SyncDot::Error,
        },
    }
}

/// Human status line for the sidebar, from the one state the dot reads.
pub fn status_line(runner: &SyncRunner, now_ms: i64) -> (String, bool) {
    match runner.phase {
        // A round is running: the dot is amber, so the line must not show the time of the
        // last finished round as if nothing were happening (SB-4).
        SyncPhase::Preparing | SyncPhase::Exchanging | SyncPhase::Applying => {
            ("Sync: syncing…".into(), false)
        }
        SyncPhase::Idle { .. } | SyncPhase::Refused { .. } => {
            status_text(runner.last.as_ref(), now_ms)
        }
    }
}

/// The line for one outcome.
pub fn status_text(status: Option<&SyncStatus>, now_ms: i64) -> (String, bool) {
    match status {
        None => ("Sync: connecting…".into(), false),
        Some(SyncStatus::Synced { at_ms }) => {
            let secs = ((now_ms - at_ms) / 1000).max(0);
            let ago = if secs < 60 {
                "just now".to_string()
            } else if secs < 3600 {
                format!("{}m ago", secs / 60)
            } else {
                format!("{}h ago", secs / 3600)
            };
            (format!("Synced {ago}"), false)
        }
        Some(SyncStatus::Partial { failed, queued }) => {
            let pages = if *failed == 1 { "page" } else { "pages" };
            if *queued == 0 {
                (format!("● Synced — {failed} {pages} could not sync"), true)
            } else {
                (
                    format!("● {failed} {pages} could not sync — {queued} changes queued"),
                    true,
                )
            }
        }
        Some(SyncStatus::Offline { queued }) => {
            (format!("● Offline — {queued} changes queued"), true)
        }
        Some(SyncStatus::LocalError { message }) => {
            (format!("● Sync: local error — {message}"), true)
        }
        Some(SyncStatus::NeedsAuth) => ("● Sync: sign-in required".into(), true),
        Some(SyncStatus::Unsupported) => ("● Sync: relay protocol unsupported".into(), true),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// ST-5: Settings → Sync shows where the graph goes, and never the token or the
    /// passphrase that got it there (§15.6).
    #[test]
    fn sync_facts_never_carry_secrets() {
        let cfg = SyncConfig {
            target: SyncTarget::Relay {
                url: "https://relay.example".into(),
                token: Some("t0ken".into()),
            },
            graph: "graph-1".into(),
            passphrase: Some("p4ssphrase".into()),
        };
        let facts = SyncFacts::from_config(&cfg);
        assert_eq!(
            facts.target.as_deref(),
            Some("Relay · https://relay.example")
        );
        assert_eq!(facts.graph.as_deref(), Some("graph-1"));
        assert!(facts.encrypted);
        let shown = format!("{facts:?}");
        assert!(!shown.contains("t0ken"), "{shown}");
        assert!(!shown.contains("p4ssphrase"), "{shown}");

        let folder = SyncConfig {
            target: SyncTarget::Folder("/tmp/shared notes".into()),
            graph: "graph-2".into(),
            passphrase: None,
        };
        let facts = SyncFacts::from_config(&folder);
        assert_eq!(facts.target.as_deref(), Some("Folder · /tmp/shared notes"));
        assert!(!facts.encrypted);

        assert!(!SyncFacts::default().is_on(), "no config means sync is off");
    }

    #[test]
    fn the_dot_tells_off_synced_syncing_and_error_apart() {
        assert_eq!(sync_dot(None), SyncDot::Off);
        // A runner with a transport and no round yet is connecting.
        let engine = utopia_sync::SyncEngine::new(&utopia_store::Store::open_in_memory().unwrap())
            .expect("engine");
        let transport: BoxTransport = Box::new(utopia_sync::relay::RelayTransport::new(
            utopia_sync::relay::Relay::new(),
            "g",
        ));
        let mut runner = SyncRunner::new(engine, transport);
        assert_eq!(sync_dot(Some(&runner)), SyncDot::Syncing);
        runner.last = Some(SyncStatus::Synced { at_ms: 0 });
        assert_eq!(sync_dot(Some(&runner)), SyncDot::Synced);
        runner.last = Some(SyncStatus::Offline { queued: 2 });
        assert_eq!(sync_dot(Some(&runner)), SyncDot::Error);
        runner.last = Some(SyncStatus::Partial {
            failed: 1,
            queued: 0,
        });
        assert_eq!(sync_dot(Some(&runner)), SyncDot::Error);
        runner.last = Some(SyncStatus::LocalError {
            message: "database: disk I/O error".into(),
        });
        assert_eq!(sync_dot(Some(&runner)), SyncDot::Error);
        // A round in flight wins over the last round's status.
        runner.last = Some(SyncStatus::Synced { at_ms: 0 });
        runner.begin_round().expect("the transport is home");
        assert_eq!(sync_dot(Some(&runner)), SyncDot::Syncing);
    }

    #[test]
    fn a_round_that_left_a_page_failed_backs_off() {
        let engine = utopia_sync::SyncEngine::new(&utopia_store::Store::open_in_memory().unwrap())
            .expect("engine");
        let transport: BoxTransport = Box::new(utopia_sync::relay::RelayTransport::new(
            utopia_sync::relay::Relay::new(),
            "g",
        ));
        let runner = SyncRunner::new(engine, transport);
        let report = SyncReport {
            pushed: 1,
            ..Default::default()
        };
        assert_eq!(runner.next_interval(&Ok(report.clone())), FAST);
        let mut report = report;
        report.failed.push(utopia_sync::Failure {
            page: None,
            kind: utopia_sync::FailureKind::Transport("HTTP 413".into()),
        });
        assert_ne!(
            runner.next_interval(&Ok(report)),
            FAST,
            "a failed page is retried on the backoff, not spun on"
        );
    }

    #[test]
    fn status_lines() {
        assert_eq!(
            status_text(Some(&SyncStatus::Synced { at_ms: 0 }), 150_000).0,
            "Synced 2m ago"
        );
        let (s, warn) = status_text(Some(&SyncStatus::Offline { queued: 3 }), 0);
        assert_eq!(s, "● Offline — 3 changes queued");
        assert!(warn);
        // A reachable relay that could not take one page is not "offline".
        let (s, warn) = status_text(
            Some(&SyncStatus::Partial {
                failed: 1,
                queued: 0,
            }),
            0,
        );
        assert_eq!(s, "● Synced — 1 page could not sync");
        assert!(warn);
        let (s, _) = status_text(
            Some(&SyncStatus::Partial {
                failed: 2,
                queued: 5,
            }),
            0,
        );
        assert_eq!(s, "● 2 pages could not sync — 5 changes queued");
        // §12.6: a round that dies in the store is not the relay being offline.
        let local = SyncStatus::LocalError {
            message: "database: no such table: sync_state".into(),
        };
        let (s, warn) = status_text(Some(&local), 0);
        assert_eq!(
            s,
            "● Sync: local error — database: no such table: sync_state"
        );
        assert!(warn);
    }

    /// §12.6: a refusal is not retried at the fast cadence, but it is not the end of sync
    /// either: the transport stays home and one slow retry follows, so a transient refusal
    /// heals while a revoked token just keeps the dot red.
    #[test]
    fn a_refusal_slows_down_and_can_recover() {
        fn store() -> utopia_store::Store {
            utopia_store::Store::open_in_memory().unwrap()
        }
        for (err, status, line) in [
            (
                SyncError::Unauthorized,
                SyncStatus::NeedsAuth,
                "● Sync: sign-in required",
            ),
            (
                SyncError::ProtocolVersion(99),
                SyncStatus::Unsupported,
                "● Sync: relay protocol unsupported",
            ),
        ] {
            let engine = utopia_sync::SyncEngine::new(&store()).expect("engine");
            let transport: BoxTransport = Box::new(utopia_sync::relay::RelayTransport::new(
                utopia_sync::relay::Relay::new(),
                "g",
            ));
            let mut runner = SyncRunner::new(engine, transport);
            let transport = runner.begin_round().expect("a round starts");
            runner.exchange();
            runner.exchange_returned();
            let report: Result<SyncReport, SyncError> = Err(err);
            runner.end_round(
                transport,
                &report,
                SyncStatus::from_result(&report, &store()),
            );
            assert!(matches!(runner.phase, SyncPhase::Refused { .. }));
            assert_eq!(runner.last, Some(status));
            assert_eq!(sync_dot(Some(&runner)), SyncDot::Error);
            assert_eq!(status_line(&runner, 0).0, line);
            assert_eq!(
                runner.interval, SLOW,
                "a refused relay is only polled at the slowest cadence"
            );
            // The refusal is not a dead end: another round can start with the same transport.
            let transport = runner
                .begin_round()
                .expect("a refused runner can retry later");
            runner.exchange();
            runner.exchange_returned();
            let ok: Result<SyncReport, SyncError> = Ok(SyncReport::default());
            runner.end_round(transport, &ok, SyncStatus::Synced { at_ms: 1 });
            assert!(matches!(runner.phase, SyncPhase::Idle { .. }));
            assert_eq!(sync_dot(Some(&runner)), SyncDot::Synced);
        }
    }

    /// SB-4: the dot and the line read the same state. During a round the dot is amber, so
    /// the line must not still claim the time of the last round.
    #[test]
    fn the_status_line_follows_the_round_phase() {
        let engine = utopia_sync::SyncEngine::new(&utopia_store::Store::open_in_memory().unwrap())
            .expect("engine");
        let transport: BoxTransport = Box::new(utopia_sync::relay::RelayTransport::new(
            utopia_sync::relay::Relay::new(),
            "g",
        ));
        let mut runner = SyncRunner::new(engine, transport);
        runner.last = Some(SyncStatus::Synced { at_ms: 0 });
        assert_eq!(status_line(&runner, 150_000).0, "Synced 2m ago");
        let transport = runner.begin_round().expect("a round starts");
        assert_eq!(
            status_line(&runner, 150_000).0,
            "Sync: syncing…",
            "the dot says syncing, so the line cannot say Synced 2m ago"
        );
        runner.exchange();
        assert_eq!(status_line(&runner, 150_000).0, "Sync: syncing…");
        runner.exchange_returned();
        assert_eq!(status_line(&runner, 150_000).0, "Sync: syncing…");
        runner.end_round(
            transport,
            &Ok(SyncReport::default()),
            SyncStatus::Synced { at_ms: 150_000 },
        );
        assert_eq!(status_line(&runner, 150_000).0, "Synced just now");
    }

    /// Only one round at a time, and the transport always comes home: `begin_round` is the
    /// only way to take it, and it refuses while a round runs. This is what made the old
    /// "no transport at prepare" path unreachable (`sync-runner-round.md`, issue 7).
    #[test]
    fn one_round_at_a_time() {
        let engine = utopia_sync::SyncEngine::new(&utopia_store::Store::open_in_memory().unwrap())
            .expect("engine");
        let transport: BoxTransport = Box::new(utopia_sync::relay::RelayTransport::new(
            utopia_sync::relay::Relay::new(),
            "g",
        ));
        let mut runner = SyncRunner::new(engine, transport);
        let transport = runner.begin_round().expect("idle round starts");
        assert!(
            runner.begin_round().is_none(),
            "a second round cannot take the transport"
        );
        runner.exchange();
        assert!(runner.begin_round().is_none(), "the exchange owns it");
        runner.exchange_returned();
        runner.end_round(
            transport,
            &Ok(SyncReport::default()),
            SyncStatus::Synced { at_ms: 0 },
        );
        assert!(runner.begin_round().is_some(), "the transport came home");
    }

    /// The phase setters must only move a round forward. `exchange()` on an idle runner used
    /// to overwrite `Idle { transport }`, dropping the transport and leaving sync dead with
    /// the dot stuck on "syncing".
    #[test]
    fn phase_transitions_only_apply_in_order() {
        let engine = utopia_sync::SyncEngine::new(&utopia_store::Store::open_in_memory().unwrap())
            .expect("engine");
        let transport: BoxTransport = Box::new(utopia_sync::relay::RelayTransport::new(
            utopia_sync::relay::Relay::new(),
            "g",
        ));
        let mut runner = SyncRunner::new(engine, transport);
        runner.exchange();
        assert!(
            matches!(runner.phase, SyncPhase::Idle { .. }),
            "exchange() without a round must not take the transport out of Idle"
        );
        runner.exchange_returned();
        assert!(
            matches!(runner.phase, SyncPhase::Idle { .. }),
            "exchange_returned() without an exchange must not move the phase"
        );
        runner.end_round(
            Box::new(utopia_sync::relay::RelayTransport::new(
                utopia_sync::relay::Relay::new(),
                "g",
            )),
            &Ok(SyncReport::default()),
            SyncStatus::Synced { at_ms: 0 },
        );
        assert!(
            matches!(runner.phase, SyncPhase::Idle { .. }) && runner.last.is_none(),
            "end_round() without an apply must not invent a round"
        );
        // The transport was never dropped, so a round still starts.
        assert!(
            runner.begin_round().is_some(),
            "the transport is still home"
        );
    }
}
