//! Utopia desktop app (§13.1 `utopia-app`): window, menus, keymap, onboarding, and the
//! headless `doctor` / `export` commands.

mod cli;
mod hotkey;
mod paths;
mod shot;
mod sync_setup;

use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{Duration, Instant};

use gpui::{
    div, prelude::*, px, size, App, Application, Bounds, Context, Entity, EntityInputHandler,
    Global, Menu, MenuItem, Subscription, TitlebarOptions, Window, WindowBounds, WindowHandle,
    WindowOptions,
};
use utopia_store::Store;
use utopia_ui::actions::*;
use utopia_ui::app_state::AppState;
use utopia_ui::line_input::{LineInput, LineInputEvent};
use utopia_ui::settings::{SettingsView, Tab as SettingsTab};
use utopia_ui::theme::{theme, Theme};
use utopia_ui::workspace::Workspace;

/// `utopia://` links delivered by the OS (or argv), drained on the main thread.
static PENDING_URLS: Mutex<Vec<String>> = Mutex::new(Vec::new());

/// Act on one `utopia://` link.
fn follow_link(url: &str, ws: &Entity<Workspace>, window: &mut Window, cx: &mut App) {
    use utopia_api::UrlAction;
    let Some(action) = utopia_api::parse_url(url) else {
        eprintln!("utopia: unsupported link {url}");
        return;
    };
    ws.update(cx, |w, cx| match action {
        UrlAction::Page(t) => w.open_title(&t, window, cx),
        UrlAction::Journal(d) => {
            let d = d.unwrap_or_else(|| w.app().read(cx).store.today());
            w.open_date(d, false, true, window, cx)
        }
        UrlAction::Block(uid) => w.open_block(&uid, window, cx),
        UrlAction::Search(q) => w.search_for(&q, window, cx),
        UrlAction::Capture(text) => w.capture_text(&text, cx),
    });
}

/// Workspace commands for `UTOPIA_SHOT_SCRIPT`.
struct ScriptHooks(gpui::WeakEntity<Root>);

impl ScriptHooks {
    fn workspace(&self, cx: &App) -> Option<Entity<Workspace>> {
        match &self.0.upgrade()?.read(cx).state {
            RootState::Ready(ready) => Some(ready.ws.clone()),
            _ => None,
        }
    }
}

impl shot::Hooks for ScriptHooks {
    fn drop_files(&self, paths: &[PathBuf], window: &mut Window, cx: &mut App) {
        if let Some(ws) = self.workspace(cx) {
            ws.update(cx, |w, cx| w.drop_files(paths, window, cx));
        }
    }

    fn ws(&self, args: &[String], window: &mut Window, cx: &mut App) {
        let Some(ws) = self.workspace(cx) else {
            return;
        };
        let this = ScriptWs(ws);
        this.run(args, window, cx);
    }
}

struct ScriptWs(Entity<Workspace>);

impl ScriptWs {
    fn run(&self, args: &[String], window: &mut Window, cx: &mut App) {
        let num = |i: usize| args.get(i).and_then(|v| v.parse::<f32>().ok());
        match args.first().map(String::as_str) {
            // The first PDF attached on the open page.
            Some("open-pdf") => self.0.update(cx, |w, cx| {
                let found = w.main_page(cx).and_then(|p| {
                    let store = &mut w.app().clone();
                    store.update(cx, |a, _| {
                        let st = a.store.page_state(&p).ok()?.clone();
                        st.live_preorder().into_iter().find_map(|(id, _)| {
                            utopia_core::attach::blob_refs(&st.block(&id)?.text)
                                .into_iter()
                                .find(|b| b.kind == utopia_core::attach::BlobKind::Pdf)
                                .map(|b| (b, id))
                        })
                    })
                });
                if let Some((b, host)) = found {
                    let name = w
                        .app()
                        .read(cx)
                        .store
                        .attachment(&b.hash)
                        .ok()
                        .flatten()
                        .map_or(b.name, |a| a.name);
                    w.open_pdf(b.hash, name, None, Some(host), window, cx);
                }
            }),
            Some("highlight") => {
                if let (Some(p), Some(x), Some(y), Some(wd), Some(h)) =
                    (num(1), num(2), num(3), num(4), num(5))
                {
                    if let Some(view) = self.0.read(cx).pdf_view() {
                        view.update(cx, |v, cx| v.highlight_area(p as usize, [x, y, wd, h], cx));
                    }
                }
            }
            // `ws select <page> <x0> <y0> <x1> <y1>`: a text-selection highlight.
            Some("select") => {
                if let (Some(p), Some(x0), Some(y0), Some(x1), Some(y1)) =
                    (num(1), num(2), num(3), num(4), num(5))
                {
                    if let Some(view) = self.0.read(cx).pdf_view() {
                        view.update(cx, |v, cx| {
                            v.highlight_text(p as usize, (x0, y0), (x1, y1), cx);
                        });
                    }
                }
            }
            other => eprintln!("shot: unknown ws command {other:?}"),
        }
    }
}

