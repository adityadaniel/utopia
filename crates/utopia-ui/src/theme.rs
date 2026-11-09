//! Design tokens, from DESIGN.md §2. Light and dark define the **same** token names
//! (§2.1); there is no third palette and nothing below this module may name a colour, a
//! type size, a radius or a layout dimension of its own.
//!
//! The colour tokens are exactly DESIGN.md §2.1's list (`paper` … `status_progress`). A
//! handful of roles the spec does not cover yet keep their own tokens at the bottom of
//! [`Theme`] (danger, warning, success, the text selection and the PDF overlays), because
//! a call site may not invent a colour (§2's "don't hardcode values outside this list").
//! [`tests`] checks every text/background pair the spec implies at 4.5:1, and that the
//! two non-text greys are never promoted to text.
//!
//! Type (§2.2) is a set of [`TypeRole`]s: size, line height and weight travel together,
//! so a call site cannot pair a 36 px title with a 28 px line box. gpui 0.2.2 has no
//! letter-spacing, and macOS glyph rendering is antialiased by the platform text system,
//! so those two parts of §2.2 are not expressible here (see the phase report).
//!
//! Spatial tokens (§2.3, §2.4) are exported as `f32` points: the [spacing
//! scale](SPACING), the radii, and the content column, sidebar and top bar dimensions.

use gpui::{
    div, prelude::*, px, rgb, transparent_black, App, Div, ElementId, FontWeight, Global, Hsla,
    SharedString, Stateful, WindowAppearance,
};

/// A type role (DESIGN.md §2.2): everything a call site may say about type at once.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct TypeRole {
    pub size: f32,
    pub line: f32,
    pub weight: FontWeight,
}

impl TypeRole {
    /// Apply the role to a `Div`. Most call sites set `text_size` and `line_height` by
    /// hand (the line box often comes from the density setting), so this is a shortcut
    /// for the places that draw a role exactly as written.
    pub fn apply(self, d: Div) -> Div {
        d.text_size(px(self.size))
            .line_height(px(self.line))
            .font_weight(self.weight)
    }
}

const SANS: FontWeight = FontWeight::NORMAL;
const SANS_MEDIUM: FontWeight = FontWeight::MEDIUM;
const SANS_SEMIBOLD: FontWeight = FontWeight::SEMIBOLD;

/// The page title on a page view: 36 / 44, semibold (DESIGN.md §2.2).
pub const PAGE_TITLE: TypeRole = TypeRole {
    size: 36.,
    line: 44.,
    weight: SANS_SEMIBOLD,
};
/// A day's title in the Daily Notes feed: 30 / 38, semibold (§2.2, §3.5).
pub const DAY_TITLE: TypeRole = TypeRole {
    size: 30.,
    line: 38.,
    weight: SANS_SEMIBOLD,
};
/// A section heading (and the top outline heading): 20 / 28, semibold (§2.2).
pub const SECTION: TypeRole = TypeRole {
    size: 20.,
    line: 28.,
    weight: SANS_SEMIBOLD,
};
/// The title of a folded (empty) day row: 17 / 26, semibold (§2.2, §3.6).
pub const EMPTY_DAY_TITLE: TypeRole = TypeRole {
    size: 17.,
    line: 26.,
    weight: SANS_SEMIBOLD,
};
/// Block text in the outline: 16 / 28, regular (§2.2, §3.1).
pub const BODY: TypeRole = TypeRole {
    size: 16.,
    line: 28.,
    weight: SANS,
};
/// Text of a block shown inside the Linked References panel: 15 / 26, regular (§2.2, §3.7).
pub const REFERENCE: TypeRole = TypeRole {
    size: 15.,
    line: 26.,
    weight: SANS,
};
/// Nav, buttons and search: 14 / 22, regular or medium (§2.2).
pub const UI: TypeRole = TypeRole {
    size: 14.,
    line: 22.,
    weight: SANS,
};
/// Meta, footer and property keys: 13 / 20, regular (§2.2).
pub const SMALL: TypeRole = TypeRole {
    size: 13.,
    line: 20.,
    weight: SANS,
};
/// The tighter of the two small sizes: 12 / 18 (§2.2).
pub const SMALL_TIGHT: TypeRole = TypeRole {
    size: 12.,
    line: 18.,
    weight: SANS,
};
/// A mono label ("PAGES", "TAGS", a property group): 12 / 16, uppercase (§2.2).
pub const LABEL: TypeRole = TypeRole {
    size: 12.,
    line: 16.,
    weight: SANS_MEDIUM,
};
/// The sidebar's mono label: 11 / 16 (§2.2).
pub const SIDEBAR_LABEL: TypeRole = TypeRole {
    size: 11.,
    line: 16.,
    weight: SANS_MEDIUM,
};
/// Counts, kbd and weekday labels: mono 12 (§2.2).
pub const COUNT: TypeRole = TypeRole {
    size: 12.,
    line: 16.,
    weight: SANS,
};

