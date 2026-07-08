# Phase 7 Validation

Date: 2026-07-06

## Scope

Implemented in-person meeting transcript v0:

- Added `comlink meet start`, `comlink meet stop`, and `comlink meet export`.
- Added consent reminder on session start.
- Added visible session status, elapsed time, recorder PID, artifact paths, and stop-time duration.
- Added long-form microphone capture through the FFmpeg segment muxer, writing bounded WAV chunks instead of one long in-memory recording.
- Added filesystem-backed meeting session state under the configured data directory.
- Added saved JSONL segment records plus final JSON and Markdown transcript exports.
- Added retention policy metadata to JSON, JSONL, and Markdown exports.
- Added ASR-segment stitching that preserves sub-chunk/VAD timing when the ASR engine supplies it and falls back to chunk boundaries otherwise.
- Added conservative repeated-phrase loop collapse for final transcript and segment display while preserving the original ASR text in `raw_text`.

Out-of-scope items were not implemented: Zoom/Teams system audio, diarization, live transcript UI, meeting notes, and summaries.

## Commands Run

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo run -- doctor
cargo run -- meet --help
cargo run -- meet export --help
scripts/e2e/phase-7-in-person-meeting-transcript.sh
```

Results:

- `cargo fmt --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test --all` passed: 50 library tests, 0 binary tests, 0 doc tests.
- `cargo run -- doctor` passed with real local FFmpeg, FFprobe, whisper-cli, model path, clipboard, data path, and microphone info checks.
- `cargo run -- meet --help` passed and lists `start`, `stop`, and `export`.
- `cargo run -- meet export --help` passed and lists `--format json|md`.
- Phase 7 E2E passed and wrote artifacts under `docs/validation/artifacts/phase-7/`.

## Agent E2E

Ran `scripts/e2e/phase-7-in-person-meeting-transcript.sh`. It:

- Builds `target/debug/comlink` from source.
- Uses isolated `COMLINK_HOME` and `COMLINK_DATA_DIR`.
- Uses mocked FFmpeg, FFprobe, whisper.cpp, and model paths.
- Runs a short simulated 6-chunk meeting with 30-second chunks, representing 3 minutes.
- Runs a longer synthetic 72-chunk meeting with 30-second chunks, representing 36 minutes.
- Verifies start status is `recording` and stop status is `stopped`.
- Verifies ordered segment stitching and monotonic timestamps.
- Verifies JSONL records are valid JSON and include one session record plus expected segment records.
- Verifies JSON and Markdown exports include session metadata, retention policy, inactivity auto-stop metadata, and transcript segments.
- Checks stderr for transcript leakage.

Artifacts retained:

- `docs/validation/artifacts/phase-7/short-start.json`
- `docs/validation/artifacts/phase-7/short-stop.json`
- `docs/validation/artifacts/phase-7/short-export.json`
- `docs/validation/artifacts/phase-7/short-export.md`
- `docs/validation/artifacts/phase-7/short-segments.jsonl`
- `docs/validation/artifacts/phase-7/long-start.json`
- `docs/validation/artifacts/phase-7/long-stop.json`
- `docs/validation/artifacts/phase-7/long-export.json`
- `docs/validation/artifacts/phase-7/long-export.md`
- `docs/validation/artifacts/phase-7/long-segments.jsonl`

## Known Gaps

- Inactivity auto-stop is not shipped in Phase 7. The current FFmpeg segment capture adapter does not expose a reliable realtime silence signal, so shipping auto-stop here would be flaky. Exports explicitly record `inactivity_auto_stop.enabled=false`.
- VAD-aware stitching is implemented where ASR segment timing is available. The current whisper.cpp text adapter still returns chunk-level segments, so real exports may report `segmenting.strategy=chunk-boundaries` until the ASR adapter parses timed segment output.
- `ggml-tiny.en.bin` is acceptable for smoke tests but can produce garbled, repetitive transcripts in noisy rooms. For the manual gate, prefer `ggml-base.en.bin`, `ggml-small.en.bin`, or larger if local performance allows it.
- Real microphone permission, real room acoustics, and 30-60 minute recorder behavior still require the manual gate below.

## Manual Test Checklist

1. Run `cargo run -- doctor` and confirm FFmpeg, whisper-cli, model path, data path, and microphone info are acceptable.
   Prefer a non-tiny Whisper model for real meeting audio:

   ```bash
   cargo run -- models select small --path /path/to/ggml-small.en.bin
   ```

2. Start with a 10-15 minute real meeting or monologue:

   ```bash
   cargo run -- meet start --format json --chunk-seconds 300 --no-llm
   ```

3. Confirm the consent reminder is visible and the start output includes `status=recording`, `elapsed_ms`, `recorder_pid`, `session_id`, and artifact paths.
4. Copy the `session_id` from start output and stop safely:

   ```bash
   cargo run -- meet stop <session-id> --format json
   ```

5. Confirm stop output includes `status=stopped`, elapsed time, duration, chunk count, segment count, retention policy, JSONL path, JSON path, and Markdown path.
6. Export and review Markdown:

   ```bash
   cargo run -- meet export <session-id> --format md > /tmp/comlink-meeting.md
   ```

7. Export and review JSON:

   ```bash
   cargo run -- meet export <session-id> --format json > /tmp/comlink-meeting.json
   ```

8. Inspect the saved `segments.jsonl` path from stop output and confirm segment timestamps are ordered.
9. Confirm normal stderr did not contain transcript text.
10. Confirm retention behavior matches local config, especially whether audio chunks are retained or deleted.
11. If the 10-15 minute run passes, repeat with a 30-60 minute in-person meeting.
12. During the longer run, verify the Mac remains responsive and disk usage grows by chunks rather than one large in-memory recording.
13. Stop the longer run and confirm final Markdown, JSON, and JSONL artifacts are readable.
14. Confirm no Zoom/Teams/system audio, diarization, live transcript UI, notes, or summaries were expected from this phase.
15. Approve or reject the meeting lifecycle before any Phase 8 system-audio work begins.