/// Process start, for the startup probe (§15.1 cold start budget).
static START: OnceLock<Instant> = OnceLock::new();

/// `UTOPIA_PROBE=1`: measure time to the first frame and keystroke → frame cost, print
/// them, and quit. Each keystroke goes through the editor's platform input handler and is
/// followed by a full synchronous render, layout and paint of the window.
fn run_probe(ws: Entity<Workspace>, window: &mut Window) {
    window.on_next_frame(move |window, cx| {
        let first_frame = START.get().map_or(Duration::ZERO, |s| s.elapsed());
        let editor = ws.read(cx).editor();
        let mut samples = Vec::new();
        for _ in 0..80 {
            let t = Instant::now();
            editor.update(cx, |e, cx| e.replace_text_in_range(None, "x", window, cx));
            window.draw(cx).clear();
            samples.push(t.elapsed());
        }
        samples.sort();
        let ms = |d: Duration| d.as_secs_f64() * 1000.0;
        let p = |q: f64| ms(samples[((samples.len() - 1) as f64 * q).round() as usize]);
        println!(
            "probe first_frame_ms={:.1} key_to_frame_p50_ms={:.2} key_to_frame_p95_ms={:.2} hotkey={}",
            ms(first_frame),
            p(0.5),
            p(0.95),
            u8::from(hotkey::register())
        );
        cx.quit();
    });
}

enum RootState {
    Onboarding {
        input: Entity<LineInput>,
        error: Option<String>,
        _sub: Subscription,
    },
    /// The graph is encrypted at rest and no stored passphrase opened it (P2).
    Locked {
        path: PathBuf,
        input: Entity<LineInput>,
        error: Option<String>,
        _sub: Subscription,
    },
    Ready(Ready),
    /// The graph could not be opened; there is no retry UI (§9.3).
    Failed(String),
}

/// Everything that exists only while a graph is open: the workspace and the side tasks
/// serving it (issue 3). Dropping `Ready` stops them, so no side task can outlive its
/// graph.
struct Ready {
    ws: Entity<Workspace>,
    /// The global quick-capture hotkey (FR-2.19), polled every 150 ms; registered only in
    /// [`RunMode::Interactive`].
    _hotkey: Option<gpui::Task<()>>,
    _sync: Entity<sync_setup::Coordinator>,
    /// `utopia://` links and the automation API (P2); always running while a graph is open.
    _inbox: gpui::Task<()>,
}

/// The open graph, held at app level rather than inside the window so the quit path can
/// still flush its store after the window has closed (issue 2).
struct Running {
    /// `None` once the main window has gone: there is no editor left to finish.
    root: Option<gpui::WeakEntity<Root>>,
    app: Entity<AppState>,
}

impl Global for Running {}

/// How this process runs: a person at the window, a `UTOPIA_SHOT_SCRIPT`, or the startup
/// probe (§15.1). Read from the environment once and carried by [`Root`], so the script,
/// the hotkey and the probe all decide from one value (issue 1).
#[derive(Clone, Debug, PartialEq)]
enum RunMode {
    Interactive,
    Scripted(Vec<shot::Step>),
    Probe,
}

impl RunMode {
    /// A `UTOPIA_SHOT_SCRIPT` that does not parse is an error, not a fallback: the run that
    /// set it is unattended and would otherwise sit at a window nothing drives, with no
    /// hotkey and no step that quits it (issue 1).
    fn from_env() -> Result<Self, String> {
        let script = std::env::var("UTOPIA_SHOT_SCRIPT").ok();
        Self::from_parts(
            script.as_deref(),
            std::env::var_os("UTOPIA_PROBE").is_some(),
        )
    }

    fn from_parts(script: Option<&str>, probe: bool) -> Result<Self, String> {
        if let Some(source) = script {
            return shot::parse(source).map(RunMode::Scripted);
        }
        Ok(if probe {
            RunMode::Probe
        } else {
            RunMode::Interactive
        })
    }
}

