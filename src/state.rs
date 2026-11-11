//! The app's state and everything that changes it. Views only read this and name a method.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rune::{Player, PlayerState, RecordFormat, Recorder, RecorderState, TextEdit, Tx, Waker};
use rune_kit::PaneState;
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use crate::catalog::{Catalog, Cloning, Instruct, Kind, Model};
use crate::engine::{Engine, Msg};
use crate::{paths, wav};

/// Bars in a take's waveform.
pub const PEAKS: usize = 96;
/// Bars in a clip chip's waveform.
pub const CLIP_PEAKS: usize = 40;
const HISTORY_LIMIT: usize = 300;
const LOG_LINES: usize = 400;

pub const AUDIO_EXTENSIONS: &[&str] = &["wav", "mp3", "m4a", "aac", "flac", "aif", "aiff", "caf", "mp4", "mov"];

const DEFAULT_TEXT: &str = "The puppet never knew whose voice it was. It only knew that when the hand moved, the words came out sounding like someone it had almost been.";

pub enum Install {
    Missing,
    Installing { log: Vec<String> },
    Ready,
    /// Being deleted from this Mac.
    Removing,
    Failed { log: Vec<String> },
}

impl Install {
    pub fn is_ready(&self) -> bool {
        matches!(self, Install::Ready)
    }

    /// Whether the model belongs in the sidebar: downloaded, or on its way.
    pub fn is_listed(&self) -> bool {
        matches!(self, Install::Ready | Install::Installing { .. })
    }
}

/// What the main pane shows.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Page {
    /// The selected model's bench.
    Bench,
    /// Every model in the catalog, to download or remove.
    Models,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub enum TakeStatus {
    Queued,
    /// Running, with what the worker last said it was doing.
    Running(String),
    #[default]
    Done,
    Failed(String),
    Cancelled,
}

/// One run of one model: what went in, and the audio that came out.
#[derive(Clone, Debug, Serialize, Deserialize)]
pub struct Take {
    pub id: u64,
    pub model: String,
    pub kind: Kind,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub voice: Option<String>,
    #[serde(default)]
    pub reference: Option<PathBuf>,
    #[serde(default)]
    pub ref_text: String,
    #[serde(default)]
    pub instruct: String,
    #[serde(default)]
    pub source: Option<PathBuf>,
    /// What a conversation model said back, as text.
    #[serde(default)]
    pub reply: String,
    /// The tool a conversation model called to answer, and what it gave back.
    #[serde(default)]
    pub tool: String,
    pub out: PathBuf,
    #[serde(default)]
    pub status: TakeStatus,
    /// Seconds spent generating, without loading the model.
    #[serde(default)]
    pub gen_seconds: f64,
    /// Seconds spent loading the model, when this run had to (0 when it was already loaded).
    #[serde(default)]
    pub load_seconds: f64,
    #[serde(default)]
    pub audio_seconds: f64,
    /// The most memory the run held at once, in GB (0 when the engine can't say).
    #[serde(default)]
    pub peak_gb: f64,
    /// Unix seconds.
    #[serde(default)]
    pub created: u64,
    #[serde(default)]
    pub peaks: Vec<f32>,
}

impl Take {
    pub fn is_pending(&self) -> bool {
        matches!(self.status, TakeStatus::Queued | TakeStatus::Running(_))
    }

    /// Seconds of audio made per second of work: above 1 is faster than real time.
    pub fn speed(&self) -> Option<f64> {
        (self.gen_seconds > 0.0 && self.audio_seconds > 0.0).then(|| self.audio_seconds / self.gen_seconds)
    }
}

/// An audio file the user gave a model: a voice to imitate, or speech to convert or answer.
#[derive(Clone, Debug)]
pub struct Clip {
    pub path: PathBuf,
    pub name: String,
    pub seconds: Option<f64>,
    pub peaks: Vec<f32>,
}

