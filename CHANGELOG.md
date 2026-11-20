# Changelog

Each release's notes. `scripts/release` publishes the part under the version it is releasing,
on GitHub and in the app's update window.

## 0.2.0

- Reverb-TTS adds 16 preset voices and voice cloning in seven languages. Whisper Small
  transcribes your voice sample locally. You can edit its words and speak a line in a different language.
- Three new models: Inflect Mini, Micro and Atom, small English voices that run on the CPU
  many times faster than real time. Mini plays vocal events written into the line, such as
  `[laugh]` or `[yawn]`, and can run a take through its 48 kHz enhancer: write 48 kHz in
  Direction.
- Ventriloquist can now download signed updates in the background and install them when
  you quit. This is on by default. Choose **Ventriloquist › Update Settings…** to change it.

## 0.1.1

- ZONOS3 Mini downloads and loads again. Its publisher renamed one of the model's files,
  which left a new download without it; the app now asks for the file by its new name, at a
  fixed revision of the model, so a later rename can't break it.

## 0.1.0

The first release.

- A bench for open voice models that run on Apple silicon: text to speech, voice cloning,
  voice conversion and spoken conversation, with every take's speed and memory beside it.
- 23 models in the catalog, each downloaded only when you ask for it.
- Built-in voices with previews, cloning from a sample, and direction where a model takes it.
- Conversation with Sophon Realtime 32B Flash, ported to Apple silicon, and tool calling
  with LFM2.5 Audio.
- Enhance and Scene: a local language model that marks up a line, or writes dialogue from a
  direction, for the voice model that will speak it.
- Updates itself from this release on.
