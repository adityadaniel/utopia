//! Background sync for the desktop app (§12.3, §13.3): the round state machine and its
//! rules live in [`utopia_sync::runner`], shared with the headless daemon. This module adds
//! what only the window has: the GPUI task that drives the loop, the global the sidebar and
//! Settings read, the dot and status line, and the "Live updates" policy that decides how
//! long the realtime socket stays open. Store work (prepare/apply) runs on the UI thread in
//! short steps; network exchange runs on a background thread.

use std::ops::{Deref, DerefMut};
use std::time::Duration;

pub use utopia_sync::runner::{
    back_off, build_transport, exchange_owned, next_interval, BoxTransport, RefusalReason,
    SyncConfig, SyncPhase, SyncRunner as Round, SyncTarget, FAST, SLOW,
};

pub use crate::appearance::RealtimeMode;

use utopia_sync::realtime::Listener;
use utopia_sync::SyncStatus;

/// How long a `while_active` socket stays open after the last edit or focus change (§12.3):
/// two quiet minutes, then the WebSocket closes and polling runs alone until the user is
/// back. Nothing is lost — the next poll still pulls, it just is not cut short early.
pub const LIVE_IDLE: Duration = Duration::from_secs(120);

/// Where the realtime socket connects, kept while sync runs so the policy can reopen it.
/// The token is the one sync already holds; it never reaches `settings.json` or a
/// screenshot (§15.6).
#[derive(Clone, Debug)]
pub struct LiveTarget {
    pub url: String,
    pub graph: String,
    pub token: Option<String>,
    /// Shared with the round: a notice makes the next round start at once.
    pub nudge: std::sync::Arc<std::sync::atomic::AtomicBool>,
}

/// An owned realtime socket (§12.3). The app holds a [`Listener`]; a test holds a stand-in,
/// so the activity policy can be driven without a relay.
pub trait LiveSocket: Send + 'static {
    /// Cancel and join it. The caller runs this off the UI thread: joining must not hold a
    /// frame.
    fn stop(self: Box<Self>);
}

impl LiveSocket for Listener {
    fn stop(self: Box<Self>) {
        Listener::stop(*self)
    }
}

/// Opens a socket for a target; `None` when its thread cannot start (rule 11).
pub type LiveOpener = Box<dyn Fn(&LiveTarget) -> Option<Box<dyn LiveSocket>>>;

/// The one real opener: the listener the app has always used.
pub fn open_listener(target: &LiveTarget) -> Option<Box<dyn LiveSocket>> {
    match Listener::start(
        target.url.clone(),
        target.graph.clone(),
        target.token.clone(),
        target.nudge.clone(),
    ) {
        Ok(listener) => Some(Box::new(listener)),
        Err(e) => {
            eprintln!("utopia: realtime listener not started: {e}");
            None
        }
    }
}

/// The workspace that owns the app's one realtime socket, so any window can report
/// activation to it (§12.3).
///
/// The socket lives with the main window, but the user may be in another window of the same
/// app — Settings, most importantly. gpui reports activation per window and has no app-level
/// event, so every window forwards its own change here and the workspace applies it to the
/// one policy. Without that, `while_active` would close the socket after the idle timeout and
/// never reopen it while Settings had the keyboard, and would never close it when the whole
/// app went to the background with Settings in front.
#[derive(Clone)]
pub struct LivePresence(pub gpui::WeakEntity<crate::workspace::Workspace>);

impl gpui::Global for LivePresence {}

/// Schedule a presence report to the window that owns the realtime socket (§12.3).
///
/// A task, not `cx.defer`: a deferred callback runs inside the outermost update, and
/// updating an entity that update holds licensed panics ("cannot update … while it is
/// already being updated"). A task leaves the update first, and by the time it polls every
/// window's activation event of this round has been delivered, so `App::active_window`
/// answers for the whole app rather than for whichever window reported first. That is what
/// makes opening Settings — which deactivates the main window while the app stays in front —
/// read as the user arriving, not leaving.
pub fn report_presence(cx: &mut gpui::App) {
    let app = cx.to_async();
    cx.foreground_executor()
        .spawn(async move {
            app.update(presence_changed).ok();
        })
        .detach();
}

