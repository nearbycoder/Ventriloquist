"""Ventriloquist's model worker: one long-lived process per Python environment.

The app writes one JSON request per line on stdin and reads JSON events, one per line, from
stdout. Models load on first use and stay loaded, so a second run measures the model and not
its start-up.

    request  {"id": 7, "model": "kokoro", "text": "...", "voice": "af_heart",
              "ref_audio": null, "ref_text": "", "instruct": "", "source_audio": null,
              "out": "/path/take.wav"}
             {"op": "enhance", "id": 9, "model": "enhancer", "target": "kitten",
              "text": "...", "instruct": "", "preprompt": "standing instructions"}
             {"op": "scene", "id": 10, "model": "enhancer", "target": "drama3",
              "text": "what happens in the scene", "length": "short", "preprompt": ""}
    events   {"event": "status", "id": 7, "message": "Loading Kokoro"}
             {"event": "done", "id": 7, "gen_seconds": 0.4, "load_seconds": 2.1,
              "audio_seconds": 8.6, "sample_rate": 24000, "peak_gb": 1.2, "text": ""}
             {"event": "error", "id": 7, "message": "..."}

Everything a library prints goes to stderr, which the app keeps as a log.
"""

import inspect
import json
import os
import re
import sys
import time
import traceback
import wave
from pathlib import Path

# The protocol keeps the real stdout; anything else that prints lands on stderr.
PROTOCOL = os.fdopen(os.dup(1), "w", buffering=1)
os.dup2(2, 1)
sys.stdout = sys.stderr

HERE = Path(__file__).resolve().parent
CATALOG = json.loads((HERE / "catalog.json").read_text())
MODELS = {m["id"]: m for m in CATALOG["models"]}
WHISPER = "mlx-community/whisper-large-v3-turbo-asr-fp16"


def emit(event, **fields):
    PROTOCOL.write(json.dumps({"event": event, **fields}) + "\n")
    PROTOCOL.flush()


def accepted(fn, kwargs):
    """The kwargs `fn` takes: all of them if it has **kwargs, else the ones it names."""
    params = inspect.signature(fn).parameters
    if any(p.kind is inspect.Parameter.VAR_KEYWORD for p in params.values()):
        return kwargs
    return {k: v for k, v in kwargs.items() if k in params}


def write_wav(path, audio, sample_rate):
    """Writes mono or interleaved float audio as 16-bit PCM, the format everything reads."""
    import numpy as np

    audio = np.asarray(audio, dtype=np.float32)
    if audio.ndim > 1:
        # (channels, samples) or (samples, channels): fold to mono either way.
        audio = audio.mean(axis=0 if audio.shape[0] < audio.shape[-1] else -1)
    audio = np.nan_to_num(audio.reshape(-1))
    if audio.size == 0:
        raise RuntimeError("The model returned no audio.")
    pcm = (np.clip(audio, -1.0, 1.0) * 32767.0).astype("<i2")
    Path(path).parent.mkdir(parents=True, exist_ok=True)
    with wave.open(str(path), "wb") as f:
        f.setnchannels(1)
        f.setsampwidth(2)
        f.setframerate(int(sample_rate))
        f.writeframes(pcm.tobytes())
    return audio.size / float(sample_rate)


def short_espeak_data():
    """Points espeak-ng at its data through a short path.

    The espeak-ng bundled with Kokoro's text front-end ignores a data path longer than about
    160 bytes and then looks where it was compiled, which exits the process. A link in /tmp
    keeps the path short wherever the environment lives.
    """
    try:
        import espeakng_loader
    except ImportError:
        return
    link = f"/tmp/ventriloquist-espeak-{os.getuid()}"
    target = espeakng_loader.get_data_path()
    if not (os.path.islink(link) and os.readlink(link) == target):
        if os.path.lexists(link):
            os.unlink(link)
        os.symlink(target, link)
    os.environ["ESPEAK_DATA_PATH"] = link
    import misaki.espeak  # noqa: F401  (sets the long path; undone below)
    from phonemizer.backend.espeak.wrapper import EspeakWrapper

    EspeakWrapper.set_data_path(None)


def trim_silence(audio, sample_rate, floor=0.02, pad=0.12):
    """Cuts the quiet before the first sound and after the last, leaving a short margin."""
    import numpy as np

    audio = np.asarray(audio, dtype=np.float32).reshape(-1)
    level = np.abs(audio)
    if level.size == 0 or level.max() <= 0:
        return audio
    loud = np.flatnonzero(level > floor * level.max())
    margin = int(pad * sample_rate)
    return audio[max(loud[0] - margin, 0) : loud[-1] + margin]


# ----- MLX (mlx-audio) -----


