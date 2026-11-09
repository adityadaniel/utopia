//! Appearance preferences (Settings → Appearance): which font the notes and the interface
//! are set in.
//!
//! They live in `<config dir>/settings.json`, next to `keymap.json` and resolved the same way
//! (`UTOPIA_CONFIG_DIR`). Window and UI preferences are local to the machine and never sync
//! (§12.5, architecture rule 9), so nothing here goes near a CRDT shard. The file is a small
//! versioned object meant to grow more general preferences:
//!
//! ```json
//! {
//!   "version": 1,
//!   "font": "ibm-plex-sans",
//!   "density": "comfortable",
//!   "ref_children": 2,
//!   "pinned": [],
//!   "right_sidebar_width": 380.0
//! }
//! ```
//!
//! `font` is `"system"` (the default) or `"ibm-plex-sans"`; `density` is
//! `"comfortable"` (the default) or `"compact"` (ALL-5); `ref_children` is how many
//! children a Linked Reference shows inline, 0 to `REF_CHILDREN_MAX` (LR-4, default 2,
//! 0 folds every child behind the "+N more" chip); `pinned` is the list of page ids the
//! sidebars keep, in the order they were pinned — one list shared by the left sidebar's
//! Pinned group (SB-3) and the right sidebar's Pinned tab (RS-3); `right_sidebar_width`
//! is the width that sidebar was dragged to (RS-4). A missing file means defaults; an
//! unreadable file, an unknown value or a newer version is logged and read as far as it can
//! be, never a panic (rule 11). Keys this version does not know are kept when the file is
//! rewritten, so a newer release's preferences survive a round trip through an older one.

use gpui::{App, Global, SharedString};
use serde_json::{Map, Value};
use std::path::{Path, PathBuf};
use utopia_core::PageId;

use crate::fonts::{register_ibm_plex_mono, register_ibm_plex_sans, IBM_PLEX_SANS};
use crate::theme::{BODY_SIZE, LINE_HEIGHT};

pub const FILE: &str = "settings.json";
pub const VERSION: u64 = 1;
/// How many children a Linked Reference shows inline by default (LR-4).
pub const DEFAULT_REF_CHILDREN: usize = 2;
/// The largest "Children shown in references" the Appearance tab offers.
pub const REF_CHILDREN_MAX: usize = 3;

/// The font the UI and notes are set in. Code blocks stay in Menlo either way, and mono
/// labels always use the bundled IBM Plex Mono (DESIGN.md §2.2).
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum UiFont {
    /// The platform's UI font (SF Pro on macOS), kept as an option for readers who
    /// prefer it; not the default.
    System,
    /// IBM Plex Sans, bundled with the app (`crate::fonts`). The default (DESIGN.md §2.2).
    #[default]
    IbmPlexSans,
}

impl UiFont {
    pub const ALL: &'static [UiFont] = &[UiFont::System, UiFont::IbmPlexSans];

    /// How `settings.json` spells it.
    pub fn key(self) -> &'static str {
        match self {
            UiFont::System => "system",
            UiFont::IbmPlexSans => "ibm-plex-sans",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|f| f.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            UiFont::System => "System",
            UiFont::IbmPlexSans => "IBM Plex Sans",
        }
    }

    /// The family gpui is asked for.
    pub fn family(self) -> &'static str {
        match self {
            UiFont::System => ".SystemUIFont",
            UiFont::IbmPlexSans => IBM_PLEX_SANS,
        }
    }
}

/// How tightly blocks are set (ALL-5). Local to the machine, like the font.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Density {
    /// The body at 15 px on 24 px lines, as before the setting existed.
    #[default]
    Comfortable,
    /// 13.5 px body, 18 px lines and 4 px between blocks.
    Compact,
}

impl Density {
    pub const ALL: &'static [Density] = &[Density::Comfortable, Density::Compact];

