//! The app icon. `bundle` builds `Contents/Resources/AppIcon.icns` from one square
//! picture, `crates/utopia-app/assets/icon.png`: to change the icon, replace that file.
//!
//! The picture is full-bleed artwork with square corners. macOS does not round an
//! `.icns` itself, so the tile shape, its margin and its shadow are drawn here.

use std::path::Path;

use anyhow::{ensure, Context, Result};
use image::imageops::{self, FilterType};
use image::{GrayImage, Luma, Rgba, Rgba32FImage, RgbaImage};

/// Apple's macOS icon grid: an 824 px tile centred on a 1024 px canvas. The margin holds
/// the shadow and keeps the icon the same size as its neighbours in the Dock.
const CANVAS: u32 = 1024;
const TILE: u32 = 824;
const INSET: u32 = (CANVAS - TILE) / 2;

/// The tile is the superellipse `|x|^n + |y|^n <= 1`, which is close to the system's
/// continuous corners; a rounded rectangle's corners look pinched next to it.
const CORNER_EXPONENT: f32 = 5.0;

/// The tile's shadow: its own shape, moved down and blurred. A paper-white icon has no
/// edge on a light Dock without one.
const SHADOW_DROP: u32 = 10;
const SHADOW_BLUR: f32 = 10.0;
const SHADOW_ALPHA: f32 = 0.3;

/// Every file `iconutil` wants in an `.iconset`, with its side in pixels.
const SIZES: [(u32, &str); 10] = [
    (16, "icon_16x16.png"),
    (32, "icon_16x16@2x.png"),
    (32, "icon_32x32.png"),
    (64, "icon_32x32@2x.png"),
    (128, "icon_128x128.png"),
    (256, "icon_128x128@2x.png"),
    (256, "icon_256x256.png"),
    (512, "icon_256x256@2x.png"),
    (512, "icon_512x512.png"),
    (1024, "icon_512x512@2x.png"),
];

/// How much of the tile's pixel `(x, y)` lies inside the shape, from 4 × 4 samples.
fn coverage(x: u32, y: u32) -> f32 {
    const SAMPLES: u32 = 4;
    let unit = |pixel: u32, sample: u32| {
        (pixel as f32 + (sample as f32 + 0.5) / SAMPLES as f32) / TILE as f32 * 2.0 - 1.0
    };
    let mut inside = 0;
    for sy in 0..SAMPLES {
        for sx in 0..SAMPLES {
            let (u, v) = (unit(x, sx), unit(y, sy));
            if u.abs().powf(CORNER_EXPONENT) + v.abs().powf(CORNER_EXPONENT) <= 1.0 {
                inside += 1;
            }
        }
    }
    inside as f32 / (SAMPLES * SAMPLES) as f32
}

/// The 1024 px icon: the picture cut to the tile shape, over the tile's shadow, on a
/// clear canvas. The picture is taken as opaque.
fn master(art: &RgbaImage) -> RgbaImage {
    let tile = imageops::resize(art, TILE, TILE, FilterType::Lanczos3);
    let mut shape = GrayImage::new(CANVAS, CANVAS);
    let mut shadow = GrayImage::new(CANVAS, CANVAS);
    for y in 0..TILE {
        for x in 0..TILE {
            let c = Luma([(coverage(x, y) * 255.0).round() as u8]);
            shape.put_pixel(x + INSET, y + INSET, c);
            shadow.put_pixel(x + INSET, y + INSET + SHADOW_DROP, c);
        }
    }
    let shadow = imageops::blur(&shadow, SHADOW_BLUR);
    RgbaImage::from_fn(CANVAS, CANVAS, |x, y| {
        let tile_alpha = shape.get_pixel(x, y)[0] as f32 / 255.0;
        let shadow_alpha =
            shadow.get_pixel(x, y)[0] as f32 / 255.0 * SHADOW_ALPHA * (1.0 - tile_alpha);
        let alpha = tile_alpha + shadow_alpha;
        if alpha <= 0.0 {
            return Rgba([0, 0, 0, 0]);
        }
        // The shape is zero outside the tile, so a covered pixel is inside the picture.
        let colour = if tile_alpha > 0.0 {
            tile.get_pixel(x - INSET, y - INSET).0
        } else {
            [0; 4]
        };
        // The shadow is black, so only the picture adds colour.
        let channel = |i: usize| (colour[i] as f32 * tile_alpha / alpha).round() as u8;
        Rgba([
            channel(0),
            channel(1),
            channel(2),
            (alpha * 255.0).round() as u8,
        ])
    })
}