/// Every keyword in a typical list, ordered; §2.3 allows only these gaps (4 px base).
pub const SPACING: [f32; 12] = [4., 8., 12., 16., 20., 24., 32., 40., 48., 56., 64., 96.];

/// The content column's widest line: 760 px, centred (§2.3).
pub const CONTENT_MAX_WIDTH: f32 = 760.;
/// Air above the content column (§2.3).
pub const CONTENT_PADDING_TOP: f32 = 48.;
/// Air above a page view, which carries a title (§2.3).
pub const CONTENT_PADDING_TOP_PAGE: f32 = 56.;
/// Air to the left and right of the content column (§2.3).
pub const CONTENT_PADDING_X: f32 = 40.;
/// Air below the last block (§2.3).
pub const CONTENT_PADDING_BOTTOM: f32 = 96.;
/// Space above and below a day section in the feed, with a 1 px `line_soft` divider (§2.3).
pub const DAY_GAP: f32 = 40.;

/// The left sidebar: 264 px wide, never below 240, 12 px padding (§2.3).
pub const SIDEBAR_WIDTH: f32 = 264.;
pub const SIDEBAR_MIN_WIDTH: f32 = 240.;
pub const SIDEBAR_PADDING: f32 = 12.;

/// The top bar: at least 52 px tall, 8 px by 20 px padding (§2.3).
pub const TOP_BAR_MIN_HEIGHT: f32 = 52.;
pub const TOP_BAR_PADDING_Y: f32 = 8.;
pub const TOP_BAR_PADDING_X: f32 = 20.;

/// Radius 4: kbd, ref-type label, small chips, checkboxes (§2.4).
pub const RADIUS_CHIP: f32 = 4.;
/// Radius 5: tag chips (§2.4).
pub const RADIUS_TAG: f32 = 5.;
/// Radius 6: nav items, ghost buttons, property rows (§2.4).
pub const RADIUS_CONTROL: f32 = 6.;
/// Radius 8: inputs, search field, button groups (§2.4).
pub const RADIUS_INPUT: f32 = 8.;
/// Radius 10: panels such as Linked references (§2.4).
pub const RADIUS_PANEL: f32 = 10.;
/// Radius 13: the status pill and count badges, fully round (§2.4).
pub const RADIUS_PILL: f32 = 13.;

/// The line grid a block row follows: 28 px (§3.1).
pub const LINE_GRID: f32 = 28.;

#[derive(Clone, Debug)]
pub struct Theme {
    // ---- DESIGN.md §2.1 ---------------------------------------------------------------
    /// The main writing surface.
    pub paper: Hsla,
    /// The sidebar background, and a property row under the pointer.
    pub shelf: Hsla,
    /// The fill of a reference panel.
    pub well: Hsla,
    /// Ghost button hover, kbd background.
    pub hover: Hsla,
    /// An active sidebar item.
    pub selected: Hsla,
    /// A sidebar item under the pointer.
    pub nav_hover: Hsla,
    /// Borders, thread lines, the sidebar edge.
    pub line: Hsla,
    /// Dividers between days and sections, count badge background.
    pub line_soft: Hsla,
    /// Input, button and chip borders.
    pub line_strong: Hsla,
    /// Primary text.
    pub ink: Hsla,
    /// Nav text and reference text.
    pub ink_2: Hsla,
    /// Secondary text and icons.
    pub ink_3: Hsla,
    /// Labels, placeholders, counts.
    pub muted: Hsla,
    /// Bullets and carets only. Never text (§2.1).
    pub bullet: Hsla,
    /// Disabled icon strokes and the offline dot. Never text (§2.1).
    pub disabled: Hsla,
    /// Page links, date links, focus, today.
    pub link: Hsla,
    /// Link hover and pressed.
    pub link_hover: Hsla,
    /// The "Today" pill's fill (`link` at 10%, solid).
    pub link_tint: Hsla,
    /// Tag chip and avatar fill.
    pub tag_bg: Hsla,
    /// Tag chip text and avatar initials.
    pub tag_ink: Hsla,
    /// The "in progress" status dot; the only status colour so far.
    pub status_progress: Hsla,

