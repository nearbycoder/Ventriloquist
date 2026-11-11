//! Choosing a voice: a button that names the current one, a list of all of them to search,
//! and a play button on each to hear it before picking.

use rune::{El, Tx, col, el, icon, row, text};
use rune_kit::{IconButton, IconButtonVariant, Kit, Loader, Placement, TextInput, glyphs, popover};

use super::icons;
use crate::state::{App, PlayKey};
use crate::theme;

const ANCHOR: &str = "voices-trigger";

/// The play button for one voice: a spinner while its preview is being made, a stop while it
/// plays. In the list it takes no focus, so the search field keeps the keyboard while voices
/// are auditioned.
fn preview_button(s: &App, model: usize, voice: usize, name: &str, key: String, in_list: bool, k: &Kit) -> El {
    if s.is_previewing(model, voice) {
        return row().key(key).size(28.0, 28.0).center().child(Loader::new().size(14.0).build(k)).help("Making the preview");
    }
    let playing = s.is_playing(PlayKey::Voice { model, voice });
    let glyph = if playing { &icons::STOP } else { &icons::PLAY };
    let label = if playing { format!("Stop {name}") } else { format!("Hear {name}") };
    if in_list {
        return k
            .interactive(
                row()
                    .key(key)
                    .role("button")
                    .label(label)
                    .size(28.0, 28.0)
                    .center()
                    .radius(k.radius.sm)
                    .child(icon(glyph, 14.0).color(if playing { theme::accent(k) } else { k.c.icon_muted }))
                    .on_press(move |s: &mut App| s.preview_voice(model, voice)),
            )
            .preserve_focus_on_press(true);
    }
    IconButton::new(glyph)
        .variant(IconButtonVariant::GhostMuted)
        .label(label)
        .on_press(move |s: &mut App| s.preview_voice(model, voice))
        .build(k)
        .key(key)
}

/// The list that opens under the button: a search field, then every voice under its heading.
fn list(s: &App, model: usize, k: &Kit) -> El {
    let c = &k.c;
    let m = &s.catalog.models[model];
    let current = s.voice_index(m);
    let query = s.voice_filter.text.trim().to_lowercase();
    let mut rows = col().key("voice-rows").w_full().padding(4.0).gap(1.0).preserve_focus_on_press(true);
    let (mut shown, mut heading) = (0, "");
    for (i, v) in m.voices.iter().enumerate().filter(|(_, v)| v.matches(&query)) {
        if v.group != heading {
            heading = &v.group;
            rows = rows.child(text(v.group.clone()).font(k.ty.small_medium).color(c.text_faint).px(8.0).pt(if shown == 0 { 4.0 } else { 10.0 }).pb(4.0));
        }
        shown += 1;
        let mut line = row()
            .key(format!("voice-{i}"))
            .role("option")
            .label(v.name.clone())
            .w_full()
            .h(34.0)
            .items_center()
            .gap(6.0)
            .px(4.0)
            .radius(k.radius.sm)
            .child(preview_button(s, model, i, &v.name, format!("hear-{i}"), true, k))
            .child(text(v.name.clone()).font(k.ty.ui_medium).color(c.text_base))
            .child(text(v.detail.clone()).font(k.ty.small).color(c.text_faint).truncate().grow(1.0).min_w(0.0))
            .child(el().size(16.0, 16.0).children((current == Some(i)).then(|| icon(k.glyph.check, 16.0).color(theme::accent(k)))))
            .on_press(move |s: &mut App| s.choose_voice(model, i));
        if !v.detail.is_empty() {
            line = line.help(v.detail.clone());
        }
        rows = rows.child(k.interactive(line));
    }
    if shown == 0 {
        rows = rows.child(text("No voice matches that.").font(k.ty.ui).color(c.text_faint).padding(12.0));
    }
    let mut body = col().key("voice-list").w(420.0).max_h(440.0).min_h(0.0);
    // A handful of voices needs no search.
    if m.voices.len() > 10 {
        body = body.child(
            el().w_full().padding(8.0).pb(4.0).child(
                TextInput::new(&s.voice_filter)
                    .label("Search voices")
                    .placeholder(format!("Search {} voices", m.voices.len()))
                    .leading_icon(&glyphs::SEARCH)
                    .width(None)
                    .on_edit(|s: &mut App| &mut s.voice_filter)
                    .on_key(|s: &mut App, ev: &rune::KeyEvent, _: &mut Tx| {
                        if matches!(ev.key, rune::Key::Escape) {
                            s.voice_menu = false;
                            return true;
                        }
                        false
                    })
                    .build(k)
                    .key("voice-search"),
            ),
        );
    }
    body.child(popover::fit_scroll(rows))
}

/// The voice control in the composer: hear the current voice, or open the list to change it.
pub fn voice_picker(s: &App, k: &Kit, layer: &mut Vec<El>) -> El {
    let c = &k.c;
    let model = s.selected;
    let m = s.model();
    let current = s.voice_index(m);
    let (name, group) = current.map(|i| (m.voices[i].name.clone(), m.voices[i].group.clone())).unwrap_or_else(|| ("Choose a voice".into(), String::new()));
    let button = k
        .interactive(
            row()
                .key("voice-button")
                .role("combobox")
                .label("Voice")
                .expanded(s.voice_menu)
                .focusable(true)
                .h(32.0)
                .items_center()
                .gap(8.0)
                .px(10.0)
                .radius(k.radius.md)
                .border(1.0, c.border_base)
                .child(icon(&icons::VOICE, 14.0).color(c.icon_muted))
                .child(text(name.clone()).key("voice-name").font(k.ty.ui_medium).color(c.text_base))
                .child(text(group).font(k.ty.small).color(c.text_faint))
                .child(icon(k.glyph.chevron_down, 14.0).color(c.icon_muted))
                .on_press(|s: &mut App| s.toggle_voice_menu()),
        )
        .help(format!("{} voices. Open the list to hear and choose one.", m.voices.len()));
    let mut control = row().key(ANCHOR).items_center().gap(4.0);
    if let Some(i) = current {
        control = control.child(preview_button(s, model, i, &name, "hear-current".into(), false, k));
    }
    control = control.child(button);
    if s.voice_menu {
        layer.push(rune_kit::scrim("voices-scrim", |s: &mut App| s.voice_menu = false));
        layer.push(popover::anchored(k, popover::raised(k, list(s, model, k), k.radius.md), ANCHOR, Placement::BottomStart, 4.0));
    }
    control
}
