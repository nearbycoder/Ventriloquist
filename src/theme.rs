//! The app's look: warm graphite and paper, with one amber accent for whatever makes sound.

use rune::{Color, oklch};
use rune_kit::{Kit, PaletteSeed};

use crate::catalog::Kind;

pub fn kit(dark: bool) -> Kit {
    let seed = if dark {
        PaletteSeed { bg: oklch(0.185, 0.006, 70.0), fg: oklch(0.955, 0.008, 85.0), accent: oklch(0.78, 0.15, 65.0) }
    } else {
        PaletteSeed { bg: oklch(0.985, 0.004, 85.0), fg: oklch(0.22, 0.012, 60.0), accent: oklch(0.64, 0.17, 50.0) }
    };
    let mut k = Kit::from_palette(seed);
    k.radius.md = 8.0;
    k.radius.lg = 12.0;
    k
}

/// The surface cards and clip slots sit on: a step off the page, without the accent's tint.
pub fn card(k: &Kit) -> Color {
    // The kit's raised surface is right in the dark; in the light it picks up the accent.
    if k.dark { k.c.surface_raised } else { k.c.bg_base.mix(k.c.text_base, 0.012) }
}

/// The accent, for marks drawn by hand (waveforms, the record dot's ring).
pub fn accent(k: &Kit) -> Color {
    k.c.text_accent
}

/// Recording, and things that went wrong.
pub fn red(k: &Kit) -> Color {
    if k.dark { oklch(0.7, 0.19, 25.0) } else { oklch(0.56, 0.21, 27.0) }
}

pub fn green(k: &Kit) -> Color {
    if k.dark { oklch(0.76, 0.15, 150.0) } else { oklch(0.56, 0.15, 150.0) }
}

/// Each kind of model has its own tint, so the sidebar reads at a glance.
pub fn kind_hue(kind: Kind) -> f32 {
    match kind {
        Kind::Tts => 65.0,
        Kind::Vc => 300.0,
        Kind::Chat => 195.0,
        Kind::Helper => 140.0,
    }
}

pub fn kind_ink(kind: Kind, k: &Kit) -> Color {
    if k.dark { oklch(0.8, 0.12, kind_hue(kind)) } else { oklch(0.5, 0.14, kind_hue(kind)) }
}

pub fn kind_wash(kind: Kind, k: &Kit) -> Color {
    if k.dark { oklch(0.3, 0.045, kind_hue(kind)) } else { oklch(0.93, 0.04, kind_hue(kind)) }
}
