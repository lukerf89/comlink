# Phase 9 Validation

Date: 2026-07-08

## Scope

Implemented Online Meeting Capture v0 only:

- Added `meet start --source mic-only|system-only|mic-plus-system`; default is
  `mic-only` so Phase 7 usage remains unchanged.
- Added BlackHole-backed system-audio capture planning through the existing
  system-audio probe and FFmpeg/AVFoundation segmented capture path.
- Captured separate `user_mic` and `system_audio` chunk streams when both are
  available; used `mixed` when mic-plus-system is explicitly configured to the
  same input rather than claiming speaker identity.
- Added additive meeting source metadata to session state, JSON export, JSONL,
  Markdown, and segment records.
- Extended doctor and privacy audit with system-audio permission/routing
  diagnostics and retention posture.
- Preserved local-first behavior: no network, no cloud transcription, no bot
  attendance, no automatic summaries, no speaker diarization.

## Commands Run

```bash
cargo fmt
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo run -- doctor
cargo run -- doctor --format json
cargo run -- meet start --help
cargo run -- privacy audit --format json
scripts/e2e/phase-9-online-meeting-capture.sh
```

Results:

- `cargo fmt` passed.
- `cargo fmt --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test --all` passed: 65 library tests, 0 binary tests, 5
  integration tests, 0 doc tests.
- `cargo run -- doctor` passed. Real local doctor reported all required
  checks healthy, `system-audio` non-required
  `missing-dependency`, and `system_audio_permissions:
  microphone=manual-check; routing=setup-required`.
- `cargo run -- doctor --format json` passed. Real local JSON retained
  `schema_version: "comlink.doctor.v1"`, `ok: true`, and included additive
  `system_audio.permissions`.
- `cargo run -- meet start --help` passed and showed
  `--source <SOURCE>` with possible values `mic-only`, `system-only`, and
  `mic-plus-system`.
- `cargo run -- privacy audit --format json` passed and reported
  `system_audio.raw_audio_retained=false` with local BlackHole status
  `missing-dependency`.
- `scripts/e2e/phase-9-online-meeting-capture.sh` passed and wrote artifacts
  under `docs/validation/artifacts/phase-9/`.

## Tests Added

Unit tests:

- `system_audio::tests::capture_plan_uses_blackhole_as_avfoundation_audio_input`
  covers probe-to-capture planning for BlackHole.

Integration tests:

- `meet_mic_plus_system_exports_source_labels_for_teams_shape` in
  `tests/meet_lifecycle.rs` starts a mocked mic-plus-system meeting, verifies
  separate `user_mic` and `system_audio` recorders, stops/export the session,
  checks source labels in JSON/JSONL, and verifies raw chunks are deleted when
  audio retention is disabled.
- `meet_start_cleans_up_mic_recorder_when_system_recorder_fails` verifies a
  partial mic-plus-system startup failure stops the already-started mic recorder
  and clears the active-session marker.

E2E:

- `scripts/e2e/phase-9-online-meeting-capture.sh`

The E2E script:

- Builds `target/debug/comlink`.
- Uses isolated `COMLINK_HOME` and `COMLINK_DATA_DIR`.
- Uses mocked FFmpeg AVFoundation device listing, mocked FFprobe, mocked
  whisper.cpp, and a mocked model.
- Drives a simulated Teams-shaped `mic-plus-system` session through
  `meet start`, `meet stop`, and `meet export`.
- Verifies JSONL records are valid and segment start timestamps are monotonic.
- Verifies `user_mic`, `system_audio`, and `mixed` source-label behavior.
- Verifies JSON and Markdown exports include session metadata, source metadata,
  retention policy, and segment labels.
- Verifies raw audio chunks are deleted by default and retained only when
  `COMLINK_RETAIN_AUDIO=true`.
- Verifies doctor/privacy audit system-audio fields.
- Verifies missing BlackHole and invalid system-device preflights fail with
  actionable diagnostics.

Artifacts retained:

- `docs/validation/artifacts/phase-9/doctor.json`
- `docs/validation/artifacts/phase-9/privacy-audit.json`
- `docs/validation/artifacts/phase-9/mic-plus-system-*.json`
- `docs/validation/artifacts/phase-9/mic-plus-system-*.md`
- `docs/validation/artifacts/phase-9/mic-plus-system-segments.jsonl`
- `docs/validation/artifacts/phase-9/retained-audio-*.json`
- `docs/validation/artifacts/phase-9/retained-audio-*.md`
- `docs/validation/artifacts/phase-9/retained-audio-segments.jsonl`
- `docs/validation/artifacts/phase-9/mixed-*.json`
- `docs/validation/artifacts/phase-9/mixed-*.md`
- `docs/validation/artifacts/phase-9/mixed-segments.jsonl`
- Failure diagnostic artifacts for missing BlackHole and a bad system device.

## Source Labels

- `user_mic`: microphone stream.
- `system_audio`: BlackHole stream receiving routed Teams/Zoom/system output.
- `mixed`: one combined stream where Comlink cannot reliably separate mic and
  system audio. Phase 9 does not infer diarization or participant identity.

## Privacy and Retention

- Transcripts remain retention-gated by the existing transcript toggle.
- Raw meeting audio chunks are deleted at stop by default because
  `retention.audio` defaults to `false`.
- When `COMLINK_RETAIN_AUDIO=true`, chunk paths are retained and exported.
- Device names are redacted from export metadata when metadata retention is
  disabled.
- Normal diagnostics do not print transcript text.
- `privacy audit` now reports system-audio strategy, status, device visibility,
  permission/routing posture, and whether raw audio is retained.

## Known Gaps

- No live Microsoft Teams or Zoom call was captured by the agent because
  BlackHole routing and meeting hardware are not available in this environment.
- Correct routing of remote participant audio into BlackHole cannot be proven
  without live hardware and a controlled call.
- macOS Microphone permission and BlackHole routing are diagnosed with
  actionable guidance, but TCC permission prompts are still manual OS behavior.
- Zoom live validation is deferred until after the Teams-first manual gate.
- Speaker diarization, participant identity, automatic summaries, bot meeting
  attendance, and cloud transcription remain out of scope.

## Manual Test Checklist

1. Install BlackHole 2ch on the test Mac.
2. In Audio MIDI Setup, create a Multi-Output Device or Aggregate Device that
   includes the normal speakers/headphones and BlackHole 2ch.
3. Route Microsoft Teams output to that device, or set macOS output to that
   device before joining Teams.
4. Run `cargo run -- doctor --format json` and verify:
   `system_audio.available=true`, `system_audio.status="ok"`, and
   `system_audio.dependency.device_name` is `BlackHole 2ch` or the approved
   BlackHole input.
5. Run `cargo run -- privacy audit --format json` and verify
   `system_audio.raw_audio_retained=false` unless audio retention was
   explicitly enabled.
6. Join a short controlled Microsoft Teams call with one local speaker and one
   remote speaker.
7. Start capture:
   `cargo run -- meet start --source mic-plus-system --chunk-seconds 30 --no-llm --format json`.
8. Speak locally and have the remote participant speak clearly for at least one
   chunk.
9. Stop capture with the reported session id:
   `cargo run -- meet stop <session-id> --format json`.
10. Export JSON and Markdown:
    `cargo run -- meet export <session-id> --format json` and
    `cargo run -- meet export <session-id> --format md`.
11. Verify the transcript represents both local and remote participant audio.
12. Verify segment timestamps are useful and source labels include
    `user_mic` for local mic chunks and `system_audio` for routed Teams audio.
13. Verify no raw chunk audio remains under the session `chunks` directory when
    retention audio is disabled.
14. Repeat with `COMLINK_RETAIN_AUDIO=true` and verify chunk audio remains and
    exported segment `chunk_path` values are present.
15. Run `cargo run -- privacy audit --format json` again and confirm the
    retention state matches the setting used.
16. Record any Teams routing issues, missing remote audio, or timestamp/source
    label problems before approving Phase 9.
17. After Teams passes, repeat the same controlled-call flow with Zoom as a
    follow-up validation; do not start Phase 10 until the Phase 9 manual gate is
    approved.
