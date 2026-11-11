//! The window: models down the side, the selected model's bench in the middle.

pub mod app_icon;
mod composer;
mod icons;
mod models;
mod takes;
mod voices;

use rune::{Color, Cx, El, Icon, col, el, icon, row, text};
use rune_kit::shell::{PaneMotion, Sidebar, SidebarRow, SidebarSection, SidebarToggle, SplitView, Toolbar};
use rune_kit::{Badge, BadgeVariant, Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, Loader, glyphs};

use crate::catalog::{Cloning, Kind, Model};
use crate::state::{App, Install, Page};
use crate::theme;

pub fn view(s: &App, cx: &mut Cx<App>) -> El {
    if let Some(art) = s.icon {
        return app_icon::app_icon(art);
    }
    let k = theme::kit(cx.system_dark());
    let mut layer = Vec::new();
    let (main, title) = match s.page {
        Page::Bench => (main_pane(s, &k, &mut layer), s.model().kind.title()),
        Page::Models => (models::models(s, &k), "Models"),
    };
    let toolbar = Toolbar::new().title(title).trailing(engines(s, &k));
    let shell = SplitView::new(main)
        .sidebar(&s.sidebar, |s: &mut App| &mut s.sidebar, sidebar(s, &k))
        .toolbar(toolbar)
        .titlebar_inset(cx.titlebar_inset())
        .frame_inset(8.0)
        .toolbar_in_panel()
        .sidebar_padding(14.0, 8.0)
        .sidebar_toggle(SidebarToggle::new().size(ControlSize::Large).past_lights(16.0).edge(18.0).gap(4.0).in_header())
        .pane_motion(PaneMotion::new().fade_tail(0.15))
        .reduced_motion(cx.is_reduced_motion())
        .build(&k);
    rune_kit::root(shell, layer)
}

/// A rounded tile with the model's initial, tinted by what kind of model it is.
fn avatar(m: &Model, size: f32, k: &Kit) -> El {
    let initial = m.name.chars().next().unwrap_or('?').to_ascii_uppercase().to_string();
    let font = if size > 24.0 { k.ty.body_medium } else { k.ty.small_medium };
    row()
        .size(size, size)
        .radius(size * 0.28)
        .center()
        .fill(theme::kind_wash(m.kind, k))
        .child(text(initial).font(font).color(theme::kind_ink(m.kind, k)))
}

fn dot(color: Color) -> El {
    el().size(7.0, 7.0).radius(3.5).fill(color)
}

fn sidebar(s: &App, k: &Kit) -> El {
    let running = s.active.and_then(|id| s.takes.iter().find(|t| t.id == id)).map(|t| t.model.as_str());
    let mut sb = Sidebar::new().label("Models").inset(0.0).row_gap(2.0);
    for kind in Kind::ALL {
        let rows: Vec<El> = s
            .catalog
            .models
            .iter()
            .enumerate()
            // Only what is on this Mac, or on its way; the rest live on the Models page.
            .filter(|(i, m)| m.kind == kind && s.install[*i].is_listed())
            .map(|(i, m)| {
                let status = match &s.install[i] {
                    _ if running == Some(m.id.as_str()) => Loader::new().size(12.0).build(k),
                    Install::Installing { .. } => Loader::new().size(12.0).build(k).help("Downloading"),
                    _ => dot(theme::green(k)).help("Downloaded"),
                };
                SidebarRow::new(m.name.clone())
                    .height(44.0)
                    .avatar(avatar(m, 20.0, k))
                    // The maker and when it came out, so newer models stand out down the list.
                    .subtitle(format!("{} · {}", m.maker, m.released_label()))
                    .selected(s.page == Page::Bench && s.selected == i)
                    .trailing(row().w(16.0).center().child(status))
                    .on_select(move |s: &mut App, _| s.select(i))
                    .build(k)
                    .key(format!("model-{}", m.id))
            })
            .collect();
        if !rows.is_empty() {
            sb = sb.section(SidebarSection::new(kind.title()).key(format!("kind-{}", kind.title())).rows(rows));
        }
    }
    // The way to everything else: download more, or remove what isn't wanted.
    let have = s.downloaded();
    sb = sb.footer(
        SidebarRow::new("Models")
            .height(36.0)
            .icon(&icons::SHELF)
            .detail(format!("{have} of {}", s.catalog.models.len()))
            .selected(s.page == Page::Models)
            .on_select(|s: &mut App, _| s.show_models())
            .build(k)
            .key("manage-models"),
    );
    sb.build(k)
}

