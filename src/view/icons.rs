//! The app's own icons, drawn on a 16-unit grid in the stroke weight of rune-kit's glyphs.

use rune::Icon;

pub static PLAY: Icon = Icon {
    name: "v-play",
    view: 16.0,
    body: r#"<path d="M5.25 3.6v8.8a.5.5 0 0 0 .76.43l7.1-4.4a.5.5 0 0 0 0-.86l-7.1-4.4a.5.5 0 0 0-.76.43Z" fill="currentColor"/>"#,
};

pub static PAUSE: Icon = Icon {
    name: "v-pause",
    view: 16.0,
    body: r#"<rect x="4" y="3.25" width="2.75" height="9.5" rx="0.8" fill="currentColor"/><rect x="9.25" y="3.25" width="2.75" height="9.5" rx="0.8" fill="currentColor"/>"#,
};

pub static STOP: Icon = Icon {
    name: "v-stop",
    view: 16.0,
    body: r#"<rect x="4" y="4" width="8" height="8" rx="1.6" fill="currentColor"/>"#,
};

pub static RECORD: Icon = Icon { name: "v-record", view: 16.0, body: r#"<circle cx="8" cy="8" r="4.25" fill="currentColor"/>"# };

pub static TRASH: Icon = Icon {
    name: "v-trash",
    view: 16.0,
    body: r#"<path d="M3 4.5h10M6.5 4.5V3.25h3V4.5M4.5 4.5l.5 8.25h6l.5-8.25M6.75 7v3.5M9.25 7v3.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

pub static REVEAL: Icon = Icon {
    name: "v-reveal",
    view: 16.0,
    body: r#"<path d="M2.5 4.25c0-.55.45-1 1-1h2.6l1.4 1.5h5c.55 0 1 .45 1 1v6c0 .55-.45 1-1 1h-9c-.55 0-1-.45-1-1v-7.5Z" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" fill="none"/>"#,
};

pub static OPEN_FILE: Icon = Icon {
    name: "v-open-file",
    view: 16.0,
    body: r#"<path d="M8 10.25V3M5.25 5.5 8 2.75l2.75 2.75M3 9.75v2.5c0 .55.45 1 1 1h8c.55 0 1-.45 1-1v-2.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

pub static DOWNLOAD: Icon = Icon {
    name: "v-download",
    view: 16.0,
    body: r#"<path d="M8 2.75V10M5.25 7.5 8 10.25l2.75-2.75M3 9.75v2.5c0 .55.45 1 1 1h8c.55 0 1-.45 1-1v-2.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// A voice to imitate: a head and shoulders.
pub static VOICE: Icon = Icon {
    name: "v-voice",
    view: 16.0,
    body: r#"<circle cx="8" cy="5.5" r="2.5" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M3.25 13c.5-2.3 2.4-3.5 4.75-3.5s4.25 1.2 4.75 3.5" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" fill="none"/>"#,
};

/// Speech going in: a small waveform.
pub static WAVE: Icon = Icon {
    name: "v-wave",
    view: 16.0,
    body: r#"<path d="M2.5 7v2M5.25 5v6M8 3v10M10.75 5.5v5M13.5 7v2" stroke="currentColor" stroke-width="1.3" stroke-linecap="round" fill="none"/>"#,
};

pub static WARNING: Icon = Icon {
    name: "v-warning",
    view: 16.0,
    body: r#"<path d="M8 2.75 14 13H2L8 2.75Z" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" fill="none"/><path d="M8 6.75v3" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/><circle cx="8" cy="11.4" r=".7" fill="currentColor"/>"#,
};

pub static COMPARE: Icon = Icon {
    name: "v-compare",
    view: 16.0,
    body: r#"<path d="M3 4h10M3 8h10M3 12h10" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" fill="none"/><circle cx="5.5" cy="4" r="1.5" fill="currentColor"/><circle cx="10.5" cy="8" r="1.5" fill="currentColor"/><circle cx="7" cy="12" r="1.5" fill="currentColor"/>"#,
};

/// The catalog: a shelf of boxes.
pub static SHELF: Icon = Icon {
    name: "v-shelf",
    view: 16.0,
    body: r#"<rect x="2.5" y="2.75" width="4.5" height="4.5" rx="1" stroke="currentColor" stroke-width="1.2" fill="none"/><rect x="9" y="2.75" width="4.5" height="4.5" rx="1" stroke="currentColor" stroke-width="1.2" fill="none"/><rect x="2.5" y="9" width="4.5" height="4.5" rx="1" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M11.25 9.25v4M9.25 11.25h4" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};

/// Standing instructions: a note with lines of writing on it.
pub static NOTE: Icon = Icon {
    name: "v-note",
    view: 16.0,
    body: r#"<rect x="3" y="2.5" width="10" height="11" rx="1.6" stroke="currentColor" stroke-width="1.2" fill="none"/><path d="M5.5 5.75h5M5.5 8h5M5.5 10.25h3" stroke="currentColor" stroke-width="1.2" stroke-linecap="round"/>"#,
};

/// A scene: two speech bubbles.
pub static SCENE: Icon = Icon {
    name: "v-scene",
    view: 16.0,
    body: r#"<path d="M2.5 3.75c0-.55.45-1 1-1h5.5c.55 0 1 .45 1 1v3.5c0 .55-.45 1-1 1H5.75L3.75 10V8.25H3.5c-.55 0-1-.45-1-1v-3.5Z" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" fill="none"/><path d="M12 6h.5c.55 0 1 .45 1 1v3.25c0 .55-.45 1-1 1h-.25V13l-2-1.75H7.75" stroke="currentColor" stroke-width="1.2" stroke-linecap="round" stroke-linejoin="round" fill="none"/>"#,
};

/// A tool a model called: a wrench.
pub static TOOL: Icon = Icon {
    name: "v-tool",
    view: 16.0,
    body: r#"<path d="M10.4 2.6a3.4 3.4 0 0 0-3.2 4.5L2.9 11.4a1.2 1.2 0 0 0 1.7 1.7l4.3-4.3a3.4 3.4 0 0 0 4.4-4.1l-2 2-1.7-.4-.4-1.7 2-2a3.4 3.4 0 0 0-.8 0Z" stroke="currentColor" stroke-width="1.2" stroke-linejoin="round" fill="none"/>"#,
};
