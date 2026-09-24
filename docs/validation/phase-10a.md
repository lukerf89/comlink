# Phase 10a Validation: Meeting Service Core, `meet status`, Detached Finalize

Date: 2026-09-23

## Scope

Phase 10a covers only the following. It adds no MCP, JSON-RPC or HTTP code, and does not change capture, ASR or text-processing behaviour.

- **Service extraction.** The meeting lifecycle now lives in `src/meet_service.rs`. It exposes `start`, `stop` (synchronous), `stop_detached`, `finalize`, `status`, `export` and `list`. Each returns a typed result (`MeetStartStatus`, `MeetStopStatus`, `MeetDetachedStatus`, `MeetStatusReport`, `MeetSessionList`) and never writes to stdout or stderr. Warnings and the consent reminder are returned as data. The CLI `meet_*` functions are now thin wrappers that load config, call the service, and print. Their stdout output is byte-identical to main (97988d2), which the golden fixtures verify.
- **`comlink meet status [id] --format text|json`.** It reports:
  - recorder health per stream
  - finalizer health
  - chunk count
  - the latest completed-chunk audio level, with a near-silent warning
  - stale detection

  It exits 0 when there is no session (`status: "none"`).
- **`comlink meet stop --detach`.** It stops the recorders, marks the session `transcribing`, and launches `comlink meet finalize <id>` in its own process group (stderr goes to `<session_dir>/finalize.log`). It then clears the active session and returns.
- **`comlink meet finalize <id>`.** It runs the existing transcribe → segments → process → export path and commits `stopped`, or `failed` with an `error`. It is idempotent and recovers from crashes.
- **Additive schema.** `comlink.meeting.v1` gains the status values `transcribing` and `failed` and an optional `error`. `docs/output-contract.md` is updated. Existing fields and exit codes are unchanged.
- **Primitives.**
  - Atomic same-directory temp+rename writes for `session.json`, the active pointer, `transcript.json`, `transcript.md` and `segments.jsonl`.
  - A per-session lifecycle lock: an OS advisory `flock` on `<session_dir>/lifecycle.lock`, which the kernel releases when the holder dies.
  - A generalized `record::ProcessIdentity` (pid plus start time, and optionally a command needle).

## Decisions

- **`meet status` with no session** exits 0 with `status: "none"` and `session_id: null`. An unknown id exits 1.
- **Status resolution with no id** is: the active recording session, then the newest `transcribing` session, then `none`. Once a detached session reaches `stopped`, a bare `meet status` reports `none`, so poll by id (`meet status <id>`) to watch the whole transition. The id is printed by `stop --detach`.
- **`meet finalize` is a visible subcommand** documented as internal and safe to rerun. On a `stopped` session it reprints the stop payload from the validated export and does not run ASR again.
- **Detaching clears the active pointer**, so a new `meet start` is allowed while the earlier meeting is still transcribing.
- **Plain `meet stop` ordering is unchanged.** It marks the session stopped and clears the active pointer before ASR, so an ASR failure still leaves the session `stopped` with no exports. Only the detached path uses `transcribing` and `failed`. It now also takes the lifecycle lock, which is additive.
- **Device resolution** (`resolve_record_device` / `resolve_mic_device`) stays in `src/cli.rs` because `record` shares it and LF-80 edits that area. The service takes an already-resolved device.
- **There is no `meet list` CLI.** `meet_service::list` is a typed API that LF-162 (the MCP server) will use.
- **`rust-version = "1.89"`** is set in `Cargo.toml` because `std::fs::File::try_lock` needs it. No new dependencies.
- **`meet finalize --lock-wait-seconds`** (default 30) bounds how long finalize waits for the lock held by the detaching parent.

## Commands Run

```bash
cargo build --all-targets
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo run -- doctor
cargo run -- meet status --format json
scripts/e2e/phase-10a-meet-service.sh
```

Results (in the LF-161 worktree, macOS, rustc 1.96.1):

- `cargo fmt --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test --all` passed:

  | Suite | Tests |
  | --- | --- |
  | library unit tests | 101 |
  | `audio_levels` | 2 |
  | `meet_lifecycle` | 13 |
  | `meet_service` | 11 |
  | `meet_service_stdout` probe (`harness = false`) | reports 0 bytes on stdout and stderr |

- `cargo run -- doctor` exited 0.
- `cargo run -- meet status --format json` exited 0 with `status: "none"` and `session_id: null`.
- `scripts/e2e/phase-10a-meet-service.sh` passed. `stop --detach` took 0.384s of wall time while whisper was held on the barrier. Artifacts are under `docs/validation/artifacts/phase-10a/`.

## Tests Added