/// The icon at a smaller size. Colours are multiplied by alpha while resizing: averaging
/// the picture's edge with the clear pixels next to it would otherwise darken the rim.
fn scaled(master: &RgbaImage, size: u32) -> RgbaImage {
    let mut premultiplied = Rgba32FImage::new(master.width(), master.height());
    for (x, y, p) in master.enumerate_pixels() {
        let a = p[3] as f32 / 255.0;
        let c = |i: usize| p[i] as f32 / 255.0 * a;
        premultiplied.put_pixel(x, y, Rgba([c(0), c(1), c(2), a]));
    }
    let small = imageops::resize(&premultiplied, size, size, FilterType::Lanczos3);
    RgbaImage::from_fn(size, size, |x, y| {
        let p = small.get_pixel(x, y);
        let a = p[3].clamp(0.0, 1.0);
        if a <= 0.0 {
            return Rgba([0, 0, 0, 0]);
        }
        let c = |i: usize| ((p[i] / a).clamp(0.0, 1.0) * 255.0).round() as u8;
        Rgba([c(0), c(1), c(2), (a * 255.0).round() as u8])
    })
}

/// Write `out` (an `.icns`) from the square picture at `source`. Needs macOS's `iconutil`.
pub fn icns(source: &Path, out: &Path) -> Result<()> {
    let art = image::open(source)
        .with_context(|| format!("reading {}", source.display()))?
        .to_rgba8();
    ensure!(
        art.width() == art.height() && art.width() >= CANVAS,
        "{} is {} × {}: the icon picture must be square and at least {CANVAS} px",
        source.display(),
        art.width(),
        art.height(),
    );
    let master = master(&art);
    let dir = tempfile::tempdir()?;
    let set = dir.path().join("AppIcon.iconset");
    std::fs::create_dir(&set)?;
    for (size, name) in SIZES {
        let icon = if size == CANVAS {
            master.clone()
        } else {
            scaled(&master, size)
        };
        icon.save(set.join(name))?;
    }
    crate::run(
        std::process::Command::new("iconutil")
            .args(["--convert", "icns", "--output"])
            .arg(out)
            .arg(&set),
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    const ART: [u8; 4] = [200, 100, 50, 255];

    fn art() -> RgbaImage {
        RgbaImage::from_pixel(CANVAS, CANVAS, Rgba(ART))
    }

    #[test]
    fn the_picture_fills_the_tile_and_the_corners_are_clear() {
        let icon = master(&art());
        assert_eq!((icon.width(), icon.height()), (CANVAS, CANVAS));
        let middle = CANVAS / 2;
        assert_eq!(icon.get_pixel(middle, middle).0, ART);
        // The tile reaches its box at the middle of each side.
        assert_eq!(icon.get_pixel(middle, INSET + 1).0, ART);
        assert_eq!(icon.get_pixel(INSET + 1, middle).0, ART);
        // The canvas corner, and the corner of the tile's box, which the shape cuts off.
        assert_eq!(icon.get_pixel(0, 0)[3], 0);
        assert_eq!(icon.get_pixel(INSET + 2, INSET + 2)[3], 0);
        assert_eq!(icon.get_pixel(INSET + TILE - 3, INSET + 2)[3], 0);
    }

    #[test]
    fn the_shadow_falls_below_the_tile() {
        let icon = master(&art());
        let middle = CANVAS / 2;
        let below = icon.get_pixel(middle, INSET + TILE + 4);
        let above = icon.get_pixel(middle, INSET - 5);
        assert!(below[3] > 0, "no shadow under the tile");
        assert_eq!(&below.0[..3], &[0, 0, 0], "the shadow is black");
        assert!(
            below[3] > above[3],
            "the shadow is moved down: {} below, {} above",
            below[3],
            above[3]
        );
    }

    #[test]
    fn a_small_size_keeps_the_rim_the_colour_of_the_picture() {
        let white = RgbaImage::from_pixel(CANVAS, CANVAS, Rgba([255; 4]));
        let icon = scaled(&master(&white), 32);
        assert_eq!((icon.width(), icon.height()), (32, 32));
        assert_eq!(icon.get_pixel(16, 16).0, [255; 4]);
        // Down the middle column from the top edge to the centre there is no shadow, so
        // every pixel that shows at all is the picture's white, never a darkened mix.
        for y in 0..16 {
            let p = icon.get_pixel(16, y);
            if p[3] > 8 {
                assert!(p[0] >= 240, "row {y} is darkened: {:?}", p.0);
            }
        }
    }

    #[test]
    fn the_iconset_ends_with_the_full_size() {
        assert_eq!(SIZES.len(), 10);
        assert!(SIZES.iter().all(|(size, _)| *size <= CANVAS));
        assert_eq!(SIZES[9], (CANVAS, "icon_512x512@2x.png"));
    }
}
