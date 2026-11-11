//! The model catalog: one JSON file (`sidecar/catalog.json`) that the app, the Python worker and
//! the installer all read, so a model is described in exactly one place.

use serde::Deserialize;

pub const CATALOG_JSON: &str = include_str!("../sidecar/catalog.json");

#[derive(Clone, Copy, Debug, Deserialize, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "lowercase")]
pub enum Kind {
    /// Text in, speech out.
    Tts,
    /// A recording in, the same words out in another voice.
    Vc,
    /// Speech in, a spoken reply out.
    Chat,
    /// Not a voice model: something the app itself uses, such as the prompt enhancer.
    Helper,
}

impl Kind {
    /// The kinds with a bench of their own, in sidebar order.
    pub const ALL: [Kind; 3] = [Kind::Tts, Kind::Vc, Kind::Chat];
    /// Everything that can be downloaded, in the order the Models page lists it.
    pub const DOWNLOADABLE: [Kind; 4] = [Kind::Tts, Kind::Vc, Kind::Chat, Kind::Helper];

    pub fn title(self) -> &'static str {
        match self {
            Kind::Tts => "Text to speech",
            Kind::Vc => "Voice conversion",
            Kind::Chat => "Conversation",
            Kind::Helper => "Helpers",
        }
    }
}

/// Whether a model speaks in a voice it is shown a sample of.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Cloning {
    #[default]
    None,
    Optional,
    Required,
}

/// Whether a model takes a line of direction: a voice described in words, or how to deliver it.
#[derive(Clone, Copy, Debug, Default, Deserialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
pub enum Instruct {
    #[default]
    None,
    Optional,
    Required,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Voice {
    pub id: String,
    pub name: String,
    /// The heading it is listed under: its language, or what kind of voice it is.
    #[serde(default)]
    pub group: String,
    /// Who it is, in a line: "Female, young adult · Warm and unhurried".
    #[serde(default)]
    pub detail: String,
    /// The language it speaks natively, which a preview is written in.
    #[serde(default = "english")]
    pub lang: String,
}

fn english() -> String {
    "en".into()
}

impl Voice {
    /// A line for the voice to read when previewed, in its own language.
    pub fn preview_text(&self) -> &'static str {
        match self.lang.as_str() {
            "zh" => "你好，这是我朗读时的声音。",
            "ja" => "こんにちは。これが私の読み上げる声です。",
            "ko" => "안녕하세요. 제가 문장을 읽을 때의 목소리입니다.",
            "es" => "Hola. Así sueno cuando leo una frase para ti.",
            "fr" => "Bonjour. Voici ma voix quand je lis une phrase pour vous.",
            "hi" => "नमस्ते। यह मेरी आवाज़ है जब मैं आपके लिए एक पंक्ति पढ़ूँ।",
            "it" => "Ciao. Ecco come suona la mia voce quando leggo una frase.",
            "pt" => "Olá. É assim que eu soo quando leio uma frase para você.",
            "de" => "Hallo. So klinge ich, wenn ich dir einen Satz vorlese.",
            "ru" => "Здравствуйте. Так звучит мой голос, когда я читаю вам строчку.",
            "ar" => "مرحباً. هكذا يبدو صوتي عندما أقرأ لك جملة.",
            "tr" => "Merhaba. Size bir cümle okuduğumda sesim böyle duyulur.",
            "nl" => "Hallo. Zo klink ik als ik een zin voor je voorlees.",
            "pl" => "Cześć. Tak brzmi mój głos, gdy czytam dla ciebie zdanie.",
            "id" => "Halo. Beginilah suara saya saat membacakan sebuah kalimat untuk Anda.",
            "te" => "నమస్కారం. మిమ్మల్ని కలవడం చాలా సంతోషంగా ఉంది.",
            _ => "Hi there. This is how I sound when I read a line for you.",
        }
    }

    /// Whether a search for `query` (already lowercased) finds this voice.
    pub fn matches(&self, query: &str) -> bool {
        query.is_empty() || [&self.name, &self.group, &self.detail].iter().any(|field| field.to_lowercase().contains(query))
    }
}

/// What the enhancer may write for a model. The worker reads the rest of this entry (the marks
/// the model understands, worked examples); the app only needs to know about voices.
#[derive(Clone, Debug, Default, Deserialize)]
pub struct Enhance {
    /// How many characters the model can voice in one take, each with a speaker tag. One or
    /// none means a single voice.
    #[serde(default)]
    pub speakers: u8,
}

