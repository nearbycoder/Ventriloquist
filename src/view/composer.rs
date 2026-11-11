//! What goes into a run: the words, the voice, the clips, and the button that starts it.

use rune::{El, FileDialogOptions, Icon, Tx, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, Loader, Placement, Popover, SegmentedControl, TextArea, TextInput, glyphs};

use super::{icons, quiet, takes::waveform, voices};
use crate::catalog::{Cloning, Instruct, Kind};
use crate::state::{AUDIO_EXTENSIONS, App, PlayKey, Slot, clock, is_run_key};
use crate::theme;

fn choose_file(slot: Slot) -> impl Fn(&mut App, &mut Tx) {
    move |_: &mut App, tx: &mut Tx| {
        let options = FileDialogOptions { title: "Choose audio".into(), ..Default::default() }.filter("Audio", AUDIO_EXTENSIONS);
        tx.open_file(options, move |s: &mut App, result, _tx| match result {
            Ok(Some(path)) => s.set_clip(slot, &path),
            Ok(None) => {}
            Err(e) => s.notice = Some(e.to_string()),
        });
    }
}

/// A place for one audio clip: drop a file on it, choose one, or record. Once filled it plays,
/// shows its shape and can be emptied.
fn clip_slot(s: &App, slot: Slot, title: &str, hint: &str, mark: &'static Icon, k: &Kit) -> El {
    let c = &k.c;
    let id = match slot {
        Slot::Reference => "reference",
        Slot::Source => "source",
    };
    let over = s.drop_over == Some(slot);
    let frame = row()
        .key(format!("slot-{id}"))
        .w_full()
        .h(48.0)
        .items_center()
        .gap(10.0)
        .px(12.0)
        .radius(k.radius.md)
        .fill(if over { theme::kind_wash(s.model().kind, k) } else { theme::card(k) })
        .on_drop_files(move |s: &mut App, files, _| {
            if let Some(file) = files.first() {
                s.set_clip(slot, file);
            }
        })
        .accepts_extensions(AUDIO_EXTENSIONS)
        .on_drop_hover(move |s: &mut App, over, _| s.drop_over = over.then_some(slot));

    let body = if let Some((_, rec)) = s.recording.as_ref().filter(|(r, _)| *r == slot) {
        // The dot swells with the voice: its scale is the recorder's own meter, drawn without
        // re-running the view.
        let meter = el().size(22.0, 22.0).center().child(el().size(20.0, 20.0).radius(10.0).fill(theme::red(k)).live_scale(&rec.live_level()));
        frame
            .border(1.0, theme::red(k))
            .child(row().size(22.0, 22.0).center().child(el().absolute(8.0, 8.0).size(6.0, 6.0).radius(3.0).fill(theme::red(k))).child(meter))
            .child(text("Recording").font(k.ty.ui_medium).color(c.text_base))
            .child(text(clock(rec.elapsed())).key(format!("elapsed-{id}")).font(k.ty.mono).color(c.text_muted).grow(1.0))
            .child(
                Button::new("Stop")
                    .variant(ButtonVariant::Neutral)
                    .icon(&icons::STOP)
                    .on_press(move |s: &mut App| s.toggle_record(slot))
                    .build(k)
                    .key(format!("stop-{id}")),
            )
    } else if let Some(clip) = s.clip(slot) {
        let key = PlayKey::Slot(slot);
        let path = clip.path.clone();
        let playing = s.is_playing(key);
        frame
            .border(1.0, c.border_weak)
            .child(
                IconButton::new(if playing { &icons::PAUSE } else { &icons::PLAY })
                    .variant(IconButtonVariant::Neutral)
                    .label(if playing { "Pause" } else { "Play" })
                    .on_press({
                        let path = path.clone();
                        move |s: &mut App| s.toggle_play(key, &path)
                    })
                    .build(k)
                    .key(format!("play-{id}")),
            )
            .child(
                col()
                    .w(150.0)
                    .gap(1.0)
                    .child(text(clip.name.clone()).font(k.ty.ui_medium).color(c.text_base).truncate())
                    .child(text(format!("{title}{}", clip.seconds.map(|d| format!(" · {}", clock(d))).unwrap_or_default())).font(k.ty.small).color(c.text_faint).truncate()),
            )
            .child(waveform(&clip.peaks, s.progress(key), 22.0, key, path, k).grow(1.0))
            .child(quiet(&glyphs::CLOSE, "Remove", k).on_press(move |s: &mut App| s.clear_clip(slot)).build(k).key(format!("clear-{id}")))
    } else {
        frame
            .border(1.0, if over { theme::kind_ink(s.model().kind, k) } else { c.border_base })
            .border_dash(4.0, 3.0)
            .child(icon(mark, 16.0).color(c.icon_muted))
            .child(
                col()
                    .grow(1.0)
                    .min_w(0.0)
                    .gap(1.0)
                    .child(text(title.to_owned()).font(k.ty.ui_medium).color(c.text_base))
                    .child(text(hint.to_owned()).font(k.ty.small).color(c.text_faint).truncate()),
            )
            .child(
                Button::new("Choose")
                    .variant(ButtonVariant::Ghost)
                    .icon(&icons::OPEN_FILE)
                    .help("Choose an audio file, or drop one here.")
                    .on_press_tx(choose_file(slot))
                    .build(k)
                    .key(format!("choose-{id}")),
            )
            .child(
                Button::new("Record")
                    .variant(ButtonVariant::Ghost)
                    .icon(&icons::RECORD)
                    .on_press(move |s: &mut App| s.toggle_record(slot))
                    .build(k)
                    .key(format!("record-{id}")),
            )
    };
    body
}