    // ---- roles DESIGN.md does not define yet -------------------------------------------
    /// The text selection fill behind a highlighted range.
    pub selection: Hsla,
    /// A code block's card fill.
    pub code_bg: Hsla,
    /// The sync banner's fill.
    pub banner_bg: Hsla,
    /// A search hit or a find-in-page highlight.
    pub highlight_bg: Hsla,
    pub danger_bg: Hsla,
    pub danger: Hsla,
    pub warning: Hsla,
    /// A healthy state that is not a link: the in-sync dot (SB-4).
    pub success: Hsla,
    /// The dimming layer under a modal palette.
    pub scrim: Hsla,
    /// PDF highlight overlays (independent of light/dark paper).
    pub pdf_highlight: Hsla,
    pub pdf_selection: Hsla,
}

impl Global for Theme {}

impl Theme {
    pub fn light() -> Self {
        Self {
            paper: rgb(0xffffff).into(),
            shelf: rgb(0xf6f6f4).into(),
            well: rgb(0xfafaf8).into(),
            hover: rgb(0xf1f1ee).into(),
            selected: rgb(0xe9e9e5).into(),
            nav_hover: rgb(0xededea).into(),
            line: rgb(0xe8e7e3).into(),
            line_soft: rgb(0xefeeea).into(),
            line_strong: rgb(0xe1e0db).into(),
            ink: rgb(0x1b1b19).into(),
            ink_2: rgb(0x3a3a35).into(),
            ink_3: rgb(0x57574f).into(),
            muted: rgb(0x6f6f68).into(),
            bullet: rgb(0x8a8a83).into(),
            disabled: rgb(0xb9b8b1).into(),
            link: rgb(0x2d5bd2).into(),
            link_hover: rgb(0x2249b0).into(),
            link_tint: rgb(0xeaeffb).into(),
            tag_bg: rgb(0xe9ecf4).into(),
            tag_ink: rgb(0x34415f).into(),
            status_progress: rgb(0xc27a12).into(),

            selection: rgb(0xb7d2fb).into(),
            code_bg: rgb(0xf0f0ee).into(),
            banner_bg: rgb(0xeaf2ff).into(),
            highlight_bg: rgb(0xfff1a8).into(),
            danger_bg: rgb(0xfbe4e4).into(),
            danger: rgb(0xb42318).into(),
            warning: rgb(0xd97706).into(),
            success: rgb(0x1f7a3d).into(),
            scrim: gpui::hsla(0., 0., 0., 0.18),
            pdf_highlight: gpui::hsla(0.14, 0.95, 0.55, 0.35),
            pdf_selection: gpui::hsla(0.6, 0.9, 0.55, 0.25),
        }
    }

    /// The dark palette (DESIGN.md §2.1: the same names, dark values, contrast-checked by
    /// [`tests::every_text_pair_reaches_contrast`]). Warm greys, so the paper still reads
    /// as paper at night.
    pub fn dark() -> Self {
        Self {
            paper: rgb(0x1b1b19).into(),
            shelf: rgb(0x151514).into(),
            well: rgb(0x202020).into(),
            hover: rgb(0x262624).into(),
            selected: rgb(0x2c2c29).into(),
            nav_hover: rgb(0x232322).into(),
            line: rgb(0x33332f).into(),
            line_soft: rgb(0x2a2a27).into(),
            line_strong: rgb(0x3d3d38).into(),
            ink: rgb(0xe9e8e3).into(),
            ink_2: rgb(0xc6c5be).into(),
            ink_3: rgb(0xa3a29a).into(),
            muted: rgb(0x8f8e86).into(),
            bullet: rgb(0x6e6d66).into(),
            disabled: rgb(0x55554f).into(),
            link: rgb(0x7fa3f0).into(),
            link_hover: rgb(0x9db9f5).into(),
            link_tint: rgb(0x262f45).into(),
            tag_bg: rgb(0x2a3040).into(),
            tag_ink: rgb(0xc3cee6).into(),
            status_progress: rgb(0xe0a445).into(),

            selection: rgb(0x264f8f).into(),
            code_bg: rgb(0x22221f).into(),
            banner_bg: rgb(0x222e42).into(),
            highlight_bg: rgb(0x4e461f).into(),
            danger_bg: rgb(0x4a2222).into(),
            danger: rgb(0xff9c94).into(),
            warning: rgb(0xf0b45a).into(),
            success: rgb(0x6fd08c).into(),
            scrim: gpui::hsla(0., 0., 0., 0.42),
            pdf_highlight: gpui::hsla(0.14, 0.95, 0.55, 0.35),
            pdf_selection: gpui::hsla(0.6, 0.9, 0.55, 0.25),
        }
    }

