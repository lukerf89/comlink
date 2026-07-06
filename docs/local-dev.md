# Local Install and Dev Run

Comlink is a local-first CLI. The default transcription path uses local FFmpeg,
local whisper.cpp, and a local ggml model file.

## Install for Local Use

Build and install the current checkout:

```bash
cargo install --path .
```

Or run without installing:

```bash
cargo run -- doctor --format json
cargo run -- transcribe tests/fixtures/audio/short.wav --mode memo --format json
```

## Required Runtime Tools

Set explicit paths when binaries are not on `PATH`:

```bash
export COMLINK_FFMPEG="/opt/homebrew/bin/ffmpeg"
export COMLINK_FFPROBE="/opt/homebrew/bin/ffprobe"
export COMLINK_WHISPER_CPP="/path/to/whisper-cli"
export COMLINK_WHISPER_MODEL="/path/to/ggml-model.bin"
```

You can also persist a model selection:

```bash
comlink models select tiny --path /path/to/ggml-tiny.en.bin
```

Run diagnostics after changing paths:

```bash
comlink doctor
comlink doctor --format json
```

## Recording and Clipboard

Record from the default macOS AVFoundation microphone device:

```bash
comlink record --mode memo --copy --format json
```

If the default device is wrong, override it:

```bash
COMLINK_RECORD_DEVICE=":1" comlink record --mode memo --copy
```

Clipboard delivery uses `pbcopy` by default. Tests and custom environments can
override it:

```bash
COMLINK_PBCOPY=/path/to/pbcopy-compatible-command comlink record --copy
```

`--restore-clipboard` is opt-in and requires `--copy`. It reads the previous
clipboard through `pbpaste` or `COMLINK_PBPASTE`, writes the final text, then
restores the previous clipboard. This is useful for adapter testing and future
paste workflows; the default copy-first workflow leaves the final text on the
clipboard.

## Work Surface Modes

Built-in deterministic modes include:

- `terminal`: single-line command or agent prompt text; dictated line breaks
  become `&&` separators and trailing sentence punctuation is removed.
- `editor`: editor-safe prose that honors dictated `new line`.
- `outlook`: email-composer prose that honors dictated `new paragraph`.
- `slack`: chat-composer text that stays concise.
- `memo`: note text with sentence-ending punctuation.

Examples:

```bash
comlink modes apply --mode terminal --text "cargo test --all new line git status --short"
comlink modes apply --mode outlook --text "thanks new paragraph I can review today"
```

## Deferred From Phase 6

Global hotkey, active paste, and app-specific automation remain deferred. Phase 6
keeps the supported delivery model copy-first so failures are visible, local,
and easy to recover from.