class MlxTts:
    """Any mlx-audio text-to-speech model. The catalog's `options` say how to call it."""

    stt = None

    def __init__(self, spec):
        from mlx_audio.tts.utils import load_model

        self.options = spec.get("options", {})
        if self.options.get("espeak"):
            short_espeak_data()
        self.model = load_model(spec["repo"])
        if self.options.get("warm"):
            # The text front-end builds itself on first use (about ten seconds); that is
            # start-up, not generation, so it happens here.
            voice = spec.get("voice_default")
            kwargs = {"voice": voice, "lang_code": voice[0]} if voice and self.options.get("lang_from_voice") else {}
            for _ in self.model.generate(text="Hello.", **accepted(self.model.generate, {"verbose": False, **kwargs})):
                pass

    transcripts = {}

    def transcribe(self, path):
        """What a voice sample says. Kept per file, so a sample is transcribed once however
        many lines are spoken in it."""
        from mlx_audio.stt import load as load_stt

        stamp = (path, os.path.getmtime(path), os.path.getsize(path))
        if stamp not in MlxTts.transcripts:
            if MlxTts.stt is None:
                MlxTts.stt = load_stt(WHISPER)
            MlxTts.transcripts[stamp] = MlxTts.stt.generate(path).text.strip()
        return MlxTts.transcripts[stamp]

    def run(self, req, status):
        import numpy as np
        from mlx_audio.utils import load_audio

        o = self.options
        kwargs = dict(o.get("gen", {}))
        reference = req.get("ref_audio")
        instruct = (req.get("instruct") or "").strip()
        if reference:
            if o.get("ref_text"):
                ref_text = (req.get("ref_text") or "").strip()
                if not ref_text:
                    status("Transcribing the voice sample")
                    ref_text = self.transcribe(reference)
                kwargs["ref_text"] = ref_text
            keep_path = getattr(self.model, "preserve_ref_audio_path", False) is True
            kwargs["ref_audio"] = reference if keep_path else load_audio(reference, sample_rate=self.model.sample_rate)
        elif req.get("voice") and not (instruct and o.get("instruct_replaces_voice")):
            voice = req["voice"]
            files = o.get("voice_files")
            if files and voice != "default":
                # The voice is a file in a repository of voices; the model takes its path.
                from huggingface_hub import hf_hub_download

                kwargs["voice"] = hf_hub_download(files["repo"], files["path"].format(id=voice))
            elif not files:
                kwargs[o.get("voice_arg", "voice")] = voice
            if o.get("lang_from_voice"):
                kwargs["lang_code"] = voice[0]
        if instruct and o.get("instruct"):
            kwargs["instruct"] = instruct
            if o.get("instruct_replaces_voice") and not reference:
                kwargs.update(o.get("instruct_gen", {}))

        method = getattr(self.model, o.get("method", "generate"))
        status("Generating")
        chunks, rate = [], None
        for result in method(text=req["text"], **accepted(method, {"verbose": False, **kwargs})):
            chunk = np.array(result.audio, dtype=np.float32).reshape(-1)
            if chunk.size:
                chunks.append(chunk)
            rate = getattr(result, "sample_rate", rate)
        if not chunks:
            raise RuntimeError("The model returned no audio.")
        return np.concatenate(chunks), rate or self.model.sample_rate, ""


class MlxSts:
    """An mlx-audio speech-to-speech model: a recording in, a spoken reply (and its text) out.

    The same models can also read text aloud; the catalog lists that as a text-to-speech entry.
    """

    def __init__(self, spec):
        from mlx_audio.sts import load

        self.options = spec.get("options", {})
        self.speaks_text = spec["kind"] == "tts"
        self.model = load(spec["repo"])

    def run(self, req, status):
        import numpy as np

        kwargs = dict(self.options.get("gen", {}))
        if self.speaks_text:
            status("Generating")
            if req.get("voice"):
                kwargs["voice"] = req["voice"]
            out = self.model.generate(text=req["text"], **accepted(self.model.generate, kwargs))
            if getattr(out, "audio", None) is None:
                raise RuntimeError("The model returned no audio.")
            return np.array(out.audio, dtype=np.float32), out.sample_rate, ""
        status("Listening and answering")
        out = self.model.generate(req["source_audio"], **accepted(self.model.generate, kwargs))
        if getattr(out, "audio", None) is None:
            raise RuntimeError(f"The model answered in text only: {getattr(out, 'text', '')!r}")
        audio = np.array(out.audio, dtype=np.float32)
        if self.options.get("trim"):
            # A full-duplex model's answer sits on the recording's timeline: silence while it
            # listened, then speech, then whatever was left of the time it was given.
            audio = trim_silence(audio, out.sample_rate)
        return audio, out.sample_rate, (getattr(out, "text", "") or "").strip()