- **Golden fixtures** in `tests/fixtures/meet/`: `start.json`, `stop.json`, `export.json` and `export.md`. They were captured from 97988d2 before any production change, in their own commit on this branch. To regenerate them, run `COMLINK_UPDATE_GOLDENS=1 cargo test --test meet_lifecycle meet_cli_output_matches_golden_fixtures`. Only the temp root, session id, pids and times are normalized; key order and nulls are compared as raw text.
- **`tests/meet_lifecycle.rs`** (built binary plus mock runtime, with deterministic whisper `STARTED`/`BARRIER`/`COUNTER` hooks):
  - golden regression
  - status for the none, active and stale-recorder cases
  - `stop --detach` returns in under 1s and shows `transcribing` with a live finalizer, then polling reaches `stopped`; the exports equal the synchronous goldens
  - repeated finalize is identical and does not re-run whisper
  - forced whisper failure → `failed` with `error`, then a rerun → `stopped`; finalize surfaces exit 3
  - SIGKILL of the finalizer mid-ASR → status `stale: true`, the lock is released by the kernel, and a rerun → `stopped` with golden-equal exports
  - plain stop stays synchronous
- **`tests/meet_service.rs`** (in-process, runtime injected, no environment mutation):
  - `ThreadLauncher` lifecycle race, repeated 5 times: a terminal state is never overwritten and the finalizer is cleared
  - two-process lock contention (`MeetingLifecycleBusy`, exit 1, state untouched); the lock is released on drop
  - `FailingLauncher` → `failed`, active cleared, retry succeeds
  - finalize on a recording session is rejected
  - constructed crash checkpoints:
    - only `segments.jsonl` written → rewrite
    - JSON written but no Markdown, chunks present → rewrite
    - all artifacts written but state still `transcribing` → validated recovery with no ASR, Markdown and JSONL regenerated byte-identical
    - retention off with chunks gone → recovery from the export; with invalid JSON → `failed`
  - status audio level with real WAV fixtures: the live stream's newest (truncated) chunk is skipped, selection across streams is deterministic, a near-silent warning appears, and non-PCM gives `null`
  - list: mixed statuses, `started_at_ms` ties broken by session id, a corrupt session goes to `skipped`
  - status resolution order
- **`tests/meet_service_stdout.rs`**: `harness = false`. It re-runs itself as a child that drives start → status → stop_detached → finalize → export → list → sync stop, and asserts zero bytes on stdout and stderr.
- **Unit tests**:
  - status serde round-trip
  - legacy `session.json` loads, and the new fields are omitted when unset
  - transitions clear stale fields
  - an atomic write leaves no temp file
  - the lock is exclusive and released on drop
  - `validate_export_for_recovery` accept and reject cases
  - list and scan
  - `ProcessIdentity` start-time and command mismatch
  - a stale classification table
  - latest-chunk selection
  - new error variants exit 1
  - a no-print source guard

## Bugs Found While Validating

- The meeting test helper `wait_for_chunks` had a 2s budget, which could time out when the machine was loaded (8 tests failed at once in one of 4 repeated runs). The budget was raised to 10s, and the loop still exits as soon as the chunks appear.

## Known Gaps / Limitations

- The detached finalizer runs in its own process group, so it survives Ctrl-C and terminal hangup. It does not survive logout or reboot. After that, `meet status <id>` reports `stale: true`, and recovery is to run `comlink meet finalize <id>`.
- The lifecycle lock is an advisory `flock`, which is not reliable on network filesystems. The data dir is expected to be local.
- `stop --detach` still includes the existing 250ms recorder settle plus recorder shutdown before it returns.
- For a session that the synchronous `meet stop` finished, `meet finalize <id>` reports `chunks_processed: 0`, because the synchronous path does not record it (its `session.json` is intentionally unchanged).
- `finalize.log` and the `error` field contain only error display strings (paths and chunk indexes), never transcript text. A test asserts this for the whisper-failure case.

## Manual Test Instructions (pause gate)

Use your normal config with a real microphone and whisper model.

1. **Detached stop releases the terminal immediately.**
   ```bash
   cargo run -- meet start --format json          # note session_id
   # speak for ~2 minutes
   cargo run -- meet status                       # status: recording, recorder alive=true, chunk_count > 0, stale: false
   time cargo run -- meet stop --detach           # returns in about a second; status: transcribing
   cargo run -- meet status <session_id>          # status: transcribing, finalizer alive=true
   # repeat until done
   cargo run -- meet status <session_id>          # status: stopped
   cargo run -- meet export <session_id> --format md
   ```
   Confirm that the terminal came back immediately after `stop --detach`, and that status went from `transcribing` to `stopped`.
2. **Plain stop is unchanged.** Run `meet start`, speak briefly, then `meet stop`. Confirm it blocks until the transcript is written and prints the same fields as before.
3. **Export is unchanged.** Run `cargo run -- meet export --format json` and `--format md` for the stopped session and confirm the output looks as it did before this phase.
4. **Optional recovery check.** During a detached transcription, kill the finalizer with `kill -9 <finalizer_pid>`. Confirm `meet status <id>` shows `stale: true` with a `comlink meet finalize <id>` hint, then run that command and confirm it reaches `stopped`.
