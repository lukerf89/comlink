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
- A synchronous `meet stop` whose ASR fails still leaves the session `stopped` with no exports (historical ordering, pinned by `sync_stop_asr_failure_leaves_stopped_without_exports_and_finalize_recovers`). A bare `meet status` reports `none` for it. `meet finalize <id>` now transcribes it, and with `retention.audio=false` `privacy audit` lists its kept chunks until then.
- `finalize.log` and the `error` field contain only error display strings (paths and chunk indexes), never transcript text. A test asserts this for the whisper-failure case.

## Fix Round (LF-161 review findings)

### Changes

- **F1 (privacy): cleanup before `stopped`.** Finalize now runs export, then deletes unretained chunks, then commits `stopped`. If the chunk delete fails, the session is marked `failed` with `meeting audio chunk cleanup failed ...` (new `MeetingChunkCleanupFailed`, exit 1), and a rerun recovers from the export with no ASR, retries the delete, and stops. On a `stopped` session with a valid export, `meet finalize` deletes chunks left while `retention.audio=false` (for example, from the old ordering) and regenerates any missing Markdown/JSONL first. Nothing is deleted while the session's `retention.audio` is `true`. `privacy audit` has a new `meeting_audio` object (`clean`, `unretained_leftovers`) listing every non-recording session that has retention off and chunk WAVs on disk.
- **F2: finalizer reaping.** `ProcessFinalizeLauncher` hands the `Child` to a background thread that only calls `wait()`, so a long-lived caller never keeps zombies. The process group, the log redirection, and the survival of CLI exit, Ctrl-C and SIGHUP are unchanged: the thread never signals the child. `ProcessFinalizeLauncher::with_executable` lets a test launch a mock binary through the production path.
- **F3a.** `meet status` returns `MeetingSessionUnreadable` (exit 1) instead of `none` when the named session, the active session, or (for a bare status with no recording or transcribing session) any session has a `session.json` that exists but cannot be parsed. Missing sessions and directories without `session.json` behave as before.
- **F3b.** A failed save of the `failed` state no longer replaces the original error. The original error and its exit code are returned (a whisper failure still exits 3), and the save failure is appended to `finalize.log`. The launch-failure path does the same and also records the launch error in `finalize.log`.
- **F3c.** `MeetStatusReport.finalize_log` (JSON and text) is the log path when the file exists, else `null`. The stale-`transcribing` reason and the `failed` warning reference it only when it exists.
- **F3d.** Synchronous stop records `chunks_processed` in `session.json`. Its stdout is unchanged, and the goldens are byte-identical.

### Tests added

`tests/meet_service.rs`:
- `finalize_cleanup_failure_is_not_stopped_and_rerun_cleans_up`
- `stopped_session_with_leftover_chunks_is_cleaned_by_finalize`
- `retained_audio_is_never_deleted_by_finalize`
- `finalize_save_failure_keeps_the_original_error_and_logs_the_save_failure`
- `sync_stop_records_chunks_processed_for_finalize`
- `stop_detach_without_chunks_stops_and_launches_nothing`
- `stop_that_lost_the_race_does_not_transcribe_or_launch_again`
- `sync_stop_asr_failure_leaves_stopped_without_exports_and_finalize_recovers`
- `status_surfaces_unreadable_session_instead_of_none`
- `status_reports_finalize_log_and_references_it_in_warnings`
- `process_launcher_reaps_the_finalizer_so_no_zombie_remains` (real child via the production launcher, `ps` state polled with a 20s bound)

`tests/meet_lifecycle.rs`:
- `meet_detach_and_finalize_text_output_and_status_finalize_log`
- `privacy_audit_flags_leftover_meeting_audio_until_finalize_cleans_it`

### Mutation checks (fix reverted, test red, restored, green)

