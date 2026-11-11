"""Sophon-Realtime-32B, the half that listens and decides what to say, on Apple Silicon.

The model is published for vLLM-Omni on NVIDIA GPUs. This is a port of what a single turn
needs, built from the published weights and the Apache-2.0 serving code
(github.com/adventists-ai/sophon-realtime and the duplexjev-vllm package):

- the "thinker" is a Qwen3 32B language model whose linear layers ship as FP8. `convert`
  turns them into an MLX model once (4-bit by default) that mlx-lm loads like any other;
- its ears are a Qwen3-ASR audio encoder and an Ultravox-style projector, small enough to run
  in PyTorch as written: speech becomes a run of embeddings that stand where `<|audio|>` is
  in the prompt;
- the reply is text with sound tags, which the model's own voice stage then speaks.

What is not ported: the live session around it (listening while speaking, deciding when the
user has finished, barge-in), tool calling, and the camera. Here one recording goes in and one
reply comes out.
"""

import json
import re
import shutil
from fnmatch import fnmatch
from pathlib import Path

# The language model as published, inside the repository. Only `convert` reads these.
SHARDS = "thinker/model-*.safetensors"
AUDIO_TOKEN = "<|audio|>"
SAMPLE_RATE = 16000
HOP = 160  # samples per mel frame
MEL_CHUNK = 3000  # mel frames the encoder sees at once: 30 s

# The system prompt the model was tuned with (flash/config/system_prompt.json upstream);
# without it it writes fewer sound tags.
SYSTEM = {
    "zh": "你是智子（Sophon），AI降临派（Adventists AI）训练的 Sophon 系列模型，能听、能看，也能说。你会直接听到用户的语音，你的回复会被直接合成为语音说出来。请像真人对话一样，用自然的口语一两句话回应用户，注意用户的情绪。只写要说出口的话，不要写括号说明、表情或格式符号；数字、日期、金额、符号和英文缩写都直接写成口语念法。需要时可以用声音标签表现笑声、咳嗽和重读。",
    "en": "You are Sophon, a model of the Sophon series trained by Adventists AI (AI降临派); you can hear, see and speak. You hear the user's speech directly, and your reply will be spoken aloud. Respond the way a person would in conversation, in one or two natural spoken sentences, paying attention to the user's emotion. Write only the words to be spoken: no parenthetical notes, emoji or formatting; write numbers and symbols the way they are read aloud. When it fits, you may use sound tags for laughter, coughs and emphasis.",
}

TOKENIZER_FILES = (
    "tokenizer.json",
    "tokenizer_config.json",
    "vocab.json",
    "merges.txt",
    "added_tokens.json",
    "special_tokens_map.json",
    "chat_template.jinja",
    "generation_config.json",
)


# ----- Converting the thinker's language model to MLX -----