/// The run button and, beside it, the same run on every installed model of this kind.
fn actions(s: &App, verb: &str, k: &Kit) -> El {
    let blocked = s.blocked(s.selected);
    let busy = s.active.is_some();
    let mut r = row().items_center().gap(8.0);
    if busy {
        let waiting = s.takes.iter().filter(|t| t.is_pending()).count();
        r = r.child(
            Button::new(if waiting > 1 { format!("Cancel {waiting}") } else { "Cancel".to_owned() })
                .variant(ButtonVariant::Ghost)
                .help("Stop the run in progress and drop the ones waiting.")
                .on_press(|s: &mut App| s.cancel())
                .build(k)
                .key("cancel"),
        );
    }
    let others = s.runnable();
    if others > 1 {
        r = r.child(
            Button::new(format!("Compare {others}"))
                .variant(ButtonVariant::Neutral)
                .control_size(ControlSize::Large)
                .icon(&icons::COMPARE)
                .help("Run this on every installed model of this kind, one after another.")
                .on_press(|s: &mut App| s.generate_all())
                .build(k)
                .key("compare"),
        );
    }
    let mut go = Button::new(verb.to_owned()).variant(ButtonVariant::Contrast).control_size(ControlSize::Large).disabled(blocked.is_some()).shortcut("↩");
    go = match blocked {
        Some(why) => go.help(why),
        None => go.on_press(|s: &mut App| s.generate()),
    };
    r.child(go.build(k).key("go"))
}

/// The Enhance button: the local language model marks the line up for the selected voice
/// model. While it works the button is a spinner; afterwards Revert puts the old line back.
/// The Instructions button beside Enhance: what the enhancer should do to every line, in the
/// user's own words. Framed while there are any.
fn preprompt(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let c = &k.c;
    let set = !s.preprompt.text.trim().is_empty();
    // Labelled, not a bare icon: as an icon alone it was there and nobody could find it.
    let button = Button::new("Instructions")
        .variant(if set { ButtonVariant::Neutral } else { ButtonVariant::Ghost })
        .icon(&icons::NOTE)
        .help(if set { "Standing instructions are set for Enhance. Open to change them." } else { "Tell Enhance how to treat every line." })
        .on_press(|s: &mut App| s.toggle_preprompt())
        .build(k)
        .key("preprompt-button");
    Popover::new("preprompt")
        .trigger(button)
        .title("Standing instructions")
        .open(s.preprompt_open)
        .placement(Placement::TopStart)
        .on_close(|s: &mut App| s.close_preprompt())
        .child(
            col()
                .w(296.0)
                .gap(8.0)
                .child(
                    text("Enhance follows these on every line, for every model. Leave it empty for none.")
                        .font(k.ty.small)
                        .color(c.text_muted)
                        .wrap(),
                )
                .child(
                    TextArea::new(&s.preprompt)
                        .label("Standing instructions")
                        .placeholder("Make every line sound tired and a little sarcastic. Never add laughter.")
                        .width(None)
                        .auto_grow(4, 9)
                        .on_edit(|s: &mut App| &mut s.preprompt)
                        .build(k)
                        .key("preprompt-text"),
                ),
        )
        .build(k, layer)
}

