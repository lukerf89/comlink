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
- `failed`: a detached finalize failed, or a synchronous `meet stop` failed after ASR (an export write or the chunk cleanup). The session carries an `error` string (error text only, never transcript content). Rerun `comlink meet finalize <id>` to retry.

`transcribing` and `failed` are additive in Phase 10a. A plain synchronous `meet stop` never produces `transcribing`, and it produces `failed` only when it fails after ASR (an export write or the chunk cleanup). A successful synchronous stop ends `stopped` as before.

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
- A `stopped` session with no JSON export at all but with chunk WAVs on disk (a synchronous `meet stop` whose ASR failed) is transcribed as if it were `transcribing`.
- An existing JSON export is never overwritten: if it exists but fails validation while chunks remain (in any status), finalize exits 1 with `MeetingExportInvalid` and says to fix or remove the export at `<path>`, then rerun `comlink meet finalize <id>`.
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
    {
      "session_id": "...",
      "status": "stopped",
      "chunk_files": 2,
      "chunks_dir": "...",
      "session_dir": "...",
      "reason": "retention.audio=false but chunk WAVs remain (status stopped)",
      "remedy": "comlink meet finalize <id>"
    }
  ],
  "scan_errors": [ { "path": "...", "reason": "..." } ]
}
```

The scan is conservative and best-effort. `unretained_leftovers` lists:

- every non-`recording` meeting whose `retention.audio` is `false` but whose chunk WAVs are still on disk (`stopped`, `failed`, or still `transcribing`); remedy `comlink meet finalize <id>`. When that meeting's JSON export exists but is invalid, `meet finalize` will not overwrite it, so `reason` says the export is invalid and `remedy` reads: fix or remove the invalid export at `<path>`, then run `comlink meet finalize <id>`;
- chunk WAVs are counted both under the chunk paths recorded in `session.json` and under `<session_dir>/chunks` on disk (two levels deep), without double counting. WAVs found only on disk (a moved or restored data dir whose recorded absolute paths point elsewhere) are listed with a `reason` that says so and a `remedy` naming the on-disk directory, because `meet finalize` deletes only the recorded chunks directory;
- every `recording` meeting whose `retention.audio` is `false`, whose chunk WAVs are on disk, and whose recorder is not verified running (a stale recording); remedy `comlink meet stop <id>`. A recording whose recorder is verified running is not listed;
- any such meeting whose chunks directory, or a per-stream chunks directory under it, cannot be inspected (`chunk_files` is `0`, and `reason` names the path that failed). Only a directory that does not exist counts as empty: a parent `chunks/` that is not searchable is reported, not read as empty;
- any session directory whose `session.json` is missing or unreadable but which holds chunk WAVs (searched two levels deep under `<session_dir>/chunks`) or whose chunks directory cannot be read. Its retention policy is unknown, so `status` is `unknown`, `session_id` is the directory name, and `remedy` names the directory.

A directory that cannot be read never fails the audit. A failure to list the meetings store itself is recorded in `scan_errors` against the store root; a store entry whose type cannot be read is recorded against that entry's path. `clean` is `true` only when both lists are empty. `session_dir`, `reason` and `scan_errors` were added in the LF-161 micro-round (additive). `--format text` prints `meeting_audio: clean=<bool> unretained_leftovers=<n> scan_errors=<n>`, then one `meeting_audio_leftover:` line per entry (ending in `reason=...`) and one `meeting_audio_scan_error:` line per scan error.

### Meeting exit codes

The codes in the table below are unchanged. The meeting-specific cases are:

- `meet status` exits `0` for any readable state, including `none`. An unknown session id exits `1`.
- `meet status` exits `1` (`meeting session <id> is unreadable ...`) when the session it would report has a `session.json` that exists but cannot be read or parsed: the named session, the active session, or, for a bare `meet status` with no recording or `transcribing` session, any session in the store. It never reports `none` in that case. A directory without a `session.json` is not a session and is ignored.
- `meet finalize`, and a synchronous `meet stop`, exit `1` when the unretained chunk cleanup fails, including when the chunks directory cannot be inspected. The error names the chunks directory. The session is left `failed`, not `stopped`, with its exports on disk, and `meet finalize <id>` recovers from the export without re-running ASR, retries the delete and commits `stopped`. A successful `meet stop` prints the same output as before.
- A synchronous `meet stop` whose export write fails after ASR saves the session `failed` with that error and exits with the error's code (`1` for an I/O error); the chunks stay on disk and `meet finalize <id>` retries. If only the final `stopped` save fails, a later `meet finalize <id>` repairs `session.json` (`segment_count`, `duration_ms`, `stopped_at_ms`) from the valid export.
- `meet finalize` on a `stopped` session transcribes the chunks only when no JSON export exists at all (a synchronous stop whose ASR failed). In any status, if a JSON export exists but does not validate (for example, it was edited) and chunks remain, `meet finalize` exits `1` with an error that reads: meeting export at `<path>` is invalid (...); fix or remove the invalid export at `<path>`, then rerun `comlink meet finalize <id>`. It never overwrites that export and never deletes the chunks. A `stopped` session is left unchanged; a `transcribing` or `failed` one is saved `failed` with that error.
- `meet export` with no id exits `1` when the newest non-recording session is still `transcribing` or its finalize `failed`, instead of exporting an older meeting. The error names the session and the `meet status <id>` command.
- Another comlink process holding the session's lifecycle lock exits `1` (`meeting session is busy`).
- A detached finalizer that cannot be launched exits `1`, and the session is marked `failed`.
- `meet finalize` exits with the underlying error's code, so a whisper.cpp failure exits `3`.

## MCP Server (`comlink mcp`, Phase 10b)

`comlink mcp` serves the Model Context Protocol over stdio. The client
launches it as a subprocess. stdout carries only JSON-RPC 2.0 frames and
stderr stays empty; a fatal startup or transport failure prints one `error:`
line to stderr and exits `1`. It opens no network listener, keeps no state
of its own (everything is in the meeting store the CLI uses), and reloads the
config file on every call.

**Tool results that contain transcripts are sent to the calling model.** This
covers `meeting_get_transcript` and both transcript resources. `meeting_status`
and `meeting_list` carry no transcript text.

### Protocol versions

The server accepts `2025-03-26`, `2025-06-18` and `2025-11-25` (rmcp 3.4.1).
A client that asks for a supported version gets it back. Any other version
(older, newer or unknown) gets `2025-11-25`, and the client decides whether to
continue.

### Tools

Every tool returns `structuredContent` (the JSON below) plus text content: one
or more human-readable lines, then the same JSON pretty-printed. For
`meeting_get_transcript` with `format: "md"`, the last text block is the
Markdown itself. There is no `outputSchema`.

| Tool | Input | `structuredContent` on success |
| --- | --- | --- |
| `meeting_start` | `source` (`mic-only` \| `system-only` \| `mic-plus-system`, required), `mode` (required), `device?`, `system_device?`, `no_llm?` | the `meet start --format json` object (`comlink.meeting.v1`) plus `input_device` (`{avfoundation_input, name, selected_by}`, where `selected_by` is `--device`, `COMLINK_RECORD_DEVICE`, `system default input` or `fallback`; `null` for `system-only`). The first text block is the consent reminder, which the agent should pass on; the next names the microphone being recorded. |
| `meeting_status` | `id?` | the `meet status --format json` object, all fields included: `stale`, `stale_reason`, `error`, `finalize_log`, `warnings`, `audio_level`, `recorders`, `finalizer`. Each warning is also a `warning: ...` text line. |
| `meeting_stop` | `id?` | the `meet stop --detach --format json` object (`status: "transcribing"`). The tool always takes the detached path: poll `meeting_status` with the id until `stopped` (or `failed`). |
| `meeting_get_transcript` | `id?`, `format` (`md` \| `json`, required) | `TranscriptResult` (below) |
| `meeting_list` | none | `{sessions: [{session_id, status, started_at_ms, stopped_at_ms, duration_ms, segment_count, source_mode}], skipped: [{dir, reason}]}`, newest first |

`meeting_start` uses the default 300-second chunks and the same validation
order as `meet start`: mode, model, dependencies, device, then source.

`TranscriptResult`:

```json
{
  "schema_version": "comlink.meeting.v1",
  "session_id": "...",
  "status": "stopped",
  "format": "md",
  "content": "# Meeting ...",
  "transcript_retained": true,
  "warnings": ["..."],
  "audio_level": {"mean_dbfs": -30.1, "peak_dbfs": -12.0, "near_silent": false}
}
```

- For `format: "json"`, `content` is the JSON export as an object (the same
  document as `meet export --format json`), not a string.
- `warnings` and `audio_level` always come from the validated JSON export,
  even for `md`. So a session whose JSON export is missing or invalid returns
  `meeting_export_unavailable`, even if the Markdown file exists.
- `transcript_retained` is the session's `retention.transcripts`. When it is
  `false` the call still succeeds. The export's transcript fields are `null`
  by design and a text note says so.
- Session selection with no `id` is the same as `meet export`. The newest
  stopped meeting is used, unless a newer meeting is still `transcribing`
  (`meeting_still_transcribing`) or `failed` (`meeting_finalize_failed`). An
  active recording does not block reading an older stopped meeting.

### Tool annotations

`openWorldHint` is `false` on every tool: nothing leaves the machine except
the result itself.

| Tool | `readOnlyHint` | `destructiveHint` | `idempotentHint` | Why |
| --- | --- | --- | --- | --- |
| `meeting_start` | false | false | false | Turns on the microphone; a second call returns `meeting_already_active`. Clients should ask before calling it. |
| `meeting_stop` | false | true | false | Stops the recorders for good, and the detached finalize deletes the audio chunks when `retention.audio` is off. |
| `meeting_status` | true | false | true | Reads the store and process table only. |
| `meeting_get_transcript` | true | false | true | Reads exports only. |
| `meeting_list` | true | false | true | Reads the store only. |

### Tool errors

A service error never becomes a JSON-RPC error. It is a normal result with
`isError: true`, `structuredContent: {"error_code": "...", "message": "..."}`,
and one text block `error (<error_code>): <message>`. `message` is the
same text the CLI prints after `error: `. Arguments that do not match the
input schema (for example an unknown `source`) are rejected by the SDK before
any service call, as `isError: true` with a text block starting
`failed to deserialize parameters` and no `structuredContent`.

`error_code` is stable. It is `ComlinkError::error_code()`, an exhaustive
match, so a new error cannot ship without a code. Codes an agent will see:

| `error_code` | When |
| --- | --- |
| `mcp_start_disabled` | `meeting_start` while `mcp.allow_start` is false; the message names `comlink config set mcp.allow_start true` |
| `meeting_already_active` | `meeting_start` while a meeting is recording |
| `meeting_no_active_session` | `meeting_stop` with no id and nothing recording; `meeting_get_transcript` with no meetings |
| `meeting_session_not_found` | unknown `id`, or an id that is not a plain session name (`/`, `..`, empty) |
| `meeting_not_recording` | `meeting_stop` on a session that is not recording |
| `meeting_not_stopped` | `meeting_get_transcript` on a session that is still recording |
| `meeting_still_transcribing` | `meeting_get_transcript` while finalize runs |
| `meeting_finalize_failed` | `meeting_get_transcript` on a `failed` session; the message includes the recorded error and `comlink meet finalize <id>` |
| `meeting_export_unavailable` | the session is stopped but its JSON export is missing or invalid, or an export path in `session.json` resolves outside the session directory |
| `meeting_session_unreadable` | a `session.json` exists but cannot be read or parsed; or (transcript reads) it names a different session, or the session directory is a symlink |
| `meeting_lifecycle_busy` | another comlink process holds the session lock, or another meeting start did not finish within 10 s |
| `meeting_finalize_launch_failed` | the detached finalizer could not be started (the session is `failed`) |
| `mode_not_found`, `model_missing`, `model_path_missing`, `dependency_missing`, `dependency_path_missing`, `dependency_not_executable`, `audio_capture_failed`, `invalid_config_value` | `meeting_start` setup failures, same as `meet start` |
| `internal_panic` | the service call panicked (the message carries the panic text); the server keeps running |
| `internal_cancelled` | the service call was cancelled before it finished (e.g. the server shutting down) |

Other codes (`io`, `json`, `storage`, `config_parse`, `whisper_failed`,
`meeting_chunk_cleanup_failed`, `meeting_export_invalid`, ...) follow the same
snake_case naming as the `ComlinkError` variant.

### Resources

| URI template | `mimeType` | Content |
| --- | --- | --- |
| `comlink://meetings/{id}/transcript.md` | `text/markdown` | the Markdown export |
| `comlink://meetings/{id}/transcript.json` | `application/json` | the JSON export (pretty-printed) |