/// The toolbar's trailing end: how many engines hold models in memory, and a way to let go.
fn engines(s: &App, k: &Kit) -> El {
    let n = s.engine.running();
    if n == 0 {
        return row();
    }
    let label = if n == 1 { "1 engine loaded".to_owned() } else { format!("{n} engines loaded") };
    row()
        .items_center()
        .gap(8.0)
        .child(text(label).font(k.ty.small).color(k.c.text_faint))
        .child(
            Button::new("Unload")
                .variant(ButtonVariant::Ghost)
                .control_size(ControlSize::Small)
                .help("Quit the Python engines and free the memory their models hold.")
                .on_press(|s: &mut App| s.stop_engines())
                .build(k)
                .key("unload"),
        )
}

fn fact(label: impl Into<String>, k: &Kit) -> El {
    Badge::new(label).variant(BadgeVariant::Subtle).build(k)
}

fn header(s: &App, k: &Kit) -> El {
    let (c, m) = (&k.c, s.model());
    let mut facts = row().items_center().gap(6.0).flex_wrap();
    facts = facts.child(fact(m.license.clone(), k).help("Licence of the weights"));
    if !m.caution.is_empty() {
        facts = facts.child(Badge::new(m.caution.clone()).variant(BadgeVariant::Accent).build(k));
    }
    facts = facts.child(fact(format!("Released {}", m.released_label()), k).help("When the weights were first published"));
    facts = facts.child(fact(m.size.clone(), k).help("Download size"));
    if !m.languages.is_empty() {
        facts = facts.child(fact(m.languages.clone(), k));
    }
    match (m.kind, m.cloning) {
        (Kind::Tts, Cloning::Optional | Cloning::Required) => facts = facts.child(fact("Voice cloning", k)),
        (Kind::Tts, Cloning::None) if !m.voices.is_empty() => facts = facts.child(fact(format!("{} voices", m.voices.len()), k)),
        _ => {}
    }
    row().w_full().gap(14.0).child(avatar(m, 40.0, k)).child(
        col()
            .grow(1.0)
            .min_w(0.0)
            .gap(6.0)
            .child(
                row()
                    .items_center()
                    .gap(8.0)
                    .child(text(m.name.clone()).key("model-name").font(k.ty.body_medium).color(c.text_base))
                    .child(text(m.maker.clone()).font(k.ty.body).color(c.text_faint)),
            )
            .child(text(m.summary.clone()).font(k.ty.body).color(c.text_muted).wrap())
            .child(facts),
    )
}

fn notice(message: &str, k: &Kit) -> El {
    row()
        .key("notice")
        .w_full()
        .items_center()
        .gap(10.0)
        .padding(10.0)
        .radius(k.radius.md)
        .fill(k.c.warning.bg)
        .border(1.0, k.c.warning.border)
        .child(icon(&icons::WARNING, 16.0).color(k.c.warning.icon))
        .child(text(message.to_owned()).font(k.ty.ui).color(k.c.warning.fg).wrap().grow(1.0).selectable(true))
        .child(
            IconButton::new(&glyphs::CLOSE)
                .variant(IconButtonVariant::Ghost)
                .label("Dismiss")
                .on_press(|s: &mut App| s.notice = None)
                .build(k)
                .key("notice-close"),
        )
}