class Zonos:
    """ZONOS3 Mini on Zyphra's own MLX runtime, checked out and installed from its lockfile
    by `setup.py`. The runtime is an HTTP server; this drives its engine and scheduler
    directly, the way its `tools generate` command does.

    A voice is a 192-number speaker embedding: the shipped voices come with theirs, and a
    voice sample is embedded by the model's speaker encoder. (The model can also take the
    sample as a prefix of codec frames, which needs 5.5 GB more of encoders; by the model's
    own speaker encoder the embedding stays closer to the sample, so that path is left out.)
    """

    def __init__(self, spec):
        from huggingface_hub import snapshot_download
        from zonos3_mlx.engine import Engine
        from zonos3_mlx.scheduler import Scheduler

        # The same revision `setup.py` fetched: upstream has renamed these files once already,
        # and the catalog pins the commit whose names are used below.
        weights = spec["downloads"][0]
        self.root = Path(snapshot_download(spec["repo"], revision=weights.get("revision"), allow_patterns=weights.get("allow"))) / "gguf"
        self.default = spec.get("voice_default")
        self.decoder = spec.get("options", {}).get("decoder")
        # The files are named one by one: the runtime looks for them beside the model, and in
        # the Hugging Face cache "beside" is a folder of content hashes.
        self.engine = Engine.load(
            str(self.root / "backbone-ptq.gguf"),
            zac=[str(self.root / "zac.gguf")],
            spk_encoder=str(self.root / "spk-encoder.gguf"),
            steer=str(self.root / "steer.gguf"),
        )
        self.scheduler = Scheduler(self.engine, batch=2)
        self.scheduler.start(warmup=True)
        self.sampled = {}

    def embedding(self, req):
        """The speaker embedding for a request: the sample's when there is one, else the voice's."""
        import numpy as np
        import torch

        sample = req.get("ref_audio")
        if not sample:
            voice = req.get("voice") or self.default
            return torch.from_numpy(np.load(self.root / "voices" / f"{voice}.embedding.npy").astype(np.float32))
        key = (sample, os.path.getmtime(sample))
        if key not in self.sampled:
            import soundfile
            import torchaudio
            from zonos3.generation import SAMPLE_RATE

            audio, rate = soundfile.read(sample, dtype="float32", always_2d=True)
            audio = torch.from_numpy(audio.mean(axis=1))[: rate * 60]
            if rate != SAMPLE_RATE:
                audio = torchaudio.functional.resample(audio, rate, SAMPLE_RATE)
            embedded = self.scheduler.submit(self.engine.speaker_encoder.embed, audio.numpy()).result()
            self.sampled = {key: torch.from_numpy(np.asarray(embedded, dtype=np.float32))}
        return self.sampled[key]

    def controls(self, direction):
        """Steering doses from a line such as "happy, whisper 1, british accent, slower 2".

        The names are the ones the model's own studio shows (they ship with the weights):
        each control or one of its two poles, an accent, or a preset. A number after a name is
        the amount; without one a moderate amount is used.
        """
        ui = self.engine.steer_bundle.ui
        by_id = {control["id"]: control for control in ui["controls"]}
        poles = {pole.lower(): (control, sign) for control in ui["controls"] for pole, sign in zip(control.get("poles") or [], (-1, 1))}
        labels = {control["label"].lower(): control for control in ui["controls"]}
        accents = {accent["id"]: accent for accent in ui["accents"]}
        presets = {name.lower(): doses for name, doses in ui["presets"].items()}
        moods = {mood["control"]: mood["dose"] for mood in ui["moods"]}
        doses = {}

        def apply(mapping, amount):
            for name, weight in mapping.items():
                doses[name] = doses.get(name, 0.0) + weight * amount

        def control(entry, amount, sign=None):
            """`amount` None is a moderate dose; `sign` is the pole that was named, if one was."""
            low, high = entry["range"]
            if amount is None:
                toward = entry["safe"][0 if sign == -1 else 1]
                amount = moods.get(entry["id"], round(abs(toward) * 0.75, 2))
                amount = -amount if sign == -1 else amount
            elif sign is not None:
                amount = sign * abs(amount)
            amount = max(low, min(high, amount))
            apply(entry["map"]["+" if amount >= 0 else "-"], abs(amount))

        def accent(entry, amount):
            apply(entry["map"], max(0.0, min(entry["range"][1], entry["safe"][1] if amount is None else amount)))

        for item in re.split(r"[,;\n]+", direction):
            item = item.strip().rstrip(".")
            if not item:
                continue
            found = re.fullmatch(r"(.*?)[\s:=x×]*([-+]?\d+(?:\.\d+)?)", item)
            name, amount = (found.group(1), float(found.group(2))) if found else (item, None)
            name = re.sub(r"\s+", " ", name.strip().lower())
            plain = re.sub(r"^accent:\s*|\s+accent$", "", name)
            if name in presets:
                for key, dose in presets[name].items():
                    if key.startswith("accent:"):
                        accent(accents[key.split(":", 1)[1]], dose)
                    else:
                        control(by_id[key], dose)
            elif name in by_id:
                control(by_id[name], amount)
            elif name in poles:
                control(poles[name][0], amount, poles[name][1])
            elif name in labels:
                control(labels[name], amount)
            elif plain in accents:
                accent(accents[plain], amount)
            elif name in self.engine.steer_bundle.directions:
                doses[name] = doses.get(name, 0.0) + (1.0 if amount is None else amount)
            else:
                known = sorted(set(by_id) | set(poles) | {f"{key} accent" for key in accents})
                raise ValueError(f"\"{item}\" isn't a control this model has. It knows: {', '.join(known)}; and the presets {', '.join(ui['presets'])}.")
        return {name: dose for name, dose in doses.items() if dose}

    def run(self, req, status):
        import numpy as np
        from zonos3.generation import SAMPLE_RATE
        from zonos3.tts.plan import Speaker

        request = {"text": req["text"], "stream": False, "speaker_mode": "embedding", "text_normalization": False}
        if self.decoder:
            request["decoder"] = self.decoder
        controls = self.controls(req.get("instruct") or "")
        if controls:
            request["controls"] = controls
        speaker = Speaker("embedding", embedding=self.embedding(req))
        status("Generating")
        result = self.scheduler.synthesize(request, speaker)
        if not len(result.pcm):
            raise RuntimeError("The model returned no audio.")
        return np.asarray(result.pcm, dtype=np.float32), SAMPLE_RATE, ""