    pub fn key(self) -> &'static str {
        match self {
            Density::Comfortable => "comfortable",
            Density::Compact => "compact",
        }
    }

    pub fn from_key(key: &str) -> Option<Self> {
        Self::ALL.iter().copied().find(|d| d.key() == key)
    }

    pub fn label(self) -> &'static str {
        match self {
            Density::Comfortable => "Comfortable",
            Density::Compact => "Compact",
        }
    }

    pub fn text_size(self) -> f32 {
        match self {
            Density::Comfortable => BODY_SIZE,
            Density::Compact => 13.5,
        }
    }

    pub fn line_height(self) -> f32 {
        match self {
            Density::Comfortable => LINE_HEIGHT,
            Density::Compact => 18.,
        }
    }

    /// Space between two blocks. Rows take half of it above and half below.
    pub fn block_spacing(self) -> f32 {
        match self {
            Density::Comfortable => 0.,
            Density::Compact => 4.,
        }
    }
}

/// The right sidebar's width as it was before the setting existed.
pub const RIGHT_SIDEBAR_DEFAULT_WIDTH: f32 = 380.;
/// Narrow enough to leave a readable page beside it, wide enough for a reference.
pub const RIGHT_SIDEBAR_MIN_WIDTH: f32 = 240.;
pub const RIGHT_SIDEBAR_MAX_WIDTH: f32 = 640.;

/// Keep a width — dragged, typed or read from a file — inside what the layout can show.
pub fn clamp_sidebar_width(width: f32) -> f32 {
    if width.is_finite() {
        width.clamp(RIGHT_SIDEBAR_MIN_WIDTH, RIGHT_SIDEBAR_MAX_WIDTH)
    } else {
        RIGHT_SIDEBAR_DEFAULT_WIDTH
    }
}

/// What `settings.json` holds.
#[derive(Clone, Debug, PartialEq)]
pub struct Prefs {
    pub font: UiFont,
    pub density: Density,
    /// Children shown inline under a referencing block (LR-4); 0 folds them all.
    pub ref_children: usize,
    /// Page ids kept at the top of the sidebar, in the order they were pinned (SB-3), and
    /// listed in the right sidebar's Pinned tab (RS-3). Local state: a pin is a pointer to
    /// a page, not a property of the page, so it never goes into a CRDT shard (rule 9).
    pub pinned: Vec<PageId>,
    /// How wide the right sidebar is, in points (RS-4). Clamped to
    /// [`RIGHT_SIDEBAR_MIN_WIDTH`]..=[`RIGHT_SIDEBAR_MAX_WIDTH`] when read and written.
    pub right_sidebar_width: f32,
    /// Keys this version does not understand, written back untouched.
    rest: Map<String, Value>,
}

impl Default for Prefs {
    fn default() -> Self {
        Self {
            font: UiFont::default(),
            density: Density::default(),
            ref_children: DEFAULT_REF_CHILDREN,
            pinned: Vec::new(),
            right_sidebar_width: RIGHT_SIDEBAR_DEFAULT_WIDTH,
            rest: Map::new(),
        }
    }
}

