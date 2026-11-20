//! Fonts shipped inside the binary, so a font choice works offline and inside a signed
//! bundle with no resource lookup.
//!
//! IBM Plex Sans 1.1.0, from the official release (`@ibm/plex-sans@1.1.0` on
//! github.com/IBM/plex, `fonts/complete/otf`), under the SIL Open Font License 1.1
//! (`assets/fonts/ibm-plex-sans/OFL.txt`). Only the faces the UI asks for are embedded:
//! regular, italic (quotes, `_italic_`), medium (section labels), semibold (titles and
//! headings in DESIGN.md), bold (`**bold**`) and bold italic.
//!
//! IBM Plex Mono 2.5.0 regular and medium (`@ibm/plex-mono@2.5.0`), for DESIGN.md's small
//! labels, counts and shortcuts, under the same licence (`assets/fonts/ibm-plex-mono/OFL.txt`).
//!
//! Geist 1.3.1 and Geist Mono 1.3.1, from Vercel's npm package (`geist@1.3.1`,
//! `dist/fonts/geist-sans` and `dist/fonts/geist-mono`, static TrueType), under the SIL
//! Open Font License 1.1 (`assets/fonts/geist-sans/OFL.txt`, `geist-mono/OFL.txt`). The
//! Geist pair: Sans regular, medium, semibold and bold; Mono regular and medium. The
//! release has no italic faces, so italic text in Geist is drawn upright.
//!
//! Plex Sans SemiBold and the Mono faces come from the npm packages' `fonts/complete/woff`,
//! which hold TrueType-outline fonts; WOFF 1.0 is a zlib wrapper, so they were unwrapped to
//! `.ttf` byte for byte (each matches the `totalSfntSize` its WOFF header records).

use gpui::App;
use std::borrow::Cow;

/// The family name the bundled faces register under; what `font_family` must say.
pub const IBM_PLEX_SANS: &str = "IBM Plex Sans";

/// The family name the bundled mono faces register under.
pub const IBM_PLEX_MONO: &str = "IBM Plex Mono";

/// The family name the bundled Geist Sans faces register under.
pub const GEIST_SANS: &str = "Geist";

/// The family name the bundled Geist Mono faces register under.
pub const GEIST_MONO: &str = "Geist Mono";

macro_rules! face {
    ($dir:literal, $file:literal, $face:literal, $ext:literal) => {
        (
            $face,
            include_bytes!(concat!(
                "../assets/fonts/",
                $dir,
                "/",
                $file,
                "-",
                $face,
                ".",
                $ext
            ))
            .as_slice(),
        )
    };
}

/// Face name and OpenType bytes of every bundled IBM Plex Sans face.
pub const IBM_PLEX_SANS_FACES: &[(&str, &[u8])] = &[
    face!("ibm-plex-sans", "IBMPlexSans", "Regular", "otf"),
    face!("ibm-plex-sans", "IBMPlexSans", "Italic", "otf"),
    face!("ibm-plex-sans", "IBMPlexSans", "Medium", "otf"),
    face!("ibm-plex-sans", "IBMPlexSans", "SemiBold", "ttf"),
    face!("ibm-plex-sans", "IBMPlexSans", "Bold", "otf"),
    face!("ibm-plex-sans", "IBMPlexSans", "BoldItalic", "otf"),
];

/// Face name and OpenType bytes of every bundled IBM Plex Mono face.
pub const IBM_PLEX_MONO_FACES: &[(&str, &[u8])] = &[
    face!("ibm-plex-mono", "IBMPlexMono", "Regular", "ttf"),
    face!("ibm-plex-mono", "IBMPlexMono", "Medium", "ttf"),
];

/// Face name and TrueType bytes of every bundled Geist Sans face.
pub const GEIST_SANS_FACES: &[(&str, &[u8])] = &[
    face!("geist-sans", "Geist", "Regular", "ttf"),
    face!("geist-sans", "Geist", "Medium", "ttf"),
    face!("geist-sans", "Geist", "SemiBold", "ttf"),
    face!("geist-sans", "Geist", "Bold", "ttf"),
];

/// Face name and TrueType bytes of every bundled Geist Mono face.
pub const GEIST_MONO_FACES: &[(&str, &[u8])] = &[
    face!("geist-mono", "GeistMono", "Regular", "ttf"),
    face!("geist-mono", "GeistMono", "Medium", "ttf"),
];

/// Hand the bundled Geist and Geist Mono faces to gpui's text system, together: the pair
/// is chosen as one (see [`register_ibm_plex_sans`]).
pub fn register_geist(cx: &App) {
    register(cx, GEIST_SANS_FACES, GEIST_SANS);
    register(cx, GEIST_MONO_FACES, GEIST_MONO);
}

/// Hand the bundled IBM Plex Sans faces to gpui's text system. Done on first use rather
/// than at startup, so the default (System) costs nothing at launch. A font gpui cannot
/// load is logged, and text falls back to the system font (rule 11).
pub fn register_ibm_plex_sans(cx: &App) {
    register(cx, IBM_PLEX_SANS_FACES, IBM_PLEX_SANS);
}

/// Hand the bundled IBM Plex Mono faces to gpui's text system (see
/// [`register_ibm_plex_sans`]).
pub fn register_ibm_plex_mono(cx: &App) {
    register(cx, IBM_PLEX_MONO_FACES, IBM_PLEX_MONO);
}

fn register(cx: &App, faces: &[(&str, &'static [u8])], family: &str) {
    let faces = faces
        .iter()
        .map(|(_, bytes)| Cow::Borrowed(*bytes))
        .collect();
    if let Err(e) = cx.text_system().add_fonts(faces) {
        eprintln!("utopia: could not load the bundled {family}: {e}");
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    use super::*;
    use std::sync::Arc;
    use zed_font_kit::font::Font;
    use zed_font_kit::properties::{Style, Weight};

    /// The embedded bytes are the fonts we think they are: each parses, belongs to the
    /// family the setting asks gpui for, and has the weight and slant its name says.
    #[test]
    fn bundled_faces_are_ibm_plex_sans() {
        check(IBM_PLEX_SANS_FACES, IBM_PLEX_SANS);
    }

    #[test]
    fn bundled_mono_faces_are_ibm_plex_mono() {
        check(IBM_PLEX_MONO_FACES, IBM_PLEX_MONO);
    }

    #[test]
    fn bundled_geist_faces_are_geist() {
        check(GEIST_SANS_FACES, GEIST_SANS);
        check(GEIST_MONO_FACES, GEIST_MONO);
    }

    fn check(faces: &[(&str, &[u8])], family: &str) {
        for (face, bytes) in faces {
            let font = Font::from_bytes(Arc::new(bytes.to_vec()), 0)
                .unwrap_or_else(|e| panic!("{face} does not parse: {e:?}"));
            assert_eq!(font.family_name(), family, "{face}");
            let p = font.properties();
            let weight = match *face {
                "Medium" => Weight::MEDIUM,
                "SemiBold" => Weight::SEMIBOLD,
                f if f.starts_with("Bold") => Weight::BOLD,
                _ => Weight::NORMAL,
            };
            assert_eq!(p.weight, weight, "{face}");
            let italic = face.ends_with("Italic");
            assert_eq!(p.style == Style::Italic, italic, "{face}");
        }
    }
}
