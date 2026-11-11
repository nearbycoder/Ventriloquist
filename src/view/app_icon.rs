//! The app's icon, drawn by the app: `--snap icon.png --scene icon --size 1024x1024 --scale 1`
//! renders it and `scripts/bundle` turns that into the bundle's icon.
//!
//! A V made of waveform bars, amber on graphite. macOS 26 and later draw the tile themselves
//! and want only what goes on it (`--scene icon-glyph`); given a finished tile, they shrink it
//! onto a plate of their own. Older ones want the tile whole, on their icon grid: an 824 point
//! tile on a 1024 point canvas.

use rune::{El, Icon, el, icon, oklch};

const CANVAS: f32 = 1024.0;
const TILE: f32 = 824.0;

/// What `--scene icon`, `--scene icon-glyph` and `--scene installer` draw.
#[derive(Clone, Copy, PartialEq)]
pub enum Art {
    Tile,
    Glyph,
    Installer,
}

/// The installer window, in points: `scripts/dmg` gives the disk image a window this size and
/// puts the app and the Applications folder at these centres.
pub const INSTALLER: (f32, f32) = (660.0, 400.0);
const INSTALLER_ICONS: [(f32, f32); 2] = [(180.0, 190.0), (480.0, 190.0)];

static ARROW: Icon = Icon {
    name: "v-install-arrow",
    view: 16.0,
    body: r#"<path d="M2.5 8h10M8.75 4.25 12.5 8l-3.75 3.75" fill="none" stroke="currentColor" stroke-width="1.5" stroke-linecap="round" stroke-linejoin="round"/>"#,
};

pub fn app_icon(art: Art) -> El {
    let face = match art {
        Art::Installer => return installer(),
        Art::Tile => {
            let inset = (CANVAS - TILE) / 2.0;
            let tile = el()
                .absolute(inset, inset)
                .size(TILE, TILE)
                .radius(186.0)
                .continuous_corners()
                .gradient(oklch(0.31, 0.012, 70.0), oklch(0.17, 0.008, 70.0))
                .border(2.0, oklch(0.42, 0.012, 70.0).alpha(0.6));
            bars(tile, 1.0)
        }
        Art::Glyph => bars(el().absolute(0.0, 0.0).size(CANVAS, CANVAS), CANVAS / TILE),
    };
    el().w_full().h_full().child(face)
}

/// The V, laid out for an 824 point tile and drawn `scale` times that size.
fn bars(mut face: El, scale: f32) -> El {
    const BARS: usize = 9;
    let (bar_w, pitch) = (46.0f32, 74.0f32);
    let first = (TILE - (BARS as f32 - 1.0) * pitch - bar_w) / 2.0;
    for i in 0..BARS {
        // 0 at the middle bar, 1 at the outer ones: the bars' centres trace the V.
        let out = (i as f32 - (BARS as f32 - 1.0) / 2.0).abs() / ((BARS as f32 - 1.0) / 2.0);
        let h = 170.0 + 90.0 * (1.0 - out) + if i % 2 == 1 { 44.0 } else { 0.0 };
        let centre = 560.0 - 300.0 * out;
        let warmth = 0.72 + 0.1 * (1.0 - out);
        face = face.child(
            el().absolute((first + i as f32 * pitch) * scale, (centre - h / 2.0) * scale)
                .size(bar_w * scale, h * scale)
                .radius(bar_w * scale / 2.0)
                .gradient(oklch(warmth + 0.06, 0.15, 72.0), oklch(warmth - 0.04, 0.17, 55.0)),
        );
    }
    face
}

/// What sits behind the two icons in the installer's window: an arrow from the app to the
/// Applications folder. Finder writes the icons' names in black or in white, by the system's
/// appearance and not by this picture, so the ground is a middle grey either can be read on.
fn installer() -> El {
    let (w, h) = INSTALLER;
    let [(from, y), (to, _)] = INSTALLER_ICONS;
    let size = 56.0;
    el().size(w, h)
        .gradient(oklch(0.53, 0.012, 70.0), oklch(0.47, 0.012, 70.0))
        .child(el().absolute((from + to - size) / 2.0, y - size / 2.0).child(icon(&ARROW, size).color(oklch(0.2, 0.008, 70.0).alpha(0.7))))
}
