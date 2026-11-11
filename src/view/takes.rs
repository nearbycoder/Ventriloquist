//! What came out: every run, newest first, with its audio, its shape and how long it took.

use std::path::PathBuf;

use rune::{Drag, El, Phase, ScrollSpec, col, el, icon, row, text};
use rune_kit::{Button, ButtonVariant, ControlSize, IconButton, IconButtonVariant, Kit, Loader, SegmentedControl, glyphs};

use super::{icons, quiet};
use crate::catalog::Kind;
use crate::state::{App, PlayKey, Scope, Slot, Take, TakeStatus, clock, span};
use crate::theme;

/// A clip's shape as bars. Bars already played take the accent; a press or drag seeks.
pub fn waveform(peaks: &[f32], progress: Option<f32>, height: f32, key: PlayKey, path: PathBuf, k: &Kit) -> El {
    let flat = [0.0f32; 48];
    let peaks = if peaks.is_empty() { &flat[..] } else { peaks };
    let n = peaks.len() as f32;
    let (played, rest) = (theme::accent(k), k.c.icon_faint);
    let mut bars = row().h(height).items_center().gap(2.0).min_w(0.0).cursor(rune::Cursor::Pointer).role("slider").label("Position").on_drag(
        move |s: &mut App, d: &mut Drag, _| {
            if matches!(d.phase, Phase::Down | Phase::Move) && d.w > 0.0 {
                s.seek(key, &path, d.x / d.w);
            }
        },
    );
    for (i, p) in peaks.iter().enumerate() {
        let on = progress.is_some_and(|t| (i as f32 + 0.5) / n <= t);
        bars = bars.child(el().grow(1.0).h((p * height).max(2.0)).radius(1.0).fill(if on { played } else { rest }));
    }
    bars
}

fn model_name<'a>(s: &'a App, id: &'a str) -> &'a str {
    s.catalog.models.iter().find(|m| m.id == id).map_or(id, |m| m.name.as_str())
}

fn voice_name(s: &App, t: &Take) -> Option<String> {
    if t.reference.is_some() && t.kind == Kind::Tts {
        return Some("cloned voice".into());
    }
    let id = t.voice.as_deref()?;
    let m = s.catalog.models.iter().find(|m| m.id == t.model)?;
    Some(m.voices.iter().find(|v| v.id == id).map_or(id, |v| v.name.as_str()).to_owned())
}

/// "0.42 s · 11× real time": the numbers a bench is for.
fn timing(t: &Take) -> String {
    let mut parts = vec![format!("{} to make", span(t.gen_seconds))];
    if let Some(x) = t.speed() {
        parts.push(if x >= 10.0 { format!("{x:.0}× real time") } else { format!("{x:.1}× real time") });
    }
    if t.load_seconds >= 0.5 {
        parts.push(format!("{} to load", span(t.load_seconds)));
    }
    if t.peak_gb >= 0.05 {
        parts.push(format!("{:.1} GB peak", t.peak_gb));
    }
    parts.join(" · ")
}