class LfmAudio:
    """LFM2.5-Audio: a small model that takes speech in and gives speech and text out, with
    tools it can call (see `tools.py`).

    Its chat template has a place for tools, but the published model was not trained to call
    them from speech: given a list it mostly declines, and when pushed it reaches for whichever
    comes first. What it can do is judge one tool at a time against a question it has in text.
    So a turn with tools goes in steps, all of them this model:

    1. it transcribes the recording;
    2. for each tool, alone, it is asked the question, and how likely it is to open a tool
       call is read off; the likeliest tool is called if that passes a threshold;
    3. it writes the call's arguments, the tool runs, and it answers from the result in text;
    4. it reads that answer aloud.

    When no tool is wanted it answers the recording directly, speech to speech.
    """

    CHAT = "Respond with interleaved text and audio."

    def __init__(self, spec):
        from mlx_audio.sts.models.lfm_audio import LFM2AudioModel, LFM2AudioProcessor

        self.options = spec.get("options", {})
        self.model = LFM2AudioModel.from_pretrained(spec["repo"])
        self.processor = LFM2AudioProcessor.from_pretrained(spec["repo"])
        self.opens_call = self.processor.tokenizer.convert_tokens_to_ids("<|tool_call_start|>")

    def chat(self, *turns):
        """A conversation of (role, text or audio) turns, ready for the model's reply."""
        from mlx_audio.sts.models.lfm_audio import ChatState

        chat = ChatState(self.processor)
        for role, content in turns:
            chat.new_turn(role)
            if isinstance(content, str):
                chat.add_text(content)
            else:
                chat.add_audio(content, 16000)
            chat.end_turn()
        chat.new_turn("assistant")
        return chat

    def generate(self, chat, mode, limit, **sampling):
        """Runs the model on. Returns (text, audio or None)."""
        import mlx.core as mx
        import numpy as np
        from mlx_audio.sts.models.lfm_audio import LFMModality
        from mlx_audio.sts.models.lfm_audio.model import AUDIO_EOS_TOKEN

        text, frames = [], []
        for token, modality in self.model.generate_from_chat_state(chat, mode=mode, max_new_tokens=limit, **sampling):
            if modality == LFMModality.TEXT:
                text.append(int(token.item()))
            elif int(token[0].item()) != AUDIO_EOS_TOKEN:
                frames.append(token)
        audio = None
        if frames:
            audio = np.array(self.processor.decode_audio(mx.stack(frames, axis=0).T[None], codec="detokenizer"), dtype=np.float32).reshape(-1)
        written = self.processor.tokenizer.decode(text)
        return re.sub(r"<\|[a-z_]+\|>", "", written).strip(), audio

    def wants(self, system, question):
        """How likely the model is to answer `question` by opening a tool call."""
        import mlx.core as mx

        chat = self.chat(("system", system), ("user", question))
        hidden, _ = self.model._prefill(text_tokens=chat.get_text_tokens(), audio_features=None, modalities=chat.get_modalities())
        logits = self.model.lfm.embed_tokens.as_linear(hidden[:, -1:, :])[0, -1]
        return float(mx.softmax(logits.astype(mx.float32))[self.opens_call])

    def pick(self, question):
        """The tool the question calls for, if any."""
        import tools

        thresholds = self.options.get("tools", {})
        each = [self.wants(self.listing([tool]), question) for tool in tools.TOOLS]
        best = max(range(len(each)), key=each.__getitem__)
        if each[best] >= thresholds.get("alone", 0.15) or self.wants(self.listing(tools.TOOLS), question) >= thresholds.get("together", 0.3):
            return tools.TOOLS[best]
        return None

    @staticmethod
    def listing(offered):
        import tools

        return "List of tools: <|tool_list_start|>[" + ", ".join(json.dumps(tools.schema(tool)) for tool in offered) + "]<|tool_list_end|>"

    def use(self, tool, question):
        """Calls `tool` for `question`. Returns (the answer to speak, a line about the call)."""
        import tools

        name, args = tool["name"], {}
        if tool["parameters"]["properties"]:
            chat = self.chat(("system", self.listing([tool])), ("user", question))
            chat.add_text(f"<|tool_call_start|>[{name}(")
            filled, _ = self.generate(chat, "sequential", 48)
            args = tools.parse_arguments(filled.split(")]")[0].rstrip(")"))
        call = tools.written(name, args)
        try:
            result, sentence, proof, shown = tools.call(name, args)
        except ValueError as error:
            return str(error), f"{call} failed: {error}"
        # The call as the tool understood it: the default it filled in, the sum as it read it.
        call = tools.written(name, {**args, **{key: result[key] for key in tool["parameters"]["properties"] if key in result}})
        chat = self.chat(
            ("system", self.listing([tool])),
            ("user", question),
            ("assistant", f"<|tool_call_start|>[{call}]<|tool_call_end|>"),
            ("tool", f"<|tool_response_start|>{json.dumps(result)}<|tool_response_end|>"),
        )
        answer, _ = self.generate(chat, "sequential", 96)
        # A model this small sometimes has the result in hand and still says it cannot know.
        # An answer that leaves the result out is replaced by the tool's own sentence.
        if not any(str(part).lower() in answer.lower() for part in proof):
            answer = sentence
        return answer, f"{call} → {shown}"

    def run(self, req, status):
        import mlx.core as mx
        import numpy as np
        from mlx_audio.utils import load_audio

        import tools

        heard = mx.array(np.array(load_audio(req["source_audio"], sample_rate=16000), dtype=np.float32))
        if self.options.get("tools"):
            status("Listening")
            question, _ = self.generate(self.chat(("system", "Perform ASR."), ("user", heard)), "sequential", 256)
            tool = self.pick(question) if question else None
            if tool:
                status(f"Calling {tool['name']}")
                answer, line = self.use(tool, question)
                status("Answering")
                voice = self.options.get("voice", "US female")
                speak = self.chat(("system", f"Perform TTS. Use the {voice} voice."), ("user", tools.say_numbers(answer)))
                _, audio = self.generate(speak, "sequential", 900, audio_temperature=0.8, audio_top_k=64)
                if audio is None:
                    raise RuntimeError(f"The model answered in text only: {answer!r}")
                return audio, self.model.sample_rate, answer, line
        status("Answering")
        text, audio = self.generate(self.chat(("system", self.CHAT), ("user", heard)), "interleaved", 1024, audio_temperature=1.0, audio_top_k=4)
        if audio is None:
            raise RuntimeError(f"The model answered in text only: {text!r}")
        return audio, self.model.sample_rate, text, ""


SOPHON_LANGUAGES = {"en": "english", "zh": "chinese"}


