# Comlink Output Contract

Comlink transcript payloads are intended to be stable enough for agents and scripts to parse. The v1 transcript schema is identified by:

```text
comlink.session.v1
```

## Formats

`transcribe`, `record`, and `history show` support:

- `--format text`: final transcript text only.
- `--format json`: pretty-printed v1 transcript object.
- `--format jsonl`: newline-delimited records for batch or streaming-like consumers.
- `--format md`: human-readable Markdown transcript export.

Operational commands such as `config show`, `models list`, `modes list`, `vocab list`, and `privacy audit` keep their narrower `text|json` formats.

## JSON Fields

Current transcript JSON includes:

- `schema_version`: always `comlink.session.v1` for this contract.
- `session_id`: unique local session identifier.
- `text`: compatibility alias for `final_text`.
- `raw_text`: original ASR transcript text before deterministic processing.
- `final_text`: text after the selected mode and local rules run.
- `mode`: selected text mode, such as `raw`, `memo`, or `coding-prompt`.
- `copied`: whether Comlink attempted clipboard delivery for this output.
- `engine`: ASR engine name.
- `model`: selected local model path.
- `duration_ms`: normalized source duration.
- `segments`: ordered transcript segments with `start_ms`, `end_ms`, and text.
- `source`: source metadata, including original path plus normalized audio sample rate and channels.
- `context`: context metadata. Phase 4 always emits `{"policy":"none","items":[]}`.
- `processing_steps`: deterministic processing steps applied.
- `history_session_id`: present when the transcript was saved to local history.

For saved sessions, `schema_version` and `copied` are persisted at capture time and echoed by `history show`. Where transcript retention was disabled, `history show --format json|jsonl|md` keeps the v1 metadata fields and emits retained text fields as `null` or `<not retained>`.

`context` is reserved for future local context controls. In v1 it is intentionally empty and always carries `policy: "none"` with no `items`; agents should not treat it as a signal that external context was used.

## JSONL Records

Live transcript JSONL emits records in this order:

```text
session
segment
transcript
```

The final live `transcript` record contains completed transcript metadata and text plus `record_type`, but omits `segments` because each segment has already been emitted as its own `segment` record. Saved-session JSONL emits a single self-contained `transcript` record with retained segments.

## Markdown

Markdown export includes metadata, final text, raw text, and segment details. It is meant for review, archival, and handoff to tools that prefer Markdown.

## Stdout And Stderr

Structured transcript output is written only to stdout. Diagnostics such as recording prompts, save confirmations, copy confirmations, and latency messages are written to stderr.

## Exit Codes

- `0`: success.
- `1`: general input, config, dependency, storage, IO, JSON, or not-found error.
- `2`: audio capture failure.
- `3`: missing model, missing model path, or ASR engine failure.
- `4`: no speech or recording too short.
- `5`: clipboard delivery failure.

## Meeting Commands (`comlink.meeting.v1`)

`meet start`, `meet stop`, `meet status`, `meet finalize`, and `meet export` emit meeting payloads identified by `schema_version: "comlink.meeting.v1"`. As with transcripts, stdout carries only the structured payload and stderr carries diagnostics: the consent reminder, `Stopping meeting recording: <id>`, the warning banner, and the background-transcription notice.

### Session status values

A meeting session's `status` is one of:

- `recording`: recorders are capturing chunks.
- `transcribing`: recorders are stopped and a detached `meet finalize` owns transcription (only after `meet stop --detach`).
- `stopped`: the transcript exports are written.
- `failed`: a detached finalize failed. The session carries an `error` string (error text only, never transcript content). Rerun `comlink meet finalize <id>` to retry.

`transcribing` and `failed` are additive in Phase 10a. A plain synchronous `meet stop` never produces them.

### `meet stop --detach`

This stops the recorders, marks the session `transcribing`, launches `comlink meet finalize <id>` in its own process group, clears the active session, and returns straight away:

```json
{
  "schema_version": "comlink.meeting.v1",
  "session_id": "...",
  "status": "transcribing",
  "elapsed_ms": 0,
  "preliminary_duration_ms": 0,
  "chunk_count": 0,
  "finalizer_pid": 0,
  "artifacts": { "session_dir": "...", "segments_jsonl": "...", "json_export": "...", "markdown_export": "...", "chunks_dir": null },
  "finalize_log": "<session_dir>/finalize.log"
}
```

Because detaching clears the active session, a new `meet start` is allowed while the earlier meeting is still transcribing. The finalizer's stderr goes to `finalize.log`.

### `meet finalize <id>`

This finishes a `transcribing` or `failed` session and prints the same payload shape as a synchronous `meet stop`, in `--format text|json`. It is meant for internal use (the detached launch) and for recovery. It is idempotent:

- On a `stopped` session it reprints the stop payload, rebuilt from the validated JSON export, without running ASR again. If the session's `retention.audio` is `false` and chunk WAVs are still on disk (a cleanup that failed, or a session finished before this ordering existed), it deletes them first.
- If an earlier finalize crashed after the exports were written, it recovers from the validated JSON export and regenerates the Markdown and JSONL.
- A `stopped` session with no valid JSON export but with chunk WAVs on disk (a synchronous `meet stop` whose ASR failed) is transcribed as if it were `transcribing`.
- On a `recording` session it exits 1.

