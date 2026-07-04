# Phase 2 Validation

Date: 2026-07-04

## Scope

Implemented the config, storage, model registry, and privacy core:

- OS-appropriate local paths, with `COMLINK_HOME` and `COMLINK_DATA_DIR` isolation overrides.
- `comlink config show --format text|json`.
- Config precedence from defaults, JSON config file, env vars, and CLI model override.
- SQLite history storage under the local data directory.
- Session and segment history tables.
- Retention controls for metadata, transcripts, and audio.
- `comlink history list`, `history show`, and `history prune --all`.
- `comlink models list` and `models select`.
- `comlink privacy audit`.
- `transcribe --save` and `record --save` to persist history when history is enabled.
- Saved JSON transcript output includes `history_session_id`.

Default retention posture:

- Metadata: retained.
- Transcripts: retained.
- Audio: not retained.

## Commands Run

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
COMLINK_HOME=/private/tmp/comlink-phase2-smoke.CPKCcz cargo run --quiet -- config show --format json
COMLINK_HOME=/private/tmp/comlink-phase2-smoke.CPKCcz cargo run --quiet -- history list --format json
COMLINK_HOME=/private/tmp/comlink-phase2-smoke.CPKCcz cargo run --quiet -- privacy audit --format json
scripts/e2e/phase-2-config-storage-privacy.sh
```

Results:

- `cargo fmt --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test --all` passed: 14 tests.
- Isolated `config show --format json` passed and showed defaults plus `COMLINK_HOME` paths.
- Isolated `history list --format json` passed and returned `[]`.
- Isolated `privacy audit --format json` passed and showed retention/model/LLM status.
- `scripts/e2e/phase-2-config-storage-privacy.sh` passed.

The first `cargo test --all` needed network access to fetch the new SQLite dependency and then passed.

## Agent E2E

Ran `scripts/e2e/phase-2-config-storage-privacy.sh`. It:

- Uses isolated `COMLINK_HOME`.
- Uses mock `ffmpeg`, `whisper.cpp`, and a mock local model.
- Verifies `config show --format json` exposes paths, defaults, retention, and source precedence.
- Registers and selects a local model with `models select`.
- Verifies `models list --format json`.
- Runs `transcribe --save --format json`.
- Verifies the saved transcript includes `history_session_id`.
- Verifies `history list --format json` includes the saved session.
- Verifies `history show --format json` includes retained transcript and segment text.
- Runs another saved transcription with `COMLINK_RETAIN_TRANSCRIPTS=false`.
- Verifies future history preserves metadata while omitting transcript and segment text.
- Verifies `privacy audit --format json` reports retention, ASR, and LLM posture.
- Runs `history prune --all --format json`.
- Verifies history is empty after pruning.
- Writes artifacts under `docs/validation/artifacts/phase-2/`.

## Known Gaps

- The config file format is JSON for Phase 2. TOML/YAML can be added later if there is a strong reason.
- Model download UX is not implemented; Phase 2 only registers and selects existing local model paths.
- Strict network sandboxing is not implemented.
- Meeting retention remains out of scope.
- Cloud endpoints remain out of scope; `privacy audit` reports LLM status as disabled.
- Audio retention is implemented but defaults off and was not exercised in the E2E script beyond verifying the default posture.

## Manual Test Script

1. Inspect resolved paths and defaults:

```bash
cargo run -- config show --format json
cargo run -- privacy audit --format json
```

2. Register a local model:

```bash
cargo run -- models select tiny --path "$HOME/Library/Caches/comlink/models/ggml-tiny.en.bin"
cargo run -- models list --format json
```

3. Save a file transcript:

```bash
cargo run -- transcribe tests/fixtures/audio/short.wav --mode memo --format json --save
cargo run -- history list --format json
```

4. Use the returned `history_session_id`:

```bash
cargo run -- history show <history_session_id> --format json
```

5. Verify transcript retention can be disabled for future saves:

```bash
COMLINK_RETAIN_TRANSCRIPTS=false cargo run -- transcribe tests/fixtures/audio/short.wav --mode memo --format json --save
cargo run -- history show <new_history_session_id> --format json
```

Pass criteria:

- Config and data paths are understandable.
- History rows appear only when `--save` is used and history is enabled.
- Transcript text is omitted from newly saved sessions when transcript retention is off.
- Metadata remains available when only transcript retention is off.
- `privacy audit` clearly reports retention, selected model status, local ASR posture, and disabled LLM posture.
- `history prune --all` removes saved records.