    pub fn for_appearance(a: WindowAppearance) -> Self {
        match a {
            WindowAppearance::Dark | WindowAppearance::VibrantDark => Self::dark(),
            _ => Self::light(),
        }
    }
}

pub fn theme(cx: &App) -> &Theme {
    cx.global::<Theme>()
}

/// A quiet control (DESIGN.md §3.4): 6 px radius, a 13 px `ink_3` label and one hairline
/// that appears only on hover, so controls read as controls without shouting over the
/// notes. Use [`quiet_menu`] when a menu opens from it.
pub fn quiet_control(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    th: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .py_0p5()
        .rounded(px(RADIUS_CONTROL))
        .border_1()
        .border_color(transparent_black())
        .text_size(px(SMALL.size))
        .text_color(th.ink_3)
        .cursor_pointer()
        .hover(|d| d.bg(th.hover).border_color(th.line).text_color(th.ink))
        .child(label.into())
}

/// A destructive quiet control: "Delete page" in a menu, red at rest and on hover.
pub fn quiet_danger(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    th: &Theme,
) -> Stateful<Div> {
    div()
        .id(id)
        .px_2()
        .py_0p5()
        .rounded(px(RADIUS_CONTROL))
        .border_1()
        .border_color(transparent_black())
        .text_size(px(SMALL.size))
        .text_color(th.danger)
        .cursor_pointer()
        .hover(|d| {
            d.bg(th.danger_bg)
                .border_color(th.line)
                .text_color(th.danger)
        })
        .child(label.into())
}

/// A control that opens a menu; the chevron says so.
pub fn quiet_menu(
    id: impl Into<ElementId>,
    label: impl Into<SharedString>,
    th: &Theme,
) -> Stateful<Div> {
    let label: SharedString = label.into();
    quiet_control(id, format!("{label} ▾"), th)
}

/// The one monospace family for code: Menlo, whatever the notes are set in.
pub const MONO: &str = "Menlo";

/// The mono family for labels, counts, shortcuts and weekday labels (§2.2): the bundled
/// IBM Plex Mono, registered by [`crate::fonts::register_ibm_plex_mono`].
pub const LABEL_MONO: &str = crate::fonts::IBM_PLEX_MONO;

/// The small count that follows a name (LR-2, DESIGN.md §3.4): mono 12 in `ink_3` on a
/// `line_soft` badge, fully round.
pub fn count_chip(count: usize, th: &Theme) -> Div {
    div()
        .flex_none()
        .font_family(LABEL_MONO)
        .text_size(px(COUNT.size))
        .text_color(th.ink_3)
        .bg(th.line_soft)
        .px_2()
        .py(px(1.))
        .rounded_full()
        .child(count.to_string())
}

/// A control's label size: the UI role's size (rows size their own line box).
pub const CONTROL_SIZE: f32 = UI.size;

/// The one type scale (DESIGN.md §2.2): page title down to caption. An outline heading
/// never draws larger than the page title. These are the scalar views of the roles above,
/// for call sites that already take their line height from the row (body) or the density.
pub const TITLE_SIZE: f32 = PAGE_TITLE.size;
pub const H1_SIZE: f32 = SECTION.size;
pub const H2_SIZE: f32 = 18.;
pub const H3_SIZE: f32 = 17.;
pub const BODY_SIZE: f32 = BODY.size;
pub const CAPTION_SIZE: f32 = SMALL_TIGHT.size;
/// Body text and leading at the default (Comfortable) density. `Density` in
/// [`crate::appearance`] picks these or the compact pair.
pub const TEXT_SIZE: f32 = BODY.size;
pub const LINE_HEIGHT: f32 = BODY.line;
pub const INDENT: f32 = 22.;
/// The UI caps visual indentation; deeper blocks show a depth badge (§6.1).
pub const MAX_VISUAL_DEPTH: usize = 12;