`resources/list` lists both URIs for every `stopped` session.
`resources/templates/list` returns exactly these two templates. `{id}` is one
percent-decoded path segment made of `[A-Za-z0-9._-]` with no `..`. Resource
reads cannot carry `isError`, so failures are JSON-RPC errors with
`data: {"error_code", "message"}`:

| Failure | JSON-RPC `code` | `data.error_code` |
| --- | --- | --- |
| URI does not match a template (wrong scheme or host, unsupported suffix, extra segments, bad id or encoding) | `-32602` (invalid params) | `invalid_resource_uri` |
| Unknown session | `-32002` (resource not found) | `meeting_session_not_found` |
| Session not readable yet: recording, transcribing, failed, or export missing | `-32600` (invalid request) | `meeting_not_stopped`, `meeting_still_transcribing`, `meeting_finalize_failed`, `meeting_export_unavailable` |
| Anything else | `-32603` (internal error) | the error's code |

### `config set` and `privacy audit`

`comlink config set mcp.allow_start true|false` is the only settable key
(`unknown_config_key` and exit `1` otherwise). It rewrites the config file
atomically and changes nothing else. Environment values are never persisted.
It prints `set mcp.allow_start=<bool> in <path>` on stdout. If
`COMLINK_MCP_ALLOW_START` is set and disagrees, it adds a stderr note that
the variable takes precedence. `config show` prints
`mcp: allow_start=<bool>`.

`privacy audit --format json` adds an `mcp` object next to `meeting_audio`
(additive):

```json
"mcp": {
  "transport": "stdio",
  "network_listener": false,
  "allow_start": false,
  "transcripts_sent_to_calling_model": true,
  "note": "..."
}
```

`--format text` adds one line:
`mcp: transport=stdio network_listener=false allow_start=<bool> transcripts_sent_to_calling_model=true`.
`doctor` adds two checks, `mcp-server` (the binary path and a
`claude mcp add` hint) and `mcp-allow-start`. Both are `required: false` with
status `ok` or `info`, so they never fail doctor.

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