| Reverted | Failing test |
|---|---|
| F1: `stopped` saved before chunk delete, and the outer handler skips `failed` | `finalize_cleanup_failure_is_not_stopped_and_rerun_cleans_up` ("must not be stopped") |
| F1: stopped fast path without cleanup | `stopped_session_with_leftover_chunks_is_cleaned_by_finalize`, `privacy_audit_flags_leftover_meeting_audio_until_finalize_cleans_it` |
| F2: `Child` dropped without the reaper thread | `process_launcher_reaps_the_finalizer_so_no_zombie_remains` (`ps state "Z"`) |
| F3a, F3b, F3c, F3d, the under-lock re-check, the no-chunks detach guard, the sync-stop recovery | the matching test above |

### Commands and results

```bash
cargo fmt --check                              # ok
cargo clippy --all-targets -- -D warnings      # ok
cargo test --all                               # ok (meet_lifecycle 15, meet_service 23, stdout probe 0 bytes)
cargo run -- doctor                            # ok
cargo run -- meet status --format json         # status none, finalize_log null
bash scripts/e2e/phase-10a-meet-service.sh     # passed
cargo test --test meet_lifecycle --test meet_service   # 3 more runs, all green
```

### New known limitations

- The cleanup-failure test makes a directory read-only, so it cannot fail as root. The suite is not run as root.
- ~~`privacy audit` does not look inside session directories whose `session.json` is unreadable.~~ Fixed in the micro-round below.
- If both the `failed` save and the `finalize.log` append fail (for example, on a read-only session directory with no existing log), the save failure is lost. The original error is still returned.
- While a session is `failed` because of a cleanup failure, `meet export` refuses it until `meet finalize <id>` succeeds. The transcript files stay on disk.

## Micro-Round (LF-161 confirming-review findings)

### Changes

- **M1 (privacy): the `meeting_audio` scan is conservative and never fails the audit.** `meet_service::meeting_audio_audit` replaces `unretained_audio_leftovers` and reads each session directory on its own. It lists: (a) a directory whose `session.json` is missing or unreadable but which holds chunk WAVs, or whose chunks dir cannot be read (status `unknown`, named by `session_dir`, with a `reason`); (b) a readable session with `retention.audio=false` whose chunks dir cannot be read (`reason: unreadable chunks dir under <chunks_dir>: ...`); (c) a `recording` session with `retention.audio=false` and chunk WAVs whose recorder is not verified running (`session_recorder_is_verified_running`), with remedy `comlink meet stop <id>`. A failure to list the store itself goes into a new `scan_errors` list. `clean` is true only when both lists are empty. Previously, one unreadable chunks dir aborted the whole `privacy audit` with a bare `Permission denied`. New fields `session_dir`, `reason` and `scan_errors` are additive. See `docs/output-contract.md`.
- **M2 (privacy): synchronous `meet stop` now uses the finalize ordering.** After ASR, sync stop writes the exports, deletes unretained chunks, then saves the final `stopped`. If the delete fails, it saves `failed` with the cleanup error (and `chunks_processed`) and returns `MeetingChunkCleanupFailed` (exit 1). `meet finalize <id>` then recovers from the export with no ASR, deletes the chunks, and commits `stopped`. The preliminary `stopped` save before ASR is unchanged, so an ASR failure still leaves `stopped` without exports. Successful stop stdout is unchanged (goldens byte-identical).
- **M3a.** The `MeetingChunkCleanupFailed` reason is `could not delete <chunks_dir>: <io error>`.
- **M3b.** On the finalizer launch-failure path, a failure to clear the active-session pointer is appended to `finalize.log` (with the original launch error) instead of being dropped.
- **M3c (restriction).** On a `stopped` session, `meet finalize` falls through to ASR only when no JSON export exists at all. A JSON export that exists but fails validation (for example, a user-edited file) returns `MeetingExportUnavailable` (exit 1). The export, the chunks and the session state are left untouched, so the user's edit is never silently overwritten. ~~`transcribing` and `failed` sessions are unchanged: an invalid export there is still rewritten from the chunks, because it can only come from an interrupted finalize.~~ Corrected in the final round (N3): that claim stopped being true once M2 let a synchronous stop leave `failed` sessions with complete exports, and the JSON export is written atomically, so an interrupted finalize leaves it absent or complete, never invalid. The don't-overwrite rule now applies in every status.