/// What stands where the composer will be until the model is on disk.
fn install_panel(s: &App, k: &Kit) -> El {
    let (c, m, index) = (&k.c, s.model(), s.selected);
    let panel = col().key("install").w_full().gap(12.0).padding(16.0).radius(k.radius.lg).fill(theme::card(k)).border(1.0, c.border_weak);
    let log_view = |log: &[String]| {
        let tail = log[log.len().saturating_sub(12)..].join("\n");
        col().w_full().padding(10.0).radius(k.radius.md).fill(c.bg_deep).child(text(tail).font(k.ty.mono).color(c.text_muted).wrap().selectable(true))
    };
    match &s.install[index] {
        Install::Installing { log } => {
            let last = log.last().cloned().unwrap_or_else(|| "Starting".into());
            let mut p = panel.child(
                row()
                    .w_full()
                    .items_center()
                    .gap(10.0)
                    .child(Loader::new().build(k))
                    .child(
                        col()
                            .grow(1.0)
                            .min_w(0.0)
                            .gap(2.0)
                            .child(text(format!("Installing {}", m.name)).font(k.ty.ui_medium).color(c.text_base))
                            .child(text(last).key("install-line").font(k.ty.small).color(c.text_faint).truncate()),
                    )
                    .child(
                        Button::new(if s.show_log { "Hide log" } else { "Show log" })
                            .variant(ButtonVariant::Ghost)
                            .on_press(|s: &mut App| s.show_log = !s.show_log)
                            .build(k)
                            .key("log-toggle"),
                    ),
            );
            if s.show_log && !log.is_empty() {
                p = p.child(log_view(log));
            }
            p
        }
        Install::Failed { log } => panel
            .child(
                row()
                    .w_full()
                    .items_center()
                    .gap(10.0)
                    .child(icon(&icons::WARNING, 16.0).color(theme::red(k)))
                    .child(text(format!("{} didn't install", m.name)).font(k.ty.ui_medium).color(c.text_base).grow(1.0))
                    .child(Button::new("Try again").variant(ButtonVariant::Neutral).on_press(move |s: &mut App| s.install(index)).build(k).key("retry")),
            )
            .child(log_view(log)),
        _ => panel.child(
            row()
                .w_full()
                .items_center()
                .gap(12.0)
                .child(
                    col()
                        .grow(1.0)
                        .min_w(0.0)
                        .gap(2.0)
                        .child(text(format!("{} isn't installed yet", m.name)).font(k.ty.ui_medium).color(c.text_base))
                        .child(
                            text(format!("Installing builds its Python environment and downloads {} of weights. Everything runs on this Mac.", m.size))
                                .font(k.ty.small)
                                .color(c.text_faint)
                                .wrap(),
                        ),
                )
                .child(
                    Button::new("Install")
                        .variant(ButtonVariant::Contrast)
                        .icon(&icons::DOWNLOAD)
                        .on_press(move |s: &mut App| s.install(index))
                        .build(k)
                        .key("install-button"),
                ),
        ),
    }
}

fn main_pane(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let mut top = col().w_full().gap(16.0).padding(20.0).pb(12.0);
    if let Some(message) = &s.notice {
        top = top.child(notice(message, k));
    }
    top = top.child(header(s, k));
    if !s.model().note.is_empty() {
        top = top.child(text(s.model().note.clone()).font(k.ty.small).color(k.c.text_faint).wrap());
    }
    top = top.child(if s.install[s.selected].is_ready() { composer::composer(s, k, layer) } else { install_panel(s, k) });
    col().key("bench").w_full().h_full().min_h(0.0).child(top).child(takes::takes(s, k))
}

/// An icon button without a frame, for a row's quiet actions.
fn quiet(i: &'static Icon, label: &str, k: &Kit) -> IconButton {
    let _ = k;
    IconButton::new(i).variant(IconButtonVariant::GhostMuted).help(label.to_owned())
}

#[cfg(test)]
mod tests {
    use rune::{Env, Runtime};

    use super::*;
    use crate::catalog::Kind;
    use crate::state::TakeStatus;

    fn window(app: App) -> Runtime<App> {
        let mut rt = Runtime::new(app, view, (1120.0, 760.0), Env { system_dark: true, scale: 2.0 });
        rt.frame(0.0);
        rt
    }

    fn click(rt: &mut Runtime<App>, key: &str) {
        let (x, y) = rt.find(key).unwrap_or_else(|| panic!("no element keyed {key}"));
        rt.pointer_down(x, y);
        rt.pointer_up(x, y);
        rt.frame(1.0 / 60.0);
    }

    fn ready() -> App {
        let mut app = App::new(false);
        app.install.iter_mut().for_each(|s| *s = Install::Ready);
        app.page = Page::Bench;
        app
    }

