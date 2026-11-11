//! Named states for headless snapshots (`ventriloquist --snap out.png --scene takes`), so the
//! window can be looked at and tested without a model installed.

use crate::catalog::{Cloning, Kind};
use crate::state::{App, CLIP_PEAKS, Clip, Install, PEAKS, Page, Take, TakeStatus};
use crate::view::app_icon::Art;

pub const SCENES: &[(&str, fn(&mut App))] = &[
    ("fresh", |_| {}),
    ("models", models),
    ("installing", installing),
    ("takes", takes),
    ("convert", convert),
    ("failed", failed),
    ("direct", direct),
    ("enhanced", enhanced),
    ("instructions", instructions),
    ("scene", scene),
    ("chat", chat),
    ("tools", tools),
    ("icon", |s| s.icon = Some(Art::Tile)),
    ("icon-glyph", |s| s.icon = Some(Art::Glyph)),
    ("installer", |s| s.icon = Some(Art::Installer)),
];

/// A speech-shaped envelope: syllables that swell and fall, with pauses.
fn peaks(n: usize, seed: f32) -> Vec<f32> {
    (0..n)
        .map(|i| {
            let x = i as f32 + seed * 37.0;
            let syllable = (x * 0.55).sin().abs();
            let phrase = (0.5 + 0.5 * (x * 0.09 + seed).sin()).powf(0.6);
            let pause = if ((x * 0.031 + seed * 0.7).sin()) > 0.86 { 0.08 } else { 1.0 };
            (0.12 + 0.88 * syllable * phrase * pause).clamp(0.04, 1.0)
        })
        .collect()
}

fn all_ready(s: &mut App) {
    s.install.iter_mut().for_each(|i| *i = Install::Ready);
    s.page = Page::Bench;
}

/// The Models page partway through setting up: some downloaded, one arriving, one that failed.
fn models(s: &mut App) {
    let at = |s: &App, id: &str| s.catalog.index(id).expect("a model in the catalog");
    for id in ["kokoro", "ming", "seed-vc"] {
        let i = at(s, id);
        s.install[i] = Install::Ready;
    }
    let (arriving, failed, asked) = (at(s, "qwen3-voices"), at(s, "chatterbox-vc"), at(s, "ming"));
    s.install[arriving] = Install::Installing {
        log: vec!["Downloading mlx-community/Qwen3-TTS-12Hz-1.7B-CustomVoice-bf16".into(), "model.safetensors: 62%|######2   | 2.81G/4.52G [00:41<00:24, 71.2MB/s]".into()],
    };
    s.install[failed] = Install::Failed { log: vec!["Downloading ResembleAI/chatterbox".into(), "ConnectionError: the download stopped at 412 MB of 1.1 GB".into()] };
    s.confirm_remove = Some(asked);
    s.show_models();
}

fn take(s: &App, id: u64, model: usize, gen_seconds: f64, audio_seconds: f64, load_seconds: f64, peak_gb: f64) -> Take {
    let m = &s.catalog.models[model];
    Take {
        id,
        model: m.id.clone(),
        kind: m.kind,
        text: if m.kind == Kind::Tts { s.text.text.clone() } else { String::new() },
        voice: m.voice_default.clone(),
        reference: None,
        ref_text: String::new(),
        instruct: String::new(),
        source: None,
        reply: String::new(),
        tool: String::new(),
        out: format!("/tmp/{}-{id:04}.wav", m.id).into(),
        status: TakeStatus::Done,
        gen_seconds,
        load_seconds,
        audio_seconds,
        peak_gb,
        created: 0,
        peaks: peaks(PEAKS, id as f32),
    }
}

fn installing(s: &mut App) {
    s.select(0);
    s.install[0] = Install::Installing {
        log: vec!["Resolved 87 packages in 1.2s".into(), "Downloading model.safetensors (312 MB of 327 MB)".into()],
    };
    s.show_log = true;
}

fn takes(s: &mut App) {
    all_ready(s);
    let tts: Vec<usize> = (0..s.catalog.models.len()).filter(|&i| s.catalog.models[i].kind == Kind::Tts).collect();
    // Figures in the range these models measure on an M5 Max, so a snapshot reads true.
    let timings = [(0.17, 8.3, 2.2, 2.5), (0.22, 7.2, 0.6, 0.8), (1.5, 10.3, 4.1, 1.9), (1.6, 8.2, 1.4, 4.1)];
    for (n, (&model, &(g, a, l, p))) in tts.iter().zip(timings.iter()).enumerate() {
        let t = take(s, n as u64 + 1, model, g, a, l, p);
        s.takes.push(t);
    }
    if let Some(&model) = tts.first() {
        let mut t = take(s, 9, model, 0.0, 0.0, 0.0, 0.0);
        t.status = TakeStatus::Running("Generating".into());
        t.peaks.clear();
        s.active = Some(t.id);
        s.takes.push(t);
    }
}

