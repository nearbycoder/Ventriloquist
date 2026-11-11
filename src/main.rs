//! Ventriloquist: a native macOS bench for speech models that run on this Mac. Type a line or
//! record a clip, pick a model, and compare what comes out and how long it took.
//!
//! The window is Rune (github.com/anomalyco/rune); the models run in Python workers the app
//! starts on demand (see `sidecar/`).

mod catalog;
mod engine;
mod paths;
mod scenes;
mod state;
mod theme;
mod updates;
mod view;
mod wav;

use rune::{AppSpec, Chrome, KeyEvent, MenuItem, Tx, Waker, WindowOptions};
use rune_kit::shell;

use state::App;

fn init() -> App {
    // A headless run (a snapshot, a script) starts no processes and writes nothing.
    App::new(rune::tooling::headless_run().is_none())
}

fn setup(s: &mut App, waker: Waker) {
    s.engine.set_waker(waker.clone());
    s.waker = Some(waker);
    if rune::tooling::headless_run().is_none() {
        updates::start();
    }
}

fn poll(s: &mut App, _clock: f64, tx: &mut Tx) -> bool {
    s.poll(tx)
}

fn on_key(s: &mut App, ev: &KeyEvent, _: &mut Tx) -> bool {
    if shell::is_sidebar_toggle(ev) {
        s.sidebar.toggle();
        return true;
    }
    if state::is_run_key(ev) {
        s.generate();
        return true;
    }
    if ev.is_command('.') && s.active.is_some() {
        s.cancel();
        return true;
    }
    false
}

fn on_quit(s: &mut App, _: &mut Tx) {
    // So the next launch opens on the same text, clips and model.
    s.save_settings();
    s.engine.stop_all();
}

fn main() {
    rune::prewarm_gpu();
    rune_mac::launch();
    let size = rune_mac::saved_size("main").unwrap_or((1120.0, 760.0));
    let spec = AppSpec {
        min_size: Some((820.0, 560.0)),
        scenes: scenes::SCENES,
        families: ("Inter", "ui-monospace"),
        setup: Some(setup),
        poll: Some(poll),
        on_key: Some(on_key),
        ..AppSpec::new("Ventriloquist", size, init, view::view)
    };
    // The traffic lights sit centred in the toolbar band of the framed pane.
    let chrome = Chrome { full_size_content: true, traffic_lights: Some((16.0, 25.0)), drag_regions: true };
    // The icon scenes are rendered for the bundle's icon, so what they don't draw stays clear.
    let icon = std::env::args().skip_while(|arg| arg != "--scene").nth(1).is_some_and(|scene| scene.starts_with("icon"));
    let options = WindowOptions { transparent: icon, ..WindowOptions::default() };
    let mut app = rune::App::new(spec).chrome(chrome).window_options(options).standard_menus().on_quit(on_quit);
    if updates::available() {
        app = app.menu_item(MenuItem::new("Check for Updates…", |_: &mut App, _: &mut Tx| updates::check()));
    }
    app.run();
}