impl Prefs {
    pub fn from_json(json: &str) -> (Self, Vec<String>) {
        let mut warnings = Vec::new();
        let mut rest = match serde_json::from_str::<Value>(json) {
            Ok(Value::Object(m)) => m,
            Ok(_) => {
                warnings.push(format!("{FILE}: expected an object; using the defaults"));
                return (Self::default(), warnings);
            }
            Err(e) => {
                warnings.push(format!("{FILE}: {e}; using the defaults"));
                return (Self::default(), warnings);
            }
        };
        match rest.remove("version") {
            Some(Value::Number(n)) if n.as_u64() == Some(VERSION) => {}
            None => {}
            Some(v) => warnings.push(format!(
                "{FILE}: version {v} is not {VERSION}; reading what this version understands"
            )),
        }
        let font = match rest.remove("font") {
            None => UiFont::default(),
            Some(Value::String(s)) => UiFont::from_key(&s).unwrap_or_else(|| {
                warnings.push(format!(
                    "{FILE}: unknown font {s:?}; using {}",
                    UiFont::default().label()
                ));
                UiFont::default()
            }),
            Some(v) => {
                warnings.push(format!(
                    "{FILE}: font should be a string, not {v}; using {}",
                    UiFont::default().label()
                ));
                UiFont::default()
            }
        };
        let density = match rest.remove("density") {
            None => Density::default(),
            Some(Value::String(s)) => Density::from_key(&s).unwrap_or_else(|| {
                warnings.push(format!("{FILE}: unknown density {s:?}; using Comfortable"));
                Density::default()
            }),
            Some(v) => {
                warnings.push(format!(
                    "{FILE}: density should be a string, not {v}; using Comfortable"
                ));
                Density::default()
            }
        };
        let right_sidebar_width = match rest.remove("right_sidebar_width") {
            None => RIGHT_SIDEBAR_DEFAULT_WIDTH,
            Some(Value::Number(n)) => match n.as_f64() {
                Some(w) if w.is_finite() => clamp_sidebar_width(w as f32),
                _ => {
                    warnings.push(format!(
                        "{FILE}: right_sidebar_width {n} is not a width; using \
                         {RIGHT_SIDEBAR_DEFAULT_WIDTH}"
                    ));
                    RIGHT_SIDEBAR_DEFAULT_WIDTH
                }
            },
            Some(v) => {
                warnings.push(format!(
                    "{FILE}: right_sidebar_width should be a number, not {v}; using \
                     {RIGHT_SIDEBAR_DEFAULT_WIDTH}"
                ));
                RIGHT_SIDEBAR_DEFAULT_WIDTH
            }
        };
        let ref_children = match rest.remove("ref_children") {
            None => DEFAULT_REF_CHILDREN,
            Some(Value::Number(n)) => match n.as_u64() {
                Some(n) if n <= REF_CHILDREN_MAX as u64 => n as usize,
                _ => {
                    warnings.push(format!(
                        "{FILE}: ref_children should be 0 to {REF_CHILDREN_MAX}, not {n}; \
                         using {DEFAULT_REF_CHILDREN}"
                    ));
                    DEFAULT_REF_CHILDREN
                }
            },
            Some(v) => {
                warnings.push(format!(
                    "{FILE}: ref_children should be a number, not {v}; \
                     using {DEFAULT_REF_CHILDREN}"
                ));
                DEFAULT_REF_CHILDREN
            }
        };
        let pinned = match rest.remove("pinned") {
            None => Vec::new(),
            Some(Value::Array(items)) => {
                let mut pinned: Vec<PageId> = Vec::with_capacity(items.len());
                for item in items {
                    match item {
                        Value::String(id) if !id.is_empty() => {
                            let id = PageId(id);
                            if !pinned.contains(&id) {
                                pinned.push(id);
                            }
                        }
                        other => warnings.push(format!(
                            "{FILE}: ignoring pinned page {other}; page ids are strings"
                        )),
                    }
                }
                pinned
            }
            Some(v) => {
                warnings.push(format!(
                    "{FILE}: pinned should be a list of page ids, not {v}; using none"
                ));
                Vec::new()
            }
        };
        // The branch this preference came from stored the sidebar pins under
        // `pinned_pages`; the left sidebar's group (SB-3) and the right sidebar's tab (RS-3)
        // now share `pinned`, so fold the old list in rather than dropping it.
        let mut pinned = pinned;
        match rest.remove("pinned_pages") {
            None => {}
            Some(Value::Array(items)) => {
                for item in items {
                    match item {
                        Value::String(id) if !id.is_empty() => {
                            let id = PageId(id);
                            if !pinned.contains(&id) {
                                pinned.push(id);
                            }
                        }
                        other => warnings.push(format!(
                            "{FILE}: ignoring pinned page {other}; page ids are strings"
                        )),
                    }
                }
            }
            Some(v) => warnings.push(format!(
                "{FILE}: pinned_pages should be a list of page ids, not {v}; ignoring it"
            )),
        }
        (
            Self {
                font,
                density,
                ref_children,
                pinned,
                right_sidebar_width,
                rest,
            },
            warnings,
        )
    }

    pub fn to_json(&self) -> String {
        let mut m = Map::new();
        m.insert("version".into(), VERSION.into());
        m.insert("font".into(), self.font.key().into());
        m.insert("density".into(), self.density.key().into());
        m.insert(
            "right_sidebar_width".into(),
            serde_json::Number::from_f64(clamp_sidebar_width(self.right_sidebar_width) as f64)
                .map_or(Value::Null, Value::Number),
        );
        m.insert("ref_children".into(), self.ref_children.into());
        m.insert(
            "pinned".into(),
            Value::Array(
                self.pinned
                    .iter()
                    .map(|p| Value::String(p.as_str().to_string()))
                    .collect(),
            ),
        );
        for (k, v) in &self.rest {
            m.insert(k.clone(), v.clone());
        }
        serde_json::to_string_pretty(&Value::Object(m)).unwrap_or_default() + "\n"
    }