impl Clip {
    pub fn open(path: PathBuf, name: Option<String>) -> Clip {
        let shape = wav::shape(&path, CLIP_PEAKS);
        Clip {
            name: name.unwrap_or_else(|| paths::display_name(&path)),
            seconds: shape.as_ref().map(|s| s.seconds),
            peaks: shape.map(|s| s.peaks).unwrap_or_default(),
            path,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Slot {
    /// The voice to speak in.
    Reference,
    /// The speech to convert, or to reply to.
    Source,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum PlayKey {
    Take(u64),
    Slot(Slot),
    /// A voice's preview: indices into the catalog's models and that model's voices.
    Voice { model: usize, voice: usize },
}

/// Request ids from here up are voice previews; below are takes.
const PREVIEW_IDS: u64 = 1 << 40;
/// And from here up, lines sent to the prompt enhancer.
const ENHANCE_IDS: u64 = 1 << 41;
/// The catalog id of the language model behind the Enhance button.
pub const ENHANCER: &str = "enhancer";

/// A voice preview the worker is making.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Previewing {
    pub model: usize,
    pub voice: usize,
    id: u64,
}

pub struct Playing {
    pub key: PlayKey,
    pub player: Player,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Scope {
    All,
    Model,
}

pub struct App {
    pub catalog: Catalog,
    /// False in snapshots and tests: no processes are started and nothing is written to disk.
    pub live: bool,
    pub sidebar: PaneState,
    pub page: Page,
    pub selected: usize,
    /// The model whose Remove button was pressed once and is asking to be sure.
    pub confirm_remove: Option<usize>,
    pub install: Vec<Install>,
    pub text: TextEdit,
    pub ref_text: TextEdit,
    /// A voice described in words, or how to deliver the line, for models that take direction.
    pub instruct: TextEdit,
    /// The voice chosen for each model that has voices, by model id.
    pub voices: HashMap<String, String>,
    pub voice_menu: bool,
    /// What is typed in the voice list's search field.
    pub voice_filter: TextEdit,
    pub previewing: Option<Previewing>,
    next_preview: u64,
    /// The line the enhancer is rewriting: its request id and the text as it was sent.
    enhancing: Option<(u64, String)>,
    next_enhance: u64,
    /// The text and direction as they were before the last enhancement, to go back to.
    pub enhanced_from: Option<(String, String)>,
    /// Standing instructions the enhancer follows on every line, kept between launches.
    pub preprompt: TextEdit,
    pub preprompt_open: bool,
    /// A scene described in words, for the enhancer to write the dialogue of.
    pub scene: TextEdit,
    pub scene_open: bool,
    /// How much dialogue to write: 0 short, 1 medium, 2 long.
    pub scene_length: usize,
    /// The pending rewrite is a scene being written, not a line being enhanced.
    pub writing_scene: bool,
    pub reference: Option<Clip>,
    pub source: Option<Clip>,
    pub recording: Option<(Slot, Recorder)>,
    pub drop_over: Option<Slot>,
    /// Oldest first; the view shows them newest first.
    pub takes: Vec<Take>,
    pub next_id: u64,
    pub active: Option<u64>,
    /// When the running take started, for the time shown beside it.
    pub active_since: Option<std::time::Instant>,
    pub playing: Option<Playing>,
    pub scope: Scope,
    pub notice: Option<String>,
    pub show_log: bool,
    /// Draw the app icon instead of the window (`--scene icon`), to render the bundle's icon.
    pub icon: Option<crate::view::app_icon::Art>,
    pub engine: Engine,
    pub waker: Option<Waker>,
    /// What each environment's worker last printed, for the error a failed run shows.
    logs: HashMap<String, VecDeque<String>>,
}

fn now() -> u64 {
    SystemTime::now().duration_since(UNIX_EPOCH).map(|d| d.as_secs()).unwrap_or(0)
}

impl App {
    pub fn new(live: bool) -> App {
        let catalog = Catalog::load();
        let mut app = App {
            install: catalog.models.iter().map(|_| Install::Missing).collect(),
            voices: catalog.models.iter().filter_map(|m| Some((m.id.clone(), m.voice_default.clone()?))).collect(),
            catalog,
            live,
            sidebar: PaneState::new(264.0).range(200.0, 420.0),
            page: Page::Bench,
            selected: 0,
            confirm_remove: None,
            text: TextEdit::new(DEFAULT_TEXT),
            ref_text: TextEdit::new(""),
            instruct: TextEdit::new(""),
            voice_menu: false,
            voice_filter: TextEdit::new(""),
            previewing: None,
            next_preview: PREVIEW_IDS,
            enhancing: None,
            next_enhance: ENHANCE_IDS,
            enhanced_from: None,
            preprompt: TextEdit::new(""),
            preprompt_open: false,
            scene: TextEdit::new(""),
            scene_open: false,
            scene_length: 0,
            writing_scene: false,
            reference: None,
            source: None,
            recording: None,
            drop_over: None,
            takes: Vec::new(),
            next_id: 1,
            active: None,
            active_since: None,
            playing: None,
            scope: Scope::All,
            notice: None,
            show_log: false,
            icon: None,
            engine: Engine::new(),
            waker: None,
            logs: HashMap::new(),
        };
        if live {
            if let Err(e) = paths::install_sidecar() {
                app.notice = Some(format!("Couldn't write to {}: {e}", paths::data_dir().display()));
            }
            app.refresh_installed();
            app.load_history();
            app.load_settings();
        }
        // Open on something that can run, or on the list of models when nothing is downloaded.
        app.settle_selection();
        app
    }

    pub fn model(&self) -> &Model {
        &self.catalog.models[self.selected]
    }

    pub fn refresh_installed(&mut self) {
        for (i, m) in self.catalog.models.iter().enumerate() {
            if matches!(self.install[i], Install::Installing { .. } | Install::Removing) {
                continue;
            }
            let ready = paths::installed_marker(&m.id).is_file() && paths::env_python(&m.env).is_file();
            if ready {
                self.install[i] = Install::Ready;
            } else if self.install[i].is_ready() {
                self.install[i] = Install::Missing;
            }
        }
    }

    /// Keeps the bench on a model the sidebar lists. With none, the Models page takes over.
    fn settle_selection(&mut self) {
        if self.install[self.selected].is_listed() && self.model().kind != Kind::Helper {
            return;
        }
        // A helper has no bench: only voice models are landed on.
        let bench = |i: usize| self.catalog.models[i].kind != Kind::Helper;
        let pick = |want: fn(&Install) -> bool| (0..self.install.len()).find(|&i| bench(i) && want(&self.install[i]));
        match pick(Install::is_ready).or_else(|| pick(Install::is_listed)) {
            Some(i) => self.selected = i,
            None => self.page = Page::Models,
        }
    }

    pub fn show_models(&mut self) {
        self.page = Page::Models;
        self.voice_menu = false;
    }

    pub fn select(&mut self, index: usize) {
        if index < self.catalog.models.len() {
            self.page = Page::Bench;
            self.confirm_remove = None;
            self.selected = index;
            self.voice_menu = false;
            self.show_log = false;
        }
    }

    pub fn voice(&self, model: &Model) -> Option<&str> {
        self.voices.get(&model.id).map(String::as_str)
    }

    /// The chosen voice's place in the model's list.
    pub fn voice_index(&self, model: &Model) -> Option<usize> {
        let id = self.voice(model)?;
        model.voices.iter().position(|v| v.id == id)
    }

    pub fn toggle_voice_menu(&mut self) {
        self.voice_menu = !self.voice_menu;
        if self.voice_menu {
            self.voice_filter = TextEdit::new("");
            self.voice_filter.focused = true;
        }
    }

    pub fn choose_voice(&mut self, model: usize, voice: usize) {
        let m = &self.catalog.models[model];
        if let Some(v) = m.voices.get(voice) {
            self.voices.insert(m.id.clone(), v.id.clone());
        }
        self.voice_menu = false;
    }

    // ----- The prompt enhancer -----

    pub fn enhancer_ready(&self) -> bool {
        self.catalog.index(ENHANCER).is_some_and(|i| self.install[i].is_ready())
    }

    pub fn is_enhancing(&self) -> bool {
        self.enhancing.is_some()
    }

    /// Has the local language model rewrite the line for the selected voice model: punctuation
    /// for delivery, numbers as words, and the expression marks that model understands. For a
    /// model that takes direction it also writes that, when the field is empty.
    pub fn enhance(&mut self) {
        let Some(helper) = self.catalog.index(ENHANCER) else { return };
        if !self.install[helper].is_ready() {
            // Not downloaded: take them to where it can be.
            self.show_models();
            return;
        }
        let text = self.text.text.trim().to_owned();
        if self.enhancing.is_some() || text.is_empty() || self.model().kind != Kind::Tts {
            return;
        }
        let id = self.next_enhance;
        self.next_enhance += 1;
        if self.live {
            let request = self.enhance_request(id, &text);
            let env = self.catalog.models[helper].env.clone();
            if let Err(e) = self.engine.send(&env, &request) {
                self.notice = Some(e);
                return;
            }
        }
        self.notice = None;
        self.close_preprompt();
        self.scene_open = false;
        self.writing_scene = false;
        self.enhancing = Some((id, self.text.text.clone()));
    }

    pub const SCENE_LENGTHS: [&'static str; 3] = ["short", "medium", "long"];

    /// Opens or closes the scene writer.
    pub fn toggle_scene(&mut self) {
        self.scene_open = !self.scene_open;
        if self.scene_open {
            self.close_preprompt();
            self.scene.focused = true;
        }
    }

    /// What the enhancer is sent to write a scene: the direction, the model that will perform
    /// it (which decides the speaker tags and marks), how long, and the standing instructions.
    fn scene_request(&self, id: u64) -> Value {
        json!({
            "op": "scene",
            "id": id,
            "model": ENHANCER,
            "target": self.model().id,
            "text": self.scene.text.trim(),
            "length": Self::SCENE_LENGTHS[self.scene_length.min(2)],
            "preprompt": self.preprompt.text.trim(),
        })
    }

    /// Has the local language model write the dialogue for the described scene, in the form
    /// the selected voice model performs. It replaces the text; Revert brings the old text back.
    pub fn write_scene(&mut self) {
        let Some(helper) = self.catalog.index(ENHANCER) else { return };
        if !self.install[helper].is_ready() {
            self.show_models();
            return;
        }
        if self.enhancing.is_some() || self.scene.text.trim().is_empty() || self.model().kind != Kind::Tts {
            return;
        }
        let id = self.next_enhance;
        self.next_enhance += 1;
        if self.live {
            let request = self.scene_request(id);
            let env = self.catalog.models[helper].env.clone();
            if let Err(e) = self.engine.send(&env, &request) {
                self.notice = Some(e);
                return;
            }
        }
        self.notice = None;
        self.scene_open = false;
        self.writing_scene = true;
        self.enhancing = Some((id, self.text.text.clone()));
    }

    /// What the enhancer is sent: the line, the model it is for, and the standing instructions.
    fn enhance_request(&self, id: u64, text: &str) -> Value {
        json!({
            "op": "enhance",
            "id": id,
            "model": ENHANCER,
            "target": self.model().id,
            "text": text,
            "instruct": self.instruct.text.trim(),
            "preprompt": self.preprompt.text.trim(),
        })
    }

    /// Opens or closes the standing instructions. Closing saves them.
    pub fn toggle_preprompt(&mut self) {
        self.preprompt_open = !self.preprompt_open;
        if self.preprompt_open {
            self.scene_open = false;
            self.preprompt.focused = true;
        } else {
            self.save_settings();
        }
    }

    pub fn close_preprompt(&mut self) {
        if self.preprompt_open {
            self.preprompt_open = false;
            self.save_settings();
        }
    }

    /// Brings back what was in the window when it last closed: the standing instructions, the
    /// text and its direction, the clips in the slots, the voices chosen and the model open.
    fn load_settings(&mut self) {
        let Ok(text) = std::fs::read_to_string(paths::settings_file()) else { return };
        if let Ok(value) = serde_json::from_str::<Value>(&text) {
            self.restore(&value);
        }
    }

    fn restore(&mut self, value: &Value) {
        let field = |key: &str| value.get(key).and_then(Value::as_str);
        if let Some(preprompt) = field("preprompt") {
            self.preprompt = TextEdit::new(preprompt);
        }
        if let Some(text) = field("text").filter(|t| !t.trim().is_empty()) {
            self.text = TextEdit::new(text);
        }
        for (key, edit) in [("instruct", &mut self.instruct), ("ref_text", &mut self.ref_text), ("scene", &mut self.scene)] {
            if let Some(text) = value.get(key).and_then(Value::as_str) {
                *edit = TextEdit::new(text);
            }
        }
        self.scene_length = value.get("scene_length").and_then(Value::as_u64).unwrap_or(0).min(2) as usize;
        for (key, slot) in [("reference", Slot::Reference), ("source", Slot::Source)] {
            let clip = value.get(key);
            let path = clip.and_then(|c| c.get("path")).and_then(Value::as_str).map(PathBuf::from);
            let name = clip.and_then(|c| c.get("name")).and_then(Value::as_str).map(str::to_owned);
            // A clip whose file has since gone is simply not there any more.
            if let Some(path) = path.filter(|p| p.is_file()) {
                *self.clip_mut(slot) = Some(Clip::open(path, name));
            }
        }
        if let Some(voices) = value.get("voices").and_then(Value::as_object) {
            for (model, voice) in voices {
                let known = self.catalog.models.iter().any(|m| &m.id == model && m.voices.iter().any(|v| Some(v.id.as_str()) == voice.as_str()));
                if let (true, Some(voice)) = (known, voice.as_str()) {
                    self.voices.insert(model.clone(), voice.to_owned());
                }
            }
        }
        if let Some(i) = field("model").and_then(|id| self.catalog.index(id)) {
            self.selected = i;
        }
    }

    pub fn save_settings(&self) {
        if !self.live {
            return;
        }
        let tmp = paths::settings_file().with_extension("json.tmp");
        if std::fs::write(&tmp, self.session().to_string()).is_ok() {
            std::fs::rename(tmp, paths::settings_file()).ok();
        }
    }

    /// What is kept between launches.
    fn session(&self) -> Value {
        let clip = |c: &Option<Clip>| c.as_ref().map(|c| json!({ "path": c.path, "name": c.name }));
        json!({
            "preprompt": self.preprompt.text.trim(),
            "text": self.text.text,
            "instruct": self.instruct.text,
            "ref_text": self.ref_text.text,
            "scene": self.scene.text,
            "scene_length": self.scene_length,
            "reference": clip(&self.reference),
            "source": clip(&self.source),
            "voices": self.voices,
            "model": self.model().id,
        })
    }

    /// Puts back what was there before the last enhancement.
    pub fn revert_enhance(&mut self) {
        if let Some((text, instruct)) = self.enhanced_from.take() {
            self.text.replace_text(text);
            self.instruct.replace_text(instruct);
        }
    }

    fn on_enhance(&mut self, id: u64, event: &str, value: &Value) {
        let Some((_, sent)) = self.enhancing.as_ref().filter(|(pending, _)| *pending == id).cloned() else { return };
        let field = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or("").trim().to_owned();
        match event {
            "done" => {
                self.enhancing = None;
                // Typing while it worked wins: the answer was for a line that is no longer there.
                if self.text.text != sent {
                    return;
                }
                let (line, direction) = (field("text"), field("direction"));
                if line.is_empty() {
                    return;
                }
                self.enhanced_from = Some((sent, self.instruct.text.clone()));
                self.text.replace_text(line);
                if !direction.is_empty() && self.instruct.text.trim().is_empty() {
                    self.instruct.replace_text(direction);
                }
            }
            "error" => {
                self.enhancing = None;
                let what = if self.writing_scene { "write that scene" } else { "enhance that" };
                self.notice = Some(format!("Couldn't {what}: {}", field("message")));
            }
            _ => {}
        }
    }

    // ----- Voice previews -----

    /// Plays a short line in a voice. The first listen has the model make it (and load, if
    /// it hasn't yet); after that it plays from disk. Pressing again while it plays stops it.
    pub fn preview_voice(&mut self, model: usize, voice: usize) {
        let key = PlayKey::Voice { model, voice };
        if self.is_playing(key) {
            self.playing = None;
            return;
        }
        let m = &self.catalog.models[model];
        let Some(v) = m.voices.get(voice) else { return };
        let path = paths::preview_path(&m.id, &v.id);
        if !self.live {
            self.previewing = Some(Previewing { model, voice, id: self.next_preview });
            return;
        }
        if path.is_file() {
            self.previewing = None;
            self.play(key, &path);
            return;
        }
        let id = self.next_preview;
        self.next_preview += 1;
        let request = json!({
            "id": id,
            "model": m.id,
            "text": v.preview_text(),
            "voice": v.id,
            "ref_audio": null,
            "ref_text": "",
            "instruct": "",
            "source_audio": null,
            "out": path,
        });
        let env = m.env.clone();
        match self.engine.send(&env, &request) {
            Ok(()) => self.previewing = Some(Previewing { model, voice, id }),
            Err(e) => self.notice = Some(e),
        }
    }

    /// Forgets a preview or an enhancement whose engine was just stopped: its answer will
    /// never come.
    fn drop_preview_in(&mut self, env: &str) {
        if self.previewing.is_some_and(|p| self.catalog.models[p.model].env == env) {
            self.previewing = None;
        }
        if self.catalog.index(ENHANCER).is_some_and(|i| self.catalog.models[i].env == env) {
            self.enhancing = None;
        }
    }

    pub fn is_previewing(&self, model: usize, voice: usize) -> bool {
        self.previewing.is_some_and(|p| p.model == model && p.voice == voice)
    }

    fn on_preview(&mut self, id: u64, event: &str, message: String) {
        let Some(p) = self.previewing.filter(|p| p.id == id) else { return };
        match event {
            "done" => {
                self.previewing = None;
                let m = &self.catalog.models[p.model];
                let path = paths::preview_path(&m.id, &m.voices[p.voice].id);
                self.play(PlayKey::Voice { model: p.model, voice: p.voice }, &path);
            }
            "error" => {
                self.previewing = None;
                self.notice = Some(format!("Couldn't preview that voice: {message}"));
            }
            _ => {}
        }
    }

    pub fn clip(&self, slot: Slot) -> Option<&Clip> {
        match slot {
            Slot::Reference => self.reference.as_ref(),
            Slot::Source => self.source.as_ref(),
        }
    }

    fn clip_mut(&mut self, slot: Slot) -> &mut Option<Clip> {
        match slot {
            Slot::Reference => &mut self.reference,
            Slot::Source => &mut self.source,
        }
    }

    // ----- What a model needs before it can run -----

    /// Why the selected model can't run yet, in words for the button's help. `None` when it can.
    pub fn blocked(&self, index: usize) -> Option<&'static str> {
        let m = &self.catalog.models[index];
        if !self.install[index].is_ready() {
            return Some("Install this model first.");
        }
        match m.kind {
            Kind::Tts => {
                if self.text.text.trim().is_empty() {
                    return Some("Type something to say.");
                }
                if m.cloning == Cloning::Required && self.reference.is_none() {
                    return Some("Add a voice sample to imitate.");
                }
                if m.instruct == Instruct::Required && self.instruct.text.trim().is_empty() {
                    return Some("Describe the voice it should speak in.");
                }
            }
            Kind::Vc => {
                if self.source.is_none() {
                    return Some("Add the speech to convert.");
                }
                if self.reference.is_none() {
                    return Some("Add a sample of the voice to convert it to.");
                }
            }
            Kind::Chat => {
                if self.source.is_none() {
                    return Some("Record something to say to it.");
                }
            }
            Kind::Helper => return Some("This isn't a voice model."),
        }
        None
    }

    // ----- Runs -----

    fn enqueue(&mut self, index: usize) {
        let m = self.catalog.models[index].clone();
        let id = self.next_id;
        self.next_id += 1;
        let uses_voice = m.cloning != Cloning::None && self.reference.is_some();
        let take = Take {
            id,
            model: m.id.clone(),
            kind: m.kind,
            text: if m.kind == Kind::Tts { self.text.text.trim().to_owned() } else { String::new() },
            // A voice sample, when there is one, wins over the model's own voices.
            voice: if uses_voice || m.kind == Kind::Vc { None } else { self.voice(&m).map(str::to_owned) },
            reference: if uses_voice || m.kind == Kind::Vc { self.reference.as_ref().map(|c| c.path.clone()) } else { None },
            ref_text: if uses_voice && m.ref_text { self.ref_text.text.trim().to_owned() } else { String::new() },
            instruct: if m.kind == Kind::Tts && m.instruct != Instruct::None { self.instruct.text.trim().to_owned() } else { String::new() },
            source: if m.kind == Kind::Tts { None } else { self.source.as_ref().map(|c| c.path.clone()) },
            reply: String::new(),
            tool: String::new(),
            out: paths::take_path(&m.id, id),
            status: TakeStatus::Queued,
            gen_seconds: 0.0,
            load_seconds: 0.0,
            audio_seconds: 0.0,
            peak_gb: 0.0,
            created: now(),
            peaks: Vec::new(),
        };
        self.takes.push(take);
    }

    /// Runs the selected model on what the composer holds.
    pub fn generate(&mut self) {
        if self.blocked(self.selected).is_some() {
            return;
        }
        self.notice = None;
        self.enqueue(self.selected);
        self.pump();
    }

    /// Runs every installed model of the selected kind on the same input, to compare them.
    pub fn generate_all(&mut self) {
        let kind = self.model().kind;
        let runnable: Vec<usize> =
            (0..self.catalog.models.len()).filter(|&i| self.catalog.models[i].kind == kind && self.blocked(i).is_none()).collect();
        if runnable.is_empty() {
            return;
        }
        self.notice = None;
        for i in runnable {
            self.enqueue(i);
        }
        self.scope = Scope::All;
        self.pump();
    }

    /// How long the running take has been going, when `id` is it.
    pub fn elapsed(&self, id: u64) -> Option<f64> {
        (self.active == Some(id)).then(|| self.active_since.map(|t| t.elapsed().as_secs_f64())).flatten()
    }

    /// How many models "run all" would run.
    pub fn runnable(&self) -> usize {
        let kind = self.model().kind;
        (0..self.catalog.models.len()).filter(|&i| self.catalog.models[i].kind == kind && self.blocked(i).is_none()).count()
    }

    /// Starts the next queued take when nothing is running. One at a time, so timings are fair.
    fn pump(&mut self) {
        while self.active.is_none() {
            let Some(pos) = self.takes.iter().position(|t| t.status == TakeStatus::Queued) else { return };
            let take = self.takes[pos].clone();
            let Some(model) = self.catalog.models.iter().find(|m| m.id == take.model).cloned() else {
                self.takes[pos].status = TakeStatus::Failed("This model is no longer in the catalog.".into());
                continue;
            };
            if !self.live {
                self.takes[pos].status = TakeStatus::Running("Loading model".into());
                self.active = Some(take.id);
                self.active_since = Some(std::time::Instant::now());
                return;
            }
            let request = json!({
                "id": take.id,
                "model": take.model,
                "text": take.text,
                "voice": take.voice,
                "ref_audio": take.reference,
                "ref_text": take.ref_text,
                "instruct": take.instruct,
                "source_audio": take.source,
                "out": take.out,
            });
            let cold = !self.engine.is_running(&model.env);
            match self.engine.send(&model.env, &request) {
                Ok(()) => {
                    self.takes[pos].status = TakeStatus::Running(if cold { "Starting engine" } else { "Starting" }.into());
                    self.active = Some(take.id);
                    self.active_since = Some(std::time::Instant::now());
                }
                Err(e) => self.takes[pos].status = TakeStatus::Failed(e),
            }
        }
    }

    fn take_mut(&mut self, id: u64) -> Option<&mut Take> {
        self.takes.iter_mut().find(|t| t.id == id)
    }

    /// Stops the running take by ending its engine, and drops everything still queued.
    pub fn cancel(&mut self) {
        for t in self.takes.iter_mut().filter(|t| t.status == TakeStatus::Queued) {
            t.status = TakeStatus::Cancelled;
        }
        if let Some(id) = self.active.take() {
            let env = self.take_mut(id).map(|t| {
                t.status = TakeStatus::Cancelled;
                t.model.clone()
            });
            let env = env.and_then(|model| self.catalog.models.iter().find(|m| m.id == model).map(|m| m.env.clone()));
            if let Some(env) = env {
                self.engine.stop(&env);
                self.drop_preview_in(&env);
            }
        }
        self.takes.retain(|t| t.status != TakeStatus::Cancelled);
    }

    pub fn stop_engines(&mut self) {
        self.previewing = None;
        self.enhancing = None;
        self.cancel();
        self.engine.stop_all();
    }

    fn log_tail(&self, env: &str, lines: usize) -> String {
        let Some(log) = self.logs.get(env) else { return String::new() };
        let skip = log.len().saturating_sub(lines);
        log.iter().skip(skip).cloned().collect::<Vec<_>>().join("\n")
    }

    fn on_worker(&mut self, value: &Value) {
        let Some(id) = value.get("id").and_then(Value::as_u64) else { return };
        let event = value.get("event").and_then(Value::as_str).unwrap_or("");
        let text = |key: &str| value.get(key).and_then(Value::as_str).unwrap_or("").to_owned();
        if id >= ENHANCE_IDS {
            return self.on_enhance(id, event, value);
        }
        if id >= PREVIEW_IDS {
            return self.on_preview(id, event, text("message"));
        }
        let num = |key: &str| value.get(key).and_then(Value::as_f64).unwrap_or(0.0);
        match event {
            "status" => {
                if let Some(t) = self.take_mut(id).filter(|t| t.is_pending()) {
                    t.status = TakeStatus::Running(text("message"));
                }
            }
            "done" => {
                let autoplay = self.playing.as_ref().is_none_or(|p| !p.player.is_playing())
                    && !self.takes.iter().any(|t| t.status == TakeStatus::Queued);
                let mut play = None;
                if let Some(t) = self.take_mut(id) {
                    t.status = TakeStatus::Done;
                    t.gen_seconds = num("gen_seconds");
                    t.load_seconds = num("load_seconds");
                    t.audio_seconds = num("audio_seconds");
                    t.peak_gb = num("peak_gb");
                    t.reply = text("text");
                    t.tool = text("tool");
                    if let Some(shape) = wav::shape(&t.out, PEAKS) {
                        t.peaks = shape.peaks;
                        if t.audio_seconds <= 0.0 {
                            t.audio_seconds = shape.seconds;
                        }
                    }
                    play = autoplay.then(|| (t.id, t.out.clone()));
                }
                if self.active == Some(id) {
                    self.active = None;
                }
                self.save_history();
                if let Some((id, path)) = play {
                    self.play(PlayKey::Take(id), &path);
                }
                self.pump();
            }
            "error" => {
                if let Some(t) = self.take_mut(id) {
                    t.status = TakeStatus::Failed(text("message"));
                }
                if self.active == Some(id) {
                    self.active = None;
                }
                self.pump();
            }
            _ => {}
        }
    }

    fn on_worker_exit(&mut self, env: &str) {
        // Whatever was running in that environment died with it.
        self.drop_preview_in(env);
        let Some(id) = self.active else { return };
        let model = self.takes.iter().find(|t| t.id == id).map(|t| t.model.clone());
        let same_env = model.and_then(|m| self.catalog.models.iter().find(|x| x.id == m)).is_some_and(|m| m.env == env);
        if !same_env {
            return;
        }
        let tail = self.log_tail(env, 6);
        if let Some(t) = self.take_mut(id) {
            t.status = TakeStatus::Failed(if tail.is_empty() { "The engine quit unexpectedly.".into() } else { format!("The engine quit unexpectedly.\n{tail}") });
        }
        self.active = None;
        self.pump();
    }

    // ----- Downloading and removing -----

    /// Downloads a model: builds its environment and fetches its weights.
    pub fn install(&mut self, index: usize) {
        if matches!(self.install[index], Install::Installing { .. } | Install::Ready | Install::Removing) {
            return;
        }
        let id = self.catalog.models[index].id.clone();
        if !self.live {
            self.install[index] = Install::Installing { log: vec!["Resolving packages".into()] };
            return;
        }
        self.install[index] = match self.engine.setup(&id, false) {
            Ok(()) => Install::Installing { log: Vec::new() },
            Err(e) => Install::Failed { log: vec![e] },
        };
    }

    pub fn install_all(&mut self) {
        for i in 0..self.catalog.models.len() {
            self.install(i);
        }
    }

    /// Stops a download. What it fetched stays cached, so a later download resumes.
    pub fn cancel_install(&mut self, index: usize) {
        if matches!(self.install[index], Install::Installing { .. }) {
            self.engine.cancel_setup(&self.catalog.models[index].id);
            self.install[index] = Install::Missing;
            self.settle_selection();
        }
    }

    /// Deletes a downloaded model's weights, and its environment once no other model uses it.
    pub fn remove(&mut self, index: usize) {
        self.confirm_remove = None;
        if !self.install[index].is_ready() {
            return;
        }
        let model = self.catalog.models[index].clone();
        // Nothing may be running in an environment that is about to lose its files.
        let busy = self.active.and_then(|id| self.takes.iter().find(|t| t.id == id)).is_some_and(|t| {
            self.catalog.models.iter().any(|m| m.id == t.model && m.env == model.env)
        });
        if busy {
            self.cancel();
        }
        self.engine.stop(&model.env);
        self.drop_preview_in(&model.env);
        if !self.live {
            self.install[index] = Install::Missing;
            self.settle_selection();
            return;
        }
        self.install[index] = match self.engine.setup(&model.id, true) {
            Ok(()) => Install::Removing,
            Err(e) => {
                self.notice = Some(e);
                Install::Ready
            }
        };
        self.settle_selection();
    }

    pub fn missing(&self) -> usize {
        self.install.iter().filter(|s| matches!(s, Install::Missing | Install::Failed { .. })).count()
    }

    pub fn downloaded(&self) -> usize {
        self.install.iter().filter(|s| s.is_ready()).count()
    }

    /// The download size of the models in a given state, in GB.
    pub fn size_where(&self, pick: impl Fn(&Install) -> bool) -> f64 {
        self.catalog.models.iter().zip(&self.install).filter(|(_, s)| pick(s)).map(|(m, _)| m.size_gb()).sum()
    }

    // ----- Messages from the Python side -----

    /// Applies everything the workers and installers said since the last frame.
    pub fn drain(&mut self) -> bool {
        let msgs = self.engine.drain();
        let changed = !msgs.is_empty();
        for msg in msgs {
            match msg {
                Msg::Worker { value } => self.on_worker(&value),
                Msg::WorkerLog { env, line } => {
                    let log = self.logs.entry(env).or_default();
                    if log.len() >= LOG_LINES {
                        log.pop_front();
                    }
                    log.push_back(line);
                }
                Msg::WorkerExit { env, generation } => {
                    if self.engine.exited(&env, generation) {
                        self.on_worker_exit(&env);
                    }
                }
                Msg::Setup { model, line } => {
                    if let Some(i) = self.catalog.index(&model)
                        && let Install::Installing { log } = &mut self.install[i]
                    {
                        if log.len() >= LOG_LINES {
                            log.remove(0);
                        }
                        log.push(line);
                    }
                }
                Msg::SetupExit { model, ok } => {
                    self.engine.installer_done(&model);
                    if let Some(i) = self.catalog.index(&model) {
                        // The marker file is the truth: the installer writes it last and the
                        // remover deletes it first. `refresh_installed` reads it below.
                        self.install[i] = match std::mem::replace(&mut self.install[i], Install::Missing) {
                            Install::Installing { log } if !ok => Install::Failed { log },
                            Install::Failed { log } => Install::Failed { log },
                            _ => Install::Missing,
                        };
                        if !ok && matches!(self.install[i], Install::Missing) && paths::installed_marker(&model).is_file() {
                            self.notice = Some(format!("Couldn't remove {}.", self.catalog.models[i].name));
                        }
                    }
                    self.refresh_installed();
                    self.settle_selection();
                }
            }
        }
        changed
    }

    // ----- Playback -----

    pub fn play(&mut self, key: PlayKey, path: &Path) {
        if let Some(p) = &self.playing {
            p.player.pause();
        }
        let player = Player::audio(path.to_path_buf());
        if let Some(w) = &self.waker {
            player.set_waker(w.clone());
        }
        player.play();
        self.playing = Some(Playing { key, player });
    }

    pub fn toggle_play(&mut self, key: PlayKey, path: &Path) {
        match &self.playing {
            Some(p) if p.key == key && !matches!(p.player.state(), PlayerState::Failed(_)) => {
                if p.player.is_playing() { p.player.pause() } else { p.player.play() }
            }
            _ => self.play(key, path),
        }
    }

    pub fn is_playing(&self, key: PlayKey) -> bool {
        self.playing.as_ref().is_some_and(|p| p.key == key && p.player.is_playing())
    }

    /// How far through `key`'s audio playback is, 0..1, when it is the clip loaded.
    pub fn progress(&self, key: PlayKey) -> Option<f32> {
        let p = self.playing.as_ref().filter(|p| p.key == key)?;
        if matches!(p.player.state(), PlayerState::Ended) {
            return None;
        }
        let d = p.player.duration().filter(|d| *d > 0.0)?;
        Some((p.player.time() / d).clamp(0.0, 1.0) as f32)
    }

    /// Jumps to a point in a take, starting it if it wasn't the one loaded.
    pub fn seek(&mut self, key: PlayKey, path: &Path, fraction: f32) {
        if self.playing.as_ref().is_none_or(|p| p.key != key) {
            self.play(key, path);
        }
        if let Some(p) = &self.playing {
            let length = p.player.duration().or_else(|| match key {
                PlayKey::Take(id) => self.takes.iter().find(|t| t.id == id).map(|t| t.audio_seconds),
                PlayKey::Slot(slot) => self.clip(slot).and_then(|c| c.seconds),
                PlayKey::Voice { .. } => None,
            });
            if let Some(d) = length {
                p.player.seek(d * fraction.clamp(0.0, 1.0) as f64);
            }
            if !p.player.is_playing() {
                p.player.play();
            }
        }
    }

    // ----- Clips -----

    /// Uses a file as a slot's clip. Anything that isn't a plain WAV is converted to one, which
    /// every model reads and the waveform can be drawn from.
    pub fn set_clip(&mut self, slot: Slot, path: &Path) {
        let name = paths::display_name(path);
        if wav::shape(path, 1).is_some() {
            *self.clip_mut(slot) = Some(Clip::open(path.to_path_buf(), Some(name)));
            return;
        }
        let stem = path.file_stem().map(|s| s.to_string_lossy().into_owned()).unwrap_or_else(|| "clip".into());
        let out = paths::clips_dir().join(format!("{stem}-{}.wav", now()));
        let converted = std::process::Command::new("/usr/bin/afconvert")
            .args(["-f", "WAVE", "-d", "LEI16", "-c", "1"])
            .arg(path)
            .arg(&out)
            .output();
        match converted {
            Ok(o) if o.status.success() => *self.clip_mut(slot) = Some(Clip::open(out, Some(name))),
            Ok(o) => self.notice = Some(format!("Couldn't read {name}: {}", String::from_utf8_lossy(&o.stderr).trim())),
            Err(e) => self.notice = Some(format!("Couldn't read {name}: {e}")),
        }
    }

    pub fn clear_clip(&mut self, slot: Slot) {
        if self.playing.as_ref().is_some_and(|p| p.key == PlayKey::Slot(slot)) {
            self.playing = None;
        }
        *self.clip_mut(slot) = None;
    }

    /// Uses a finished take as a clip: say it with one model, convert it with another.
    pub fn take_to_slot(&mut self, id: u64, slot: Slot) {
        let Some(t) = self.takes.iter().find(|t| t.id == id) else { return };
        let name = format!("{} take", self.catalog.models.iter().find(|m| m.id == t.model).map_or(t.model.as_str(), |m| m.name.as_str()));
        let (path, text) = (t.out.clone(), t.text.clone());
        *self.clip_mut(slot) = Some(Clip::open(path, Some(name)));
        if slot == Slot::Reference && !text.is_empty() {
            self.ref_text.replace_text(&text);
        }
    }

    /// Starts recording into a slot, or stops and keeps what was recorded.
    pub fn toggle_record(&mut self, slot: Slot) {
        if let Some((current, rec)) = self.recording.take() {
            match rec.stop() {
                Ok(recording) if recording.duration >= 0.3 => {
                    *self.clip_mut(current) = Some(Clip::open(recording.path, Some(format!("Recording, {}", clock(recording.duration)))));
                }
                Ok(recording) => {
                    std::fs::remove_file(recording.path).ok();
                }
                Err(e) => self.notice = Some(e),
            }
            if current == slot {
                return;
            }
        }
        if let Some(p) = &self.playing {
            p.player.pause();
        }
        // A snapshot or a test records its fake microphone somewhere it can throw away.
        let dir = if self.live { paths::clips_dir() } else { std::env::temp_dir() };
        let rec = Recorder::to_file(dir.join(format!("recording-{}.wav", now()))).format(RecordFormat::Wav).channels(1);
        if let Some(w) = &self.waker {
            rec.set_waker(w.clone());
        }
        rec.start();
        self.recording = Some((slot, rec));
    }

    /// Follows the recorder: a refusal or an unplugged microphone ends the recording.
    fn watch_recorder(&mut self) -> bool {
        let Some((slot, rec)) = &self.recording else { return false };
        match rec.state() {
            RecorderState::Failed(message) => {
                self.notice = Some(message);
                self.recording = None;
                true
            }
            RecorderState::Interrupted(_) => {
                let slot = *slot;
                self.toggle_record(slot);
                true
            }
            _ => true,
        }
    }

    // ----- Takes -----

    pub fn delete_take(&mut self, id: u64) {
        if self.playing.as_ref().is_some_and(|p| p.key == PlayKey::Take(id)) {
            self.playing = None;
        }
        if let Some(pos) = self.takes.iter().position(|t| t.id == id && !t.is_pending()) {
            let t = self.takes.remove(pos);
            // A take lent to a slot keeps its file until the slot lets go of it.
            let lent = [&self.reference, &self.source].iter().any(|c| c.as_ref().is_some_and(|c| c.path == t.out));
            if self.live && !lent {
                std::fs::remove_file(&t.out).ok();
            }
            self.save_history();
        }
    }

    pub fn reveal(&self, path: &Path) {
        if self.live {
            std::process::Command::new("/usr/bin/open").arg("-R").arg(path).spawn().ok();
        }
    }

    fn load_history(&mut self) {
        let Ok(text) = std::fs::read_to_string(paths::history_file()) else { return };
        let Ok(mut takes) = serde_json::from_str::<Vec<Take>>(&text) else { return };
        takes.retain(|t| t.status == TakeStatus::Done && t.out.is_file());
        self.next_id = takes.iter().map(|t| t.id).max().unwrap_or(0) + 1;
        self.takes = takes;
    }

    fn save_history(&self) {
        if !self.live {
            return;
        }
        let done: Vec<&Take> = self.takes.iter().filter(|t| t.status == TakeStatus::Done).collect();
        let keep = &done[done.len().saturating_sub(HISTORY_LIMIT)..];
        if let Ok(text) = serde_json::to_string(keep) {
            let tmp = paths::history_file().with_extension("json.tmp");
            if std::fs::write(&tmp, text).is_ok() {
                std::fs::rename(tmp, paths::history_file()).ok();
            }
        }
    }

    // ----- The frame loop -----

    /// Runs before every frame; true when something on screen changed.
    pub fn poll(&mut self, tx: &mut Tx) -> bool {
        let mut changed = self.drain();
        if self.watch_recorder() {
            tx.wake_after(Recorder::METER_INTERVAL);
            changed = true;
        }
        if let Some(p) = &self.playing {
            match p.player.state() {
                PlayerState::Playing | PlayerState::Loading => {
                    tx.wake_after(Duration::from_millis(33));
                    changed = true;
                }
                PlayerState::Failed(message) => {
                    self.notice = Some(format!("Couldn't play that: {message}"));
                    self.playing = None;
                    changed = true;
                }
                // The waker ran for the end or a pause: draw the button back.
                _ => changed = true,
            }
        }
        // A running take shows its elapsed time.
        if self.active.is_some() {
            tx.wake_after(Duration::from_millis(100));
            changed = true;
        }
        changed
    }
}

/// Command-Return, the key that starts a run from anywhere in the window.
pub fn is_run_key(ev: &rune::KeyEvent) -> bool {
    matches!(ev.key, rune::Key::Enter) && ev.mods.cmd
}

/// `1:07` for 67 seconds.
pub fn clock(seconds: f64) -> String {
    let s = seconds.max(0.0).round() as u64;
    format!("{}:{:02}", s / 60, s % 60)
}

/// A duration a person reads at a glance: `0.42 s`, `3.1 s`, `1 min 12 s`.
pub fn span(seconds: f64) -> String {
    if seconds < 1.0 {
        format!("{seconds:.2} s")
    } else if seconds < 60.0 {
        format!("{seconds:.1} s")
    } else {
        format!("{} min {:02} s", (seconds / 60.0) as u64, (seconds % 60.0) as u64)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn ready() -> App {
        let mut app = App::new(false);
        app.install.iter_mut().for_each(|s| *s = Install::Ready);
        app.page = Page::Bench;
        app
    }

    #[test]
    fn nothing_is_downloaded_until_asked_and_the_sidebar_lists_only_what_is() {
        let mut app = App::new(false);
        assert_eq!(app.downloaded(), 0);
        assert_eq!(app.page, Page::Models, "with nothing downloaded the app opens on the list of models");
        app.install(2);
        assert!(app.install[2].is_listed() && !app.install[0].is_listed());
        app.cancel_install(2);
        assert!(matches!(app.install[2], Install::Missing));
    }

    #[test]
    fn removing_the_selected_model_moves_the_bench_to_one_that_is_left() {
        let mut app = ready();
        app.select(0);
        app.remove(0);
        assert!(matches!(app.install[0], Install::Missing));
        assert_ne!(app.selected, 0);
        assert_eq!(app.page, Page::Bench);
        for i in 1..app.catalog.models.len() {
            app.remove(i);
        }
        assert_eq!(app.page, Page::Models, "nothing left to run");
        let total: f64 = app.catalog.models.iter().map(|m| m.size_gb()).sum();
        assert!((app.size_where(|s| !s.is_ready()) - total).abs() < 1e-9);
    }

    #[test]
    fn a_run_waits_for_what_its_model_needs() {
        let mut app = ready();
        let tts = app.catalog.models.iter().position(|m| m.kind == Kind::Tts && m.cloning != Cloning::Required).unwrap();
        app.select(tts);
        assert_eq!(app.blocked(tts), None);
        app.text.replace_text("   ");
        assert!(app.blocked(tts).is_some(), "nothing to say");
        app.generate();
        assert!(app.takes.is_empty());

        let vc = app.catalog.models.iter().position(|m| m.kind == Kind::Vc).unwrap();
        assert!(app.blocked(vc).is_some(), "voice conversion needs both clips");
        app.install[tts] = Install::Missing;
        app.text.replace_text("Hello");
        assert_eq!(app.blocked(tts), Some("Install this model first."));
    }

    #[test]
    fn takes_run_one_at_a_time_in_order() {
        let mut app = ready();
        let tts = app.catalog.models.iter().position(|m| m.kind == Kind::Tts && m.cloning != Cloning::Required).unwrap();
        app.select(tts);
        app.generate();
        app.generate();
        assert_eq!(app.takes.len(), 2);
        assert!(matches!(app.takes[0].status, TakeStatus::Running(_)));
        assert_eq!(app.takes[1].status, TakeStatus::Queued);
        let first = app.takes[0].id;
        app.on_worker(&json!({"event": "done", "id": first, "gen_seconds": 0.5, "audio_seconds": 4.0}));
        assert_eq!(app.takes[0].status, TakeStatus::Done);
        assert_eq!(app.takes[0].speed(), Some(8.0));
        assert!(matches!(app.takes[1].status, TakeStatus::Running(_)), "the next one starts");
        app.cancel();
        assert_eq!(app.takes.len(), 1, "cancelling drops what hadn't finished");
        assert_eq!(app.active, None);
    }

    #[test]
    fn a_conversation_take_keeps_the_reply_and_the_tool_that_was_called() {
        let mut app = ready();
        let chat = app.catalog.index("lfm25-audio").unwrap();
        app.select(chat);
        app.source = Some(Clip { path: "/tmp/question.wav".into(), name: "question".into(), seconds: Some(3.0), peaks: Vec::new() });
        app.generate();
        let id = app.takes[0].id;
        app.on_worker(&json!({"event": "done", "id": id, "gen_seconds": 0.7, "audio_seconds": 3.0, "text": "The die rolled a 14.", "tool": "roll_dice(sides=20) → 14 on a 20-sided die"}));
        assert_eq!(app.takes[0].reply, "The die rolled a 14.");
        assert_eq!(app.takes[0].tool, "roll_dice(sides=20) → 14 on a 20-sided die");
        // A model that answered without one says nothing about tools.
        app.generate();
        let id = app.takes[1].id;
        app.on_worker(&json!({"event": "done", "id": id, "gen_seconds": 0.7, "audio_seconds": 3.0, "text": "Spiders have eight legs."}));
        assert!(app.takes[1].tool.is_empty());
    }

    #[test]
    fn a_failed_run_keeps_its_reason_and_lets_the_queue_move() {
        let mut app = ready();
        let tts = app.catalog.models.iter().position(|m| m.kind == Kind::Tts && m.cloning != Cloning::Required).unwrap();
        app.select(tts);
        app.generate();
        app.generate();
        let first = app.takes[0].id;
        app.on_worker(&json!({"event": "error", "id": first, "message": "out of memory"}));
        assert_eq!(app.takes[0].status, TakeStatus::Failed("out of memory".into()));
        assert!(matches!(app.takes[1].status, TakeStatus::Running(_)));
    }

    /// Drives the real Python worker: `cargo test -- --ignored` with Kokoro installed.
    #[test]
    #[ignore = "needs the installed models"]
    fn a_live_run_makes_audio() {
        let mut app = App::new(true);
        let model = std::env::var("VENTRILOQUIST_TEST_MODEL").unwrap_or_else(|_| "kokoro".into());
        let index = app.catalog.index(&model).expect("a model id from the catalog");
        assert!(app.install[index].is_ready(), "{model} isn't installed");
        app.select(index);
        app.text.replace_text("This is a test of the whole path, from the window to the worker and back.");
        let kept = app.takes.len();
        let id = app.next_id;
        app.next_id += 1;
        app.takes.push(Take { id, out: std::env::temp_dir().join("ventriloquist-live-test.wav"), ..queued(&app, index) });
        app.pump();
        let started = std::time::Instant::now();
        while app.active.is_some() && started.elapsed().as_secs() < 300 {
            app.drain();
            std::thread::sleep(Duration::from_millis(50));
        }
        let take = app.takes[kept].clone();
        app.playing = None;
        app.engine.stop_all();
        assert_eq!(take.status, TakeStatus::Done, "the run finished");
        assert!(take.audio_seconds > 1.0 && take.gen_seconds > 0.0);
        assert_eq!(take.peaks.len(), PEAKS);
        assert!(take.peaks.iter().any(|p| *p > 0.5), "the audio isn't silence");
        // Leave the app's history as it was.
        app.delete_take(take.id);
        assert!(!take.out.exists());
    }

    /// Removes a small model and downloads it again through the real installer:
    /// `cargo test -- --ignored round_trip` (about 100 MB of network).
    #[test]
    #[ignore = "deletes and re-downloads a model"]
    fn a_live_remove_and_download_round_trip() {
        let mut app = App::new(true);
        let index = app.catalog.index("phonon").unwrap();
        assert!(app.install[index].is_ready(), "phonon isn't installed");
        let wait = |app: &mut App, done: fn(&Install) -> bool| {
            let started = std::time::Instant::now();
            while !done(&app.install[index]) && started.elapsed().as_secs() < 600 {
                app.drain();
                std::thread::sleep(Duration::from_millis(100));
            }
        };
        app.remove(index);
        assert!(matches!(app.install[index], Install::Removing));
        wait(&mut app, |s| matches!(s, Install::Missing));
        assert!(matches!(app.install[index], Install::Missing), "removed");
        assert!(!paths::installed_marker("phonon").exists());
        assert!(app.install.iter().filter(|s| s.is_ready()).count() >= 1, "the models that share its environment stay");
        assert!(paths::env_python("mlx").is_file(), "and so does the environment");

        app.install(index);
        assert!(matches!(app.install[index], Install::Installing { .. }));
        wait(&mut app, |s| !matches!(s, Install::Installing { .. }));
        assert!(app.install[index].is_ready(), "downloaded again");
    }

    /// Makes a real voice preview, then plays it again from disk:
    /// `cargo test -- --ignored preview` with Kokoro installed.
    #[test]
    #[ignore = "needs the installed models"]
    fn a_live_voice_preview_is_made_once_and_then_kept() {
        let mut app = App::new(true);
        let model = app.catalog.index("kokoro").unwrap();
        assert!(app.install[model].is_ready(), "kokoro isn't installed");
        let voice = app.catalog.models[model].voices.iter().position(|v| v.id == "ef_dora").unwrap();
        let path = paths::preview_path("kokoro", "ef_dora");
        std::fs::remove_file(&path).ok();

        app.preview_voice(model, voice);
        assert!(app.is_previewing(model, voice), "the worker is asked to make it");
        let started = std::time::Instant::now();
        while app.previewing.is_some() && started.elapsed().as_secs() < 120 {
            app.drain();
            std::thread::sleep(Duration::from_millis(50));
        }
        assert!(app.notice.is_none(), "{:?}", app.notice);
        assert!(path.is_file(), "the preview is on disk");
        assert!(wav::shape(&path, 8).is_some_and(|s| s.seconds > 1.0 && s.peaks.iter().any(|p| *p > 0.5)));
        assert!(app.playing.as_ref().is_some_and(|p| p.key == PlayKey::Voice { model, voice }), "and it plays");

        app.playing = None;
        app.preview_voice(model, voice);
        assert!(app.previewing.is_none(), "the second listen needs no worker");
        assert!(app.playing.is_some());
        app.playing = None;
        app.engine.stop_all();
        std::fs::remove_file(&path).ok();
    }

    /// Sends a line to the real enhancer: `cargo test -- --ignored enhancer` with it installed.
    #[test]
    #[ignore = "needs the installed models"]
    fn a_live_enhancer_marks_a_line_up_for_the_selected_model() {
        let mut app = App::new(true);
        assert!(app.enhancer_ready(), "the prompt enhancer isn't installed");
        let kitten = app.catalog.index("kitten").unwrap();
        app.install[kitten] = Install::Ready;
        app.select(kitten);
        app.text.replace_text("we won the grant i can hardly believe it, 2.5 million dollars. i told you never to doubt us");
        app.enhance();
        let started = std::time::Instant::now();
        while app.is_enhancing() && started.elapsed().as_secs() < 120 {
            app.drain();
            std::thread::sleep(Duration::from_millis(50));
        }
        app.engine.stop_all();
        assert!(app.notice.is_none(), "{:?}", app.notice);
        let line = app.text.text.clone();
        assert!(line.to_lowercase().contains("two point five million"), "numbers are spelled out: {line}");
        assert!(line.contains("grant") && line.contains("doubt us"), "the words are kept: {line}");
        assert!(line.contains('[') || line.contains('<') || line.contains("((("), "and Kitten's marks are used: {line}");
        assert!(app.enhanced_from.is_some());
    }

    /// Has the real enhancer write a two-person scene for Drama 3:
    /// `cargo test -- --ignored enhancer`.
    #[test]
    #[ignore = "needs the installed models"]
    fn a_live_enhancer_writes_a_scene_with_speaker_tags() {
        let mut app = App::new(true);
        assert!(app.enhancer_ready(), "the prompt enhancer isn't installed");
        let drama = app.catalog.index("drama3").unwrap();
        app.install[drama] = Install::Ready;
        app.select(drama);
        app.preprompt.replace_text("");
        app.scene.replace_text("Two friends argue about who left the front door open all night. One is furious, the other is sheepish.");
        app.write_scene();
        let started = std::time::Instant::now();
        while app.is_enhancing() && started.elapsed().as_secs() < 120 {
            app.drain();
            std::thread::sleep(Duration::from_millis(50));
        }
        app.engine.stop_all();
        assert!(app.notice.is_none(), "{:?}", app.notice);
        let script = app.text.text.clone();
        assert!(script.contains("<|speaker:0|>") && script.contains("<|speaker:1|>"), "both characters speak: {script}");
        assert!(script.lines().count() >= 3, "several turns: {script}");
        assert!(script.lines().all(|l| l.starts_with("<|speaker:")), "every turn is tagged: {script}");
    }

    /// Standing instructions change what the real enhancer writes:
    /// `cargo test -- --ignored enhancer`.
    #[test]
    #[ignore = "needs the installed models"]
    fn a_live_enhancer_follows_standing_instructions() {
        let mut app = App::new(true);
        assert!(app.enhancer_ready(), "the prompt enhancer isn't installed");
        let kokoro = app.catalog.index("kokoro").unwrap();
        app.install[kokoro] = Install::Ready;
        app.select(kokoro);
        let saved = app.preprompt.text.clone();
        app.preprompt.replace_text("Translate the line into Spanish.");
        app.text.replace_text("Good evening. Today we are going to cook something very special.");
        // Sent without `enhance()`, which would save the test's instructions as the user's.
        let request = app.enhance_request(ENHANCE_IDS + 900, "Good evening. Today we are going to cook something very special.");
        app.enhancing = Some((ENHANCE_IDS + 900, app.text.text.clone()));
        let env = app.catalog.models[app.catalog.index(ENHANCER).unwrap()].env.clone();
        app.engine.send(&env, &request).unwrap();
        let started = std::time::Instant::now();
        while app.is_enhancing() && started.elapsed().as_secs() < 120 {
            app.drain();
            std::thread::sleep(Duration::from_millis(50));
        }
        app.engine.stop_all();
        app.preprompt.replace_text(saved);
        let line = app.text.text.to_lowercase();
        assert!(line.contains("buenas") || line.contains("hoy") || line.contains("vamos"), "the line came back in Spanish: {line}");
    }

    fn queued(app: &App, index: usize) -> Take {
        let m = &app.catalog.models[index];
        Take {
            id: 0,
            model: m.id.clone(),
            kind: m.kind,
            text: app.text.text.clone(),
            voice: m.voice_default.clone(),
            reference: None,
            ref_text: String::new(),
            instruct: String::new(),
            source: None,
            reply: String::new(),
            tool: String::new(),
            out: PathBuf::new(),
            status: TakeStatus::Queued,
            gen_seconds: 0.0,
            load_seconds: 0.0,
            audio_seconds: 0.0,
            peak_gb: 0.0,
            created: 0,
            peaks: Vec::new(),
        }
    }

    #[test]
    fn enhancing_rewrites_the_line_fills_an_empty_direction_and_can_be_undone() {
        let mut app = ready();
        let design = app.catalog.index("qwen3-design").unwrap();
        app.select(design);
        app.text.replace_text("we won the grant");
        app.enhance();
        assert!(app.is_enhancing());
        let (id, _) = app.enhancing.clone().unwrap();
        app.on_worker(&json!({"event": "done", "id": id, "text": "We won the grant!", "direction": "An elated woman, speaking fast."}));
        assert!(!app.is_enhancing());
        assert_eq!(app.text.text, "We won the grant!");
        assert_eq!(app.instruct.text, "An elated woman, speaking fast.");
        assert!(app.takes.is_empty(), "enhancing is not a run");
        app.revert_enhance();
        assert_eq!((app.text.text.as_str(), app.instruct.text.as_str()), ("we won the grant", ""));

        // A direction the user wrote is theirs to keep, and typing during the wait wins.
        app.instruct.replace_text("Flat and tired.");
        app.enhance();
        let (id, _) = app.enhancing.clone().unwrap();
        app.on_worker(&json!({"event": "done", "id": id, "text": "We won the grant.", "direction": "Elated."}));
        assert_eq!(app.instruct.text, "Flat and tired.");
        app.enhance();
        let (id, _) = app.enhancing.clone().unwrap();
        app.text.replace_text("something else entirely");
        app.on_worker(&json!({"event": "done", "id": id, "text": "We won!", "direction": ""}));
        assert_eq!(app.text.text, "something else entirely");
    }

    #[test]
    fn standing_instructions_ride_along_with_every_enhancement() {
        let mut app = ready();
        app.text.replace_text("we won the grant");
        assert_eq!(app.enhance_request(7, "we won the grant")["preprompt"], "");
        app.toggle_preprompt();
        assert!(app.preprompt_open && app.preprompt.focused);
        app.preprompt.replace_text("  Make every line sound tired and sarcastic.  ");
        let request = app.enhance_request(7, "we won the grant");
        assert_eq!(request["preprompt"], "Make every line sound tired and sarcastic.");
        assert_eq!((request["op"].as_str(), request["target"].as_str()), (Some("enhance"), Some(app.model().id.as_str())));
        app.enhance();
        assert!(!app.preprompt_open, "enhancing puts the instructions away");
    }

    #[test]
    fn a_described_scene_becomes_dialogue_in_the_text_and_can_be_undone() {
        let mut app = ready();
        let drama = app.catalog.index("drama3").unwrap();
        app.select(drama);
        assert!(app.model().enhance.speakers > 1, "Drama 3 voices several characters");
        app.text.replace_text("what was here before");
        app.toggle_scene();
        assert!(app.scene_open && app.scene.focused);
        app.write_scene();
        assert!(!app.is_enhancing(), "nothing to write without a direction");
        app.scene.replace_text("two friends argue about who left the door open");
        app.scene_length = 1;
        let request = app.scene_request(3);
        assert_eq!((request["op"].as_str(), request["length"].as_str(), request["target"].as_str()), (Some("scene"), Some("medium"), Some("drama3")));
        app.write_scene();
        assert!(app.is_enhancing() && app.writing_scene && !app.scene_open);
        let (id, _) = app.enhancing.clone().unwrap();
        let script = "<|speaker:0|> You left it open.\n<|speaker:1|> I did not.";
        app.on_worker(&json!({"event": "done", "id": id, "text": script, "direction": ""}));
        assert_eq!(app.text.text, script);
        assert_eq!(app.scene.text, "two friends argue about who left the door open", "the direction stays, to try again");
        app.revert_enhance();
        assert_eq!(app.text.text, "what was here before");
        // Single-voice models are told so.
        let kokoro = app.catalog.index("kokoro").unwrap();
        assert!(app.catalog.models[kokoro].enhance.speakers <= 1);
    }

    #[test]
    fn a_restart_comes_back_to_the_same_text_clip_voice_and_model() {
        let clip = std::env::temp_dir().join(format!("ventriloquist-session-{}.wav", std::process::id()));
        std::fs::write(&clip, b"RIFF").unwrap();
        let mut app = ready();
        let ming = app.catalog.index("ming").unwrap();
        app.select(ming);
        app.text.replace_text("a line half written");
        app.instruct.replace_text("quietly");
        app.preprompt.replace_text("Always whisper.");
        app.scene.replace_text("two friends argue");
        app.scene_length = 2;
        app.voices.insert("ming".into(), "UK_George".into());
        app.reference = Some(Clip { path: clip.clone(), name: "my voice".into(), seconds: None, peaks: Vec::new() });
        app.source = Some(Clip { path: "/nowhere/gone.wav".into(), name: "deleted since".into(), seconds: None, peaks: Vec::new() });
        let saved = app.session();

        let mut again = ready();
        again.restore(&saved);
        assert_eq!(again.selected, ming);
        assert_eq!((again.text.text.as_str(), again.instruct.text.as_str()), ("a line half written", "quietly"));
        assert_eq!((again.preprompt.text.as_str(), again.scene.text.as_str(), again.scene_length), ("Always whisper.", "two friends argue", 2));
        assert_eq!(again.voice(&again.catalog.models[ming].clone()), Some("UK_George"));
        assert_eq!(again.reference.as_ref().map(|c| c.name.as_str()), Some("my voice"));
        assert!(again.source.is_none(), "a clip whose file is gone doesn't come back");
        std::fs::remove_file(clip).ok();
    }

    #[test]
    fn without_the_enhancer_downloaded_the_button_leads_to_the_models_page() {
        let mut app = ready();
        let helper = app.catalog.index(ENHANCER).unwrap();
        app.install[helper] = Install::Missing;
        app.enhance();
        assert!(!app.is_enhancing());
        assert_eq!(app.page, Page::Models);
        // And the helper never becomes the bench.
        app.install.iter_mut().for_each(|s| *s = Install::Missing);
        app.install[helper] = Install::Ready;
        app.page = Page::Bench;
        app.settle_selection();
        assert_eq!(app.page, Page::Models);
    }

    #[test]
    fn times_read_the_way_people_say_them() {
        assert_eq!(clock(67.2), "1:07");
        assert_eq!(span(0.416), "0.42 s");
        assert_eq!(span(3.14), "3.1 s");
        assert_eq!(span(72.0), "1 min 12 s");
    }
}
