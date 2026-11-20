//! Settings → Assistants: the access level, the `when_closed` switch and one copy control
//! for each AI client (PRD §13.8 MC-12; `docs/mcp-design.md` §5, §6).
//!
//! The same split as Settings → Sync: this crate cannot reach the app's loopback API or its
//! config directory, so the tab is a view that emits a [`Request`] and the app's coordinator
//! (`crates/utopia-app/src/assistants.rs`) saves the access file and starts, stops or
//! re-levels the API with no restart. The tab only shows the graph that is open (decision
//! D5); each graph has its own setting.
//!
//! The setting is local (rule 9): it lives in `<config>/mcp/<graph key>.json`, never in the
//! CRDT, so no device receives it from sync.

use std::path::PathBuf;

use gpui::{
    div, prelude::*, px, ClipboardItem, Context, Div, Entity, EventEmitter, Global, SharedString,
    Window,
};
use utopia_api::{Access, Level};
use utopia_mcp::SetupClient;

use crate::theme::{theme, Theme};

/// The view the app installs for the open graph. A component test can install its own.
pub struct AssistantsHandle(pub Entity<AssistantsSetup>);
impl Global for AssistantsHandle {}

/// What the tab asks the app to apply. The level and the switch are the whole setting, so a
/// change is one request (MC-12).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Request {
    pub access: Access,
}

/// Whether the loopback API (the bridge an assistant talks to) runs, as the app last
/// reported it.
#[derive(Clone, Debug, PartialEq, Eq, Default)]
pub enum Bridge {
    /// No server: with the app open, a tool call has nowhere to go.
    #[default]
    Off,
    /// A server listens at this URL.
    Running(String),
}

pub struct AssistantsSetup {
    pub access: Access,
    pub bridge: Bridge,
    /// Why the access file could not be read (MC-1: it is Off), until a new setting is saved.
    pub notice: Option<String>,
    /// The graph whose setting this is (D5), also pinned into each client entry (MC-13).
    graph: PathBuf,
    /// The binary a client starts; the entries name it.
    bin: PathBuf,
    copied: Option<SetupClient>,
}

impl AssistantsSetup {
    pub fn new(
        access: Access,
        bridge: Bridge,
        notice: Option<String>,
        graph: PathBuf,
        bin: PathBuf,
        _cx: &mut Context<Self>,
    ) -> Self {
        Self {
            access,
            bridge,
            notice,
            graph,
            bin,
            copied: None,
        }
    }

    /// The app's report after an apply: the setting it wrote, the bridge it now runs, and a
    /// failure to say out loud. A failed save passes the setting still in force, so the
    /// controls go back to it.
    pub fn applied(
        &mut self,
        access: Access,
        bridge: Bridge,
        notice: Option<String>,
        cx: &mut Context<Self>,
    ) {
        self.access = access;
        self.bridge = bridge;
        self.notice = notice;
        cx.notify();
    }

    /// Pick a level. The radio moves at once; the coordinator confirms it in the same effect
    /// cycle, or puts the old one back when the file cannot be written.
    pub fn choose_level(&mut self, level: Level, cx: &mut Context<Self>) {
        if self.access.level == level {
            return;
        }
        self.send(
            Access {
                level,
                ..self.access
            },
            cx,
        );
    }

    /// The switch for working while the app is closed (MC-5, decision D1).
    pub fn toggle_when_closed(&mut self, cx: &mut Context<Self>) {
        self.send(
            Access {
                when_closed: !self.access.when_closed,
                ..self.access
            },
            cx,
        );
    }

    fn send(&mut self, access: Access, cx: &mut Context<Self>) {
        self.access = access;
        self.notice = None;
        cx.emit(Request { access });
        cx.notify();
    }

    /// The text each client's copy control copies, with this binary and this graph (D6 A,
    /// MC-13). The texts are `utopia_mcp::SetupClient`'s, the same ones
    /// `utopia mcp config <client>` prints.
    pub fn entries(&self) -> Vec<(SetupClient, String)> {
        SetupClient::ALL
            .iter()
            .map(|&client| (client, client.entry(&self.bin, Some(&self.graph))))
            .collect()
    }