class SophonTalker:
    """The voice stage of Sophon-Realtime-32B-Flash: a Qwen3-TTS model tuned to perform sound
    tags, run on its own. Its voices ship as speaker embeddings; a voice sample is embedded
    by the model's own speaker encoder instead."""

    def __init__(self, spec):
        from huggingface_hub import snapshot_download
        from mlx_audio.tts.utils import load_model

        allow = [pattern for part in spec.get("downloads", []) for pattern in part.get("allow", [])]
        self.root = Path(snapshot_download(spec["repo"], allow_patterns=allow or None))
        self.model = load_model(str(self.root / "talker"))
        self.embed = self.model.extract_speaker_embedding
        self.default = spec.get("voice_default")

    def voice(self, name):
        import mlx.core as mx
        import torch

        tensor = torch.load(self.root / "voices" / f"{name}.pt", map_location="cpu", weights_only=True)
        return mx.array(tensor.float().numpy()).reshape(1, -1)

    def speak(self, text, voice=None, ref_audio=None):
        import mlx.core as mx
        import numpy as np
        from mlx_audio.utils import load_audio

        language = "auto"
        if ref_audio:
            self.model.extract_speaker_embedding = self.embed
            sample = load_audio(ref_audio, sample_rate=self.model.sample_rate)
        else:
            # A bundled voice stands in for what the speaker encoder would have heard. Each
            # was made for one language, which its name ends in.
            voice = voice or self.default
            chosen = self.voice(voice)
            self.model.extract_speaker_embedding = lambda *args, **kwargs: chosen
            sample = mx.zeros((self.model.sample_rate,))
            language = SOPHON_LANGUAGES.get(voice.rsplit("_", 1)[-1], "auto")
        chunks = [np.array(r.audio, dtype=np.float32).reshape(-1) for r in self.model.generate(text=text, ref_audio=sample, lang_code=language, verbose=False)]
        if not chunks:
            raise RuntimeError("The model returned no audio.")
        return np.concatenate(chunks), self.model.sample_rate

    def run(self, req, status):
        status("Generating")
        audio, sample_rate = self.speak(req["text"], req.get("voice"), req.get("ref_audio"))
        return audio, sample_rate, ""


class Sophon:
    """Sophon-Realtime-32B-Flash, a turn at a time: it hears a recording, decides what to say,
    and says it in its own voice. `sophon.py` is the port of the half that listens and thinks;
    the voice is `SophonTalker`.

    The published weights are for NVIDIA servers, so the first load converts the 32B language
    model to MLX and keeps the result in the app's data folder.
    """

    def __init__(self, spec):
        import sophon

        options = spec.get("options", {})
        self.default = spec.get("voice_default")
        self.talker = SophonTalker(spec)
        # <data>/envs/<env> is where this Python lives.
        converted = Path(sys.prefix).parents[1] / "models" / options.get("converted", "sophon-thinker")
        if not (converted / "config.json").is_file():
            from huggingface_hub import snapshot_download

            print("Downloading the language model as published (about 33 GB, deleted again once converted)", flush=True)
            snapshot_download(spec["repo"], allow_patterns=[sophon.SHARDS])
            print("Converting it for Apple Silicon", flush=True)
            sophon.convert(self.talker.root, converted, bits=options.get("bits", 4), say=lambda message: print(message, flush=True))
            sophon.discard_shards(self.talker.root)
        self.thinker = sophon.Thinker(self.talker.root, converted)
        self.clean = sophon.clean_reply

    def run(self, req, status):
        import numpy as np
        from mlx_audio.utils import load_audio

        voice = req.get("voice") or self.default
        if not req.get("source_audio"):
            # A voice preview: nothing to answer, only a line to read.
            status("Generating")
            audio, sample_rate = self.talker.speak(req["text"], voice)
            return audio, sample_rate, ""
        status("Listening")
        heard = np.array(load_audio(req["source_audio"], sample_rate=16000), dtype=np.float32)
        language = voice.rsplit("_", 1)[-1]
        reply = self.clean(self.thinker.reply(heard, language))
        if not re.search(r"\w", re.sub(r"\[[a-z_]+\]|</?[a-z]+>", "", reply)):
            raise RuntimeError("The model had nothing to say to that.")
        status("Answering")
        audio, sample_rate = self.talker.speak(reply, voice)
        return audio, sample_rate, reply


# Sexual content involving minors is never written, whatever the standing instructions or the
# scene ask for. A small model will not reliably refuse on its own, so the request and what
# comes back are both checked. (The same guard Figment's enhancer uses.)
_MINORS = re.compile(
    r"\b(child|children|kids?|kiddie|minors?|under-?age|underaged|teens?|teenagers?|teenaged?|"
    r"pre-?teens?|tweens?|school ?girls?|school ?boys?|lolis?|lolita|shotas?|toddlers?|infants?|"
    r"bab(y|ies)|little (girls?|boys?)|young (girls?|boys?)|(1[0-7]|[1-9])[ -]?(yo|y/o|years?[ -]old))\b",
    re.IGNORECASE,
)
_SEXUAL = re.compile(
    r"\b(nudes?|nudity|naked|topless|undress\w*|lingerie|nsfw|porn\w*|erotic\w*|hentai|sex|sexy|sexual\w*|"
    r"sensual\w*|seductive\w*|seduc\w*|aroused|arousal|orgasm\w*|genital\w*|penis\w*|vagina\w*|breasts?|nipples?|"
    r"bondage|fetish\w*|masturbat\w*|intercourse|blowjob|handjob|horny|moan\w*|kinky|lust\w*)\b",
    re.IGNORECASE,
)
MINORS_REFUSAL = "The enhancer doesn't write sexual content involving minors."


def sexualizes_minors(text):
    return bool(_MINORS.search(text) and _SEXUAL.search(text))


# ----- The prompt enhancer (a small language model through mlx-lm) -----

ENHANCE = """You are a voice director preparing a line for {name}, a text-to-speech model. Mark the line up so that it is performed, not just read.

Rules:
- {markup}
- When the model understands marks, add them: two or three in a line with strong feeling, placed where the feeling turns, and one in a quiet or reflective line. Only a plain announcement (a time, an address, an instruction) stays unmarked.
- Punctuate for delivery: a comma or full stop where a speaker would pause, a dash or an ellipsis for a hesitation, a question or exclamation mark where the tone calls for one.
- Write numbers, dates, symbols and abbreviations as spoken words.
- Keep every word of the user's line and its meaning. Marks and punctuation are additions; never add sentences, answer the line or continue it.
- Write in the same language as the user's line.
- {output}"""

