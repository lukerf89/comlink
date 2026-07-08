# Phase 8 Validation

Date: 2026-07-08

## Scope

Implemented the Zoom/Teams System Audio Spike only:

- Researched macOS system-audio capture paths and documented the decision in
  `docs/decisions/system-audio-macos.md`.
- Chose BlackHole as the recommended documented local dependency for the next
  phase, pending human approval.
- Added a `system_audio` diagnostic object to `comlink.doctor.v1` additively.
- Added a non-required `system-audio` doctor check so existing doctor health
  semantics do not depend on unshipped system-audio capture.
- Added a trait-backed system-audio capability probe with fake states for
  dependency-present, dependency-missing, unsupported-OS, and probe-error cases.
- Added source metadata prototype labels for `user_mic`, `system_audio`, and
  `mixed`.

Out-of-scope items were not implemented: production online-meeting capture,
live Zoom/Teams capture wiring, diarization, speaker identification, and a
`meet` system-audio capture command.

## Commands Run

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo run -- doctor
cargo run -- doctor --format json
scripts/e2e/phase-8-system-audio-spike.sh
```

Results:

- `cargo fmt --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test --all` passed: 61 library tests, 0 binary tests, 3 integration
  tests, 0 doc tests.
- `cargo run -- doctor` passed. Real local doctor reported required checks as
  healthy and reported `system-audio` as non-required missing dependency because
  BlackHole was not visible.
- `cargo run -- doctor --format json` passed. Real local JSON retained
  `schema_version: "comlink.doctor.v1"`, `ok: true`, and included additive
  `system_audio.status: "missing-dependency"`.
- `scripts/e2e/phase-8-system-audio-spike.sh` passed and wrote artifacts under
  `docs/validation/artifacts/phase-8/`.

Key real doctor JSON fields observed:

```json
{
  "schema_version": "comlink.doctor.v1",
  "ok": true,
  "system_audio": {
    "available": false,
    "status": "missing-dependency",
    "strategy": "blackhole-virtual-audio-device"
  }
}
```

## Tests Added

Unit tests:

- `system_audio::tests::reports_available_when_blackhole_input_is_present`
- `system_audio::tests::reports_actionable_missing_dependency_on_macos`
- `system_audio::tests::reports_wrong_os_without_requiring_host_audio`
- `system_audio::tests::parses_avfoundation_audio_devices`
- `system_audio::tests::parses_macos_versions_with_missing_patch`
- Extended `doctor::tests::required_missing_dependencies_make_report_unhealthy`
  to assert the additive non-required `system-audio` check and
  `system_audio` source labels.

E2E:

- `scripts/e2e/phase-8-system-audio-spike.sh`

The E2E script:

- Builds `target/debug/comlink`.
- Uses isolated `COMLINK_HOME` and `COMLINK_DATA_DIR`.
- Uses mocked FFmpeg, FFprobe, whisper.cpp, clipboard commands, and model path.
- Exercises the real adapter parse path by having mocked FFmpeg list
  AVFoundation audio devices with and without BlackHole.
- Exercises fake injected states for unsupported OS and probe failure.
- Verifies valid doctor JSON, additive `system_audio`, non-required
  `system-audio` check, actionable remediation, chosen strategy, dependency
  name, and source metadata labels.
- Fails on unexpected stderr.

Artifacts retained:

- `docs/validation/artifacts/phase-8/missing-doctor.json`
- `docs/validation/artifacts/phase-8/missing-doctor.err`
- `docs/validation/artifacts/phase-8/available-doctor.json`
- `docs/validation/artifacts/phase-8/available-doctor.err`
- `docs/validation/artifacts/phase-8/fake-wrong-os-doctor.json`
- `docs/validation/artifacts/phase-8/fake-wrong-os-doctor.err`
- `docs/validation/artifacts/phase-8/fake-probe-error-doctor.json`
- `docs/validation/artifacts/phase-8/fake-probe-error-doctor.err`

## Core Decision

Recommended dependency: BlackHole virtual audio device, installed and configured
locally by the user.

Reason: it is the lowest-risk local-first path for a CLI v0 because it appears
as a normal Core Audio/AVFoundation input and can be diagnosed without building
production capture. Native Core Audio taps and ScreenCaptureKit remain future
options, but both need a stronger macOS permission/app identity story than this
phase should ship.

Human gate decisions (2026-07-08):

1. Requiring a local virtual-audio dependency **is acceptable** for Phase 9.
2. **BlackHole 2ch is the documented default** (detection stays permissive for
   16ch); rationale in the decision record.
3. **Microsoft Teams is validated first** on live hardware; Zoom follows.

Still open, deferred into Phase 9 (non-blocking):

4. Should Comlink eventually invest in a signed macOS helper/app for native Core
   Audio taps?
5. What consent language should appear before online meeting capture?

## Known Gaps

- No production system-audio capture was implemented.
- No live Zoom or Teams call capture was run by the agent.
- No ScreenCaptureKit or Core Audio tap prototype binary was added.
- The doctor diagnostic detects the chosen dependency/capability, but it does
  not validate that the user's Multi-Output or Aggregate Device is routed
  correctly.
- The real local machine used for validation did not have BlackHole installed,
  so the real doctor result is `missing-dependency`. The available state is
  covered by unit tests and E2E fake/mocked adapter state.
- Live-capture proofs are explicitly deferred to manual/hardware testing after
  the human reviews and approves the decision record.

## Manual Test Checklist

Decisions 1–4 below were resolved at the gate (BlackHole acceptable; 2ch default;
Teams first). The remaining live-hardware steps stay as the Phase 9 validation
checklist.

1. Review `docs/decisions/system-audio-macos.md`. (done)
2. Requiring a local BlackHole dependency is acceptable for Phase 9. (confirmed)
3. BlackHole 2ch is the documented default. (confirmed)
4. Microsoft Teams is validated first for Phase 9 live validation. (confirmed)
5. Run `cargo run -- doctor --format json` before installing BlackHole and
   confirm `system_audio.status` is `missing-dependency` with actionable
   remediation.
6. Install BlackHole on the test Mac if approved.
7. Configure a Multi-Output Device or Aggregate Device that includes the
   speakers/headphones and BlackHole.
8. Rerun `cargo run -- doctor --format json` and confirm
   `system_audio.available=true`, `system_audio.status=ok`, and
   `system_audio.dependency.device_name` reports the BlackHole device.
9. Join a short controlled Zoom or Teams test call manually.
10. Confirm the meeting app can route output to the configured device while the
    human can still hear remote participants.
11. Do not expect Comlink to capture the live call in Phase 8; that proof is
    deferred to Phase 9 after approval.
12. Approve or reject moving to Phase 9.