fn convert(s: &mut App) {
    all_ready(s);
    if let Some(i) = s.catalog.models.iter().position(|m| m.kind == Kind::Vc) {
        s.select(i);
    }
    let clip = |name: &str, seconds: f64, seed: f32| Clip { path: format!("/tmp/{name}").into(), name: name.into(), seconds: Some(seconds), peaks: peaks(CLIP_PEAKS, seed) };
    s.source = Some(clip("interview.wav", 14.0, 2.0));
    s.reference = Some(clip("Recording, 0:08", 8.0, 5.0));
    let t = take(s, 1, s.selected, 2.4, 14.0, 7.3, 4.6);
    s.takes.push(t);
}

fn failed(s: &mut App) {
    all_ready(s);
    if let Some(i) = s.catalog.models.iter().position(|m| m.kind == Kind::Tts && m.cloning == Cloning::Required) {
        s.select(i);
    }
    let mut t = take(s, 1, s.selected, 0.0, 0.0, 0.0, 0.0);
    t.status = TakeStatus::Failed("RuntimeError: reference audio is shorter than 1 second\n  File \"worker.py\", line 212, in run".into());
    s.takes.push(t);
    s.notice = Some("Couldn't read notes.ogg: the file isn't audio this Mac can decode.".into());
}

/// A model that takes a voice, a sample and a line of direction, with its voice menu open.
fn direct(s: &mut App) {
    all_ready(s);
    if let Some(i) = s.catalog.models.iter().position(|m| m.kind == Kind::Tts && m.cloning == Cloning::Optional && !m.voices.is_empty()) {
        s.select(i);
    }
    s.voice_menu = true;
    // One voice's preview on its way, as after a press on its play button.
    s.preview_voice(s.selected, 3);
}

fn chat(s: &mut App) {
    all_ready(s);
    if let Some(i) = s.catalog.models.iter().position(|m| m.kind == Kind::Chat) {
        s.select(i);
    }
    s.source = Some(Clip { path: "/tmp/question.wav".into(), name: "Recording, 0:05".into(), seconds: Some(5.0), peaks: peaks(CLIP_PEAKS, 3.0) });
    let mut t = take(s, 1, s.selected, 5.1, 13.0, 2.9, 28.2);
    t.reply = "Hi! The capital of France is Paris. It's famous for its beautiful architecture, like the Eiffel Tower and Notre-Dame Cathedral, as well as its art, fashion, and romantic atmosphere.".into();
    s.takes.push(t);
}

/// A conversation model that called tools on the way to two of its answers.
fn tools(s: &mut App) {
    all_ready(s);
    if let Some(i) = s.catalog.index("lfm25-audio") {
        s.select(i);
    }
    s.scope = crate::state::Scope::Model;
    s.source = Some(Clip { path: "/tmp/question.wav".into(), name: "Recording, 0:03".into(), seconds: Some(3.0), peaks: peaks(CLIP_PEAKS, 5.0) });
    let asked = [
        ("", "Spiders have eight legs. Each leg is equipped with tiny hairs that help them move and sense their surroundings.", 1.5, 6.9),
        ("calculate(expression=\"1234 * 56\") → 69,104", "1234 times 56 is 69,104.", 1.1, 5.8),
        ("roll_dice(sides=20) → 14 on a 20-sided die", "The twenty-sided die rolled a number of 14.", 0.7, 3.0),
    ];
    for (i, (tool, reply, made, length)) in asked.into_iter().enumerate() {
        let mut t = take(s, i as u64 + 1, s.selected, made, length, 0.0, 3.4);
        t.tool = tool.into();
        t.reply = reply.into();
        s.takes.push(t);
    }
}

/// A line the enhancer has marked up for a model with tags of its own, with Revert on offer.
fn enhanced(s: &mut App) {
    all_ready(s);
    if let Some(i) = s.catalog.index("kitten") {
        s.select(i);
    }
    s.enhanced_from = Some(("we won the grant i can hardly believe it, 2.5 million dollars. i told you never to doubt us".into(), String::new()));
    s.text.replace_text("[excited] We won the grant! <gasp> I can hardly believe it. Two point five million dollars. I told you (((never))) to doubt us!");
}

/// The standing instructions open beside Enhance.
fn instructions(s: &mut App) {
    enhanced(s);
    s.enhanced_from = None;
    s.text.replace_text("we won the grant i can hardly believe it");
    s.preprompt.replace_text("Make every line sound tired and a little sarcastic. Never add laughter.");
    s.preprompt_open = true;
}

/// The scene writer open on a model that voices several characters.
fn scene(s: &mut App) {
    all_ready(s);
    if let Some(i) = s.catalog.index("drama3") {
        s.select(i);
    }
    s.text.replace_text("<|speaker:0|> Are you serious? We passed that sign five miles ago.\n<|speaker:1|> I thought I had it. The next one should be...\n<|speaker:0|> Next? We've been on this road for twenty minutes!\n<|speaker:1|> [sighs] Well, we're almost there. It's just a slight adjustment.");
    s.scene.replace_text("A couple in the car after he misses the exit again. She is furious; he stays calm and makes it worse.");
    s.scene_open = true;
}