/// The point size and line box of an outline heading (PG-3). H1 is the section heading,
/// 20 / 28, and never passes the page title; H2 and H3 step down from it.
pub fn heading_metrics(level: u8) -> (f32, f32) {
    match level {
        1 => (SECTION.size, SECTION.line),
        2 => (H2_SIZE, 26.),
        _ => (H3_SIZE, 24.),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use gpui::Rgba;

    /// DESIGN.md §2.1: both palettes define every token under the same name, and the two
    /// are not the same colours with different names.
    #[test]
    fn every_token_is_named_in_both_palettes() {
        let l = Theme::light();
        let d = Theme::dark();
        let spec = [
            ("paper", l.paper, d.paper),
            ("shelf", l.shelf, d.shelf),
            ("well", l.well, d.well),
            ("hover", l.hover, d.hover),
            ("selected", l.selected, d.selected),
            ("nav_hover", l.nav_hover, d.nav_hover),
            ("line", l.line, d.line),
            ("line_soft", l.line_soft, d.line_soft),
            ("line_strong", l.line_strong, d.line_strong),
            ("ink", l.ink, d.ink),
            ("ink_2", l.ink_2, d.ink_2),
            ("ink_3", l.ink_3, d.ink_3),
            ("muted", l.muted, d.muted),
            ("bullet", l.bullet, d.bullet),
            ("disabled", l.disabled, d.disabled),
            ("link", l.link, d.link),
            ("link_hover", l.link_hover, d.link_hover),
            ("link_tint", l.link_tint, d.link_tint),
            ("tag_bg", l.tag_bg, d.tag_bg),
            ("tag_ink", l.tag_ink, d.tag_ink),
            ("status_progress", l.status_progress, d.status_progress),
        ];
        for (name, light, dark) in spec {
            assert_ne!(light, dark, "a dark theme has its own value for {name}");
        }
        assert_ne!(l.danger, l.link, "danger is not the link colour");
    }

    /// DESIGN.md §2.1's table, hex for hex: a typo in the light palette is a failing test,
    /// not a lighter sidebar. (`link_tint` is the spec's solid rendering of `link` at 10%.)
    #[test]
    fn the_light_palette_is_the_spec_table() {
        let th = Theme::light();
        let table = [
            ("paper", th.paper, 0xffffff),
            ("shelf", th.shelf, 0xf6f6f4),
            ("well", th.well, 0xfafaf8),
            ("hover", th.hover, 0xf1f1ee),
            ("selected", th.selected, 0xe9e9e5),
            ("nav_hover", th.nav_hover, 0xededea),
            ("line", th.line, 0xe8e7e3),
            ("line_soft", th.line_soft, 0xefeeea),
            ("line_strong", th.line_strong, 0xe1e0db),
            ("ink", th.ink, 0x1b1b19),
            ("ink_2", th.ink_2, 0x3a3a35),
            ("ink_3", th.ink_3, 0x57574f),
            ("muted", th.muted, 0x6f6f68),
            ("bullet", th.bullet, 0x8a8a83),
            ("disabled", th.disabled, 0xb9b8b1),
            ("link", th.link, 0x2d5bd2),
            ("link_hover", th.link_hover, 0x2249b0),
            ("link_tint", th.link_tint, 0xeaeffb),
            ("tag_bg", th.tag_bg, 0xe9ecf4),
            ("tag_ink", th.tag_ink, 0x34415f),
            ("status_progress", th.status_progress, 0xc27a12),
        ];
        for (name, colour, hex) in table {
            let rgba = Rgba::from(colour);
            let got = ((rgba.r * 255.).round() as u32) << 16
                | ((rgba.g * 255.).round() as u32) << 8
                | (rgba.b * 255.).round() as u32;
            let channel = |shift: u32| ((got >> shift) & 0xff) as i32;
            let want = |shift: u32| (hex >> shift) & 0xff;
            for shift in [16, 8, 0] {
                assert!(
                    (channel(shift) - want(shift)).abs() <= 1,
                    "{name} is {got:#08x}, the spec says {hex:#08x}"
                );
            }
        }
    }

    /// DESIGN.md §2.1: "Every text and background pair must reach at least 4.5:1". These
    /// are the pairs the spec defines and the app draws: readable text on the three
    /// surfaces, link and tag foregrounds on their own fills, and the foregrounds of the
    /// transient highlight surfaces. `status_progress` is a dot, not a text role.
    #[test]
    fn every_text_pair_reaches_contrast() {
        for (palette, th) in [("light", Theme::light()), ("dark", Theme::dark())] {
            let surfaces = [("paper", th.paper), ("shelf", th.shelf), ("well", th.well)];
            for (fg_name, fg) in [
                ("ink", th.ink),
                ("ink_2", th.ink_2),
                ("ink_3", th.ink_3),
                ("muted", th.muted),
                ("link", th.link),
                ("link_hover", th.link_hover),
            ] {
                for (bg_name, bg) in surfaces {
                    let ratio = contrast(fg, bg);
                    assert!(
                        ratio >= 4.5,
                        "{palette}: {fg_name} on {bg_name} is {ratio:.2}:1, below 4.5:1"
                    );
                }
            }
            for (fg_name, fg, bg_name, bg) in [
                ("tag_ink", th.tag_ink, "tag_bg", th.tag_bg),
                ("link", th.link, "link_tint", th.link_tint),
                ("ink", th.ink, "hover", th.hover),
                ("ink", th.ink, "selected", th.selected),
                ("ink", th.ink, "nav_hover", th.nav_hover),
            ] {
                let ratio = contrast(fg, bg);
                assert!(
                    ratio >= 4.5,
                    "{palette}: {fg_name} on {bg_name} is {ratio:.2}:1, below 4.5:1"
                );
            }
            for (name, c) in [("bullet", th.bullet), ("disabled", th.disabled)] {
                let ratio = contrast(c, th.paper);
                assert!(
                    ratio < 4.5,
                    "{palette}: {name} reaches {ratio:.2}:1 on paper; the spec reserves it for \
                     bullets and disabled strokes, never text"
                );
            }
        }
    }

    /// DESIGN.md §2.1: `bullet` and `disabled` are never text colours. The app draws them
    /// only as fills and strokes; a `text_color` call with either is a bug (carets and
    /// bullets are glyphs in `bullet`, which §3.1 allows, but not prose).
    #[test]
    fn bullet_and_disabled_are_never_text_colours() {
        for (path, src) in crate::sources::ui_sources() {
            for pattern in [
                ["text_color(th.", "disabled)"].concat(),
                ["text_color(theme.", "disabled)"].concat(),
            ] {
                assert!(
                    !src.contains(&pattern),
                    "{} uses {pattern}; disabled is a stroke colour, never text",
                    path.display()
                );
            }
            for (i, line) in src.lines().enumerate() {
                if line.contains(&["text_color(th.", "bullet)"].concat()) {
                    // The only allowed use is a bullet or caret glyph, never a sentence.
                    assert!(
                        line.contains("\"•\"") || line.contains("\"▸\"") || line.contains("\"▾\""),
                        "{}:{}: bullet used as text: {}",
                        path.display(),
                        i + 1,
                        line.trim()
                    );
                }
            }
        }
    }

    /// DESIGN.md §2.4: "No shadows anywhere." And §2: no colour is named outside this
    /// module. Both are checked against the source, so a stray `.shadow_md()` or `rgb()`
    /// fails the build's tests, not a review.
    #[test]
    fn no_shadows_and_no_raw_colours_outside_the_theme() {
        for (path, src) in crate::sources::ui_sources() {
            if path.file_name().and_then(|n| n.to_str()) == Some("theme.rs") {
                continue;
            }
            for (i, line) in src.lines().enumerate() {
                assert!(
                    !line.contains(".shadow_"),
                    "{}:{}: DESIGN.md §2.4 forbids shadows: {}",
                    path.display(),
                    i + 1,
                    line.trim()
                );
                assert!(
                    !line.contains("rgb(0x") && !line.contains("hsla("),
                    "{}:{}: a colour outside theme.rs: {}",
                    path.display(),
                    i + 1,
                    line.trim()
                );
            }
        }
    }

    /// DESIGN.md §2.3: the spacing scale is exactly the allowed values, and the layout
    /// dimensions are the ones the spec states.
    #[test]
    fn spacing_scale_and_frame_match_the_spec() {
        assert_eq!(
            SPACING,
            [4., 8., 12., 16., 20., 24., 32., 40., 48., 56., 64., 96.]
        );
        assert_eq!((CONTENT_MAX_WIDTH, CONTENT_PADDING_X), (760., 40.));
        assert_eq!((CONTENT_PADDING_TOP, CONTENT_PADDING_TOP_PAGE), (48., 56.));
        assert_eq!((SIDEBAR_WIDTH, SIDEBAR_MIN_WIDTH), (264., 240.));
        assert_eq!((TOP_BAR_MIN_HEIGHT, TOP_BAR_PADDING_Y), (52., 8.));
        for gap in [DAY_GAP, CONTENT_PADDING_X, CONTENT_PADDING_TOP] {
            assert!(SPACING.contains(&gap), "{gap} is not on the 4 px scale");
        }
        assert_eq!(
            [
                RADIUS_CHIP,
                RADIUS_TAG,
                RADIUS_CONTROL,
                RADIUS_INPUT,
                RADIUS_PANEL,
                RADIUS_PILL,
            ],
            [4., 5., 6., 8., 10., 13.],
            "DESIGN.md §2.4"
        );
        assert_eq!(
            LINE_GRID, BODY.line,
            "a block row follows the 28 px line grid"
        );
    }

    /// DESIGN.md §2.2: the type roles are the sizes and line boxes in the table, and the
    /// scale descends.
    #[test]
    fn type_roles_match_the_spec() {
        assert_eq!((PAGE_TITLE.size, PAGE_TITLE.line), (36., 44.));
        assert_eq!((DAY_TITLE.size, DAY_TITLE.line), (30., 38.));
        assert_eq!((SECTION.size, SECTION.line), (20., 28.));
        assert_eq!((EMPTY_DAY_TITLE.size, EMPTY_DAY_TITLE.line), (17., 26.));
        assert_eq!((BODY.size, BODY.line), (16., 28.));
        assert_eq!((REFERENCE.size, REFERENCE.line), (15., 26.));
        assert_eq!((UI.size, UI.line), (14., 22.));
        assert_eq!(SMALL.size, 13.);
        assert_eq!(SMALL_TIGHT.size, 12.);
        assert_eq!((LABEL.size, LABEL.line), (12., 16.));
        assert_eq!(SIDEBAR_LABEL.size, 11.);
        assert_eq!(COUNT.size, 12.);

        let scale = [
            PAGE_TITLE.size,
            DAY_TITLE.size,
            SECTION.size,
            EMPTY_DAY_TITLE.size,
            BODY.size,
            REFERENCE.size,
            UI.size,
            SMALL.size,
            SMALL_TIGHT.size,
        ];
        assert!(
            scale.windows(2).all(|w| w[0] > w[1]),
            "the scale descends: {scale:?}"
        );
        for role in [
            PAGE_TITLE,
            DAY_TITLE,
            SECTION,
            EMPTY_DAY_TITLE,
            BODY,
            REFERENCE,
            UI,
            SMALL,
            SMALL_TIGHT,
            LABEL,
            SIDEBAR_LABEL,
            COUNT,
        ] {
            assert!(
                role.line >= role.size,
                "the line box holds the role: {role:?}"
            );
        }
    }

    /// PG-3: the three outline steps, each below the title and above the body.
    #[test]
    fn outline_headings_step_down_from_the_title() {
        assert_eq!((H1_SIZE, H2_SIZE, H3_SIZE), (20., 18., 17.));
        let mut last = TITLE_SIZE;
        for level in 1..=3 {
            let (size, line) = heading_metrics(level);
            assert!(size < last && size > BODY_SIZE, "level {level}: {size}");
            assert!(line >= size, "the line box holds level {level}: {line}");
            last = size;
        }
        assert!(
            heading_metrics(1).1 > heading_metrics(2).1
                && heading_metrics(2).1 > heading_metrics(3).1,
            "each heading takes less vertical room than the one above it"
        );
    }

    /// WCAG relative luminance of an sRGB colour.
    fn luminance(c: Hsla) -> f32 {
        let c = Rgba::from(c);
        let f = |v: f32| {
            if v <= 0.04045 {
                v / 12.92
            } else {
                ((v + 0.055) / 1.055).powf(2.4)
            }
        };
        0.2126 * f(c.r) + 0.7152 * f(c.g) + 0.0722 * f(c.b)
    }

    fn contrast(a: Hsla, b: Hsla) -> f32 {
        let (a, b) = (luminance(a), luminance(b));
        let (hi, lo) = if a > b { (a, b) } else { (b, a) };
        (hi + 0.05) / (lo + 0.05)
    }
}
