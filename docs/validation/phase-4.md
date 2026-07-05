# Phase 4 Validation

Date: 2026-07-04

## Scope

Implemented the stable agent-friendly output contract:

- Added transcript schema versioning with `comlink.session.v1`.
- Added stable `session_id` to live transcript output.
- Aligned `history_session_id` with `session_id` when a transcript is saved.
- Added explicit context metadata with `policy=none` and empty context items.
- Added `--format text|json|jsonl|md` for `transcribe`, `record`, and `history show`.
- Added JSONL records for session metadata, segments, and final transcript output.
- Added Markdown transcript export for live file transcription and saved sessions.
- Kept structured transcript payloads on stdout and diagnostics on stderr.
- Documented exit codes and agent parsing examples in `docs/output-contract.md`.

## Commands Run

```bash
cargo fmt
cargo fmt --check
cargo test --all
cargo clippy --all-targets -- -D warnings
scripts/e2e/phase-4-output-contract.sh
```

Results:

- `cargo fmt` completed.
- `cargo fmt --check` passed.
- `cargo test --all` passed: 26 tests.
- `cargo clippy --all-targets -- -D warnings` passed.
- `scripts/e2e/phase-4-output-contract.sh` passed.

## Agent E2E

Ran `scripts/e2e/phase-4-output-contract.sh`. It:

- Uses isolated `COMLINK_HOME`.
- Uses mocked `ffmpeg`, `whisper.cpp`, and local model files.
- Runs file transcription with `--format json`, `--format jsonl`, and `--format md`.
- Verifies JSON has schema version, session ID, raw text, final text, mode, engine, model, duration, segments, source metadata, and context policy.
- Verifies saved output aligns `history_session_id` and `session_id`.
- Parses JSONL and verifies the `session`, `segment`, and final `transcript` records.
- Verifies no diagnostic text appears on stdout for JSON and JSONL modes.
- Verifies saved-session Markdown and JSONL exports through `history show`.
- Asserts exit codes for missing file, missing model, and no speech.
- Writes artifacts under `docs/validation/artifacts/phase-4/`.

## Known Gaps

- JSONL is streaming-like but still produced after local transcription completes; there is no live ASR streaming transport yet.
- Context metadata is a placeholder until later phases add app, meeting, or profile context.
- Exit code coverage for clipboard delivery failure is covered by the enum contract, not the Phase 4 E2E script, because record-and-copy requires interactive capture.
- Meeting-specific source labels remain out of scope for this phase.

## Manual Test Script

1. Inspect JSON output:

```bash
cargo run -- transcribe tests/fixtures/audio/short.wav --format json
```

Pass criteria:

- Output is valid JSON.
- `schema_version` is `comlink.session.v1`.
- `session_id`, `raw_text`, `final_text`, `segments`, `source`, and `context.policy` are present.
- No diagnostic text appears in stdout.

2. Inspect JSONL output:

```bash
cargo run -- transcribe tests/fixtures/audio/short.wav --format jsonl
```

Pass criteria:

- Every line is valid JSON.
- The final line has `record_type=transcript`.
- The final line includes the same required contract fields as JSON.

3. Inspect Markdown output:

```bash
cargo run -- transcribe tests/fixtures/audio/short.wav --format md
```

Pass criteria:

- Markdown includes schema, session, source, context policy, final text, raw text, and segments.

4. Save and export a session:

```bash
cargo run -- transcribe tests/fixtures/audio/short.wav --format json --save
cargo run -- history show "$SESSION_ID" --format md
```

Pass criteria:

- The saved transcript can be retrieved.
- Markdown output is useful for review and external agent handoff.