/// Tell the workspace whether any window of this app has the keyboard (§12.3). A report
/// only counts with a workspace to receive it; tests without one are silently fine.
fn presence_changed(cx: &mut gpui::App) {
    let present = cx.active_window().is_some();
    let Some(owner) = cx.try_global::<LivePresence>().cloned() else {
        return;
    };
    owner
        .0
        .update(cx, |workspace, cx| workspace.set_present(present, cx))
        .ok();
}

/// The "Live updates" policy for the realtime socket (§12.3, Settings → Sync).
///
/// `Always` keeps the socket open for as long as sync runs: the behaviour every release had
/// before the setting. `WhileActive` opens it when a window gets focus or the user edits,
/// and closes it after [`LIVE_IDLE`] with no activity or when the whole app goes to the
/// background. Polling is the same in both (5 s / 60 s), so a closed socket changes only
/// how soon a remote change is noticed.
///
/// One socket at most: the handle is an `Option`, a transition replaces it, and the socket
/// that is closed goes back to the caller to be joined off the UI thread.
pub struct LiveUpdates {
    mode: RealtimeMode,
    target: Option<LiveTarget>,
    socket: Option<Box<dyn LiveSocket>>,
    /// A window of the app has the keyboard. Only a mode switch reads it: switching to
    /// `while_active` while the app is in the front keeps the socket it already has.
    focused: bool,
    /// `while_active` wants a socket now: a focus change or an edit opened it and no idle
    /// timeout has closed it since.
    wanted: bool,
    opener: LiveOpener,
}

impl LiveUpdates {
    pub fn new(mode: RealtimeMode) -> Self {
        Self {
            mode,
            target: None,
            socket: None,
            focused: false,
            wanted: false,
            opener: Box::new(open_listener),
        }
    }

    pub fn mode(&self) -> RealtimeMode {
        self.mode
    }

    /// Whether a socket is open (there is never more than one).
    pub fn is_open(&self) -> bool {
        self.socket.is_some()
    }

    /// Replace the opener: the app never does, a test installs a stand-in here.
    pub fn set_opener(&mut self, opener: LiveOpener) {
        self.opener = opener;
    }

    /// Switch policy without dropping the socket: the change applies at the next reconcile.
    pub fn set_mode(&mut self, mode: RealtimeMode) {
        self.mode = mode;
        if mode == RealtimeMode::WhileActive {
            // Coming from `Always` with the app in the background: close like any other
            // `while_active` socket that nothing is happening in.
            self.wanted = self.focused;
        }
    }

    /// Point the socket at a relay, or drop it (`None`: sync off, a folder target).
    pub fn set_target(&mut self, target: Option<LiveTarget>) {
        self.target = target;
    }

    /// A window of the app got the keyboard: open the socket, and remember that the app is
    /// in the front (a switch to `while_active` reads it).
    pub fn focused(&mut self) {
        self.focused = true;
        self.wanted = true;
    }

    /// The user edited: a sign of life that reopens the socket and restarts the quiet
    /// minutes. Unlike a focus change it does not say the app is in the front — only the
    /// platform does — so the caller (`Workspace::live_activity`) decides whether an edit
    /// counts.
    pub fn activity(&mut self) {
        self.wanted = true;
    }

    /// Every window of the app lost focus.
    pub fn deactivate(&mut self) {
        self.focused = false;
        self.wanted = false;
    }

    /// [`LIVE_IDLE`] passed with no edit and no focus change.
    pub fn idle(&mut self) {
        if self.mode == RealtimeMode::WhileActive {
            self.wanted = false;
        }
    }

    /// Whether the caller should be running the idle countdown.
    pub fn counting_down(&self) -> bool {
        self.mode == RealtimeMode::WhileActive && self.socket.is_some()
    }