### Tests added

`tests/meet_service.rs`:
- `meeting_audio_audit_names_a_corrupt_session_that_still_holds_chunks`
- `meeting_audio_audit_survives_an_unreadable_chunks_dir_and_names_it`
- `meeting_audio_audit_flags_stale_recordings_but_not_live_ones`
- `meeting_audio_audit_records_an_unreadable_store_root_instead_of_failing`
- `sync_stop_cleanup_failure_is_failed_and_finalize_finishes_it` (M2 and M3a)
- `launch_failure_logs_an_active_pointer_clear_failure` (M3b)
- `finalize_never_overwrites_an_invalid_export_on_a_stopped_session` (M3c)

`tests/meet_lifecycle.rs`:
- `privacy_audit_survives_unreadable_meeting_dirs_and_names_them` (real CLI: exit 0, `clean=false`, both paths named, in JSON and text)

### Mutation checks (fix reverted, test red, restored, green)

| Reverted | Failing test |
|---|---|
| M1: whole scan reverted to the old `list_sessions` logic | `privacy_audit_survives_unreadable_meeting_dirs_and_names_them`, `meeting_audio_audit_names_a_corrupt_session_that_still_holds_chunks`, `meeting_audio_audit_survives_an_unreadable_chunks_dir_and_names_it`, `meeting_audio_audit_flags_stale_recordings_but_not_live_ones`, `meeting_audio_audit_records_an_unreadable_store_root_instead_of_failing` |
| M1: unreadable `session.json` entries dropped | `meeting_audio_audit_names_a_corrupt_session_that_still_holds_chunks`, `privacy_audit_survives_unreadable_meeting_dirs_and_names_them` |
| M1: unreadable chunks dir dropped | `meeting_audio_audit_survives_an_unreadable_chunks_dir_and_names_it`, `privacy_audit_survives_unreadable_meeting_dirs_and_names_them` |
| M1: all `recording` sessions skipped / the verified-running check removed | `meeting_audio_audit_flags_stale_recordings_but_not_live_ones` |
| M2: sync stop saves `stopped` before the chunk delete again | `sync_stop_cleanup_failure_is_failed_and_finalize_finishes_it` |
| M3a: bare io error as the reason | `sync_stop_cleanup_failure_is_failed_and_finalize_finishes_it` |
| M3b: `let _ = clear_active_if_matches(..)` | `launch_failure_logs_an_active_pointer_clear_failure` |
| M3c: a stopped session with an invalid export and chunks falls through to ASR | `finalize_never_overwrites_an_invalid_export_on_a_stopped_session` |

### Commands and results

```bash
cargo fmt --check                              # ok
cargo clippy --all-targets -- -D warnings      # ok
cargo test --all                               # ok (meet_lifecycle 16, meet_service 30, stdout probe 0 bytes)
cargo run -- doctor                            # ok
cargo run -- meet status --format json         # status none
bash scripts/e2e/phase-10a-meet-service.sh     # passed
cargo test --test meet_lifecycle --test meet_service   # 3 more runs, all green
```

### New known limitations

- A session directory whose `session.json` is unreadable is listed only when it holds chunk WAVs (searched two levels deep under `<session_dir>/chunks`) or its chunks dir cannot be read. WAVs stored anywhere else in such a directory are not seen.
- A stale `recording` session with no chunk WAVs is not listed, because it holds no audio.
- The M1, M2 and M3b tests make a directory unreadable or read-only, so they cannot fail as root. The suite is not run as root.
- The audit also lists a session directory whose `session.json` has not been written yet but whose chunks exist (a `meet start` in the gap between creating the chunks dir and saving `session.json`). This is conservative, and in practice the chunks dir is empty at that point.

## Final Round (LF-161 confirming-review findings N1-N5)

### Changes

