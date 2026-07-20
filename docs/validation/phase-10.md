# Phase 10 (supplemental): Real-Audio End-to-End

Phases 0–9 validate the CLI contract with **mock** FFmpeg/whisper for
determinism. This supplemental suite closes the remaining gap: it exercises the
**real** local stack — macOS `say` → real FFmpeg normalize → real whisper.cpp
model — and asserts the output contract on genuine ASR output.

## What it does

`scripts/e2e/phase-10-real-audio.sh`:

1. Generates a deterministic pangram utterance with `say`, normalizes it to
   16 kHz mono WAV with FFmpeg.
2. Runs `doctor --format json` against the real stack and asserts
   `ok == true` with `ffmpeg`, `asr`, and `model-path` checks all `ok`.
3. Runs `transcribe --mode clean --no-llm` in **text, json, jsonl, and md**.
4. Asserts, per format: non-empty stdout, exit 0, valid schema (json required
   keys + non-empty timed segments; every jsonl line parses; md section
   headers), and a normalized-contains match on the pangram
   `quick brown fox jumps over the lazy dog`. Brand words are not asserted
   (ASR-variable).

Artifacts land under `docs/validation/artifacts/phase-10/`, path-scrubbed
(`<tmp>`, `<repo>`, `<home>`, `<model>`, `<whisper>`, `<ffmpeg-path>`).

## Prerequisites & CI behavior

Requires a real whisper ggml model + `whisper-cli` + macOS `say`. Model
resolution: `COMLINK_WHISPER_MODEL`, else `~/.local/share/whisper/ggml-*.bin`
(prefers `ggml-medium.en.bin`).

When any real dependency is missing, the suite **SKIPS with exit 0** and a
`SKIP:` reason on stderr — so it is safe as an opt-in CI gate on runners without
a model. `cargo` missing is still a hard failure (exit 1).

## Run

```bash
scripts/e2e/phase-10-real-audio.sh
# or point at a specific model
COMLINK_WHISPER_MODEL=/path/to/ggml-small.en.bin scripts/e2e/phase-10-real-audio.sh
```

## Result on the validated machine (2026-07-20)

Passed with `ggml-medium.en.bin`. `say` fixture (~4.5 s) transcribed as
`testing comlink transcription. The quick brown fox jumps over the lazy dog.`;
pangram verified across all four formats; `doctor` reported `ok: true`.

## Known limitations

- Not hermetic: real ASR output varies by model/version, so assertions are
  normalized-contains, not exact equality.
- Live microphone `record` and live meeting capture (`meet`, incl. BlackHole
  system audio) still require a human + macOS permission grants and are not
  covered here.