    /// Defaults when there is no file; a warning when there is one that cannot be read.
    pub fn load(config_dir: &Path) -> (Self, Vec<String>) {
        match std::fs::read_to_string(config_dir.join(FILE)) {
            Ok(s) => Self::from_json(&s),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => (Self::default(), Vec::new()),
            Err(e) => (
                Self::default(),
                vec![format!("{FILE}: {e}; using the defaults")],
            ),
        }
    }

    /// Write atomically: a crash mid-write leaves the old file, never half a new one.
    pub fn save(&self, config_dir: &Path) -> std::io::Result<()> {
        std::fs::create_dir_all(config_dir)?;
        let tmp = config_dir.join(format!(".{FILE}.tmp"));
        std::fs::write(&tmp, self.to_json())?;
        std::fs::rename(&tmp, config_dir.join(FILE))
    }
}

/// The preferences in force, shared by every window.
#[derive(Default)]
pub struct Appearance {
    pub prefs: Prefs,
    /// Where `settings.json` lives (set by [`install`]); empty in tests that never install
    /// one, where preferences still apply for the session but are not written.
    config_dir: PathBuf,
    plex_registered: bool,
    mono_registered: bool,
}

impl Global for Appearance {}

/// Read `settings.json` and put it in force. Returns the warnings, already logged.
pub fn install(cx: &mut App, config_dir: &Path) -> Vec<String> {
    let (prefs, warnings) = Prefs::load(config_dir);
    for w in &warnings {
        eprintln!("utopia: {w}");
    }
    let font = prefs.font;
    cx.set_global(Appearance {
        prefs,
        config_dir: config_dir.to_path_buf(),
        plex_registered: false,
        mono_registered: false,
    });
    apply(cx, font);
    warnings
}

/// The font in force. The bundled IBM Plex Sans when nothing was installed (tests, early
/// startup), matching `settings.json`'s default (DESIGN.md §2.2).
pub fn ui_font(cx: &App) -> UiFont {
    cx.try_global::<Appearance>()
        .map_or(UiFont::default(), |a| a.prefs.font)
}

/// The family every window's root sets, so all text below it follows the setting.
pub fn font_family(cx: &App) -> SharedString {
    ui_font(cx).family().into()
}

/// The density in force. Comfortable when nothing was installed (tests, early startup).
pub fn density(cx: &App) -> Density {
    cx.try_global::<Appearance>()
        .map_or(Density::default(), |a| a.prefs.density)
}

/// The right sidebar's width in force (RS-4). 380 when nothing was installed.
pub fn right_sidebar_width(cx: &App) -> f32 {
    cx.try_global::<Appearance>()
        .map_or(RIGHT_SIDEBAR_DEFAULT_WIDTH, |a| {
            clamp_sidebar_width(a.prefs.right_sidebar_width)
        })
}

/// Set the sidebar's width in memory while it is being dragged; the file follows on release
/// through [`remember`] (RS-4: one write per drag, not one per pixel).
pub fn set_right_sidebar_width(cx: &mut App, width: f32) {
    cx.default_global::<Appearance>().prefs.right_sidebar_width = clamp_sidebar_width(width);
    cx.refresh_windows();
}

/// Put the sidebar back to its default width and write that down (Settings → Appearance).
pub fn reset_right_sidebar_width(cx: &mut App, config_dir: &Path) -> std::io::Result<()> {
    cx.default_global::<Appearance>().prefs.right_sidebar_width = RIGHT_SIDEBAR_DEFAULT_WIDTH;
    cx.refresh_windows();
    cx.global::<Appearance>().prefs.save(config_dir)
}

/// How many children a Linked Reference shows inline (LR-4), clamped to the offered range.
pub fn ref_children(cx: &App) -> usize {
    cx.try_global::<Appearance>()
        .map_or(DEFAULT_REF_CHILDREN, |a| a.prefs.ref_children)
        .min(REF_CHILDREN_MAX)
}

/// Pages pinned to the top of the sidebar, in pin order (SB-3). Empty when nothing was
/// installed (tests, early startup).
pub fn pinned(cx: &App) -> Vec<PageId> {
    cx.try_global::<Appearance>()
        .map_or_else(Vec::new, |a| a.prefs.pinned.clone())
}