- **N1 (privacy, gating): an unsearchable parent `chunks/` no longer reads as empty.** `chunk_paths_in_dir` used `is_dir()`, which returns `false` on `EACCES`. For the dual-stream layout (`chunks/<label>/`), a `chunks/` at mode `000` made every per-stream dir look empty, so the audit reported `clean=true` while WAVs remained. It now uses `fs::metadata`: `NotFound` (or not a directory) counts as empty, and any other error, including a failed `read_dir` entry or `file_type`, is returned with the path it concerns. The audit turns that into an `unreadable chunks dir under <chunks_dir>: <stream path>: Permission denied` entry (`clean=false`). The same swallow in `delete_unretained_chunks` (`exists()`) is also fixed: only `NotFound` is a no-op, so an uninspectable chunks dir fails the cleanup, and the session is not committed as `stopped`. `finalize`'s "does a JSON export exist?" check uses the same rule (`meet::path_may_exist`). Other `is_file()` checks on the finalize path decide only whether to rewrite an artifact, so a swallowed error there rewrites a file. It never deletes audio or hides it from the audit.
- **N2 (privacy): moved or restored data dir.** The audit now also lists WAVs under `<session_dir>/chunks` as found on disk, two levels deep. It dedupes them against the recorded paths by canonical path. WAVs found only on disk are listed with a reason that says they are outside the recorded chunks dir. The remedy names the on-disk directory, because `meet finalize` deletes only the recorded chunks dir.
- **N3: truthful remedy for an invalid export.** When a listed non-`recording` session's JSON export exists but does not validate, the audit's reason says the export is invalid and that finalize never overwrites an existing export. The remedy reads: fix or remove the invalid export at `<path>`, then run `comlink meet finalize <id>`. `meet finalize` returns a new `MeetingExportInvalid` error (exit 1, the existing general code) with the same instruction. For consistency, the don't-overwrite rule now also applies to `transcribing` and `failed` sessions that still have chunks. When no chunks remain, the existing `no recoverable export` error is kept, because there is nothing to rebuild the export from.
- **N4: sync stop failures after ASR.** A `write_segments_jsonl` or `write_exports` failure now goes through `mark_failed` and `record_failed_state`, as the chunk-delete failure already did. The original error and its exit code are preserved, the session is `failed` rather than a preliminary `stopped` with `error=null`, and `meet finalize` retries. Sync stop also records `chunks_processed` on the preliminary snapshot before writing artifacts. If only the final `stopped` save fails, `meet finalize <id>` on that `stopped` session now repairs `session.json` (`segment_count`, `duration_ms`, `stopped_at_ms`) from the validated export. It keeps `chunks_processed` rather than returning success over the stale snapshot.
- **N5.** A store entry whose `file_type()` fails is recorded in `scan_errors` against that entry's path. Only an entry that cannot be read at all, and so has no path, is recorded against the store root.

Goldens are byte-identical, the stdout probe still reports 0 bytes, and no exit code changed. The schema change is additive (reason/remedy text and one new error variant). See `docs/output-contract.md`.

### Tests added

`tests/meet_service.rs`:
- `dual_stream_audit_is_not_clean_when_the_parent_chunks_dir_is_unsearchable` (N1: dual-stream, retention off, `chunks/` at `000`; `chunk_files` errors naming the stream dir; audit `clean=false`, stream path named)
- `chunk_cleanup_fails_instead_of_skipping_an_uninspectable_chunks_dir` (N1, cleanup path)
- `meeting_audio_audit_counts_wavs_under_a_moved_session_dir` (N2, plus no double count in the normal layout)
- `invalid_export_remedy_is_truthful_and_failed_sessions_never_overwrite_it` (N3, stopped and failed; following the remedy works)
- `sync_stop_export_write_failure_is_failed_and_finalize_retries` (N4a)
- `finalize_repairs_a_preliminary_stopped_snapshot_left_by_a_failed_final_save` (N4b)
- `finalize_never_overwrites_an_invalid_export_on_a_stopped_session` now asserts `MeetingExportInvalid` and the instruction text.

`src/meet_service.rs` unit test: `session_dir_scan_names_the_entry_whose_type_cannot_be_read` (N5). `src/error.rs`: `MeetingExportInvalid` exits 1.

