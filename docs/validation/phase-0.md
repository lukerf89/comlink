# Phase 0 Validation

Date: 2026-07-04

## Scope

Implemented the repo scaffold and local ASR spike for Phase 0:

- Rust `comlink` CLI crate.
- `comlink doctor` dependency diagnostics.
- `comlink transcribe FILE --format text|json`.
- FFmpeg normalization to 16 kHz mono WAV.
- `whisper.cpp` subprocess adapter behind an `AsrEngine` trait.
- Minimal JSON envelope with `text`, `engine`, `model`, `duration_ms`, `segments`, and `source`.
- Temp WAV/transcript files scoped to temp directories and removed on normal success/failure.
- Phase 0 fixture generation and mock-backed E2E script.

## Commands Run

```bash
cargo --version
rustc --version
rustfmt --version
cargo clippy --version
command -v ffmpeg; command -v ffprobe; command -v whisper-cli; command -v whisper.cpp; command -v main; command -v whisper
scripts/dev/generate-short-fixture.sh
ffprobe -v error -show_entries stream=codec_name,sample_rate,channels,duration -of json tests/fixtures/audio/short.wav
bash -n scripts/dev/generate-short-fixture.sh
bash -n scripts/e2e/phase-0-file-transcribe.sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
COMLINK_WHISPER_MODEL="$HOME/Library/Caches/comlink/models/ggml-tiny.en.bin" cargo run -- doctor
scripts/e2e/phase-0-file-transcribe.sh
COMLINK_WHISPER_MODEL="$HOME/Library/Caches/comlink/models/ggml-tiny.en.bin" cargo run -- transcribe tests/fixtures/audio/short.wav --format text
COMLINK_WHISPER_MODEL="$HOME/Library/Caches/comlink/models/ggml-tiny.en.bin" cargo run -- transcribe tests/fixtures/audio/short.wav --format json
```

Results:

- Rust installed through Homebrew.
- `rustc 1.96.0`, `cargo 1.96.0`, `rustfmt 1.9.0`, and `clippy 0.1.96` are on PATH.
- `ffmpeg` found at `/opt/homebrew/bin/ffmpeg`.
- `ffprobe` found at `/opt/homebrew/bin/ffprobe`.
- `whisper-cli` found at `/opt/homebrew/bin/whisper-cli`.
- Real model installed at `/Users/lukefreeman/Library/Caches/comlink/models/ggml-tiny.en.bin`.
- Model SHA-256: `921e4cf8686fdd993dcd081a5da5b6c365bfde1162e72b08d75ac75289920b1f`.
- `tests/fixtures/audio/short.wav` generated successfully.
- Fixture metadata: `pcm_s16le`, 16 kHz, mono, 2.08 seconds.
- Phase 0 shell scripts passed `bash -n`.
- `cargo fmt --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test --all` passed: 3 tests.
- `cargo run -- doctor` passed with the real model path.
- `scripts/e2e/phase-0-file-transcribe.sh` passed.
- Real text transcription of `tests/fixtures/audio/short.wav` returned `Comlink Phase 0 fixture.`
- Real JSON transcription artifact written to `docs/validation/artifacts/phase-0/short-real.json`.

## Agent E2E

Ran `scripts/e2e/phase-0-file-transcribe.sh`. It:

- Uses an isolated temp directory.
- Generates `tests/fixtures/audio/short.wav` if missing.
- Runs text and JSON transcription against a mock `whisper.cpp` adapter.
- Validates JSON contains `text`, `engine`, `model`, `duration_ms`, `segments`, and `source`.
- Checks missing-file failure.
- Checks invalid `COMLINK_WHISPER_CPP` is reported by `doctor`.
- Writes artifacts under `docs/validation/artifacts/phase-0/`.

## Known Gaps

- Phase 0 creates one whole-file segment from the transcript; richer timestamp parsing is deferred.
- `COMLINK_WHISPER_MODEL` or `--model` is still required unless a default model config is added in a later phase.
- `doctor` exits non-zero when required ASR pieces are missing, which remains expected for setup diagnostics.

## Manual Test Script

1. Set:

```bash
export COMLINK_WHISPER_MODEL="$HOME/Library/Caches/comlink/models/ggml-tiny.en.bin"
```

2. Run:

```bash
scripts/dev/generate-short-fixture.sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo run -- doctor
cargo run -- transcribe tests/fixtures/audio/short.wav --format text
cargo run -- transcribe tests/fixtures/audio/short.wav --format json
```

Pass criteria:

- `doctor` reports FFmpeg, whisper.cpp, and the model as `[ok]`.
- Text mode prints only transcript text to stdout.
- JSON mode emits valid JSON with the Phase 0 contract fields.
- No temp files are left in the repo after normal operation.