    /// The client whose entry was copied last, if any.
    pub fn copied(&self) -> Option<SetupClient> {
        self.copied
    }

    fn copy(&mut self, client: SetupClient, cx: &mut Context<Self>) {
        let text = client.entry(&self.bin, Some(&self.graph));
        cx.write_to_clipboard(ClipboardItem::new_string(text));
        self.copied = Some(client);
        cx.notify();
    }

    /// One line of state: the bridge runs or it does not (MC-12). With the level Off and
    /// `UTOPIA_API` set, the API still listens for scripts, and the line says so.
    pub fn bridge_line(&self) -> String {
        match &self.bridge {
            Bridge::Running(url) if self.access.level == Level::Off => {
                format!("The API runs at {url} for scripts. Assistant access is off.")
            }
            Bridge::Running(url) => format!("The bridge runs at {url}."),
            Bridge::Off => "The bridge is off.".to_string(),
        }
    }
}

impl EventEmitter<Request> for AssistantsSetup {}

/// A level button. The selected one is drawn in the link colour with a tinted fill, as the
/// Appearance radios are.
fn level_control(
    id: &'static str,
    label: &'static str,
    on: bool,
    th: &Theme,
) -> gpui::Stateful<Div> {
    let th = th.clone();
    div()
        .id(id)
        .debug_selector(move || id.into())
        .px_3()
        .py_2()
        .rounded(px(crate::theme::RADIUS_CONTROL))
        .border_1()
        .border_color(if on { th.link } else { th.line })
        .when(on, |d| d.bg(th.line_soft).text_color(th.link))
        .when(!on, |d| {
            d.text_color(th.ink)
                .cursor_pointer()
                .hover(|x| x.bg(th.hover))
        })
        .child(label)
}

/// A drawn switch (DESIGN.md §2.4: no image assets; every size is a token). A true control,
/// so the whole row is the target and the knob shows the state.
fn switch(on: bool, th: &Theme) -> Div {
    div()
        .w(px(34.))
        .h(px(20.))
        .flex_none()
        .rounded_full()
        .bg(if on { th.link } else { th.line_strong })
        .p(px(2.))
        .flex()
        .items_center()
        .when(on, |d| d.justify_end())
        .when(!on, |d| d.justify_start())
        .child(div().size(px(16.)).rounded_full().bg(th.paper))
}

