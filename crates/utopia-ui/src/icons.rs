//! The 16 px line icons of DESIGN.md §2.5, embedded from `assets/icons` so a signed
//! bundle needs no resource lookup and both themes share one drawing.
//!
//! Every file is from Lucide (ISC, `assets/icons/LICENSE`) on its 24-unit grid with a
//! 3-unit stroke: drawn at 16 px that is the spec's 2 px stroke. gpui renders a monochrome
//! SVG as an alpha mask tinted with the element's `text-color`, so a call site says
//! [`icon`] and picks the colour (`ink_3` by default, `link` for an active nav row,
//! `disabled` when there is nothing to do).
//!
//! In tests and any app built without [`Icons`] as its asset source the file is missing;
//! gpui then paints nothing and keeps the element's box, which is why layout tests still
//! measure the controls around an icon.

use std::borrow::Cow;

use gpui::{px, svg, AssetSource, Hsla, Result, SharedString, Styled, Svg};

/// The icon set the spec names. `More` (the ⋯ menu) is the one addition §3.3 needs.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Icon {
    Search,
    Calendar,
    Page,
    Person,
    SidebarLeft,
    SidebarRight,
    ArrowLeft,
    ArrowRight,
    ChevronDown,
    ChevronLeft,
    ChevronRight,
    Plus,
    Filter,
    Star,
    Settings,
    List,
    DashedCircle,
    More,
}

impl Icon {
    pub const ALL: &'static [Icon] = &[
        Icon::Search,
        Icon::Calendar,
        Icon::Page,
        Icon::Person,
        Icon::SidebarLeft,
        Icon::SidebarRight,
        Icon::ArrowLeft,
        Icon::ArrowRight,
        Icon::ChevronDown,
        Icon::ChevronLeft,
        Icon::ChevronRight,
        Icon::Plus,
        Icon::Filter,
        Icon::Star,
        Icon::Settings,
        Icon::List,
        Icon::DashedCircle,
        Icon::More,
    ];

    /// The file name under `assets/icons`.
    pub fn name(self) -> &'static str {
        match self {
            Icon::Search => "search",
            Icon::Calendar => "calendar",
            Icon::Page => "page",
            Icon::Person => "person",
            Icon::SidebarLeft => "sidebar-left",
            Icon::SidebarRight => "sidebar-right",
            Icon::ArrowLeft => "arrow-left",
            Icon::ArrowRight => "arrow-right",
            Icon::ChevronDown => "chevron-down",
            Icon::ChevronLeft => "chevron-left",
            Icon::ChevronRight => "chevron-right",
            Icon::Plus => "plus",
            Icon::Filter => "filter",
            Icon::Star => "star",
            Icon::Settings => "settings",
            Icon::List => "list",
            Icon::DashedCircle => "dashed-circle",
            Icon::More => "more",
        }
    }

    fn bytes(self) -> &'static [u8] {
        macro_rules! icon {
            ($name:literal) => {
                include_bytes!(concat!("../assets/icons/", $name, ".svg")).as_slice()
            };
        }
        match self {
            Icon::Search => icon!("search"),
            Icon::Calendar => icon!("calendar"),
            Icon::Page => icon!("page"),
            Icon::Person => icon!("person"),
            Icon::SidebarLeft => icon!("sidebar-left"),
            Icon::SidebarRight => icon!("sidebar-right"),
            Icon::ArrowLeft => icon!("arrow-left"),
            Icon::ArrowRight => icon!("arrow-right"),
            Icon::ChevronDown => icon!("chevron-down"),
            Icon::ChevronLeft => icon!("chevron-left"),
            Icon::ChevronRight => icon!("chevron-right"),
            Icon::Plus => icon!("plus"),
            Icon::Filter => icon!("filter"),
            Icon::Star => icon!("star"),
            Icon::Settings => icon!("settings"),
            Icon::List => icon!("list"),
            Icon::DashedCircle => icon!("dashed-circle"),
            Icon::More => icon!("more"),
        }
    }

    /// Where [`Icons`] serves it from.
    pub fn path(self) -> String {
        format!("icons/{}.svg", self.name())
    }
}

/// An icon at `size` px, tinted `color`. Callers put it in a sized control (a 32 px ghost
/// button, a 16 px row glyph); the element itself only carries the colour.
pub fn icon(icon: Icon, size: f32, color: Hsla) -> Svg {
    svg()
        .path(icon.path())
        .w(px(size))
        .h(px(size))
        .text_color(color)
}

/// DESIGN.md §2.5's size for a control icon.
pub const ICON_SIZE: f32 = 16.;
/// The panel icons are 18 px with a 1.75 px stroke; Lucide's 3-unit stroke at 18 px is
/// 2.25, so the panel passes its own size and lives with the same rounded drawing.
pub const PANEL_ICON_SIZE: f32 = 18.;

/// The asset source the app installs (`Application::with_assets`) so `svg()` can find
/// these files. A missing path is not an error: gpui draws nothing rather than failing.
pub struct Icons;

impl AssetSource for Icons {
    fn load(&self, path: &str) -> Result<Option<Cow<'static, [u8]>>> {
        let Some(name) = path
            .strip_prefix("icons/")
            .and_then(|p| p.strip_suffix(".svg"))
        else {
            return Ok(None);
        };
        Ok(Icon::ALL
            .iter()
            .find(|i| i.name() == name)
            .map(|i| Cow::Borrowed(i.bytes())))
    }

    fn list(&self, path: &str) -> Result<Vec<SharedString>> {
        if path.trim_end_matches('/') != "icons" {
            return Ok(Vec::new());
        }
        Ok(Icon::ALL
            .iter()
            .map(|i| SharedString::from(i.path()))
            .collect())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Every icon in the set is embedded, is an SVG document, and is served from the path
    /// the element asks for. A missing file would quietly paint nothing at runtime, so the
    /// check belongs in the build's tests.
    #[test]
    fn every_icon_is_embedded() {
        for &i in Icon::ALL {
            let bytes = Icons
                .load(&i.path())
                .expect("asset source")
                .unwrap_or_else(|| panic!("{} is not embedded", i.name()));
            let text = std::str::from_utf8(&bytes).expect("an SVG is UTF-8");
            assert!(
                text.starts_with("<svg") && text.trim_end().ends_with("</svg>"),
                "{} is not an SVG document",
                i.name()
            );
            assert!(
                text.contains("viewBox=\"0 0 24 24\""),
                "{} is not on Lucide's 24-unit grid",
                i.name()
            );
        }
        assert!(Icons.load("icons/nope.svg").unwrap().is_none());
        assert!(Icons.load("fonts/x.otf").unwrap().is_none());
        assert_eq!(Icons.list("icons").unwrap().len(), Icon::ALL.len());
        assert!(Icons.list("nothing").unwrap().is_empty());
    }

    /// The spec's 2 px stroke at 16 px: Lucide's 24-unit grid needs a 3-unit stroke.
    #[test]
    fn stroke_renders_two_pixels_at_sixteen() {
        for &i in Icon::ALL {
            // `more` is a filled glyph, the one exception (the ⋯ dots).
            if i == Icon::More {
                continue;
            }
            let text = std::str::from_utf8(i.bytes()).unwrap();
            assert!(
                text.contains("stroke-width=\"3\""),
                "{} needs a 3-unit stroke to draw 2 px at 16 px",
                i.name()
            );
        }
        assert_eq!(ICON_SIZE * 3. / 24., 2.);
    }
}