# What the user asked for on every line. It goes in the system prompt and again right before
# the line, where a small model weighs it most: stated once, above three worked examples, it
# was ignored. It may loosen the rule about keeping the user's words (a stutter, another
# register, another language); which marks exist and what to output stay as stated.
STANDING = """

The user has standing instructions for every line, given with the line as "Standing instructions". They come first: follow them even where they ask for a different mood than the words suggest, or for more change to the wording than the rules above allow. Which marks the model understands, and what to output, stay as stated above."""
WITH_STANDING = """Standing instructions: {preprompt}

Apply them to this line.
Line: {text}"""
PREPROMPT_LIMIT = 2000

ONLY_LINE = "Output only the marked-up line. No quotes, labels or explanation."
WITH_DIRECTION = {
    "delivery": 'Output two lines and nothing else. First "LINE: " and the marked-up line. Then "DIRECTION: " and one short phrase, under fifteen words, telling a voice actor how to deliver it: the emotion, the pace, the volume.',
    "voice": 'Output two lines and nothing else. First "LINE: " and the marked-up line. Then "DIRECTION: " and one sentence, under twenty-five words, describing the speaker rather than the line: who they sound like, the quality of their voice, their mood and their pace.',
}
# Three worked examples, shown to the model as earlier turns: a hurt line, a happy one and a
# plain announcement. A voice model with marks of its own gives its versions in the catalog.
EXAMPLES = [
    "i cant believe you did that, you promised me you would never tell anyone",
    "we did it, after 3 years of trying we finally did it",
    "the train to boston leaves at 9:15 from platform 2",
]
PLAIN_EXAMPLES = [
    "I can't believe you did that. You promised me... you would never tell anyone.",
    "We did it! After three years of trying... we finally did it!",
    "The train to Boston leaves at nine fifteen, from platform two.",
]
EXAMPLE_DIRECTIONS = {
    "delivery": ["Hurt and quiet, close to tears, slow.", "Overjoyed and breathless, fast and loud.", "Calm and clear, at an even pace."],
    "voice": [
        "A young woman with a soft, low voice, hurt and close to tears, speaking slowly.",
        "A man in his thirties with a warm voice, overjoyed and out of breath, speaking fast.",
        "A calm, clear station announcer with an even, unhurried pace.",
    ],
}


SCENE = """You are a screenwriter. Write a short scene to be performed aloud by {name}, a text-to-speech model, from the user's direction.

Rules:
- Write only words that are spoken. No narration, no scene heading, no prose stage directions, no character names, no quotation marks.
- {speakers}
- {markup}
- Where the model understands marks, use a few: where a feeling turns, where someone hesitates, laughs, or lowers their voice.
- Spoken language: contractions, short sentences, interruptions, people not finishing a thought. Numbers and dates as words.
- {length}
- Write in the language of the user's direction unless it asks for another.
- Output only the script."""

MANY_VOICES = "Up to {n} characters may speak. Start every line with its speaker's tag, written exactly like <|speaker:0|> or <|speaker:1|> (numbers 0 to {last}), and keep one number for one character throughout. One line per turn."
ONE_VOICE = "This model has one voice, so write for one character only: what they say, to someone who is not heard or to themselves. One flowing piece, no speaker labels."
SCENE_LENGTH = {
    "short": ("About four turns of one or two sentences each.", "About four sentences.", 260),
    "medium": ("About eight turns of one or two sentences each.", "About eight sentences.", 520),
    "long": ("About fourteen turns of one to three sentences each.", "About fourteen sentences.", 900),
}
SCENE_EXAMPLE = "two people in bed at night, one hears a noise downstairs and the other does not believe them at first"
PLAIN_SCENE_EXAMPLE = "Did you hear that? No, listen... there it is again. Someone's on the stairs. I am not imagining it this time."