/// Top-level view: onboarding when there is no graph yet (§9.3), else the workspace.
struct Root {
    state: RootState,
    config: PathBuf,
    /// Decided once, before the window opens (see [`RunMode`]).
    mode: RunMode,
    _appearance: Subscription,
}

impl Root {
    fn new(
        graph: Option<PathBuf>,
        config: PathBuf,
        mode: RunMode,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) -> Self {
        let appearance = cx.observe_window_appearance(window, |_, window, cx| {
            cx.set_global(Theme::for_appearance(window.appearance()));
            cx.notify();
        });
        // The first state is decided before the root exists, so the placeholder
        // `Failed("")` that could never be shown is gone (issue 4).
        let state = match graph {
            Some(path) => {
                let key = utopia_store::is_encrypted(&path)
                    .then(|| paths::db_passphrase(&path))
                    .flatten();
                Self::load_graph(path, key, &config, &mode, window, cx)
            }
            None => Self::onboarding(window, cx),
        };
        let mut root = Self {
            state,
            config,
            mode,
            _appearance: appearance,
        };
        root.start_script(window, cx);
        root
    }

    /// Spawn the shot script parsed at startup. The mode is the only thing that decides
    /// whether a script runs and whether a hotkey is registered, so a script that failed
    /// to parse can no longer leave a window with neither (issue 1).
    fn start_script(&mut self, window: &mut Window, cx: &mut Context<Self>) {
        let RunMode::Scripted(steps) = &self.mode else {
            return;
        };
        let steps = steps.clone();
        let handle = window.window_handle();
        let hooks = Box::new(ScriptHooks(cx.entity().downgrade()));
        cx.spawn(async move |_, cx| shot::run(steps, handle, hooks, cx).await)
            .detach();
    }

    /// Commit the block being edited before the store is flushed on quit (rule 5, AC-1.1).
    fn end_edit(&mut self, cx: &mut Context<Self>) {
        if let RootState::Ready(ready) = &self.state {
            ready.ws.update(cx, |ws, cx| ws.finish_edit(cx));
        }
    }

    fn onboarding(window: &mut Window, cx: &mut Context<Root>) -> RootState {
        let input = cx.new(|cx| LineInput::new("Name your graph", cx));
        let sub = cx.subscribe_in(
            &input,
            window,
            |this, input, e: &LineInputEvent, window, cx| {
                if let LineInputEvent::Submit { .. } = e {
                    let name = input.read(cx).text().to_string();
                    match paths::graph_file_name(&name) {
                        Some(file) => this.open_graph(paths::graphs_dir().join(file), window, cx),
                        None => {
                            if let RootState::Onboarding { error, .. } = &mut this.state {
                                *error = Some("Please enter a name.".into());
                            }
                            cx.notify();
                        }
                    }
                }
            },
        );
        input.update(cx, |i, _| i.focus(window));
        RootState::Onboarding {
            input,
            error: None,
            _sub: sub,
        }
    }

    fn open_graph(&mut self, path: PathBuf, window: &mut Window, cx: &mut Context<Self>) {
        let key = utopia_store::is_encrypted(&path)
            .then(|| paths::db_passphrase(&path))
            .flatten();
        self.open_graph_with(path, key, window, cx);
    }