def convert(source: Path, target: Path, bits: int = 4, say=print) -> None:
    """Writes the thinker's language model as a quantized MLX model at `target`.

    The checkpoint stores each linear layer as FP8 (E4M3) with one scale per output row;
    everything else is bf16. Layers are dequantized, re-quantized and evaluated one at a time,
    so the 32B model never sits in memory at full precision.
    """
    import mlx.core as mx
    import mlx.nn as nn
    from mlx.utils import tree_map
    from mlx_lm.models import qwen3
    from mlx_lm.utils import save_config, save_model

    thinker = source / "thinker"
    text = json.loads((thinker / "config.json").read_text())["text_config"]
    config = {
        "model_type": "qwen3",
        "hidden_size": text["hidden_size"],
        "num_hidden_layers": text["num_hidden_layers"],
        "intermediate_size": text["intermediate_size"],
        "num_attention_heads": text["num_attention_heads"],
        "num_key_value_heads": text["num_key_value_heads"],
        "head_dim": text["head_dim"],
        "rms_norm_eps": text["rms_norm_eps"],
        "vocab_size": text["vocab_size"],
        "max_position_embeddings": text["max_position_embeddings"],
        # Speech and text take ordinary positions, where Qwen3-VL's three-axis rotary
        # embedding is the plain one.
        "rope_theta": text["rope_parameters"]["rope_theta"],
        "tie_word_embeddings": False,
        "bos_token_id": text["bos_token_id"],
        "eos_token_id": text["eos_token_id"],
    }
    model = qwen3.Model(qwen3.ModelArgs.from_dict(config))

    weights = {}
    shards = sorted(source.glob(SHARDS))
    if not shards:
        raise FileNotFoundError(f"No thinker weights in {thinker}")
    for shard in shards:
        weights.update(mx.load(str(shard)))  # lazy: nothing is read until it is used
    prefix = "language_model."

    def tensor(name):
        value = weights[prefix + name]
        if value.dtype == mx.uint8:  # FP8, with its per-row scale beside it
            value = mx.from_fp8(value, mx.float32) * weights[prefix + name + "_scale"].astype(mx.float32)
        return value

    def half(module):
        module.update(tree_map(lambda p: p.astype(mx.float16) if mx.issubdtype(p.dtype, mx.floating) else p, module.parameters()))
        mx.eval(module.parameters())

    layers = model.model.layers
    for index, layer in enumerate(layers):
        stem = f"model.layers.{index}."
        names = [key[len(prefix) :] for key in weights if key.startswith(prefix + stem) and not key.endswith("_scale")]
        layer.load_weights([(name[len(stem) :], tensor(name)) for name in names], strict=True)
        nn.quantize(layer, group_size=64, bits=bits)
        half(layer)
        if index % 8 == 7 or index == len(layers) - 1:
            say(f"Converted {index + 1} of {len(layers)} layers")

    # The two vocabulary-sized tables stay at 8 bits: they are small next to the layers, and
    # the audio embeddings have to sit among the token embeddings.
    model.model.embed_tokens.load_weights([("weight", tensor("model.embed_tokens.weight"))])
    model.model.embed_tokens = nn.QuantizedEmbedding.from_embedding(model.model.embed_tokens, 64, 8)
    model.lm_head.load_weights([("weight", tensor("lm_head.weight"))])
    model.lm_head = nn.QuantizedLinear.from_linear(model.lm_head, 64, 8)
    model.model.norm.load_weights([("weight", tensor("model.norm.weight"))])
    half(model.model.embed_tokens)
    half(model.lm_head)
    half(model.model.norm)

    config["quantization"] = {
        "group_size": 64,
        "bits": bits,
        "mode": "affine",
        "model.embed_tokens": {"group_size": 64, "bits": 8},
        "lm_head": {"group_size": 64, "bits": 8},
    }
    partial = target.with_name(target.name + ".partial")
    shutil.rmtree(partial, ignore_errors=True)
    partial.mkdir(parents=True)
    say("Writing the converted model")
    save_model(partial, model, donate_model=True)
    save_config(config, partial / "config.json")
    for name in TOKENIZER_FILES:
        if (thinker / name).is_file():
            shutil.copyfile(thinker / name, partial / name)
    shutil.rmtree(target, ignore_errors=True)
    partial.rename(target)


def discard_shards(source: Path) -> None:
    """Deletes the published language-model weights from the Hugging Face cache once the
    converted copy exists: 33 GB nothing reads again. Done through the library that wrote
    them, which knows where the cache really keeps a file and what else points at it."""
    from huggingface_hub import scan_cache_dir

    cache = scan_cache_dir()
    files = [
        file
        for repo in cache.repos
        for revision in repo.revisions
        if revision.snapshot_path.samefile(source)
        for file in revision.files
        if fnmatch(file.file_path.relative_to(revision.snapshot_path).as_posix(), SHARDS)
    ]
    if files:
        cache.delete_files(*files).execute()


# ----- Ears: the audio encoder and the projector into the language model -----