pub fn is_pinned(cx: &App, page: &PageId) -> bool {
    cx.try_global::<Appearance>()
        .is_some_and(|a| a.prefs.pinned.contains(page))
}

/// Pin or unpin `page` (SB-3) and remember it in `settings.json`. Returns the new state.
/// A failed write is returned, not fatal: the pin still applies for this session (rule 11).
pub fn toggle_pinned(cx: &mut App, page: &PageId) -> std::io::Result<bool> {
    let a = cx.default_global::<Appearance>();
    let on = match a.prefs.pinned.iter().position(|p| p == page) {
        Some(i) => {
            a.prefs.pinned.remove(i);
            false
        }
        None => {
            a.prefs.pinned.push(page.clone());
            true
        }
    };
    let saved = remember(cx);
    // The sidebar and an open page menu both show the pin state.
    cx.refresh_windows();
    saved.map(|()| on)
}

/// Unpin every page (Settings → Appearance). Returns whether a write failed.
pub fn clear_pinned(cx: &mut App) -> std::io::Result<()> {
    cx.default_global::<Appearance>().prefs.pinned.clear();
    let saved = remember(cx);
    cx.refresh_windows();
    saved
}

/// Write the preferences in force to the installed `settings.json`; nothing to do for a
/// test that never installed a config dir. A view that owns a preference (a pin, the right
/// sidebar's dragged width, RS-4) calls this instead of being handed the path.
pub fn remember(cx: &mut App) -> std::io::Result<()> {
    let a = cx.default_global::<Appearance>();
    if a.config_dir.as_os_str().is_empty() {
        return Ok(());
    }
    a.prefs.save(&a.config_dir)
}

/// Switch font in every open window now, and remember it in `config_dir/settings.json`.
/// A failed write is returned, not fatal: the font still applies for this session.
pub fn set_font(cx: &mut App, font: UiFont, config_dir: &Path) -> std::io::Result<()> {
    apply(cx, font);
    cx.global::<Appearance>().prefs.save(config_dir)
}

/// Switch density in every open window now and remember it (rule 9: local UI state).
pub fn set_density(cx: &mut App, density: Density, config_dir: &Path) -> std::io::Result<()> {
    cx.default_global::<Appearance>().prefs.density = density;
    cx.refresh_windows();
    cx.global::<Appearance>().prefs.save(config_dir)
}

/// Change how many children a reference shows inline (LR-4) and remember it. Open reference
/// panels notice the change on their next render and rebuild their rows.
pub fn set_ref_children(cx: &mut App, count: usize, config_dir: &Path) -> std::io::Result<()> {
    cx.default_global::<Appearance>().prefs.ref_children = count.min(REF_CHILDREN_MAX);
    cx.refresh_windows();
    cx.global::<Appearance>().prefs.save(config_dir)
}

/// Make `font` drawable: the bundled faces are handed to gpui the first time they are
/// needed (the setting, or Settings showing a sample of each choice), not at every launch.
/// IBM Plex Mono is registered whatever the UI font is: labels, counts and shortcuts use
/// it by DESIGN.md §2.2, not by preference.
pub fn load_font(cx: &mut App, font: UiFont) {
    let (mono, sans) = {
        let a = cx.default_global::<Appearance>();
        let mono = !a.mono_registered;
        let sans = font == UiFont::IbmPlexSans && !a.plex_registered;
        a.mono_registered |= mono;
        a.plex_registered |= sans;
        (mono, sans)
    };
    if mono {
        register_ibm_plex_mono(cx);
    }
    if sans {
        register_ibm_plex_sans(cx);
    }
}

