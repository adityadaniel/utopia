//! The sync round, without a UI (§12.3, §12.6, §13.3).
//!
//! This is the state machine the desktop's background runner and the headless daemon
//! (`utopia sync --continuous`) share: which half of a round is running and where the
//! transport is, one round at a time, the poll cadence with its backoff and reset, the
//! refusal that pins that cadence to [`SLOW`], and the nudge a realtime notice or a local
//! write raises. Store work (prepare/apply) and the network exchange are carried by the
//! caller because they run on different threads there; the rules about when they may run
//! live here and nowhere else.

use std::time::Duration;

use crate::{
    realtime::Listener, Incoming, Outgoing, SyncEngine, SyncError, SyncReport, SyncStatus,
    Transport,
};

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

pub type BoxTransport = Box<dyn Transport + Send>;

/// Build the transport stack for a config: relay or folder, optionally encrypted.
pub fn build_transport(cfg: &SyncConfig, client_id: &str) -> Result<BoxTransport, SyncError> {
    let inner: BoxTransport = match &cfg.target {
        SyncTarget::Relay { url, token } => Box::new(crate::http::HttpTransport::new(
            url,
            &cfg.graph,
            token.clone(),
        )?),
        SyncTarget::Folder(dir) => Box::new(crate::folder::FolderTransport::new(dir, client_id)?),
    };
    Ok(match &cfg.passphrase {
        Some(p) => {
            let key = crate::e2e::GraphKey::derive(p, &cfg.graph)?;
            Box::new(crate::e2e::Encrypted::new(inner, key, cfg.graph.clone()))
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
    /// The caller holds the transport and gathers pushes (short store reads, no network).
    Preparing,
    /// The background thread holds the transport and is talking to the relay.
    Exchanging,
    /// The exchange returned; the caller holds the transport and imports the result.
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

/// The round's state and history. One of these is driven by the app's `Workspace` and one
/// by the headless daemon; neither adds a second copy of the rules below.
pub struct SyncRunner {
    pub engine: SyncEngine,
    /// The round's phase; the only place that says where the transport is.
    pub phase: SyncPhase,
    /// The last round's outcome; `None` = no round has finished yet ("connecting…").
    pub last: Option<SyncStatus>,
    /// Cadence between rounds: [`FAST`] while there is activity, backing off to [`SLOW`].
    pub interval: Duration,
    /// "Merged changes" notice (§9.3) with the time it was raised. The app shows it; a
    /// headless run may log it and otherwise leaves it alone.
    pub notice: Option<(String, i64)>,
    /// Raised by the realtime listener, or by a local write, to sync now instead of at the
    /// next poll.
    pub nudge: std::sync::Arc<std::sync::atomic::AtomicBool>,
    /// This runner's identity. A loop may only drive the runner it was started for, so a
    /// superseded loop cannot write into its replacement (nothing else has this number).
    pub generation: u64,
    pub realtime: Option<Listener>,
    /// Lowered when the runner is dropped (replaced or stopped). The background exchange
    /// reads it before touching the transport: a queued exchange from a runner that is gone
    /// must not reach the relay (§13.3).
    alive: std::sync::Arc<std::sync::atomic::AtomicBool>,
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
            realtime: None,
            alive: std::sync::Arc::new(std::sync::atomic::AtomicBool::new(true)),
        }
    }

    /// A flag the background exchange checks before calling the transport; false once
    /// this runner is gone.
    pub fn alive(&self) -> std::sync::Arc<std::sync::atomic::AtomicBool> {
        self.alive.clone()
    }

    /// Whether `generation` is this runner's token; false once sync was restarted.
    pub fn is_generation(&self, generation: u64) -> bool {
        self.generation == generation
    }

    /// Whether the runner is waiting at the slow retry after a refusal. Realtime nudges are
    /// ignored then, so a notice cannot bring the fast polling back (§12.6).
    pub fn is_refused(&self) -> bool {
        matches!(self.phase, SyncPhase::Refused { .. })
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

    /// The exchange returned; the caller imports it next. Only `Exchanging → Applying`.
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

impl Drop for SyncRunner {
    fn drop(&mut self) {
        self.alive.store(false, std::sync::atomic::Ordering::SeqCst);
    }
}

/// Next poll interval after a round: fast again when anything moved, else back off. A round
/// that moved something but left a page failed backs off like a failure: the failed page is
/// retried, not spun on.
pub fn next_interval(current: Duration, result: &Result<SyncReport, SyncError>) -> Duration {
    match result {
        Ok(r) if !r.is_quiet() && r.failed.is_empty() => FAST,
        _ => back_off(current),
    }
}

/// One step of exponential backoff, capped at [`SLOW`].
pub fn back_off(current: Duration) -> Duration {
    (current * 2).min(SLOW)
}

/// The background half of a round.
pub fn exchange_owned(
    mut t: BoxTransport,
    out: Outgoing,
) -> (Result<Incoming, SyncError>, BoxTransport) {
    let r = crate::exchange(t.as_mut(), out);
    (r, t)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn relay_runner() -> SyncRunner {
        let engine =
            SyncEngine::new(&utopia_store::Store::open_in_memory().unwrap()).expect("engine");
        let transport: BoxTransport = Box::new(crate::relay::RelayTransport::new(
            crate::relay::Relay::new(),
            "g",
        ));
        SyncRunner::new(engine, transport)
    }

    #[test]
    fn a_round_that_left_a_page_failed_backs_off() {
        let runner = relay_runner();
        let report = SyncReport {
            pushed: 1,
            ..Default::default()
        };
        assert_eq!(runner.next_interval(&Ok(report.clone())), FAST);
        let mut report = report;
        report.failed.push(crate::Failure {
            page: None,
            kind: crate::FailureKind::Transport("HTTP 413".into()),
        });
        assert_ne!(
            runner.next_interval(&Ok(report)),
            FAST,
            "a failed page is retried on the backoff, not spun on"
        );
    }

    /// §12.6: a refusal is not retried at the fast cadence, but it is not the end of sync
    /// either: the transport stays home and one slow retry follows, so a transient refusal
    /// heals while a revoked token just keeps the last status red.
    #[test]
    fn a_refusal_slows_down_and_can_recover() {
        for (err, status) in [
            (SyncError::Unauthorized, SyncStatus::NeedsAuth),
            (SyncError::ProtocolVersion(99), SyncStatus::Unsupported),
        ] {
            let mut runner = relay_runner();
            let transport = runner.begin_round().expect("a round starts");
            runner.exchange();
            runner.exchange_returned();
            let report: Result<SyncReport, SyncError> = Err(err);
            runner.end_round(
                transport,
                &report,
                SyncStatus::from_result(&report, &utopia_store::Store::open_in_memory().unwrap()),
            );
            assert!(matches!(runner.phase, SyncPhase::Refused { .. }));
            assert_eq!(runner.last, Some(status));
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
            assert_eq!(runner.last, Some(SyncStatus::Synced { at_ms: 1 }));
        }
    }

    /// Only one round at a time, and the transport always comes home: `begin_round` is the
    /// only way to take it, and it refuses while a round runs. This is what made the old
    /// "no transport at prepare" path unreachable (`sync-runner-round.md`, issue 7).
    #[test]
    fn one_round_at_a_time() {
        let mut runner = relay_runner();
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
    /// to overwrite `Idle { transport }`, dropping the transport and leaving sync dead.
    #[test]
    fn phase_transitions_only_apply_in_order() {
        let mut runner = relay_runner();
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
            Box::new(crate::relay::RelayTransport::new(
                crate::relay::Relay::new(),
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