#[derive(Clone, Debug, Deserialize)]
pub struct Model {
    pub id: String,
    pub name: String,
    pub maker: String,
    pub kind: Kind,
    /// The Python environment (a key of `Catalog::envs`) the model runs in.
    pub env: String,
    pub summary: String,
    pub license: String,
    /// A limit of the licence worth a second look ("Non-commercial"), shown beside it.
    #[serde(default)]
    pub caution: String,
    /// When the weights were first published, as `YYYY-MM`.
    pub released: String,
    /// Download size, as shown: "0.3 GB".
    pub size: String,
    #[serde(default)]
    pub languages: String,
    #[serde(default)]
    pub voices: Vec<Voice>,
    #[serde(default)]
    pub voice_default: Option<String>,
    #[serde(default)]
    pub cloning: Cloning,
    /// The model also wants the words spoken in the reference clip.
    #[serde(default)]
    pub ref_text: bool,
    #[serde(default)]
    pub instruct: Instruct,
    /// The direction field's placeholder: what to write there, by example.
    #[serde(default)]
    pub instruct_hint: String,
    /// Something worth knowing before a first run: a quirk of its prompts, a limit.
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub enhance: Enhance,
}

impl Model {
    /// The download size as a number, for totals.
    pub fn size_gb(&self) -> f64 {
        self.size.split_whitespace().next().and_then(|n| n.parse().ok()).unwrap_or(0.0)
    }

    /// The release month as people write it: `Sep 2026`.
    pub fn released_label(&self) -> String {
        const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
        let parsed = self.released.split_once('-').and_then(|(year, month)| Some((year, month.parse::<usize>().ok()?)));
        match parsed {
            Some((year, month)) if (1..=12).contains(&month) && year.len() == 4 => format!("{} {year}", MONTHS[month - 1]),
            _ => self.released.clone(),
        }
    }
}

#[derive(Clone, Debug, Deserialize)]
pub struct Catalog {
    pub models: Vec<Model>,
}

impl Catalog {
    pub fn load() -> Catalog {
        serde_json::from_str(CATALOG_JSON).expect("sidecar/catalog.json is valid")
    }

    pub fn index(&self, id: &str) -> Option<usize> {
        self.models.iter().position(|m| m.id == id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_release_month_reads_as_a_date() {
        let mut m = Catalog::load().models.remove(0);
        m.released = "2026-09".into();
        assert_eq!(m.released_label(), "Sep 2026");
        m.released = "2024-12".into();
        assert_eq!(m.released_label(), "Dec 2024");
    }

    #[test]
    fn the_catalog_parses_and_every_model_is_usable() {
        let c = Catalog::load();
        assert!(!c.models.is_empty());
        let raw: serde_json::Value = serde_json::from_str(CATALOG_JSON).unwrap();
        let envs = raw["envs"].as_object().expect("envs");
        for m in &c.models {
            assert!(envs.contains_key(&m.env), "{} names an env that isn't defined: {}", m.id, m.env);
            assert_eq!(c.models.iter().filter(|x| x.id == m.id).count(), 1, "duplicate id {}", m.id);
            assert!(m.size_gb() > 0.0 && m.size.ends_with(" GB"), "{}: size {:?} isn't `N GB`", m.id, m.size);
            let (month, year) = m.released_label().split_once(' ').map(|(a, b)| (a.to_owned(), b.to_owned())).unwrap_or_default();
            assert!(month.len() == 3 && year.parse::<u32>().is_ok_and(|y| (2020..2100).contains(&y)), "{}: released {:?} isn't YYYY-MM", m.id, m.released);
            if let Some(v) = &m.voice_default {
                assert!(m.voices.iter().any(|x| &x.id == v), "{}: default voice {v} isn't in its list", m.id);
            }
            for v in &m.voices {
                assert_eq!(m.voices.iter().filter(|x| x.id == v.id).count(), 1, "{}: voice {} is listed twice", m.id, v.id);
                assert!(!v.name.is_empty() && !v.group.is_empty(), "{}: voice {} needs a name and a group", m.id, v.id);
            }
            if m.kind == Kind::Tts && m.cloning != Cloning::Required {
                assert!(m.voices.is_empty() == m.voice_default.is_none(), "{}: voices need a default", m.id);
            }
        }
    }
}