    /// Bring the socket in line with the decision. A socket that has to close is pushed
    /// onto `closed` rather than joined here: see [`LiveSocket::stop`].
    pub fn reconcile(&mut self, closed: &mut Vec<Box<dyn LiveSocket>>) {
        let want = self.target.is_some()
            && match self.mode {
                RealtimeMode::Always => true,
                RealtimeMode::WhileActive => self.wanted,
            };
        if want {
            if self.socket.is_none() {
                if let Some(target) = self.target.as_ref() {
                    self.socket = (self.opener)(target);
                }
            }
        } else if let Some(socket) = self.socket.take() {
            closed.push(socket);
        }
    }
}

impl Drop for LiveUpdates {
    /// Teardown: signal without joining (the signal ends the thread, and Drop must not
    /// block a frame).
    fn drop(&mut self) {
        self.socket = None;
    }
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

    /// §12.3: `always` opens one socket with the target and closes it only when the target
    /// goes; a `while_active` socket follows activation, edits and the idle timeout, and it
    /// is never more than one.
    #[test]
    fn live_updates_follow_the_mode_and_keep_one_socket() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        use std::sync::Arc;

        struct Counted(Arc<AtomicUsize>);
        impl LiveSocket for Counted {
            fn stop(self: Box<Self>) {
                self.0.fetch_add(1, Ordering::SeqCst);
            }
        }

        let opened = Arc::new(AtomicUsize::new(0));
        let stopped = Arc::new(AtomicUsize::new(0));
        let (o, s) = (opened.clone(), stopped.clone());
        let mut live = LiveUpdates::new(RealtimeMode::Always);
        live.set_opener(Box::new(move |_| {
            o.fetch_add(1, Ordering::SeqCst);
            Some(Box::new(Counted(s.clone())))
        }));
        let target = || LiveTarget {
            url: "ws://127.0.0.1:1".into(),
            graph: "g".into(),
            token: Some("t".into()),
            nudge: Arc::default(),
        };
        let mut closed = Vec::new();
        // The caller joins what closes off the UI thread; here it is the same thread, and
        // the fake counts the stops.
        fn join(closed: &mut Vec<Box<dyn LiveSocket>>) -> usize {
            for socket in closed.drain(..) {
                socket.stop();
            }
            0
        }

        // `always` opens as soon as there is a target, with no activation, and focus
        // changes never close it.
        live.set_target(Some(target()));
        live.reconcile(&mut closed);
        assert!(live.is_open());
        live.deactivate();
        live.idle();
        live.reconcile(&mut closed);
        assert!(
            live.is_open(),
            "always keeps the socket through any of that"
        );
        assert_eq!((opened.load(Ordering::SeqCst), closed.len()), (1, 0));

        // `while_active` with the app in the background closes it; an edit reopens it, and
        // the timeout closes it again.
        live.set_mode(RealtimeMode::WhileActive);
        live.reconcile(&mut closed);
        assert!(!live.is_open());
        assert_eq!(closed.len(), 1, "the closed socket comes back to be joined");
        join(&mut closed);
        live.activity();
        live.reconcile(&mut closed);
        assert!(live.is_open(), "an edit opens the socket");
        assert!(live.counting_down(), "and starts the quiet minutes");
        live.idle();
        live.reconcile(&mut closed);
        assert!(!live.is_open(), "two quiet minutes close it");
        join(&mut closed);
        assert_eq!(
            opened.load(Ordering::SeqCst),
            2,
            "one open per file, never two"
        );
        assert_eq!(stopped.load(Ordering::SeqCst), 2);

        // A target that goes away closes whatever is open.
        live.activity();
        live.reconcile(&mut closed);
        live.set_target(None);
        live.reconcile(&mut closed);
        assert!(!live.is_open());
        join(&mut closed);
        assert_eq!(stopped.load(Ordering::SeqCst), 3);
    }

    /// An opener that cannot start a thread leaves no socket and no panic (rule 11).
    #[test]
    fn a_failed_open_is_no_socket() {
        let mut live = LiveUpdates::new(RealtimeMode::Always);
        live.set_opener(Box::new(|_| None));
        live.set_target(Some(LiveTarget {
            url: "ws://127.0.0.1:1".into(),
            graph: "g".into(),
            token: None,
            nudge: Default::default(),
        }));
        live.reconcile(&mut Vec::new());
        assert!(!live.is_open());
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