def encoder_frames(mel_frames: int, window: int = 50) -> int:
    """How many frames the encoder returns for `mel_frames` of input: three stride-2
    convolutions over 100-frame chunks, 13 frames to a full chunk."""
    chunk = window * 2
    rest = mel_frames % chunk
    after = (rest - 1) // 2 + 1
    return ((after - 1) // 2 + 1 - 1) // 2 + 1 + (mel_frames // chunk) * 13


def _modules():
    """The PyTorch modules, defined on first use so importing this file needs no torch."""
    import numpy as np
    import torch
    import torch.nn.functional as F
    from torch import nn

    class Attention(nn.Module):
        def __init__(self, dim, heads):
            super().__init__()
            self.heads = heads
            self.scale = (dim // heads) ** -0.5
            self.k_proj = nn.Linear(dim, dim)
            self.v_proj = nn.Linear(dim, dim)
            self.q_proj = nn.Linear(dim, dim)
            self.out_proj = nn.Linear(dim, dim)

        def forward(self, x, lengths):
            # x: one packed sequence (frames, dim). Attention stays inside each block.
            n = x.shape[0]
            q = self.q_proj(x).reshape(n, self.heads, -1).transpose(0, 1).unsqueeze(0)
            k = self.k_proj(x).reshape(n, self.heads, -1).transpose(0, 1).unsqueeze(0)
            v = self.v_proj(x).reshape(n, self.heads, -1).transpose(0, 1).unsqueeze(0)
            out = [
                F.scaled_dot_product_attention(a, b, c, scale=self.scale)
                for a, b, c in zip(q.split(lengths, 2), k.split(lengths, 2), v.split(lengths, 2))
            ]
            return self.out_proj(torch.cat(out, 2).transpose(1, 2).reshape(n, -1))

    class Layer(nn.Module):
        def __init__(self, dim, heads, hidden):
            super().__init__()
            self.self_attn = Attention(dim, heads)
            self.self_attn_layer_norm = nn.LayerNorm(dim)
            self.fc1 = nn.Linear(dim, hidden)
            self.fc2 = nn.Linear(hidden, dim)
            self.final_layer_norm = nn.LayerNorm(dim)

        def forward(self, x, lengths):
            x = x + self.self_attn(self.self_attn_layer_norm(x), lengths)
            return x + self.fc2(F.gelu(self.fc1(self.final_layer_norm(x))))

    class Tower(nn.Module):
        """The Qwen3-ASR encoder: a convolutional front end over 1 s chunks of mel, then
        transformer layers that attend within 8 s windows."""

        def __init__(self, c):
            super().__init__()
            dim, down = c["d_model"], c["downsample_hidden_size"]
            self.window, self.window_infer = c["n_window"], c["n_window_infer"]
            self.layers = nn.ModuleList([Layer(dim, c["encoder_attention_heads"], c["encoder_ffn_dim"]) for _ in range(c["encoder_layers"])])
            self.ln_post = nn.LayerNorm(dim)
            self.conv2d1 = nn.Conv2d(1, down, 3, 2, padding=1)
            self.conv2d2 = nn.Conv2d(down, down, 3, 2, padding=1)
            self.conv2d3 = nn.Conv2d(down, down, 3, 2, padding=1)
            self.conv_out = nn.Linear(down * ((((c["num_mel_bins"] + 1) // 2 + 1) // 2 + 1) // 2), dim, bias=False)
            length, half = c["max_position_embeddings"], dim // 2
            scales = torch.exp(-np.log(10000) / (half - 1) * torch.arange(half).float())
            time = torch.arange(length)[:, None] * scales[None, :]
            self.register_buffer("positions", torch.cat([torch.sin(time), torch.cos(time)], 1), persistent=False)

        def forward(self, mel):
            # mel: (bins, frames), every frame valid.
            bins, frames = mel.shape
            chunk = self.window * 2
            padded = -(-frames // chunk) * chunk
            valid = torch.zeros(padded, dtype=torch.long)
            valid[:frames] = 1
            mel = F.pad(mel, (0, padded - frames))
            chunks = padded // chunk
            per_chunk = valid.view(chunks, chunk).sum(-1)

            x = mel.view(bins, chunks, chunk).permute(1, 0, 2).unsqueeze(1)
            x = F.gelu(self.conv2d3(F.gelu(self.conv2d2(F.gelu(self.conv2d1(x))))))
            count, channels, freq, steps = x.shape
            x = self.conv_out(x.permute(0, 3, 1, 2).reshape(count, steps, channels * freq)) + self.positions[:steps]

            kept = per_chunk.clone()
            for _ in range(3):
                kept = torch.where(kept > 0, (kept - 1) // 2 + 1, torch.zeros_like(kept))
            mask = torch.arange(steps)[None, :] < kept[:, None]
            x = x.reshape(-1, x.shape[-1])[mask.flatten()]

            # Attention blocks: 8 s of encoder frames each, and whatever is left.
            total = encoder_frames(frames, self.window)
            block = encoder_frames(int(per_chunk.max()), self.window) * (self.window_infer // chunk)
            lengths = [block] * (total // block) + ([total % block] if total % block else [])
            for layer in self.layers:
                x = layer(x, lengths)
            return self.ln_post(x)

    class RMSNorm(nn.Module):
        def __init__(self, dim):
            super().__init__()
            self.weight = nn.Parameter(torch.ones(dim))

        def forward(self, x):
            return x * torch.rsqrt(x.pow(2).mean(-1, keepdim=True) + 1e-6) * self.weight

    class Projector(nn.Module):
        """Ultravox's projector: stack neighbouring frames, then a gated two-layer map into
        the language model's embedding space."""

        def __init__(self, dim_in, hidden, dim_out, stack):
            super().__init__()
            self.stack = stack
            self.ln_pre = RMSNorm(dim_in * stack)
            self.linear_1 = nn.Linear(dim_in * stack, hidden, bias=False)
            self.ln_mid = RMSNorm(hidden // 2)
            self.linear_2 = nn.Linear(hidden // 2, dim_out, bias=False)

        def forward(self, x):
            frames, dim = x.shape
            padded = -(-frames // self.stack) * self.stack
            x = F.pad(x, (0, 0, 0, padded - frames)).view(padded // self.stack, dim * self.stack)
            x, gate = self.linear_1(self.ln_pre(x)).chunk(2, -1)
            return self.linear_2(self.ln_mid(F.silu(gate) * x))

    return Tower, Projector


class Ears:
    """Speech to the embeddings the language model reads in place of `<|audio|>`."""

    def __init__(self, audio_dir: Path):
        import torch
        from safetensors.torch import load_file
        from transformers import WhisperFeatureExtractor

        Tower, Projector = _modules()
        config = json.loads((audio_dir / "config.json").read_text())
        self.stack = config["stack_factor"]
        self.features = WhisperFeatureExtractor.from_pretrained(str(audio_dir))
        self.tower = Tower(config["audio_config"])
        self.projector = Projector(config["audio_config"]["d_model"], config["hidden_size"], config["text_config"]["hidden_size"], self.stack)
        state = {name: value.float() for name, value in load_file(str(audio_dir / "model.safetensors")).items()}
        self.tower.load_state_dict({k[len("audio_tower.") :]: v for k, v in state.items() if k.startswith("audio_tower.")}, strict=True)
        self.projector.load_state_dict({k[len("multi_modal_projector.") :]: v for k, v in state.items() if k.startswith("multi_modal_projector.")}, strict=True)
        self.tower.eval()
        self.projector.eval()
        self.torch = torch

    def __call__(self, audio):
        """`audio`: mono float samples at 16 kHz. Returns (tokens, hidden) float32."""
        import numpy as np

        torch = self.torch
        audio = np.asarray(audio, dtype=np.float32).reshape(-1)
        # Pad with silence to a whole number of audio tokens, as the model's processor does.
        step = HOP * 8 * self.stack
        length = -(-max(len(audio), 2 * HOP) // step) * step
        audio = np.pad(audio, (0, length - len(audio)))
        mel = self.features([audio], sampling_rate=SAMPLE_RATE, padding="longest", pad_to_multiple_of=HOP, truncation=False, return_tensors="pt").input_features[0]
        out = []
        with torch.inference_mode():
            for start in range(0, mel.shape[-1], MEL_CHUNK):
                part = mel[:, start : start + MEL_CHUNK].float()
                tokens = -(-part.shape[-1] // (8 * self.stack))
                out.append(self.projector(self.tower(part))[:tokens])
        return torch.cat(out, 0).numpy()


# ----- The thinker: speech in, a reply in words (and sound tags) out -----

POINT_TAGS = {"[laughter]", "[breath]", "[sigh]", "[cough]", "[quick_breath]"}
SPAN_TAGS = {"laughter", "strong"}
# Asked not to, it still signs off with an emoji now and then; the voice cannot say one.
EMOJI = re.compile("[\U0001F000-\U0001FAFF\u2600-\u27BF\uFE0F\u200D\U000E0020-\U000E007F]")
CLOSERS = {"[": "]", "<": ">", "(": ")", "（": "）", "【": "】"}


def clean_reply(reply: str) -> str:
    """What the voice stage should be given: the words and the sound tags it performs, without
    stage directions in brackets or markdown. (The upstream `tag_clean`, for a whole reply.)"""
    out, opened, rest = [], [], reply.split("<tool_call>")[0]
    while rest:
        found = [i for i in (rest.find(c) for c in list(CLOSERS) + ["*", "#", "`"]) if i >= 0]
        if not found:
            out.append(rest)
            break
        first = min(found)
        if first:
            out.append(rest[:first])
            rest = rest[first:]
        mark = rest[0]
        if mark in "*#`":
            rest = rest[1:]
            continue
        if mark == "<" and not re.match(r"</?[A-Za-z_|][^<>\n]{0,40}>", rest):
            out.append(mark)  # a less-than sign, not a tag
            rest = rest[1:]
            continue
        end = rest.find(CLOSERS[mark], 1)
        if end < 0:
            out.append(re.sub(r"[\[<(（【].*$", "", rest))
            break
        token, rest = rest[: end + 1], rest[end + 1 :]
        span = re.fullmatch(r"<(/?)([a-z_\-]+)>", token)
        if token in POINT_TAGS:
            out.append(token)
        elif span and span.group(2) in SPAN_TAGS:
            name = span.group(2)
            if not span.group(1) and name not in opened:
                opened.append(name)
                out.append(token)
            elif span.group(1) and name in opened:
                opened.remove(name)
                out.append(token)
    out.extend(f"</{name}>" for name in reversed(opened))
    spoken = EMOJI.sub("", "".join(out))
    return re.sub(r"[ \t]{2,}", " ", spoken).strip()


class Thinker:
    def __init__(self, source: Path, converted: Path):
        from mlx_lm import load

        self.model, self.tokenizer = load(str(converted))
        self.ears = Ears(source / "thinker" / "audio")
        audio_config = json.loads((source / "thinker" / "audio" / "config.json").read_text())
        self.audio_id = audio_config["audio_token_index"]
        self.stop = {self.tokenizer.eos_token_id, self.tokenizer.convert_tokens_to_ids("<|im_end|>"), self.tokenizer.convert_tokens_to_ids("<|endoftext|>")}

    def reply(self, samples, language: str = "en", max_tokens: int = 160, temperature: float = 0.7, top_p: float = 0.9) -> str:
        """The model's answer to one stretch of speech (mono float samples at 16 kHz)."""
        import mlx.core as mx
        from mlx_lm.generate import generate_step
        from mlx_lm.sample_utils import make_sampler

        heard = self.ears(samples)
        messages = [{"role": "system", "content": SYSTEM.get(language, SYSTEM["en"])}, {"role": "user", "content": AUDIO_TOKEN}]
        prompt = self.tokenizer.apply_chat_template(messages, tokenize=False, add_generation_prompt=True, enable_thinking=False)
        before, after = prompt.split(AUDIO_TOKEN)
        head = self.tokenizer.encode(before, add_special_tokens=False)
        tail = self.tokenizer.encode(after, add_special_tokens=False)
        ids = mx.array(head + [self.audio_id] * len(heard) + tail)
        embeddings = self.model.model.embed_tokens(ids)
        speech = mx.array(heard).astype(embeddings.dtype)
        embeddings = mx.concatenate([embeddings[: len(head)], speech, embeddings[len(head) + len(heard) :]], axis=0)

        written, finished = [], False
        for token, _ in generate_step(ids, self.model, max_tokens=max_tokens, sampler=make_sampler(temp=temperature, top_p=top_p), input_embeddings=embeddings):
            token = int(token)
            if token in self.stop:
                finished = True
                break
            written.append(token)
        reply = self.tokenizer.decode(written).strip()
        if not finished:
            # Out of room mid-sentence: say the sentences that were finished.
            whole = re.match(r"(?s).*[.!?。！？…](?:[\"'”’)\]]|</[a-z]+>)*", reply)
            reply = whole.group(0) if whole else reply
        return reply