Finalize commits `stopped` only as its last step: exports are written, then (when `retention.audio` is `false`) the chunk WAVs are deleted, then the session is saved as `stopped`. If the chunk cleanup fails, the session is marked `failed` with an `error` that names the cleanup (`meeting audio chunk cleanup failed ...`), the command exits `1`, and a rerun of `meet finalize <id>` recovers from the export and retries the cleanup. Chunks are never deleted while the session's `retention.audio` is `true`.

If finalize fails and the `failed` state itself cannot be saved, the original error (and its exit code) is still returned, and the save failure is appended to `<session_dir>/finalize.log`.

### `meet status [id]`

This command is read-only. With no id it reports the active recording session, or else the newest `transcribing` session, or else the newest `failed` session when it is newer than the newest `stopped` one, or else `status: "none"`. To keep polling a session after it finishes, pass its id: once it has stopped, a bare `meet status` reports `none`. A `failed` report carries a warning naming the error, the session's `finalize.log` (when it exists), and the `comlink meet finalize <id>` retry command.

```json
{
  "schema_version": "comlink.meeting.v1",
  "session_id": "..." ,
  "status": "recording | transcribing | stopped | failed | none",
  "elapsed_ms": 0,
  "recorders": [{ "source_label": "user_mic", "device": ":0", "pid": 0, "alive": true }],
  "finalizer": { "pid": 0, "alive": true },
  "chunk_count": 0,
  "audio_level": { "mean_dbfs": -30.0, "peak_dbfs": -6.0, "near_silent": false },
  "warnings": [],
  "stale": false,
  "stale_reason": null,
  "error": null,
  "finalize_log": "<session_dir>/finalize.log"
}
```

- For `status: "none"`, `session_id` and `elapsed_ms` are `null`.
- `finalize_log` (additive) is the session's `finalize.log` path when that file exists, else `null`. The stale-`transcribing` reason and the `failed` warning reference it when it exists. `--format text` prints it as a `finalize_log:` line.
- `finalizer` is `null` unless a detached finalizer is recorded.
- `audio_level` is `null` when nothing measurable is available. It comes from the newest completed chunk; the newest chunk of a live recorder is skipped because it may still be being written. A near-silent level adds a warning.
- `stale` is `true` in two cases:
  - A `recording` session has no live recorder.
  - A `transcribing` session has no live finalizer and nobody holds its lifecycle lock (for example, after a crash, logout, or reboot).

  In either case `stale_reason` names the recovery command, and `status` itself never hangs.

### `privacy audit`: meeting audio

`privacy audit --format json` adds a `meeting_audio` object (additive):

```json
"meeting_audio": {
  "clean": false,
  "unretained_leftovers": [
    { "session_id": "...", "status": "stopped", "chunk_files": 2, "chunks_dir": "...", "remedy": "comlink meet finalize <id>" }
  ]
}
```

`unretained_leftovers` lists every non-`recording` meeting whose `retention.audio` is `false` but whose chunk WAVs are still on disk (`stopped`, `failed`, or still `transcribing`). `clean` is `true` only when that list is empty. `--format text` prints `meeting_audio: clean=<bool> unretained_leftovers=<n>` plus one `meeting_audio_leftover:` line per session.

### Meeting exit codes

The codes in the table below are unchanged. The meeting-specific cases are:

- `meet status` exits `0` for any readable state, including `none`. An unknown session id exits `1`.
- `meet status` exits `1` (`meeting session <id> is unreadable ...`) when the session it would report has a `session.json` that exists but cannot be read or parsed: the named session, the active session, or, for a bare `meet status` with no recording or `transcribing` session, any session in the store. It never reports `none` in that case. A directory without a `session.json` is not a session and is ignored.
- `meet finalize` exits `1` when the unretained chunk cleanup fails; the session is left `failed`, not `stopped`.
- `meet export` with no id exits `1` when the newest non-recording session is still `transcribing` or its finalize `failed`, instead of exporting an older meeting. The error names the session and the `meet status <id>` command.
- Another comlink process holding the session's lifecycle lock exits `1` (`meeting session is busy`).
- A detached finalizer that cannot be launched exits `1`, and the session is marked `failed`.
- `meet finalize` exits with the underlying error's code, so a whisper.cpp failure exits `3`.

## Agent Examples

Parse a saved JSON transcript:

```bash
cargo run -- transcribe tests/fixtures/audio/short.wav --format json --save \
  | jq -r '.schema_version, .session_id, .final_text'
```

Consume the final record from JSONL:

```bash
cargo run -- transcribe tests/fixtures/audio/short.wav --format jsonl \
  | jq -rc 'select(.record_type == "transcript")'
```

Export Markdown:

```bash
cargo run -- transcribe tests/fixtures/audio/short.wav --format md
cargo run -- history show "$SESSION_ID" --format md
```