/// The Scene button: describe a scene, and the enhancer writes its dialogue into the text box
/// in the form the selected model performs (speaker tags for one that voices several
/// characters, a single character's lines for one that doesn't).
fn scene(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let c = &k.c;
    let m = s.model();
    let button = Button::new("Scene")
        .variant(ButtonVariant::Ghost)
        .icon(&icons::SCENE)
        .help("Describe a scene and have its dialogue written for this model.")
        .on_press(|s: &mut App| s.toggle_scene())
        .build(k)
        .key("scene-button");
    let voices = if m.enhance.speakers > 1 {
        format!("{} can voice up to {} characters in one take, so each line is marked with its speaker.", m.name, m.enhance.speakers)
    } else {
        format!("{} speaks in one voice, so the scene is written for one character. Fish Drama 3 performs several.", m.name)
    };
    let empty = s.scene.text.trim().is_empty();
    let mut write = Button::new("Write dialogue").variant(ButtonVariant::Contrast).disabled(empty);
    if !empty {
        write = write.on_press(|s: &mut App| s.write_scene());
    }
    Popover::new("scene")
        .trigger(button)
        .title("Write a scene")
        .open(s.scene_open)
        .placement(Placement::TopStart)
        .on_close(|s: &mut App| s.scene_open = false)
        .child(
            col()
                .w(296.0)
                .gap(10.0)
                .child(
                    TextArea::new(&s.scene)
                        .label("Scene direction")
                        .placeholder("Who is talking, where, and what happens: a couple in the car after he misses the exit again. She is furious; he stays calm and makes it worse.")
                        .width(None)
                        .auto_grow(4, 9)
                        .on_edit(|s: &mut App| &mut s.scene)
                        .on_submit(|s: &mut App, _: &mut Tx| s.write_scene())
                        .build(k)
                        .key("scene-text"),
                )
                .child(text(voices).font(k.ty.small).color(c.text_muted).wrap())
                .child(
                    SegmentedControl::new(&["Short", "Medium", "Long"], Some(s.scene_length.min(2)))
                        .label("How much dialogue")
                        .width(Some(296.0))
                        .on_pick(|s: &mut App, i, _| s.scene_length = i)
                        .build(k)
                        .key("scene-length"),
                )
                .child(row().w_full().justify_end().child(write.build(k).key("scene-write"))),
        )
        .build(k, layer)
}

fn enhance(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let c = &k.c;
    let mut group = row().key("enhance-group").items_center().gap(4.0);
    if s.is_enhancing() {
        return group.child(
            row()
                .key("enhancing")
                .h(32.0)
                .items_center()
                .gap(8.0)
                .px(10.0)
                .child(Loader::new().size(14.0).build(k))
                .child(text(if s.writing_scene { "Writing the scene" } else { "Enhancing" }).font(k.ty.ui_medium).color(c.text_muted)),
        );
    }
    let help = if s.enhancer_ready() {
        "Mark the line up for this model: punctuation for delivery, numbers as words, and the expression tags it understands."
    } else {
        "Download the prompt enhancer on the Models page to use this."
    };
    group = group.child(
        Button::new("Enhance")
            .variant(ButtonVariant::Ghost)
            .icon(&glyphs::SPARKLE)
            .disabled(s.text.text.trim().is_empty())
            .help(help)
            .on_press(|s: &mut App| s.enhance())
            .build(k)
            .key("enhance"),
    );
    if s.enhancer_ready() {
        group = group.child(preprompt(s, k, layer)).child(scene(s, k, layer));
    }
    if s.enhanced_from.as_ref().is_some_and(|(before, _)| *before != s.text.text) {
        group = group.child(
            Button::new("Revert")
                .variant(ButtonVariant::Ghost)
                .help("Put back the line as it was before Enhance.")
                .on_press(|s: &mut App| s.revert_enhance())
                .build(k)
                .key("revert"),
        );
    }
    group
}