fn take_row(s: &App, t: &Take, k: &Kit) -> El {
    let c = &k.c;
    let id = t.id;
    let name = model_name(s, &t.model).to_owned();
    let title = match voice_name(s, t) {
        Some(v) => format!("{name} · {v}"),
        None => name,
    };
    let card = col().key(format!("take-{id}")).w_full().gap(8.0).padding(12.0).radius(k.radius.lg).fill(theme::card(k)).border(1.0, c.border_weak);
    // What was said: the line a TTS model read, or what a conversation model answered.
    let caption = |t: &Take| {
        let words = if t.reply.is_empty() { t.text.clone() } else { format!("\u{201c}{}\u{201d}", t.reply) };
        (!words.is_empty()).then(|| text(words).font(k.ty.small).color(c.text_faint).max_lines(3).wrap().selectable(true))
    };

    match &t.status {
        TakeStatus::Queued | TakeStatus::Running(_) => {
            let what = match &t.status {
                TakeStatus::Running(message) if !message.is_empty() => message.clone(),
                TakeStatus::Running(_) => "Working".into(),
                _ => "Waiting its turn".into(),
            };
            let running = matches!(t.status, TakeStatus::Running(_));
            let what = match s.elapsed(id) {
                Some(secs) if secs >= 1.0 => format!("{what} · {}", span(secs.floor())),
                _ => what,
            };
            let lead = if running { Loader::new().build(k) } else { el().size(16.0, 16.0).center().child(el().size(6.0, 6.0).radius(3.0).fill(c.icon_faint)) };
            card.child(
                row()
                    .w_full()
                    .items_center()
                    .gap(10.0)
                    .child(row().size(28.0, 28.0).center().child(lead))
                    .child(
                        col()
                            .grow(1.0)
                            .min_w(0.0)
                            .gap(1.0)
                            .child(text(title).font(k.ty.ui_medium).color(c.text_base).truncate())
                            .child(text(what).key(format!("status-{id}")).font(k.ty.small).color(c.text_faint).truncate()),
                    ),
            )
            .children(caption(t))
        }
        TakeStatus::Failed(message) => card
            .child(
                row()
                    .w_full()
                    .items_center()
                    .gap(10.0)
                    .child(row().size(28.0, 28.0).center().child(icon(&icons::WARNING, 16.0).color(theme::red(k))))
                    .child(text(format!("{title} didn't finish")).font(k.ty.ui_medium).color(c.text_base).grow(1.0).truncate())
                    .child(quiet(&glyphs::CLOSE, "Dismiss", k).on_press(move |s: &mut App| s.delete_take(id)).build(k).key(format!("dismiss-{id}"))),
            )
            .child(text(message.clone()).font(k.ty.mono).color(c.text_muted).wrap().max_lines(8).selectable(true)),
        TakeStatus::Done | TakeStatus::Cancelled => {
            let key = PlayKey::Take(id);
            let playing = s.is_playing(key);
            let progress = s.progress(key);
            let at = progress.map(|p| p as f64 * t.audio_seconds);
            let length = match at {
                Some(at) => format!("{} / {}", clock(at), clock(t.audio_seconds)),
                None => clock(t.audio_seconds),
            };
            let play = IconButton::new(if playing { &icons::PAUSE } else { &icons::PLAY })
                .variant(IconButtonVariant::Contrast)
                .control_size(ControlSize::Large)
                .label(if playing { "Pause" } else { "Play" })
                .on_press({
                    let path = t.out.clone();
                    move |s: &mut App| s.toggle_play(key, &path)
                })
                .build(k)
                .key(format!("play-{id}"));
            let mut tools = row().items_center().gap(2.0);
            // A finished take can feed the next run: clone its voice, or convert it.
            tools = tools
                .child(
                    quiet(&icons::VOICE, "Use as the voice sample", k)
                        .on_press(move |s: &mut App| s.take_to_slot(id, Slot::Reference))
                        .build(k)
                        .key(format!("as-voice-{id}")),
                )
                .child(
                    quiet(&icons::WAVE, "Use as the speech to convert", k)
                        .on_press(move |s: &mut App| s.take_to_slot(id, Slot::Source))
                        .build(k)
                        .key(format!("as-source-{id}")),
                )
                .child(
                    quiet(&icons::REVEAL, "Show in Finder", k)
                        .on_press({
                            let path = t.out.clone();
                            move |s: &mut App| s.reveal(&path)
                        })
                        .build(k)
                        .key(format!("reveal-{id}")),
                )
                .child(quiet(&icons::TRASH, "Delete", k).on_press(move |s: &mut App| s.delete_take(id)).build(k).key(format!("delete-{id}")));
            card.child(
                row()
                    .w_full()
                    .items_center()
                    .gap(12.0)
                    .child(play)
                    .child(waveform(&t.peaks, progress, 34.0, key, t.out.clone(), k).grow(1.0))
                    .child(text(length).key(format!("length-{id}")).font(k.ty.mono).color(c.text_muted)),
            )
            .child(
                row()
                    .w_full()
                    .items_center()
                    .gap(8.0)
                    .child(text(title).font(k.ty.ui_medium).color(c.text_base).truncate())
                    .child(text(timing(t)).key(format!("timing-{id}")).font(k.ty.small).color(c.text_faint).truncate().grow(1.0))
                    .child(tools),
            )
            .children((!t.tool.is_empty()).then(|| {
                // The call a conversation model made on the way to its answer.
                row()
                    .key(format!("tool-{id}"))
                    .w_full()
                    .items_center()
                    .gap(6.0)
                    .child(icon(&icons::TOOL, 13.0).color(c.icon_muted))
                    .child(text(t.tool.clone()).font(k.ty.mono).color(c.text_muted).truncate().selectable(true))
            }))
            .children(caption(t))
        }
    }
}

pub fn takes(s: &App, k: &Kit) -> El {
    let c = &k.c;
    let model = &s.model().id;
    let shown: Vec<&Take> = s.takes.iter().rev().filter(|t| s.scope == Scope::All || &t.model == model).collect();
    let head = row()
        .w_full()
        .items_center()
        .gap(10.0)
        .px(20.0)
        .pb(8.0)
        .child(text("Takes").font(k.ty.ui_medium).color(c.text_base))
        .child(text(if shown.is_empty() { String::new() } else { shown.len().to_string() }).font(k.ty.small).color(c.text_faint).grow(1.0))
        .child(
            SegmentedControl::new(&["All models", "This model"], Some(if s.scope == Scope::All { 0 } else { 1 }))
                .label("Which takes to show")
                .fit()
                .on_pick(|s: &mut App, i, _| s.scope = if i == 0 { Scope::All } else { Scope::Model })
                .build(k)
                .key("scope"),
        );
    let list = if shown.is_empty() {
        col().w_full().grow(1.0).center().gap(6.0).padding(24.0).child(icon(&icons::WAVE, 22.0).color(c.icon_faint)).child(
            text(if s.takes.is_empty() { "Nothing yet. What you make shows up here." } else { "Nothing from this model yet." })
                .font(k.ty.ui)
                .color(c.text_faint),
        )
    } else {
        let mut rows = col().key("take-rows").w_full().gap(8.0).px(20.0).pb(20.0);
        for t in shown {
            rows = rows.child(take_row(s, t, k));
        }
        k.scroll_bar(col().key("take-list").w_full().grow(1.0).min_h(0.0).scroll(ScrollSpec { stick_bottom: false }).scroll_edge_fade(12.0).child(rows))
    };
    let _ = (Button::new(""), ButtonVariant::Ghost);
    col().key("takes").w_full().grow(1.0).min_h(0.0).child(head).child(list)
}