    #[test]
    fn a_new_install_opens_on_the_models_page_and_downloads_only_what_is_picked() {
        let mut rt = window(App::new(false));
        assert!(rt.find("models-page").is_some() && rt.find("go").is_none(), "nothing to run yet");
        let (first, second) = (rt.state.catalog.models[0].id.clone(), rt.state.catalog.models[1].id.clone());
        assert!(rt.find(&format!("model-{first}")).is_none(), "the sidebar lists no model that isn't downloaded");
        click(&mut rt, &format!("download-{first}"));
        assert!(matches!(rt.state.install[0], Install::Installing { .. }));
        assert!(matches!(rt.state.install[1], Install::Missing), "the others stay where they were");
        assert!(rt.find(&format!("progress-{first}")).is_some(), "the row shows what the download is doing");
        assert!(rt.find(&format!("model-{first}")).is_some(), "and the model joins the sidebar");
        assert!(rt.find(&format!("model-{second}")).is_none());
        click(&mut rt, &format!("cancel-{first}"));
        assert!(matches!(rt.state.install[0], Install::Missing));
    }

    #[test]
    fn removing_a_model_asks_first_and_takes_it_out_of_the_sidebar() {
        let mut rt = window(ready());
        let id = rt.state.catalog.models[1].id.clone();
        click(&mut rt, "manage-models");
        assert_eq!(rt.state.page, Page::Models);
        click(&mut rt, &format!("remove-{id}"));
        assert!(rt.state.install[1].is_ready(), "one press only asks");
        click(&mut rt, &format!("keep-{id}"));
        assert!(rt.find(&format!("confirm-remove-{id}")).is_none());
        click(&mut rt, &format!("remove-{id}"));
        click(&mut rt, &format!("confirm-remove-{id}"));
        assert!(matches!(rt.state.install[1], Install::Missing));
        assert!(rt.find(&format!("model-{id}")).is_none(), "gone from the sidebar");
        assert!(rt.find(&format!("download-{id}")).is_some(), "and offered for download again");
    }