fn apply(cx: &mut App, font: UiFont) {
    load_font(cx, font);
    cx.default_global::<Appearance>().prefs.font = font;
    // Views only re-render when notified; every window reads the font at its root.
    cx.refresh_windows();
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trip() {
        let p = Prefs {
            font: UiFont::IbmPlexSans,
            density: Density::Compact,
            ref_children: 0,
            pinned: vec![PageId("page-a".into()), PageId("page-b".into())],
            right_sidebar_width: 420.0,
            ..Default::default()
        };
        let json = p.to_json();
        let v: Value = serde_json::from_str(&json).unwrap();
        assert_eq!(
            v,
            serde_json::json!({
                "version": 1,
                "font": "ibm-plex-sans",
                "density": "compact",
                "ref_children": 0,
                "pinned": ["page-a", "page-b"],
                "right_sidebar_width": 420.0,
            })
        );
        assert_eq!(Prefs::from_json(&json), (p, vec![]));
    }

    /// Merge fix: the branch before the merge stored the sidebar pins under
    /// `pinned_pages`, and the two lists were merged into one `pinned`. A file written by
    /// that build must keep its pins instead of silently coming up with none.
    #[test]
    fn legacy_pinned_pages_are_migrated() {
        let (p, warnings) = Prefs::from_json(r#"{"version": 1, "pinned_pages": ["a", "b"]}"#);
        assert!(warnings.is_empty(), "{warnings:?}");
        assert_eq!(p.pinned, [PageId("a".into()), PageId("b".into())]);
        let v: Value = serde_json::from_str(&p.to_json()).unwrap();
        assert_eq!(v["pinned"], serde_json::json!(["a", "b"]));
        assert!(
            v.get("pinned_pages").is_none(),
            "the legacy key is migrated, not left behind: {v}"
        );

        // Both keys (a file already saved after the merge): the shared list wins and the
        // legacy entries it lacks are added once, in pin order.
        let (p, warnings) =
            Prefs::from_json(r#"{"version": 1, "pinned": ["a"], "pinned_pages": ["a", "b", 3]}"#);
        assert_eq!(p.pinned, [PageId("a".into()), PageId("b".into())]);
        assert_eq!(
            warnings.len(),
            1,
            "the junk entry is reported: {warnings:?}"
        );

        // Junk in the legacy key is reported, not a panic (rule 11).
        let (p, warnings) = Prefs::from_json(r#"{"version": 1, "pinned_pages": "a"}"#);
        assert!(p.pinned.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
    }

    /// SB-3: pins are page ids in `settings.json`, in pin order; a repeated id counts once
    /// and anything that is not a page id is skipped with a warning, never a panic.
    #[test]
    fn pinned_pages_round_trip_and_degrade() {
        let (p, warnings) = Prefs::from_json(r#"{"version": 1, "pinned": ["a", "a", "b"]}"#);
        assert!(
            warnings.is_empty(),
            "a repeated id is not an error: {warnings:?}"
        );
        assert_eq!(p.pinned, [PageId("a".into()), PageId("b".into())]);
        let v: Value = serde_json::from_str(&p.to_json()).unwrap();
        assert_eq!(v["pinned"], serde_json::json!(["a", "b"]));

        let (p, warnings) = Prefs::from_json(r#"{"version": 1, "pinned": ["a", 3, ""]}"#);
        assert_eq!(p.pinned, [PageId("a".into())], "only page ids survive");
        assert_eq!(warnings.len(), 2, "{warnings:?}");

        let (p, warnings) = Prefs::from_json(r#"{"version": 1, "pinned": "a"}"#);
        assert!(p.pinned.is_empty());
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert_eq!(Prefs::default().pinned, Vec::<PageId>::new());
    }

    /// Rule 11: whatever is in the file, the app comes up, on the default font, and says why.
    #[test]
    fn bad_files_degrade_to_the_default_font() {
        for bad in [
            "",
            "{ not json",
            "[1, 2]",
            r#"{"version": 1, "font": "comic-sans"}"#,
            r#"{"version": 1, "font": 3}"#,
        ] {
            let (p, warnings) = Prefs::from_json(bad);
            assert_eq!(p.font, UiFont::default(), "{bad:?}");
            assert_eq!(warnings.len(), 1, "{bad:?}: {warnings:?}");
        }
    }

    #[test]
    fn missing_keys_are_defaults_without_complaint() {
        assert_eq!(Prefs::from_json("{}"), (Prefs::default(), vec![]));
        assert_eq!(Prefs::default().density, Density::Comfortable);
        assert_eq!(
            Prefs::default().ref_children,
            DEFAULT_REF_CHILDREN,
            "LR-4: two children inline unless the user says otherwise"
        );
        assert_eq!(DEFAULT_REF_CHILDREN, 2);
    }

    /// LR-4: a bad "Children shown in references" value falls back to 2 and says so; 0 is a
    /// real choice (fold every child behind the chip), not a missing value.
    #[test]
    fn ref_children_is_read_and_bounded() {
        let (p, warnings) = Prefs::from_json(r#"{"version": 1, "ref_children": 0}"#);
        assert_eq!(p.ref_children, 0);
        assert!(warnings.is_empty());
        let (p, _) = Prefs::from_json(r#"{"version": 1, "ref_children": 3}"#);
        assert_eq!(p.ref_children, 3);
        for bad in ["12", "-1", "\"two\"", "true"] {
            let (p, warnings) =
                Prefs::from_json(&format!(r#"{{"version": 1, "ref_children": {bad}}}"#));
            assert_eq!(p.ref_children, DEFAULT_REF_CHILDREN, "{bad}");
            assert_eq!(warnings.len(), 1, "{bad}: {warnings:?}");
        }
    }

    /// RS-4: the sidebar width is clamped to what the layout can show, and junk is the
    /// default with a warning rather than a panic (rule 11).
    #[test]
    fn the_sidebar_width_is_clamped_and_never_fatal() {
        assert_eq!(clamp_sidebar_width(10.), RIGHT_SIDEBAR_MIN_WIDTH);
        assert_eq!(clamp_sidebar_width(10_000.), RIGHT_SIDEBAR_MAX_WIDTH);
        assert_eq!(clamp_sidebar_width(f32::NAN), RIGHT_SIDEBAR_DEFAULT_WIDTH);
        let (p, w) = Prefs::from_json(r#"{"version": 1, "right_sidebar_width": 480}"#);
        assert_eq!(p.right_sidebar_width, 480.);
        assert!(w.is_empty(), "{w:?}");
        for bad in [
            r#"{"version": 1, "right_sidebar_width": "wide"}"#,
            r#"{"version": 1, "right_sidebar_width": null}"#,
        ] {
            let (p, w) = Prefs::from_json(bad);
            assert_eq!(p.right_sidebar_width, RIGHT_SIDEBAR_DEFAULT_WIDTH, "{bad}");
            assert_eq!(w.len(), 1, "{bad}: {w:?}");
        }
        // A width outside the range is brought back into it on the way in and out.
        let (p, _) = Prefs::from_json(r#"{"version": 1, "right_sidebar_width": 4000}"#);
        assert_eq!(p.right_sidebar_width, RIGHT_SIDEBAR_MAX_WIDTH);
        assert_eq!(
            serde_json::from_str::<Value>(&p.to_json()).unwrap()["right_sidebar_width"],
            serde_json::json!(RIGHT_SIDEBAR_MAX_WIDTH as f64)
        );
    }

    /// An unknown density is the comfortable one, said out loud, never a panic (rule 11).
    #[test]
    fn an_unknown_density_falls_back_to_comfortable() {
        let (p, warnings) = Prefs::from_json(r#"{"version": 1, "density": "tiny"}"#);
        assert_eq!(p.density, Density::Comfortable);
        assert_eq!(warnings.len(), 1, "{warnings:?}");
        assert!(warnings[0].contains("tiny"), "{warnings:?}");
    }

    /// A newer release's file: what we know is read, the rest is kept for it.
    #[test]
    fn newer_files_are_read_and_kept() {
        let (p, warnings) =
            Prefs::from_json(r#"{"version": 2, "font": "ibm-plex-sans", "zoom": 1.25}"#);
        assert_eq!(p.font, UiFont::IbmPlexSans);
        assert_eq!(warnings.len(), 1, "the version is mentioned: {warnings:?}");
        let v: Value = serde_json::from_str(&p.to_json()).unwrap();
        assert_eq!(v["zoom"], 1.25);
        assert_eq!(v["version"], 1);
    }

    #[test]
    fn save_is_atomic_and_load_reads_it_back() {
        let dir = tempfile::tempdir().unwrap();
        assert_eq!(Prefs::load(dir.path()), (Prefs::default(), vec![]));
        let p = Prefs {
            font: UiFont::IbmPlexSans,
            ..Default::default()
        };
        p.save(dir.path()).unwrap();
        let names: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .map(|e| e.unwrap().file_name().into_string().unwrap())
            .collect();
        assert_eq!(names, [FILE], "no temp file left behind");
        assert_eq!(Prefs::load(dir.path()), (p, vec![]));
    }
}
