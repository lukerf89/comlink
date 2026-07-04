# Phase 1 Validation

Date: 2026-07-04

## Scope

Implemented the basic voice memo loop:

- `comlink record` with immediate terminal recording feedback and Enter-to-stop.
- FFmpeg AVFoundation microphone capture directly to the 16 kHz mono WAV path.
- Minimum-duration/no-speech guard returning exit code `4` with a recording-specific message.
- `--format text|json` for `record`.
- `--mode raw|memo` for `record` and `transcribe`, with deterministic memo cleanup.
- JSON output now preserves `raw_text` and `final_text`, while `text` remains the final text for compatibility.
- `--copy` for `record` through a macOS `pbcopy` adapter, overridable with `COMLINK_PBCOPY` for tests.
- A single whisper.cpp CPU retry (`-ng`) for GPU/Metal-related failures.
- Mock-backed Phase 1 E2E coverage in `scripts/e2e/phase-1-record-memo.sh`.

Audio files remain temporary in Phase 1. Transcript/history retention is left for the Phase 2 config/storage work so the manual privacy gate stays explicit.

## Commands Run

```bash
bash -n scripts/e2e/phase-1-record-memo.sh
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo run --quiet -- record --help
COMLINK_WHISPER_MODEL="$HOME/Library/Caches/comlink/models/ggml-tiny.en.bin" cargo run -- doctor
COMLINK_WHISPER_MODEL="$HOME/Library/Caches/comlink/models/ggml-tiny.en.bin" cargo run -- transcribe tests/fixtures/audio/short.wav --mode memo --format json
scripts/e2e/phase-1-record-memo.sh
```

Results:

- `bash -n` passed.
- `cargo fmt --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test --all` passed: 8 tests.
- `cargo run --quiet -- record --help` passed and documents `--format`, `--mode`, `--copy`, `--model`, `--min-duration-ms`, and `--device`.
- `cargo run -- doctor` passed with FFmpeg, FFprobe, whisper.cpp, and the local tiny model.
- Real file transcription with `--mode memo --format json` passed and emitted `raw_text`, `final_text`, `mode`, `copied`, `segments`, `source`, and `processing_steps`.
- Real microphone recording with `record --mode memo --format json` passed manually: it captured `8992 ms` of microphone audio, emitted the Phase 1 JSON contract, and reported `388 ms` stop-to-final latency.
- Real microphone recording with `record --mode memo --copy --format text` passed manually: it copied final text to the clipboard and reported `431 ms` stop-to-final latency.
- The first real whisper run exposed a Metal allocation failure; the adapter now retries once with `-ng` for GPU/Metal-related failures, and the real validation command passes.
- `scripts/e2e/phase-1-record-memo.sh` passed.

## Agent E2E

Ran `scripts/e2e/phase-1-record-memo.sh`. It:

- Uses isolated mock `ffmpeg`, `whisper.cpp`, `pbcopy`, and model paths.
- Validates `transcribe --mode memo --format json`.
- Simulates `record --copy --format json` with Enter-to-stop.
- Verifies JSON contains raw text, final text, `mode: memo`, `copied: true`, microphone source metadata, segments, and processing steps.
- Verifies the clipboard mock receives the final text.
- Verifies terminal recording feedback, copy success feedback, and stop-to-final latency logging.
- Verifies `record` invokes FFmpeg once on the hot path.
- Simulates too-short/no-speech audio, including missing recorder output, and asserts exit code `4`.
- Simulates early recorder failure and verifies ffmpeg stderr is surfaced instead of a broken-pipe message.
- Writes artifacts under `docs/validation/artifacts/phase-1/`.

Observed mocked stop-to-final latency: `47 ms`.

## Known Gaps

- Real microphone capture was validated manually by the user on macOS. The agent still cannot run that gate unattended because microphone permission and spoken input require user interaction.
- Phase 1 uses FFmpeg AVFoundation capture instead of a Rust `cpal` capture adapter. This keeps the implementation dependency-free and macOS-first, but device selection may need hardening.
- The live `ggml-tiny.en.bin` run mis-transcribed "Supabase" as "Superbase" and joined "memo test" as "MemoTest"; follow-up work should evaluate larger models and/or dictionary replacements for product names.
- The no-speech guard is duration/empty-transcript based. True VAD remains deferred.
- No transcript history is persisted yet; Phase 2 owns config, storage, history, and retention toggles.
- `record` defaults to AVFoundation device `:0`; use `--device` or `COMLINK_RECORD_DEVICE` if the default mic index differs.

## Manual Test Script

1. Set the model path if it is not already exported:

```bash
export COMLINK_WHISPER_MODEL="$HOME/Library/Caches/comlink/models/ggml-tiny.en.bin"
```

2. Confirm setup:

```bash
cargo run -- doctor
cargo run -- record --help
```

3. Record the manual gate phrase:

```bash
cargo run -- record --mode memo --format json
```

Say: "Comlink memo test for July fourth. Supabase API should stay readable."

Press Enter to stop.

4. Verify stdout JSON includes `raw_text`, `final_text`, `mode: "memo"`, and `source.path: "microphone"`.

5. Test clipboard delivery:

```bash
cargo run -- record --mode memo --copy --format text
```

Say the same phrase, press Enter, then paste into a text editor.

Pass criteria:

- Recording feedback appears immediately.
- Pressing Enter stops recording.
- The transcript appears in stdout.
- `--copy` places the final text on the clipboard.
- Microphone permission behavior is understandable.
- No audio files are retained in the repo or user-visible project directories.