    #[test]
    fn picking_a_model_in_the_sidebar_shows_its_bench() {
        // With only a few models downloaded, so that every row is on screen to press.
        let mut app = App::new(false);
        let vc = app.catalog.models.iter().position(|m| m.kind == Kind::Vc).unwrap();
        for i in [0, 1, vc] {
            app.install[i] = Install::Ready;
        }
        app.page = Page::Bench;
        let mut rt = window(app);
        assert!(rt.find("text").is_some(), "a text-to-speech bench to begin with");
        let key = format!("model-{}", rt.state.catalog.models[vc].id);
        click(&mut rt, &key);
        assert_eq!(rt.state.selected, vc);
        assert!(rt.find("slot-source").is_some() && rt.find("slot-reference").is_some(), "conversion takes two clips");
        for _ in 0..40 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.find("text").is_none(), "and no text");
    }

    #[test]
    fn the_run_button_queues_a_take_and_compare_queues_one_per_model() {
        let mut rt = window(ready());
        click(&mut rt, "go");
        assert_eq!(rt.state.takes.len(), 1);
        assert!(matches!(rt.state.takes[0].status, TakeStatus::Running(_)));
        assert!(rt.find("take-1").is_some(), "the take shows up in the list");
        let runnable = rt.state.runnable();
        assert!(runnable > 1);
        click(&mut rt, "compare");
        assert_eq!(rt.state.takes.len(), 1 + runnable);
        // Cancel springs in beside the run button; press it where it comes to rest.
        for _ in 0..30 {
            rt.frame(1.0 / 60.0);
        }
        click(&mut rt, "cancel");
        assert_eq!((rt.state.takes.len(), rt.state.active), (0, None), "cancel at {:?}", rt.find("cancel"));
    }

    #[test]
    fn return_in_the_text_runs_it_and_shift_return_types_a_line() {
        use rune::{Key, KeyEvent, Mods};
        let mut rt = window(ready());
        click(&mut rt, "text");
        let before = rt.state.text.text.clone();
        for (mods, takes) in [(Mods::default(), 1), (Mods { cmd: true, logo: true, ..Mods::default() }, 2)] {
            rt.key(KeyEvent { key: Key::Enter, mods });
            rt.frame(1.0 / 60.0);
            assert_eq!(rt.state.takes.len(), takes, "Return and Command-Return both run the model");
            assert_eq!(rt.state.text.text, before, "without touching the text");
        }
        rt.key(KeyEvent { key: Key::Enter, mods: Mods { shift: true, ..Mods::default() } });
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.takes.len(), 2);
        assert_eq!(rt.state.text.text.matches('\n').count(), 1, "Shift-Return is a new line");
    }

    #[test]
    fn the_voice_list_searches_previews_without_choosing_and_chooses_on_a_press() {
        use rune::{Key, KeyEvent, Mods};
        let mut app = ready();
        let ming = app.catalog.index("ming").unwrap();
        app.select(ming);
        let mut rt = window(app);
        let before = rt.state.voice_index(rt.state.model());
        click(&mut rt, "voice-button");
        assert!(rt.state.voice_menu && rt.find("voice-search").is_some(), "a long list opens with a search field");
        assert!(rt.state.voice_filter.focused, "ready to type in");

        // Hearing a voice neither picks it nor closes the list.
        click(&mut rt, "hear-1");
        assert!(rt.state.is_previewing(ming, 1));
        assert!(rt.state.voice_menu);
        assert_eq!(rt.state.voice_index(rt.state.model()), before);

        // Typing narrows the list to what matches a name, a language or a description.
        for ch in "british".chars() {
            rt.key(KeyEvent { key: Key::Text(ch.to_string()), mods: Mods::default() });
        }
        // Rows that no longer match fade out before they go.
        for _ in 0..40 {
            rt.frame(1.0 / 60.0);
        }
        assert_eq!(rt.state.voice_filter.text, "british");
        let voices = rt.state.model().voices.clone();
        let british: Vec<usize> = (0..voices.len()).filter(|&i| voices[i].detail.contains("British")).collect();
        assert_eq!(british.len(), 2);
        assert!(rt.find(&format!("voice-{}", british[0])).is_some());
        assert!(rt.find("voice-0").is_none(), "the American voices are filtered out");

        click(&mut rt, &format!("voice-{}", british[1]));
        assert_eq!(rt.state.voice_index(rt.state.model()), Some(british[1]));
        assert!(!rt.state.voice_menu, "choosing closes the list");
    }

    #[test]
    fn the_enhance_button_sends_the_line_and_only_text_benches_have_it() {
        let mut rt = window(ready());
        click(&mut rt, "enhance");
        assert!(rt.state.is_enhancing());
        assert!(rt.find("enhancing").is_some() && rt.find("revert").is_none());
        // The conversion and conversation benches have no text, so no Enhance.
        let mut app = ready();
        let vc = app.catalog.models.iter().position(|m| m.kind == Kind::Vc).unwrap();
        app.select(vc);
        assert!(window(app).find("enhance").is_none());
        // Once a line has been enhanced, Revert is on offer.
        let mut app = ready();
        app.enhanced_from = Some(("the line as typed".into(), String::new()));
        assert!(window(app).find("revert").is_some());
    }

    #[test]
    fn standing_instructions_open_beside_enhance_and_take_typing() {
        use rune::{Key, KeyEvent, Mods};
        let mut rt = window(ready());
        assert!(rt.find("preprompt-text").is_none());
        click(&mut rt, "preprompt-button");
        assert!(rt.state.preprompt_open && rt.find("preprompt-text").is_some());
        for ch in "Always whisper.".chars() {
            rt.key(KeyEvent { key: Key::Text(ch.to_string()), mods: Mods::default() });
        }
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.preprompt.text, "Always whisper.", "the field has the keyboard as it opens");
        // Without the enhancer there is nothing for instructions to steer.
        let mut app = ready();
        let helper = app.catalog.index(crate::state::ENHANCER).unwrap();
        app.install[helper] = Install::Missing;
        assert!(window(app).find("preprompt-button").is_none());
    }

    #[test]
    fn the_scene_writer_takes_a_direction_and_a_length_and_sends_it() {
        use rune::{Key, KeyEvent, Mods};
        let mut app = ready();
        let drama = app.catalog.index("drama3").unwrap();
        app.select(drama);
        let mut rt = window(app);
        click(&mut rt, "scene-button");
        assert!(rt.state.scene_open && rt.find("scene-text").is_some());
        for ch in "two friends argue".chars() {
            rt.key(KeyEvent { key: Key::Text(ch.to_string()), mods: Mods::default() });
        }
        rt.frame(1.0 / 60.0);
        assert_eq!(rt.state.scene.text, "two friends argue");
        click(&mut rt, "scene-write");
        assert!(rt.state.is_enhancing() && rt.state.writing_scene);
        assert!(!rt.state.scene_open, "the writer closes while it works");
        assert!(rt.find("enhancing").is_some());
    }

    #[test]
    fn every_model_with_voices_offers_all_of_them() {
        let app = ready();
        let count = |id: &str| app.catalog.models[app.catalog.index(id).unwrap()].voices.len();
        assert_eq!(count("kokoro"), 54);
        assert_eq!(count("qwen3-voices"), 9);
        assert_eq!(count("kitten"), 115);
        assert_eq!(count("lfm3-tts"), 2);
        assert!(count("phonon") >= 38 && count("ming") >= 200);
        for m in &app.catalog.models {
            for v in &m.voices {
                assert!(!v.preview_text().is_empty());
            }
        }
    }

    #[test]
    fn a_conversation_model_with_voices_answers_in_the_chosen_one() {
        use crate::state::Clip;
        let mut app = ready();
        let sophon = app.catalog.index("sophon").unwrap();
        app.select(sophon);
        app.source = Some(Clip { path: "/tmp/question.wav".into(), name: "question".into(), seconds: Some(4.0), peaks: Vec::new() });
        let brand = app.catalog.models[sophon].voices.iter().position(|v| v.id == "brand_en").unwrap();
        app.choose_voice(sophon, brand);
        let mut rt = window(app);
        assert!(rt.find("voice-button").is_some(), "its voices are on its bench");
        click(&mut rt, "go");
        let take = rt.state.takes.last().expect("sending queued a take");
        assert_eq!(take.voice.as_deref(), Some("brand_en"));
        assert!(take.source.is_some() && take.text.is_empty());
        // A conversation model with one voice has no list to show.
        let mut app = ready();
        let single = app.catalog.models.iter().position(|m| m.kind == Kind::Chat && m.voices.is_empty()).unwrap();
        app.select(single);
        assert!(window(app).find("voice-button").is_none());
    }

    #[test]
    fn a_take_shows_the_tool_its_model_called() {
        let mut app = ready();
        crate::scenes::SCENES.iter().find(|(name, _)| *name == "tools").unwrap().1(&mut app);
        let rt = window(app);
        assert!(rt.find("tool-1").is_none(), "the answer that needed no tool has no such line");
        assert!(rt.find("tool-2").is_some() && rt.find("tool-3").is_some());
    }

    #[test]
    fn recording_into_a_slot_leaves_a_clip_with_its_shape() {
        use crate::state::Slot;
        let mut app = ready();
        let chat = app.catalog.models.iter().position(|m| m.kind == Kind::Chat).unwrap();
        app.select(chat);
        let mut rt = window(app);
        click(&mut rt, "record-source");
        assert!(rt.state.recording.is_some());
        // The test microphone is a tone; a second and a half of it.
        for _ in 0..90 {
            rt.frame(1.0 / 60.0);
        }
        assert!(rt.find("elapsed-source").is_some(), "the slot shows the recording's time");
        click(&mut rt, "stop-source");
        let clip = rt.state.clip(Slot::Source).expect("the recording became the clip").clone();
        assert!(clip.seconds.is_some_and(|s| s > 1.0), "it has a length: {:?}", clip.seconds);
        assert!(clip.peaks.iter().any(|p| *p > 0.3), "and a shape to draw");
        assert_eq!(rt.state.blocked(chat), None, "which is all a conversation model needs");
        std::fs::remove_file(clip.path).ok();
    }

    #[test]
    fn every_scene_builds_in_both_appearances() {
        for (name, scene) in crate::scenes::SCENES {
            for dark in [true, false] {
                let mut app = App::new(false);
                scene(&mut app);
                let mut rt = Runtime::new(app, view, (1120.0, 760.0), Env { system_dark: dark, scale: 2.0 });
                rt.frame(0.0);
                rt.frame(0.5);
                assert!(rt.find("kit-root").is_some() || name.starts_with("icon") || *name == "installer", "{name} drew a window");
            }
        }
    }
}