    /// Ask for the passphrase of an encrypted graph.
    fn locked(
        path: PathBuf,
        error: Option<String>,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> RootState {
        let input = cx.new(|cx| {
            let mut i = LineInput::new("Passphrase", cx);
            i.set_masked(true, cx);
            i
        });
        let p2 = path.clone();
        let sub = cx.subscribe_in(
            &input,
            window,
            move |this, input, e: &LineInputEvent, window, cx| {
                if let LineInputEvent::Submit { .. } = e {
                    let pass = input.read(cx).text().to_string();
                    this.open_graph_with(p2.clone(), Some(pass), window, cx);
                }
            },
        );
        input.update(cx, |i, _| i.focus(window));
        RootState::Locked {
            path,
            input,
            error,
            _sub: sub,
        }
    }

    /// The transition into a graph: onboarding's name submitted, or a passphrase tried.
    fn open_graph_with(
        &mut self,
        path: PathBuf,
        key: Option<String>,
        window: &mut Window,
        cx: &mut Context<Self>,
    ) {
        self.state = Self::load_graph(path, key, &self.config, &self.mode, window, cx);
        cx.notify();
    }

    /// Open `path` and everything [`Ready`] owns, returning the state to show. One function
    /// builds the Ready state, so there is no path that half-enters it (issue 3).
    fn load_graph(
        path: PathBuf,
        key: Option<String>,
        config: &Path,
        mode: &RunMode,
        window: &mut Window,
        cx: &mut Context<Root>,
    ) -> RootState {
        match Store::open_with_key(&path, key.as_deref()) {
            Err(utopia_store::StoreError::Encrypted) => {
                let error = key.map(|_| "That passphrase does not open this graph.".to_string());
                Self::locked(path, error, window, cx)
            }
            Ok(store) => {
                paths::remember_graph(config, &path);
                // Settings → Sync can name the graph even when sync is off; the transport
                // is filled in below when one is configured.
                cx.set_global(utopia_ui::sync_runner::SyncFacts {
                    target: None,
                    graph: Some(paths::graph_id(&path)),
                    encrypted: false,
                });
                let plugins = config.join("plugins");
                let app = cx.new(|_| {
                    let mut a = AppState::new(store);
                    a.load_plugins(&plugins);
                    a
                });
                let ws = cx.new(|cx| Workspace::new(app.clone(), window, cx));
                ws.update(cx, |w, cx| w.open_launch_page(window, cx));
                // The mode decides every startup side task; nothing re-reads the
                // environment here (issue 1).
                let hotkey = match mode {
                    // A scripted run drives the window and quits at its last step.
                    RunMode::Scripted(_) => None,
                    RunMode::Probe => {
                        run_probe(ws.clone(), window);
                        None
                    }
                    RunMode::Interactive => {
                        // FR-2.19: the system-wide hotkey brings the app forward into capture.
                        let registered = hotkey::register();
                        if !registered {
                            eprintln!("utopia: global quick-capture hotkey unavailable; Cmd+Shift+Space works in-app");
                        }
                        let ws2 = ws.clone();
                        Some(cx.spawn_in(window, async move |_, cx| loop {
                            cx.background_executor()
                                .timer(Duration::from_millis(150))
                                .await;
                            if hotkey::take_fired() {
                                let r = cx.update(|window, cx| {
                                    cx.activate(true);
                                    ws2.update(cx, |w, cx| {
                                        w.quick_capture(&QuickCapture, window, cx)
                                    });
                                });
                                if r.is_err() {
                                    break;
                                }
                            }
                        }))
                    }
                };
                let sync = sync_setup::Coordinator::install(ws.clone(), &path, config, cx);
                // Links and the automation API run on the main thread against the same
                // store the UI uses (P2: local HTTP API + utopia://).
                let api = match std::env::var("UTOPIA_API").ok().filter(|v| !v.is_empty()) {
                    Some(v) => {
                        let addr = if v == "1" {
                            "127.0.0.1:0".to_string()
                        } else {
                            v
                        };
                        match utopia_api::serve(&addr, utopia_api::new_token()) {
                            Ok(server) => {
                                let file = config.join("api.json");
                                if let Err(e) = server.write_discovery(&file) {
                                    eprintln!("utopia: could not write {}: {e}", file.display());
                                }
                                eprintln!("utopia: automation API on {}", server.url());
                                Some(server)
                            }
                            Err(e) => {
                                eprintln!("utopia: automation API failed: {e}");
                                None
                            }
                        }
                    }
                    None => None,
                };
                let (ws2, app2) = (ws.clone(), app.clone());
                let inbox = cx.spawn_in(window, async move |_, cx| loop {
                    cx.background_executor()
                        .timer(Duration::from_millis(30))
                        .await;
                    let urls = std::mem::take(&mut *PENDING_URLS.lock().expect("url queue"));
                    let r = cx.update(|window, cx| {
                        for url in &urls {
                            cx.activate(true);
                            follow_link(url, &ws2, window, cx);
                        }
                        if let Some(server) = &api {
                            while let Some(call) = server.try_recv() {
                                app2.update(cx, |a, cx| {
                                    let changed = call.run(&mut a.store);
                                    if !changed.is_empty() {
                                        a.changed(changed, true, cx);
                                    }
                                });
                            }
                        }
                    });
                    if r.is_err() {
                        break;
                    }
                });
                // The quit path flushes through this handle, which therefore keeps the
                // store alive after the window is gone (issue 2).
                cx.set_global(Running {
                    root: Some(cx.weak_entity()),
                    app: app.clone(),
                });
                RootState::Ready(Ready {
                    ws,
                    _hotkey: hotkey,
                    _sync: sync,
                    _inbox: inbox,
                })
            }
            Err(e) => RootState::Failed(format!("Could not open {}: {e}", path.display())),
        }
    }
}

impl Render for Root {
    fn render(&mut self, _window: &mut Window, cx: &mut Context<Self>) -> impl IntoElement {
        let th = theme(cx).clone();
        let density = utopia_ui::appearance::density(cx);
        let shell = div()
            .size_full()
            .bg(th.paper)
            .text_color(th.ink)
            .text_size(px(density.text_size()))
            .line_height(px(density.line_height()))
            .font_family(utopia_ui::appearance::font_family(cx));
        match &self.state {
            RootState::Ready(ready) => shell.child(ready.ws.clone()),
            RootState::Onboarding { input, error, .. } => {
                shell.flex().justify_center().items_center().child(
                    div()
                        .w(px(420.))
                        .flex()
                        .flex_col()
                        .gap_2()
                        .child(
                            div()
                                .text_size(px(utopia_ui::theme::SECTION.size))
                                .font_weight(utopia_ui::theme::SECTION.weight)
                                .child("Welcome to Utopia"),
                        )
                        .child(
                            div()
                                .text_color(th.muted)
                                .child("Name your graph. It is a single file you own."),
                        )
                        .child(
                            div()
                                .px_2()
                                .py_1()
                                .border_1()
                                .border_color(th.link)
                                .rounded(px(utopia_ui::theme::RADIUS_CONTROL))
                                .child(input.clone()),
                        )
                        .child(div().text_size(px(utopia_ui::theme::CAPTION_SIZE)).text_color(th.muted).child(format!(
                            "Creates {}/<name>.utopia",
                            paths::graphs_dir().display()
                        )))
                        .children(
                            error
                                .clone()
                                .map(|e| div().text_color(th.danger).child(e)),
                        ),
                )
            }
            RootState::Locked {
                path, input, error, ..
            } => shell.flex().justify_center().items_center().child(
                div()
                    .debug_selector(|| "unlock".into())
                    .w(px(420.))
                    .flex()
                    .flex_col()
                    .gap_2()
                    .child(
                        div()
                            .text_size(px(utopia_ui::theme::SECTION.size))
                            .font_weight(utopia_ui::theme::SECTION.weight)
                            .child("This graph is locked"),
                    )
                    .child(div().text_color(th.muted).child(format!(
                        "{} is encrypted at rest. Enter its passphrase.",
                        path.file_name()
                            .map(|n| n.to_string_lossy().to_string())
                            .unwrap_or_default()
                    )))
                    .child(
                        div()
                            .px_2()
                            .py_1()
                            .border_1()
                            .border_color(th.link)
                            .rounded(px(utopia_ui::theme::RADIUS_CONTROL))
                            .child(input.clone()),
                    )
                    .child(div().text_size(px(utopia_ui::theme::CAPTION_SIZE)).text_color(th.muted).child(
                        "Tip: store it in the keychain (service utopia-db) to unlock automatically.",
                    ))
                    .children(
                        error
                            .clone()
                            .map(|e| div().text_color(th.danger).child(e)),
                    ),
            ),
            RootState::Failed(msg) => shell
                .flex()
                .justify_center()
                .items_center()
                .child(div().text_color(th.danger).child(msg.clone())),
        }
    }
}

/// The one Settings window (§9.1): `Open` only while that window exists, so a closed
/// window never leaves a stale handle behind (issue 5).
#[derive(Clone, Copy, Default)]
enum SettingsWindow {
    #[default]
    Closed,
    Open(WindowHandle<SettingsView>),
}

impl SettingsWindow {
    /// Forget a window handle once its window has closed. The app's window-closed observer
    /// is not told which window went away, so ask the handle. Returns the state to keep.
    fn forget_if_closed(self, cx: &mut App) -> Self {
        let live = match &self {
            SettingsWindow::Open(handle) => handle.update(cx, |_, _, _| ()).is_ok(),
            SettingsWindow::Closed => true,
        };
        if live {
            self
        } else {
            SettingsWindow::Closed
        }
    }
}

impl Global for SettingsWindow {}

/// Open Settings on `tab`, or bring the open one to the front and switch it to `tab`.
///
/// Deferred because gpui refuses to update a window from inside its own update, and global
/// action handlers run during one: pressing Cmd+, while Settings is focused would otherwise
/// fail to reach the existing window and open a second copy of it.
fn open_settings(tab: SettingsTab, cx: &mut App) {
    cx.defer(move |cx| open_settings_now(tab, cx));
}

fn open_settings_now(tab: SettingsTab, cx: &mut App) {
    if let SettingsWindow::Open(handle) = *cx.global::<SettingsWindow>() {
        // `update` fails if the user closed the window; fall through and make a new one.
        let raised = handle
            .update(cx, |view, window, cx| {
                view.show(tab, cx);
                window.activate_window();
            })
            .is_ok();
        if raised {
            return;
        }
        cx.set_global(SettingsWindow::Closed);
    }
    let config = paths::config_dir();
    // Wide enough that the Help column and its screenshots fit without clipping.
    let bounds = Bounds::centered(None, size(px(1100.), px(760.)), cx);
    let options = WindowOptions {
        window_bounds: Some(WindowBounds::Windowed(bounds)),
        titlebar: Some(TitlebarOptions {
            title: Some("Settings".into()),
            ..Default::default()
        }),
        window_min_size: Some(size(px(900.), px(520.))),
        ..Default::default()
    };
    match cx.open_window(options, move |window, cx| {
        cx.new(|cx| SettingsView::new(config, tab, window, cx))
    }) {
        Ok(handle) => cx.set_global(SettingsWindow::Open(handle)),
        Err(e) => eprintln!("utopia: could not open Settings: {e}"),
    }
}

fn menus() -> Vec<Menu> {
    vec![
        Menu {
            name: "Utopia".into(),
            items: vec![
                MenuItem::action("Quick Jump", QuickJump),
                MenuItem::separator(),
                MenuItem::action("Settings…", OpenSettings),
                MenuItem::separator(),
                MenuItem::action("Quit Utopia", Quit),
            ],
        },
        Menu {
            name: "Edit".into(),
            items: vec![
                MenuItem::action("Undo", Undo),
                MenuItem::action("Redo", Redo),
                MenuItem::separator(),
                MenuItem::action("Cut", Cut),
                MenuItem::action("Copy", Copy),
                MenuItem::action("Paste", Paste),
                MenuItem::action("Copy Block Reference", CopyBlockRef),
            ],
        },
        Menu {
            name: "Go".into(),
            items: vec![
                MenuItem::action("Daily Note", GoDailyNotes),
                MenuItem::action("Today", GoToday),
                MenuItem::action("Yesterday", GoYesterday),
                MenuItem::action("Previous Day", PrevDay),
                MenuItem::action("Next Day", NextDay),
                MenuItem::separator(),
                MenuItem::action("Toggle Left Sidebar", ToggleLeftSidebar),
                MenuItem::action("Toggle Right Sidebar", ToggleRightSidebar),
                MenuItem::action("Toggle References", ToggleReferences),
            ],
        },
        Menu {
            name: "Help".into(),
            items: vec![
                MenuItem::action("Keyboard Shortcuts", ShowShortcuts),
                MenuItem::action("Utopia Help", UtopiaHelp),
            ],
        },
    ]
}

/// The one way out of the app (issue 2). Cmd+Q in any window, the Quit menu item and the
/// last window closing all end here: finish the block being edited, make every pending edit
/// durable, then quit. `AppState::flush_for_quit` reports a writer failure instead of
/// swallowing it, so a quit can no longer pretend a commit reached disk (rule 2, AC-1.1).
///
/// `Running` keeps the store reachable after its window has closed, which is the case the
/// old `on_window_closed → cx.quit()` path skipped (issue 2).
fn quit_app(cx: &mut App) {
    let running = cx
        .try_global::<Running>()
        .map(|running| (running.root.clone(), running.app.clone()));
    if let Some((root, app)) = running {
        // Marked text lives only in the field until `end` commits it (rule 5).
        if let Some(root) = root.and_then(|root| root.upgrade()) {
            root.update(cx, |root, cx| root.end_edit(cx));
        }
        app.update(cx, |app, _| app.flush_for_quit());
    }
    cx.quit();
}

fn run_app(graph: Option<PathBuf>) -> anyhow::Result<()> {
    // Decided once, before a window exists: a malformed `UTOPIA_SHOT_SCRIPT` is reported
    // and the run exits, so a scripted run can never look like a normal launch (issue 1).
    let mode = RunMode::from_env().map_err(|e| anyhow::anyhow!("UTOPIA_SHOT_SCRIPT: {e}"))?;
    let config = paths::config_dir();
    let keymap = std::fs::read_to_string(config.join("keymap.json")).ok();
    let application = Application::new();
    application.on_open_urls(|urls| {
        PENDING_URLS.lock().expect("url queue").extend(urls);
    });
    application.run(move |cx: &mut App| {
        cx.set_global(Theme::light());
        utopia_ui::appearance::install(cx, &config);
        install_keymap(cx, keymap.as_deref());
        cx.set_menus(menus());
        cx.on_action(|_: &Quit, cx| quit_app(cx));
        // The backstop for a quit the app is never asked about (a logout, the system
        // terminating it): the store is still reachable through `Running` (issue 2). On
        // the normal path it finds nothing left to flush.
        cx.on_app_quit(|cx| {
            if let Some(app) = cx
                .try_global::<Running>()
                .map(|running| running.app.clone())
            {
                app.update(cx, |app, _| app.flush_for_quit());
            }
            async {}
        })
        .detach();
        cx.on_action(|_: &OpenSettings, cx| open_settings(SettingsTab::Keybindings, cx));
        cx.on_action(|_: &ShowShortcuts, cx| open_settings(SettingsTab::Shortcuts, cx));
        cx.on_action(|_: &UtopiaHelp, cx| open_settings(SettingsTab::Help, cx));
        cx.set_global(SettingsWindow::default());
        let bounds = Bounds::centered(None, size(px(1280.), px(820.)), cx);
        let options = WindowOptions {
            window_bounds: Some(WindowBounds::Windowed(bounds)),
            titlebar: Some(TitlebarOptions {
                title: Some("Utopia".into()),
                ..Default::default()
            }),
            window_min_size: Some(size(px(720.), px(480.))),
            ..Default::default()
        };
        let opened = cx.open_window(options, move |window, cx| {
            cx.set_global(Theme::for_appearance(window.appearance()));
            cx.new(|cx| Root::new(graph, config, mode, window, cx))
        });
        if let Err(e) = opened {
            eprintln!("utopia: could not open a window: {e}");
            cx.quit();
            return;
        }
        cx.on_window_closed(|cx| {
            // §9.1: a closed Settings window leaves no handle behind (issue 5).
            let settings =
                std::mem::replace(cx.global_mut::<SettingsWindow>(), SettingsWindow::Closed);
            let settings = settings.forget_if_closed(cx);
            cx.set_global(settings);
            // AC-1.1: the last window closing is a quit path like any other (issue 2).
            if cx.windows().is_empty() {
                quit_app(cx);
            }
        })
        .detach();
        cx.activate(true);
    });
    Ok(())
}

fn main() -> anyhow::Result<()> {
    START.get_or_init(Instant::now);
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.first().map(String::as_str) {
        Some("doctor") => {
            let Some(graph) = args.get(1) else {
                anyhow::bail!("{}", cli::USAGE);
            };
            let rebuild = args.iter().any(|a| a == "--rebuild");
            let ok = cli::doctor(graph.as_ref(), rebuild)?;
            std::process::exit(if ok { 0 } else { 1 });
        }
        Some("export") => {
            let (Some(graph), Some(dir)) = (args.get(1), args.get(2)) else {
                anyhow::bail!("{}", cli::USAGE);
            };
            cli::export(graph.as_ref(), dir.as_ref())
        }
        Some("import-roam") => {
            let (Some(graph), Some(file)) = (args.get(1), args.get(2)) else {
                anyhow::bail!("{}", cli::USAGE);
            };
            cli::import_roam(graph.as_ref(), file.as_ref())
        }
        Some("capture") => {
            let text = args[1..].join(" ");
            let graph = paths::resolve_graph(None, &paths::config_dir(), &paths::graphs_dir())
                .ok_or_else(|| anyhow::anyhow!("no graph found; set UTOPIA_GRAPH"))?;
            cli::capture(&graph, &text)
        }
        Some("encrypt") => {
            let Some(graph) = args.get(1) else {
                anyhow::bail!("{}", cli::USAGE);
            };
            cli::encrypt(graph.as_ref())
        }
        Some("decrypt") => {
            let Some(graph) = args.get(1) else {
                anyhow::bail!("{}", cli::USAGE);
            };
            cli::decrypt(graph.as_ref())
        }
        Some("serve-api") => {
            let Some(graph) = args.get(1) else {
                anyhow::bail!("{}", cli::USAGE);
            };
            let addr = args.get(2).map_or("127.0.0.1:0", String::as_str);
            cli::serve_api(graph.as_ref(), addr, &paths::config_dir().join("api.json"))
        }
        Some(url) if url.starts_with("utopia://") => {
            PENDING_URLS
                .lock()
                .expect("url queue")
                .push(url.to_string());
            let graph = paths::resolve_graph(None, &paths::config_dir(), &paths::graphs_dir());
            run_app(graph)
        }
        Some("-h" | "--help" | "help") => {
            println!("{}", cli::USAGE);
            Ok(())
        }
        _ => {
            let explicit = args
                .iter()
                .position(|a| a == "--graph")
                .and_then(|i| args.get(i + 1))
                .map(PathBuf::from);
            let graph = paths::resolve_graph(explicit, &paths::config_dir(), &paths::graphs_dir());
            run_app(graph)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::TestAppContext;

    /// Issue 1: a script that does not parse used to be logged and dropped while the
    /// hotkey registration was skipped anyway — a normal window that no step drove and
    /// nothing quit. It is a parse error now, reported before a window exists.
    #[test]
    fn a_malformed_shot_script_is_an_error() {
        let err = RunMode::from_parts(Some("taek 300"), false).expect_err("no step can run");
        assert!(err.contains("unknown step"), "{err}");
    }

    /// One value decides the run: a script outranks the probe, which outranks a normal
    /// launch. `Root` reads this and nothing re-reads the environment.
    #[test]
    fn one_mode_is_parsed_for_the_whole_run() {
        assert!(matches!(
            RunMode::from_parts(None, false),
            Ok(RunMode::Interactive)
        ));
        assert!(matches!(
            RunMode::from_parts(None, true),
            Ok(RunMode::Probe)
        ));
        let scripted = RunMode::from_parts(Some("wait 10"), true).unwrap();
        assert_eq!(scripted, RunMode::Scripted(shot::parse("wait 10").unwrap()));
    }

    /// A graph with one block, opened and committed in strict mode.
    fn graph(path: &Path) -> (Store, utopia_core::PageId, utopia_core::BlockId) {
        let mut store = Store::open(path).unwrap();
        let (page, block) = store.open_daily(store.today()).unwrap();
        store
            .set_block_text(&page, &block, "first", None, None)
            .unwrap();
        store.sync_index().unwrap();
        (store, page, block)
    }

    /// AC-1.1: the last keystroke survives a quit. The main window is already gone here,
    /// which is the path that used to quit without `sync_index` (issue 2); the store is
    /// still reachable through `Running`, so the flush happens where the quit does.
    #[gpui::test]
    fn ac_1_1_the_last_keystroke_survives_a_quit(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("g.utopia");
        let (store, page, block) = graph(&path);
        // `AppState::new` switches the store to debounced mode: the edit is in memory only.
        let app = cx.new(|_| AppState::new(store));
        app.update(cx, |app, _| {
            app.store
                .set_block_text(&page, &block, "hello", None, None)
                .unwrap();
            assert!(app.store.has_pending(), "the edit has not reached disk");
        });
        cx.update(|cx| {
            cx.set_global(Running {
                root: None,
                app: app.clone(),
            })
        });
        cx.update(quit_app);
        assert!(
            !app.read_with(cx, |app, _| app.store.has_pending()),
            "the quit flushed the writer"
        );
        // The relaunch reads the same file.
        let mut reopened = Store::open(&path).unwrap();
        let text = reopened
            .page_state(&page)
            .unwrap()
            .block(&block)
            .unwrap()
            .text
            .clone();
        assert_eq!(text, "hello");
    }

    /// Issue 5: the Settings window's handle is only there while the window is. It used to
    /// linger until the next Cmd+, probed it and replaced it, so "open" and "closed" could
    /// not be told apart.
    #[gpui::test]
    fn closing_the_settings_window_forgets_its_handle(cx: &mut TestAppContext) {
        let dir = tempfile::tempdir().unwrap();
        cx.update(|cx| {
            cx.set_global(Theme::light());
            install_keymap(cx, None);
            cx.set_global(SettingsWindow::Closed);
        });
        let (_view, cx) = cx.add_window_view(|window, cx| {
            SettingsView::new(
                dir.path().to_path_buf(),
                SettingsTab::Keybindings,
                window,
                cx,
            )
        });
        let handle = cx
            .windows()
            .first()
            .copied()
            .and_then(|window| window.downcast::<SettingsView>())
            .expect("the test window's root is the Settings view");
        cx.update(|_, cx| cx.set_global(SettingsWindow::Open(handle)));
        // The user closes it (Cmd+W); the app's window-closed observer then runs this.
        handle
            .update(cx, |_, window, _| window.remove_window())
            .unwrap();
        // The window is gone, so this runs in the app context the observer gets.
        cx.cx.update(|cx| {
            let state =
                std::mem::replace(cx.global_mut::<SettingsWindow>(), SettingsWindow::Closed);
            let state = state.forget_if_closed(cx);
            cx.set_global(state);
        });
        let closed = cx.read(|cx| matches!(*cx.global::<SettingsWindow>(), SettingsWindow::Closed));
        assert!(closed, "a closed window leaves no handle behind");
    }
}