class Llm:
    """Rewrites a line for the voice model that will speak it."""

    def __init__(self, spec):
        from mlx_lm import load

        self.model, self.tokenizer = load(spec["repo"])

    def enhance(self, text, target, direction, preprompt=""):
        import re

        from mlx_lm import stream_generate
        from mlx_lm.sample_utils import make_sampler

        rules = target.get("enhance", {})
        system = ENHANCE.format(
            name=target["name"],
            markup=rules.get("markup", "This model performs no tags: never write any."),
            output=WITH_DIRECTION[direction] if direction else ONLY_LINE,
        )
        preprompt = " ".join((preprompt or "").split())[:PREPROMPT_LIMIT]
        if sexualizes_minors(f"{preprompt} {text}"):
            raise ValueError(MINORS_REFUSAL)
        if preprompt:
            system += STANDING
        messages = [{"role": "system", "content": system}]
        for i, (given, shown) in enumerate(zip(EXAMPLES, rules.get("examples", PLAIN_EXAMPLES))):
            if direction:
                shown = f"LINE: {shown}\nDIRECTION: {EXAMPLE_DIRECTIONS[direction][i]}"
            messages += [{"role": "user", "content": given}, {"role": "assistant", "content": shown}]
        messages.append({"role": "user", "content": WITH_STANDING.format(preprompt=preprompt, text=text) if preprompt else text})
        # Thinking off: the answer is the line itself, straight away.
        prompt = self.tokenizer.apply_chat_template(messages, add_generation_prompt=True, tokenize=False, enable_thinking=False)
        budget = min(2048, len(self.tokenizer.encode(text)) * 2 + 120)
        out = ""
        for piece in stream_generate(self.model, self.tokenizer, prompt, max_tokens=budget, sampler=make_sampler(temp=0.7, top_p=0.95)):
            out += piece.text
        out = re.sub(r"<\|channel>.*?(<channel\|>|$)", "", out, flags=re.S).replace("<turn|>", "").strip()
        line, told = out, ""
        if direction:
            parts = re.split(r"^\s*DIRECTION:\s*", out, maxsplit=1, flags=re.I | re.M)
            line = parts[0]
            told = " ".join(parts[1].split()) if len(parts) > 1 else ""
        line = re.sub(r"^\s*LINE:\s*", "", line.strip(), flags=re.I).strip()
        if len(line) > 1 and line[0] == line[-1] and line[0] in "\"'":
            line = line[1:-1].strip()
        allowed = rules.get("tags")
        if allowed:
            # A tag the voice model doesn't know would be read aloud; drop any that slipped in.
            line = re.sub(r"\[[^\]\n]{1,30}\]|<[^>\n]{1,20}>", lambda m: m.group(0) if m.group(0) in allowed else "", line)
            line = re.sub(r"[ \t]{2,}", " ", line).strip()
        if not line:
            raise RuntimeError("The enhancer wrote nothing. Try again.")
        if sexualizes_minors(f"{preprompt} {line} {told}"):
            raise ValueError(MINORS_REFUSAL)
        return line, told.strip("\"'")

    def scene(self, direction, target, length="short", preprompt=""):
        """Writes the dialogue for a scene from a description of it, in the marks and the
        speaker tags the voice model understands."""
        import re

        from mlx_lm import stream_generate
        from mlx_lm.sample_utils import make_sampler

        rules = target.get("enhance", {})
        voices = int(rules.get("speakers", 1))
        turns, sentences, budget = SCENE_LENGTH.get(length, SCENE_LENGTH["short"])
        system = SCENE.format(
            name=target["name"],
            speakers=MANY_VOICES.format(n=voices, last=voices - 1) if voices > 1 else ONE_VOICE,
            markup=rules.get("markup", "This model performs no tags: never write any."),
            length=turns if voices > 1 else sentences,
        )
        preprompt = " ".join((preprompt or "").split())[:PREPROMPT_LIMIT]
        if sexualizes_minors(f"{preprompt} {direction}"):
            raise ValueError(MINORS_REFUSAL)
        if preprompt:
            system += STANDING
        messages = [
            {"role": "system", "content": system},
            {"role": "user", "content": SCENE_EXAMPLE},
            {"role": "assistant", "content": rules.get("scene_example", PLAIN_SCENE_EXAMPLE)},
            {"role": "user", "content": WITH_STANDING.format(preprompt=preprompt, text=direction).replace("Line:", "Scene:").replace("this line", "this scene") if preprompt else direction},
        ]
        prompt = self.tokenizer.apply_chat_template(messages, add_generation_prompt=True, tokenize=False, enable_thinking=False)
        out = ""
        for piece in stream_generate(self.model, self.tokenizer, prompt, max_tokens=budget, sampler=make_sampler(temp=0.8, top_p=0.95)):
            out += piece.text
        out = re.sub(r"<\|channel>.*?(<channel\|>|$)", "", out, flags=re.S).replace("<turn|>", "").strip()
        lines = []
        for line in out.splitlines():
            line = line.strip().strip('"').strip()
            if not line:
                continue
            if voices > 1:
                # Every turn needs its tag, and only tags the model has.
                tag = re.match(r"<\|speaker:(\d+)\|>", line)
                if not tag:
                    continue
                line = f"<|speaker:{min(int(tag.group(1)), voices - 1)}|> {line[tag.end():].strip()}"
            else:
                # A name the model slipped in ("ANNA: ...") would be read aloud.
                line = re.sub(r"^[A-Z][\w ]{0,20}:\s+", "", re.sub(r"<\|speaker:\d+\|>\s*", "", line))
            lines.append(line)
        allowed = rules.get("tags")
        script = ("\n" if voices > 1 else " ").join(lines)
        if allowed:
            script = re.sub(r"\[[^\]\n]{1,30}\]|<(?!\|)[^>\n]{1,20}>", lambda m: m.group(0) if m.group(0) in allowed else "", script)
            script = re.sub(r"[ \t]{2,}", " ", script).strip()
        if not script:
            raise RuntimeError("The enhancer wrote no dialogue. Try again, or say more about the scene.")
        if sexualizes_minors(f"{preprompt} {script}"):
            raise ValueError(MINORS_REFUSAL)
        return script


# ----- Kitten TTS 2 (its own package, on its MLX backend) -----


class Kitten:
    EXPRESSIVE = ("[", "<", "(((")

    def __init__(self, spec):
        from kittenml import KittenTTS

        self.model = KittenTTS(spec["repo"], backend="mlx")
        self.languages = {v["id"]: v.get("lang", "en") for v in spec.get("voices", [])}

    def run(self, req, status):
        text = req["text"]
        kwargs = {}
        english = True
        if req.get("ref_audio"):
            kwargs["reference"] = req["ref_audio"]
            if (req.get("ref_text") or "").strip():
                kwargs["reference_text"] = req["ref_text"].strip()
            else:
                status("Transcribing the voice sample")
        elif req.get("voice"):
            kwargs["voice"] = req["voice"]
            english = self.languages.get(req["voice"], "en") == "en"
        # Emotion tags, vocal events and emphasis want the looser sampling preset.
        if any(mark in text for mark in self.EXPRESSIVE):
            kwargs["preset"] = "expressive"
        status("Generating")
        # Its text normalizer is tuned for English and mangles numbers and dates elsewhere.
        audio = self.model.generate(text, normalize=english, **kwargs)
        return audio, self.model.sample_rate, ""


