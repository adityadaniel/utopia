//! Background sync for the desktop app (§12.3, §13.3): the round state machine and its
//! rules live in [`utopia_sync::runner`], shared with the headless daemon. This module adds
//! what only the window has: the GPUI task that drives the loop, the global the sidebar and
//! Settings read, and the dot and status line. Store work (prepare/apply) runs on the UI
//! thread in short steps; network exchange runs on a background thread.

use std::ops::{Deref, DerefMut};

pub use utopia_sync::runner::{
    back_off, build_transport, exchange_owned, next_interval, BoxTransport, RefusalReason,
    SyncConfig, SyncPhase, SyncRunner as Round, SyncTarget, FAST, SLOW,
};

use utopia_sync::SyncStatus;

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

/// The round plus the window's loop task. The task is kept here so replacing the runner
/// drops (and so cancels) the previous loop: it is replaced, never accumulated (`_tasks`
/// would only release it with the workspace).
pub struct SyncRunner {
    round: Round,
    pub loop_task: Option<gpui::Task<()>>,
}

impl SyncRunner {
    pub fn new(engine: utopia_sync::SyncEngine, transport: BoxTransport) -> Self {
        Self {
            round: Round::new(engine, transport),
            loop_task: None,
        }
    }
}

impl Deref for SyncRunner {
    type Target = Round;
    fn deref(&self) -> &Self::Target {
        &self.round
    }
}

impl DerefMut for SyncRunner {
    fn deref_mut(&mut self) -> &mut Self::Target {
        &mut self.round
    }
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
    use utopia_sync::SyncReport;

    fn relay_runner() -> SyncRunner {
        let engine = utopia_sync::SyncEngine::new(&utopia_store::Store::open_in_memory().unwrap())
            .expect("engine");
        let transport: BoxTransport = Box::new(utopia_sync::relay::RelayTransport::new(
            utopia_sync::relay::Relay::new(),
            "g",
        ));
        SyncRunner::new(engine, transport)
    }

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
        let mut runner = relay_runner();
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
                utopia_sync::SyncError::Unauthorized,
                SyncStatus::NeedsAuth,
                "● Sync: sign-in required",
            ),
            (
                utopia_sync::SyncError::ProtocolVersion(99),
                SyncStatus::Unsupported,
                "● Sync: relay protocol unsupported",
            ),
        ] {
            let mut runner = relay_runner();
            let transport = runner.begin_round().expect("a round starts");
            runner.exchange();
            runner.exchange_returned();
            let report: Result<SyncReport, utopia_sync::SyncError> = Err(err);
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
            let ok: Result<SyncReport, utopia_sync::SyncError> = Ok(SyncReport::default());
            runner.end_round(transport, &ok, SyncStatus::Synced { at_ms: 1 });
            assert!(matches!(runner.phase, SyncPhase::Idle { .. }));
            assert_eq!(sync_dot(Some(&runner)), SyncDot::Synced);
        }
    }

    /// SB-4: the dot and the line read the same state. During a round the dot is amber, so
    /// the line must not still claim the time of the last round.
    #[test]
    fn the_status_line_follows_the_round_phase() {
        let mut runner = relay_runner();
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
}