pub fn composer(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let (c, m) = (&k.c, s.model());
    let mut body = col().key("composer").w_full().gap(10.0);
    match m.kind {
        Kind::Tts => {
            body = body.child(
                TextArea::new(&s.text)
                    .label("Text to speak")
                    .placeholder("Type something to say")
                    .width(None)
                    .auto_grow(3, 7)
                    .on_edit(|s: &mut App| &mut s.text)
                    // Return runs, as in a chat composer; Shift- or Option-Return types a new line.
                    .on_submit(|s: &mut App, _: &mut Tx| s.generate())
                    .on_key(|s: &mut App, ev: &rune::KeyEvent, _: &mut Tx| {
                        if is_run_key(ev) {
                            s.generate();
                            return true;
                        }
                        false
                    })
                    .build(k)
                    .key("text"),
            );
            // The tools that write or rewrite the text sit right under it.
            let words = s.text.text.split_whitespace().count();
            body = body.child(
                row()
                    .key("text-tools")
                    .w_full()
                    .items_center()
                    .gap(10.0)
                    .child(enhance(s, k, layer))
                    .child(el().grow(1.0))
                    .child(text(format!("{words} words")).font(k.ty.small).color(c.text_faint)),
            );
            if m.cloning != Cloning::None {
                let hint = match m.cloning {
                    Cloning::Required => "A few seconds of clean speech. This model needs one.",
                    _ => "Optional: a few seconds of clean speech to imitate.",
                };
                body = body.child(clip_slot(s, Slot::Reference, "Voice sample", hint, &icons::VOICE, k));
                if m.ref_text && s.reference.is_some() {
                    body = body.child(
                        TextInput::new(&s.ref_text)
                            .label("Words spoken in the sample")
                            .placeholder("What the voice sample says, word for word (helps this model match it)")
                            .width(None)
                            .on_edit(|s: &mut App| &mut s.ref_text)
                            .build(k)
                            .key("ref-text"),
                    );
                }
            }
            if m.instruct != Instruct::None {
                body = body.child(
                    TextInput::new(&s.instruct)
                        .label(if m.instruct == Instruct::Required { "Voice description" } else { "Direction" })
                        .placeholder(m.instruct_hint.clone())
                        .width(None)
                        .on_edit(|s: &mut App| &mut s.instruct)
                        .build(k)
                        .key("instruct"),
                );
            }
            let mut foot = row().w_full().items_center().gap(10.0);
            let sampled = m.cloning != Cloning::None && s.reference.is_some();
            if !m.voices.is_empty() {
                foot = foot.child(el().opacity(if sampled { 0.45 } else { 1.0 }).child(voices::voice_picker(s, k, layer)));
                if sampled {
                    foot = foot.child(text("The voice sample is used instead").font(k.ty.small).color(c.text_faint));
                }
            }
            foot = foot.child(el().grow(1.0)).child(actions(s, "Speak", k));
            body = body.child(foot);
        }
        Kind::Vc => {
            body = body
                .child(clip_slot(s, Slot::Source, "Speech to convert", "The words and delivery to keep.", &icons::WAVE, k))
                .child(clip_slot(s, Slot::Reference, "Voice to speak in", "A few seconds of the target voice.", &icons::VOICE, k))
                .child(row().w_full().items_center().child(el().grow(1.0)).child(actions(s, "Convert", k)));
        }
        Kind::Chat => {
            body = body
                .child(clip_slot(s, Slot::Source, "Say something", "Record a question or a remark; it answers out loud.", &glyphs::MIC, k))
                .child({
                    // A conversation model with more than one voice answers in the chosen one.
                    let foot = row().w_full().items_center().gap(10.0);
                    let foot = if m.voices.is_empty() { foot } else { foot.child(voices::voice_picker(s, k, layer)) };
                    foot.child(el().grow(1.0)).child(actions(s, "Send", k))
                });
        }
        // A helper is never selected; it has no bench to draw.
        Kind::Helper => {}
    }
    body
}
