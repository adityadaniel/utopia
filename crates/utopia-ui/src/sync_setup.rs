//! Settings → Sync: local setup controls and requests handled by the app (§12, §15.6).
use crate::{line_input::LineInput, theme::theme};
use gpui::{div, prelude::*, px, Context, Entity, EventEmitter, Global, SharedString, Window};
use utopia_sync::setup::{Mode, Preferences};

pub struct SetupHandle(pub Entity<SyncSetup>);
impl Global for SetupHandle {}

#[derive(Clone)]
pub struct Request {
    /// The graph id this apply publishes under (§12.7): editable until the graph first
    /// syncs, frozen afterwards.
    pub graph: String,
    pub preferences: Preferences,
    pub token: String,
    pub passphrase: String,
    pub test_only: bool,
}

pub struct SyncSetup {
    pub graph: String,
    /// The id can still be chosen: the graph has never synced, so no relay path or E2E key
    /// depends on it yet. A copy of a graph used as a second notebook picks its own id here
    /// instead of publishing into the first one's relay graph (§12.6, §12.7).
    pub graph_editable: bool,
    pub graph_input: Entity<LineInput>,
    pub preferences: Preferences,
    pub overrides: Vec<&'static str>,
    pub notice: Option<String>,
    pub status: String,
    pub busy: bool,
    pub pairing: bool,
    pub active_url: Option<String>,
    pub url: Entity<LineInput>,
    pub folder: Entity<LineInput>,
    pub bind: Entity<LineInput>,
    pub token: Entity<LineInput>,
    pub passphrase: Entity<LineInput>,
}
impl EventEmitter<Request> for SyncSetup {}
impl SyncSetup {
    pub fn new(graph: String, graph_editable: bool, cx: &mut Context<Self>) -> Self {
        let graph_input = cx.new(|cx| {
            let mut input = LineInput::new("Graph id", cx);
            input.fill(&graph, cx);
            input
        });
        let url = cx.new(|cx| LineInput::new("https://relay.example.com", cx));
        let folder = cx.new(|cx| LineInput::new("Choose a shared folder", cx));
        let bind = cx.new(|cx| {
            let mut i = LineInput::new("0.0.0.0:8787", cx);
            i.fill("0.0.0.0:8787", cx);
            i
        });
        let token = cx.new(|cx| {
            let mut i = LineInput::new("Enter token · blank keeps saved token", cx);
            i.set_masked(true, cx);
            i
        });
        let passphrase = cx.new(|cx| {
            let mut i = LineInput::new("Enter passphrase · blank keeps saved key", cx);
            i.set_masked(true, cx);
            i
        });
        Self {
            graph,
            graph_editable,
            graph_input,
            preferences: Preferences::default(),
            overrides: vec![],
            notice: None,
            status: "Loading sync settings…".into(),
            busy: true,
            pairing: false,
            active_url: None,
            url,
            folder,
            bind,
            token,
            passphrase,
        }
    }
    pub fn loaded(
        &mut self,
        prefs: Preferences,
        overrides: Vec<&'static str>,
        notice: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.url.update(cx, |i, cx| i.fill(&prefs.url, cx));
        self.folder.update(cx, |i, cx| {
            i.fill(
                &prefs
                    .folder
                    .as_ref()
                    .map(|p| p.display().to_string())
                    .unwrap_or_default(),
                cx,
            )
        });
        if let Some(bind) = &prefs.host {
            self.bind.update(cx, |i, cx| i.fill(bind, cx));
        }
        self.preferences = prefs;
        self.overrides = overrides;
        self.notice = notice;
        cx.notify();
    }
    /// The id is in use from now on: the relay path and the E2E key both derive from it, so
    /// a later change would publish this graph into another one (§12.6).
    pub fn freeze_graph(&mut self, graph: String, cx: &mut Context<Self>) {
        self.graph_input
            .update(cx, |input, cx| input.fill(&graph, cx));
        self.graph = graph;
        self.graph_editable = false;
        cx.notify();
    }
    pub fn choose(&mut self, mode: Mode, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        self.preferences.mode = mode;
        cx.notify();
    }
    pub fn request(&mut self, test_only: bool, cx: &mut Context<Self>) {
        if self.busy || (!test_only && !self.overrides.is_empty()) {
            return;
        }
        let graph = if self.graph_editable {
            self.graph_input.read(cx).text().trim().to_string()
        } else {
            self.graph.clone()
        };
        if !utopia_sync::http::valid_graph_id(&graph) {
            self.status = "Enter a graph id: letters, digits, hyphens and underscores only.".into();
            cx.notify();
            return;
        }
        let mut preferences = self.preferences.clone();
        preferences.url = self.url.read(cx).text().trim().to_string();
        let path = self.folder.read(cx).text().trim();
        preferences.folder = (!path.is_empty()).then(|| path.into());
        if preferences.host.is_some() {
            preferences.host = Some(self.bind.read(cx).text().trim().into());
        }
        match preferences.mode {
            Mode::Off => preferences = Preferences::default(),
            Mode::Relay => {
                preferences.folder = None;
                preferences.host = None;
            }
            Mode::Nearby => {
                preferences.url.clear();
                preferences.folder = None;
            }
            Mode::Folder => {
                preferences.url.clear();
                preferences.host = None;
            }
        }
        if let Err(error) = preferences.validate() {
            self.status = error;
            cx.notify();
            return;
        }
        let request = Request {
            graph,
            preferences,
            token: self.token.read(cx).text().to_string(),
            passphrase: self.passphrase.read(cx).text().to_string(),
            test_only,
        };
        self.busy = true;
        self.status = if test_only {
            "Testing connection…"
        } else {
            "Applying sync settings…"
        }
        .into();
        cx.emit(request);
        cx.notify();
    }
    pub fn finish(&mut self, status: String, cx: &mut Context<Self>) {
        self.busy = false;
        self.status = status;
        self.token.update(cx, |i, cx| i.set_text("", cx));
        self.passphrase.update(cx, |i, cx| i.set_text("", cx));
        cx.notify();
    }
    fn pick_folder(&mut self, cx: &mut Context<Self>) {
        if self.busy {
            return;
        }
        let picked = cx.prompt_for_paths(gpui::PathPromptOptions {
            files: false,
            directories: true,
            multiple: false,
            prompt: Some("Use for sync".into()),
        });
        cx.spawn(async move |this, cx| {
            if let Ok(Ok(Some(paths))) = picked.await {
                if let Some(path) = paths.first() {
                    let _ = this.update(cx, |this, cx| {
                        this.folder
                            .update(cx, |i, cx| i.set_text(&path.display().to_string(), cx))
                    });
                }
            }
        })
        .detach();
    }
}
fn control(
    id: &'static str,
    label: impl Into<SharedString>,
    cx: &gpui::App,
) -> gpui::Stateful<gpui::Div> {
    let th = theme(cx);
    div()
        .id(id)
        .debug_selector(move || id.into())
        .px_3()
        .py_2()
        .rounded(px(crate::theme::RADIUS_CONTROL))
        // Always a border, so selecting a mode colours it instead of moving its neighbours.
        .border_1()
        .border_color(gpui::transparent_black())
        .bg(th.line_soft)
        .text_color(th.link)
        .cursor_pointer()
        .child(label.into())
}
fn field(
    label: &'static str,
    id: &'static str,
    input: Entity<LineInput>,
    cx: &gpui::App,
) -> gpui::Div {
    let th = theme(cx);
    div()
        .flex()
        .flex_col()
        .gap_1()
        .child(div().text_color(th.muted).child(label))
        .child(
            div()
                .debug_selector(move || id.into())
                .px_3()
                .py_2()
                .border_1()
                .border_color(th.line)
                .rounded(px(crate::theme::RADIUS_CONTROL))
                .child(input),
        )
}
impl Render for SyncSetup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let mode = self.preferences.mode;
        let facts = crate::sync_runner::facts(cx);
        let mut body = div()
            .debug_selector(|| "settings-sync".into())
            .w_full()
            .max_w(px(720.))
            .mx_auto()
            // The pane scrolls; the pairing panel must push the pane taller, not be squeezed.
            .flex_shrink_0()
            .flex()
            .flex_col()
            .gap_3()
            .child(div().text_size(px(18.)).child("Sync this graph"))
            .child(if self.graph_editable {
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child(field("Graph id", "sync-graph", self.graph_input.clone(), cx))
                    .child(
                        div()
                            .text_size(px(12.))
                            .text_color(th.muted)
                            .child("Names this graph on the relay or folder. Choose a different id for a copy of a graph; it cannot be changed once the graph has synced."),
                    )
            } else {
                div()
                    .text_color(th.muted)
                    .child(format!("Graph id: {}", self.graph))
            })
            .child(
                div()
                    .debug_selector(|| "settings-sync-transport".into())
                    .child(
                        facts
                            .target
                            .clone()
                            .unwrap_or_else(|| "Off · your notes stay on this Mac".into()),
                    ),
            )
            .when(facts.is_on(), |d| {
                d.child(
                    div()
                        .debug_selector(|| "settings-sync-encryption".into())
                        .text_color(th.muted)
                        .child(if facts.encrypted {
                            "End-to-end encrypted: the relay or folder only holds ciphertext"
                        } else {
                            "Not encrypted: the relay or folder holds page payloads in the clear"
                        }),
                )
            });
        if let Some(notice) = &self.notice {
            body = body.child(div().text_color(th.link).child(notice.clone()));
        }
        if !self.overrides.is_empty() {
            body = body.child(div().text_color(th.link).child(format!("Environment override for this run: {}. Relaunch without these variables to apply saved settings.", self.overrides.join(", "))));
        }
        let mut modes = div().flex().flex_wrap().gap_2();
        for (value, label, id) in [
            (Mode::Off, "Off", "sync-off"),
            (Mode::Relay, "Relay", "sync-relay"),
            (Mode::Nearby, "Nearby Mac", "sync-nearby"),
            (Mode::Folder, "Shared folder", "sync-folder"),
        ] {
            modes = modes.child(
                control(id, label, cx)
                    .when(mode == value, |d| d.border_color(th.link))
                    .on_click(cx.listener(move |this, _, _, cx| this.choose(value, cx))),
            );
        }
        body = body.child(modes);
        match mode {
            Mode::Off => {
                body = body.child(
                    div()
                        .text_color(th.muted)
                        .child("Apply to stop sync. Local edits and queued changes are kept."),
                );
            }
            Mode::Relay => {
                body = body.child(field("Relay URL", "sync-url", self.url.clone(), cx));
            }
            Mode::Nearby => {
                let hosting = self.preferences.host.is_some();
                body = body.child(div().text_color(th.muted).child("Discover a Mac hosting this graph over mDNS. For Tailscale, choose Relay on the other Mac and enter the host's Tailscale address."))
                    .child(control("sync-host", if hosting { "Host this graph from this Mac · on" } else { "Host this graph from this Mac · off" }, cx).on_click(cx.listener(|this, _, _, cx| {
                        if !this.busy { this.preferences.host = if this.preferences.host.is_some() { None } else { Some(this.bind.read(cx).text().into()) }; cx.notify(); }
                    })));
                if hosting {
                    body = body.child(field("Listen address", "sync-bind", self.bind.clone(), cx)).child(div().text_color(th.muted).child("Hosting starts when you Apply and needs a relay token; enter the same token on the other device."));
                }
            }
            Mode::Folder => {
                body = body
                    .child(field(
                        "Shared folder",
                        "sync-folder-path",
                        self.folder.clone(),
                        cx,
                    ))
                    .child(
                        control("sync-pick-folder", "Choose folder…", cx)
                            .on_click(cx.listener(|this, _, _, cx| this.pick_folder(cx))),
                    )
                    .child(div().text_color(th.muted).child(
                        "Use a dedicated folder for this graph, shared by your file sync service.",
                    ));
            }
        }
        if mode != Mode::Off {
            if mode != Mode::Folder {
                body = body.child(field("Relay token", "sync-token", self.token.clone(), cx));
            }
            body = body.child(field("End-to-end passphrase", "sync-passphrase", self.passphrase.clone(), cx))
                .child(div().text_size(px(12.)).text_color(th.muted).child("Secrets are kept only in macOS Keychain. Blank fields keep saved credentials. A saved passphrase cannot be changed; leave it blank for unencrypted sync only if none has been saved."));
        }
        body = body
            .child(
                div()
                    .flex()
                    .flex_wrap()
                    .gap_2()
                    .child(
                        control(
                            "sync-apply",
                            if self.busy { "Working…" } else { "Apply" },
                            cx,
                        )
                        // An environment override owns this run: Apply is inert, and looks it.
                        .when(!self.overrides.is_empty(), |d| d.opacity(0.5))
                        .on_click(cx.listener(|this, _, _, cx| this.request(false, cx))),
                    )
                    .when(mode != Mode::Off, |d| {
                        d.child(
                            control("sync-test", "Test connection", cx)
                                .on_click(cx.listener(|this, _, _, cx| this.request(true, cx))),
                        )
                    })
                    .child(control("sync-pair", "Connect another device", cx).on_click(
                        cx.listener(|this, _, _, cx| {
                            this.pairing = !this.pairing;
                            cx.notify();
                        }),
                    )),
            )
            .child(
                div()
                    .debug_selector(|| "sync-result".into())
                    .text_color(th.muted)
                    .child(self.status.clone()),
            );
        if self.pairing {
            let endpoint = match mode {
                Mode::Relay => {
                    let prefs = Preferences { mode: Mode::Relay, url: self.url.read(cx).text().into(), ..Default::default() };
                    if prefs.validate().is_ok() { format!("Relay URL: {}", prefs.url) } else { "Enter a valid relay URL above.".into() }
                },
                Mode::Nearby if self.preferences.host.is_some() => format!("Choose Nearby Mac on the other device. Or choose Relay and enter http://<this Mac's LAN or Tailscale IP>:{}.", self.bind.read(cx).text().rsplit(':').next().unwrap_or("8787")),
                Mode::Nearby => "Choose Nearby Mac on both devices; enable hosting on one Mac.".into(),
                Mode::Folder => "Choose the same dedicated shared folder on the other device.".into(),
                Mode::Off => "Choose and apply a sync transport first.".into(),
            };
            body = body.child(div().debug_selector(|| "sync-pairing".into()).p_4().rounded(px(crate::theme::RADIUS_INPUT)).border_1().border_color(th.line).flex().flex_col().gap_2()
                .child("Connect another device")
                .child(format!("Create or open {}.utopia on the other device. Its Sync graph id must be {}.", self.graph, self.graph))
                .child(endpoint)
                .child("Enter the relay token and the same E2E passphrase yourself on the other device. They are never displayed here."));
        }
        body
    }
}