impl Render for AssistantsSetup {
    fn render(&mut self, _: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();

        let mut levels = div()
            .debug_selector(|| "settings-assistants-level".into())
            .flex()
            .flex_wrap()
            .gap_2();
        for (level, label, id) in [
            (Level::Off, "Off", "settings-assistants-level-off"),
            (Level::Read, "Read", "settings-assistants-level-read"),
            (
                Level::Write,
                "Read and write",
                "settings-assistants-level-write",
            ),
        ] {
            let on = self.access.level == level;
            levels = levels.child(
                level_control(id, label, on, &th)
                    .on_click(cx.listener(move |this, _, _, cx| this.choose_level(level, cx))),
            );
        }

        let closed = div()
            .id("assistants-when-closed")
            .debug_selector(|| "settings-assistants-when-closed".into())
            .flex()
            .items_center()
            .justify_between()
            .gap_4()
            .cursor_pointer()
            .on_click(cx.listener(|this, _, _, cx| this.toggle_when_closed(cx)))
            .child(
                div()
                    .flex()
                    .flex_col()
                    .gap_1()
                    .child("Run while Utopia is closed")
                    .child(
                        div()
                            .text_size(px(crate::theme::CAPTION_SIZE))
                            .text_color(th.muted)
                            .child(
                                "A call opens the graph without the window. It does not sync; \
                                 the next launch does.",
                            ),
                    ),
            )
            .child(switch(self.access.when_closed, &th));

        // The cards are the Settings pattern (ST-1): a caption, then flush rows with a
        // hairline between them, in one centred readable column.
        let mut body = div()
            .debug_selector(|| "settings-assistants".into())
            .w_full()
            .max_w(px(720.))
            .mx_auto()
            .flex_shrink_0()
            .flex()
            .flex_col()
            .child(crate::settings::card_top(&th, "Assistant access"))
            .child(crate::settings::appearance_row(levels, false, &th))
            // MC-12: the consequence of turning access on, beside the control that does it.
            .child(crate::settings::appearance_row(
                div()
                    .debug_selector(|| "settings-assistants-privacy".into())
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.muted)
                    .child("Text an assistant reads goes to that assistant and to its provider."),
                false,
                &th,
            ))
            // MC-12: one line of state, beside the setting it describes, not buried under
            // three client cards.
            .child(crate::settings::appearance_row(
                div()
                    .debug_selector(|| "settings-assistants-bridge".into())
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.muted)
                    .child(self.bridge_line()),
                true,
                &th,
            ));
        // A file that was not valid is Off (MC-1); say why next to the control, so the
        // owner does not think a level was saved and quietly ignored.
        if let Some(notice) = &self.notice {
            body = body.child(
                div()
                    .debug_selector(|| "settings-assistants-notice".into())
                    .mt_2()
                    .text_size(px(crate::theme::CAPTION_SIZE))
                    .text_color(th.link)
                    .child(notice.clone()),
            );
        }
        body = body
            .child(crate::settings::card_top(&th, "While the app is closed").mt_4())
            .child(crate::settings::appearance_row(closed, true, &th))
            .child(crate::settings::card_top(&th, "Connect a client").mt_4());

        let entries = self.entries();
        let last = entries.len();
        for (i, (client, entry)) in entries.into_iter().enumerate() {
            let copied = self.copied == Some(client);
            let name = client.name();
            let copy_id: &'static str = match client {
                SetupClient::ClaudeCode => "settings-assistants-copy-claude-code",
                SetupClient::ClaudeDesktop => "settings-assistants-copy-claude-desktop",
                SetupClient::Codex => "settings-assistants-copy-codex",
            };
            let entry_id = match client {
                SetupClient::ClaudeCode => "settings-assistants-entry-claude-code",
                SetupClient::ClaudeDesktop => "settings-assistants-entry-claude-desktop",
                SetupClient::Codex => "settings-assistants-entry-codex",
            };
            // A block container, not `flex().flex_col()`: gpui 0.2 measures the last child of
            // a wrapped flex column with an unconstrained width, so the row kept the height
            // of a one-line note and painted a two-line note over the next card. Block
            // layout gives the note its final width, and the row grows by its wrapped lines.
            let row = div()
                .debug_selector(move || format!("settings-assistants-client-{name}"))
                .child(
                    div()
                        .flex()
                        .items_center()
                        .justify_between()
                        .gap_4()
                        .child(div().child(client.label()))
                        .child(
                            crate::theme::quiet_control(
                                SharedString::from(copy_id),
                                if copied { "Copied" } else { "Copy" },
                                &th,
                            )
                            .debug_selector(move || copy_id.into())
                            .on_click(cx.listener(move |this, _, _, cx| this.copy(client, cx))),
                        ),
                )
                .child(
                    div()
                        .debug_selector(move || entry_id.into())
                        .mt_2()
                        .w_full()
                        .font_family(crate::theme::code_mono())
                        .text_size(px(crate::theme::CODE_TEXT_SIZE))
                        .text_color(th.ink_2)
                        .child(entry),
                )
                .child(
                    div()
                        .debug_selector(move || format!("settings-assistants-note-{name}"))
                        .mt_2()
                        .w_full()
                        .text_size(px(crate::theme::CAPTION_SIZE))
                        .text_color(th.muted)
                        .child(client.note().to_string()),
                );
            body = body.child(crate::settings::appearance_row(row, i + 1 == last, &th));
        }

        body
    }
}