# ----- PyTorch on Metal (voice conversion) -----


def torch_device():
    import torch

    return "mps" if torch.backends.mps.is_available() else "cpu"


class ChatterboxVc:
    def __init__(self, spec):
        from chatterbox.vc import ChatterboxVC

        self.model = ChatterboxVC.from_pretrained(torch_device())

    def run(self, req, status):
        status("Converting")
        wav = self.model.generate(audio=req["source_audio"], target_voice_path=req["ref_audio"])
        return wav.squeeze().cpu().numpy(), self.model.sr, ""


class SeedVc:
    """Seed-VC, from its repository checked out inside the environment (`setup.py` clones it)."""

    def __init__(self, spec):
        repo = Path(sys.prefix) / "src" / "seed-vc"
        # Its code finds its configs and keeps its checkpoints relative to where it runs.
        os.chdir(repo)
        sys.path.insert(0, str(repo))
        from seed_vc_wrapper import SeedVCWrapper

        self.steps = spec.get("options", {}).get("steps", 25)
        self.model = SeedVCWrapper()

    def run(self, req, status):
        status("Converting")
        steps = self.model.convert_voice(req["source_audio"], req["ref_audio"], diffusion_steps=self.steps, stream_output=False)
        # A generator that returns the whole clip when it finishes.
        try:
            while True:
                next(steps)
        except StopIteration as done:
            audio = done.value
        if audio is None:
            raise RuntimeError("The model returned no audio.")
        return audio, 22050, ""


class CoquiVc:
    """A voice-conversion model from coqui-tts (OpenVoice v2)."""

    def __init__(self, spec):
        from TTS.api import TTS

        self.model = TTS(spec["options"]["model"]).to(torch_device())

    def run(self, req, status):
        status("Converting")
        wav = self.model.voice_conversion(source_wav=req["source_audio"], target_wav=req["ref_audio"])
        return wav, self.model.voice_converter.output_sample_rate, ""


ADAPTERS = {"mlx_tts": MlxTts, "mlx_sts": MlxSts, "sophon_talker": SophonTalker, "sophon": Sophon, "lfm_audio": LfmAudio, "zonos": Zonos, "llm": Llm, "kitten": Kitten, "chatterbox_vc": ChatterboxVc, "seed_vc": SeedVc, "coqui_vc": CoquiVc}
LOADED = {}


def peak_memory_gb(reset=False):
    """The most memory the run held on the GPU. MLX tracks a true peak; PyTorch's Metal
    backend only says what it holds now, which after a run is close to it."""
    try:
        if "mlx.core" in sys.modules:
            import mlx.core as mx

            if reset:
                mx.reset_peak_memory()
                return 0.0
            return mx.get_peak_memory() / 1e9
        if "torch" in sys.modules and not reset:
            import torch

            if torch.backends.mps.is_available():
                return torch.mps.driver_allocated_memory() / 1e9
    except Exception:
        pass
    return 0.0


def handle(req):
    request_id = req["id"]
    spec = MODELS[req["model"]]

    def status(message):
        emit("status", id=request_id, message=message)

    load_seconds = 0.0
    if spec["id"] not in LOADED:
        status(f"Loading {spec['name']}")
        started = time.perf_counter()
        LOADED[spec["id"]] = ADAPTERS[spec["adapter"]](spec)
        load_seconds = time.perf_counter() - started
    if req.get("op") == "scene":
        # Not a take either: write the dialogue for a scene the user described.
        started = time.perf_counter()
        script = LOADED[spec["id"]].scene(req["text"], MODELS[req["target"]], req.get("length") or "short", req.get("preprompt") or "")
        emit("done", id=request_id, text=script, direction="", gen_seconds=time.perf_counter() - started, load_seconds=load_seconds)
        return
    if req.get("op") == "enhance":
        # Not a take: rewrite the line for the model that will speak it.
        target = MODELS[req["target"]]
        wants = target.get("enhance", {}).get("direction")
        direction = wants if wants and not (req.get("instruct") or "").strip() else None
        started = time.perf_counter()
        line, told = LOADED[spec["id"]].enhance(req["text"], target, direction, req.get("preprompt") or "")
        emit("done", id=request_id, text=line, direction=told, gen_seconds=time.perf_counter() - started, load_seconds=load_seconds)
        return
    peak_memory_gb(reset=True)
    started = time.perf_counter()
    # A model that called a tool says so in a fourth value.
    audio, sample_rate, text, *more = LOADED[spec["id"]].run(req, status)
    gen_seconds = time.perf_counter() - started
    audio_seconds = write_wav(req["out"], audio, sample_rate)
    emit(
        "done",
        id=request_id,
        gen_seconds=gen_seconds,
        load_seconds=load_seconds,
        audio_seconds=audio_seconds,
        sample_rate=int(sample_rate),
        peak_gb=peak_memory_gb(),
        text=text,
        tool=more[0] if more else "",
    )


def prepare(model_id):
    """Loads a model once, so an install ends with everything it needs on disk and working."""
    spec = MODELS[model_id]
    adapter = ADAPTERS[spec["adapter"]](spec)
    if hasattr(adapter, "prepare"):
        adapter.prepare()


def main():
    if len(sys.argv) == 3 and sys.argv[1] == "--prepare":
        prepare(sys.argv[2])
        return
    emit("ready", pid=os.getpid())
    for line in sys.stdin:
        line = line.strip()
        if not line:
            continue
        try:
            req = json.loads(line)
        except ValueError:
            continue
        try:
            handle(req)
        except Exception as error:
            traceback.print_exc()
            message = f"{type(error).__name__}: {error}".strip()
            emit("error", id=req.get("id"), message=message[:2000])


if __name__ == "__main__":
    main()