### Mutation checks (fix reverted, test red, restored, green)

| Reverted | Failing test |
|---|---|
| N1: `chunk_paths_in_dir` treats any metadata error as empty (the old `is_dir()` behaviour) | `dual_stream_audit_is_not_clean_when_the_parent_chunks_dir_is_unsearchable`, at the store assertion. With that assertion removed, the audit assertion also fails, because the reason no longer names the stream dir. The N2 on-disk scan still keeps `clean=false` then, as defence in depth. |
| N1: `delete_unretained_chunks` treats a metadata error as "nothing to delete" | `chunk_cleanup_fails_instead_of_skipping_an_uninspectable_chunks_dir` |
| N2: on-disk `<session_dir>/chunks` scan removed | `meeting_audio_audit_counts_wavs_under_a_moved_session_dir` |
| N3: audit ignores the invalid export (old remedy) | `invalid_export_remedy_is_truthful_and_failed_sessions_never_overwrite_it` |
| N3: `failed` sessions overwrite an invalid export again | `invalid_export_remedy_is_truthful_and_failed_sessions_never_overwrite_it` |
| N4: export writes return through `?` again | `sync_stop_export_write_failure_is_failed_and_finalize_retries` |
| N4: stopped fast path skips the repair | `finalize_repairs_a_preliminary_stopped_snapshot_left_by_a_failed_final_save` |
| N5: entry type error recorded against the root | `session_dir_scan_names_the_entry_whose_type_cannot_be_read` |

### Commands and results

```bash
cargo fmt --check                              # ok
cargo clippy --all-targets -- -D warnings      # ok
cargo test --all                               # ok (lib 102, meet_lifecycle 16, meet_service 36, stdout probe 0 bytes)
cargo run -- doctor                            # ok
cargo run -- meet status --format json         # status none
bash scripts/e2e/phase-10a-meet-service.sh     # passed (artifacts refreshed)
cargo test --test meet_lifecycle --test meet_service   # about 40 full runs: 1 failure, then green on every rerun
```

Flake note: in one of about 40 full `meet_lifecycle` + `meet_service` runs (the second of the first three), the pre-existing `finalize_reports_busy_while_another_process_holds_the_lock` failed once while other agents were loading the machine. The panic message was not captured. It did not recur in 12 isolated runs of that test, or in 33 further full-suite runs (including the final 3). That test spawns a real `comlink meet finalize` subprocess with a 1 s lock wait, so treat it as load-sensitive until it is reproduced.

### New known limitations

- N4b is tested by writing the preliminary snapshot back to `session.json` directly. Making only the final save fail, after a chunk delete in the same directory has succeeded, needs a fault-injection hook. The early `chunks_processed` save has no direct test for the same reason.
- `meet finalize` still deletes only the recorded chunks dir. After a data-dir move, the audit reports the on-disk WAVs (N2), but they are removed by hand.
- `meet status` on a session whose chunks dir cannot be inspected now fails with an error naming the path, where before a non-searchable parent read as `chunk_count: 0`. A missing session still reports `none`.
- The N1 tests use `chmod 000`, so they cannot fail as root. The suite is not run as root.

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
5. **Privacy audit sees leftover meeting audio (fix round).** With `retention.audio` off, after a stopped meeting run `cargo run -- privacy audit` and confirm `meeting_audio: clean=true`. Copy any WAV into that session's `chunks/` directory, rerun the audit, and confirm `clean=false` with a `comlink meet finalize <id>` remedy. Run that command, then confirm the audit is clean again and the WAV is gone.
6. **Privacy audit names unreadable meeting dirs (micro-round).** With `retention.audio` off, create `<data_dir>/meetings/broken/chunks/` (use the data dir that `cargo run -- config show` reports), put any WAV in it, and write `{` to `broken/session.json`. Run `cargo run -- privacy audit` and confirm it exits 0 with `clean=false` and a `meeting_audio_leftover: session=broken status=unknown ...` line naming the directory. Delete `broken/`, then confirm the audit is clean again.
