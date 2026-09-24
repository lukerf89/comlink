# Phase 10b Validation: Local stdio MCP Server (`comlink mcp`)

Date: 2026-09-24

## Scope

Phase 10b adds `comlink mcp`, a local stdio MCP server that lets agents such as Claude Code and Claude Desktop start, monitor, stop and read meeting recordings. It builds on the Phase 10a meeting service ([PR #15](https://github.com/lukerf89/comlink/pull/15), [PR #17](https://github.com/lukerf89/comlink/pull/17)).

- **Transport.** stdio only, using the official Rust SDK `rmcp` pinned to `=3.4.1` with `default-features = false` and the features `server`, `macros` and `transport-io`. tokio is pinned to `=1.53.1` with `rt`, `macros`, `io-std` and `time`. There is no HTTP, SSE or auth code: `cargo tree` contains no hyper, reqwest or axum, and the E2E checks this. The server opens no socket, which the E2E checks with `lsof -i` on the live process.
- **Thin adapter.** `src/mcp.rs` maps each tool or resource onto a single `meet_service` call, run in `tokio::task::spawn_blocking` on a current-thread runtime. It keeps no state: config is reloaded on every call and sessions live in `FileMeetingStore`, so the CLI and MCP act on the same meetings.
- **Tools.**
  - `meeting_start`: consent reminder first, gated by `mcp.allow_start`.
  - `meeting_status`: every `MeetStatusReport` field, with warnings also shown as text.
  - `meeting_stop`: always `prepare_stop` + `stop_detached_prepared`.
  - `meeting_get_transcript`: `md` or `json`, with warnings and audio level.
  - `meeting_list`.
- **Resources.** `comlink://meetings/{id}/transcript.md` and `.json`.
- **Service additions** (in `src/meet_service.rs`, with tests):
  - `prepare_start` / `PreparedStart::into_context` / `DeviceNote`: the `meet start` validation, in its original order.
  - `transcript` / `TranscriptResult`: session selection shared with `export`.
  - `mcp_privacy`.
  - `text::validate_mode` is shared by `transcribe`, `record`, `meet start` and `meeting_start`.
- **Errors.**
  - New variants: `MeetingFinalizeFailedDetail`, `McpStartDisabled` and `UnknownConfigKey`.
  - `ComlinkError::error_code()` is an exhaustive match that returns a stable snake_case code.
- **Config.**
  - `mcp.allow_start` defaults to `false`. It can be set in the file or with `COMLINK_MCP_ALLOW_START`, and it appears in `config show`.
  - `comlink config set mcp.allow_start true|false` changes only that key.
  - `config::save` now writes atomically, so a running server never reads a half-written file.
- **Privacy / doctor.**
  - `privacy audit` gains an `mcp` object next to `meeting_audio`.
  - `doctor` gains the `mcp-server` and `mcp-allow-start` checks. Both are `required: false` with status `ok` or `info`, and they point at `doctor --probe-mic` for TCC.
- **Recorders are detached and reaped (found while testing).**
  - `record::start_segmented_capture` now starts ffmpeg in its own process group, so a client that kills the server's group does not stop the recording.
  - A reaper thread waits on the recorder, so a long-lived `comlink mcp` does not collect zombie recorders once they are stopped.
  - The finalizer already had both (LF-161).

## Decisions

- **Protocol versions:** `2025-03-26`, `2025-06-18` and `2025-11-25` (rmcp's `LATEST`). Any other requested version, including `2024-11-05`, the newer `2026-07-28` and unknown strings, is answered with `2025-11-25`. This uses rmcp's `supported_protocol_versions` hook, so `initialize` is not overridden.
- **`meeting_stop` is `destructiveHint: true`:** it permanently stops the recorders, and the detached finalize deletes the audio chunks when `retention.audio` is off. `meeting_start` is `destructiveHint: false`, `idempotentHint: false`. The read tools are `readOnlyHint: true`, `idempotentHint: true`. `openWorldHint` is `false` everywhere.
- **Service errors are tool results** (`isError: true`, `structuredContent: {error_code, message}`), never JSON-RPC errors. Resource-read failures are JSON-RPC errors carrying the same `data: {error_code, message}`:
  - `-32602` for a bad URI
  - `-32002` for an unknown session
  - `-32600` when the session is not readable yet
  - `-32603` for anything else
- **Schema violations in tool arguments** (for example `source: "radio"`) are rejected by rmcp before the service runs. They come back as `isError: true` with an rmcp text block naming the bad value and no `structuredContent`. rmcp does this, not comlink.
- **`meeting_get_transcript` needs a valid JSON export even for `md`,** because its warnings and audio level come from the JSON export. A session with only a Markdown file returns `meeting_export_unavailable`.
- **`retention.transcripts=false` is a success.** The result has `transcript_retained: false` and the export's null text.
- **Ids that could escape the store** (`/`, `..`, empty) are reported as `meeting_session_not_found` before they reach the store.
- **`meeting_start` uses 300-second chunks** (the `meet start` default). Chunk size is not a tool argument.
- **No `outputSchema`.** The service types are Serialize-only. Their shapes are documented in `docs/output-contract.md`.
- **Tests use a raw JSON-RPC client over `tokio::io::duplex`**, not rmcp's `client` feature. This asserts the exact wire frames and keeps the client feature out of the build.
- **`transcribe_file` is deferred** (it was optional in the issue).

## Commands Run

```bash
cargo build --all-targets
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo run -- doctor
cargo run -- privacy audit --format json
scripts/e2e/phase-10b-mcp.sh
# regressions
scripts/e2e/phase-10a-meet-service.sh
scripts/e2e/phase-7-in-person-meeting-transcript.sh
scripts/e2e/phase-9-online-meeting-capture.sh
```

Results (LF-162 worktree, macOS, rustc 1.96.1):

- `cargo fmt --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test --all` passed:

  | Suite | Tests |
  | --- | --- |
  | library unit tests | 146 |
  | `audio_levels` | 2 |
  | `doctor_diagnostics` | 12 |
  | `mcp_cli_interop` (new) | 4 |
  | `mcp_lifecycle` (new) | 5 |
  | `mcp_protocol` (new) | 6 |
  | `meet_lifecycle` | 16 |
  | `meet_service` | 46 |
  | `meet_service_stdout` (harness=false) | passes: 0 bytes on stdout and stderr |

- `cargo run -- doctor` exited 0. It lists `mcp-server` as `[ok]` with the binary path and `mcp-allow-start` as `[info]`.
- `cargo run -- privacy audit --format json` exited 0. It includes `"mcp": {"transport": "stdio", "network_listener": false, "allow_start": false, "transcripts_sent_to_calling_model": true, ...}`.
- `scripts/e2e/phase-10b-mcp.sh` passed with 22 JSON-RPC frames, 0 stderr bytes and no sockets. Artifacts are in `docs/validation/artifacts/phase-10b/`.
- The phase 10a, 7 and 9 E2E scripts passed unchanged. Their regenerated artifacts differ only in session ids and pids, so they were not committed.
- MSRV: every resolved dependency declares `rust-version` ≤ 1.89. The highest are rmcp, rmcp-macros and darling at 1.88. A 1.89 toolchain is not installed here, so `cargo +1.89 check` was not run.

## Tests Added

- `src/error.rs`: every variant has a unique snake_case `error_code` (the one documented alias is finalize-failed). There are also remedy and exit-code tests for the new variants.
- `src/config.rs`: default / file / env precedence; an invalid env boolean; `config set` changes only `mcp.allow_start` and never persists env; unknown key and bad boolean write nothing; a concurrent reload loop during 150 saves, where every read parses.
- `src/text.rs`: `validate_mode`.
- `src/doctor.rs`: the MCP checks never fail, mention `--probe-mic` and `claude mcp add`, and appear in the report.
- `src/mcp.rs`: strict resource-URI parsing, including traversal and encoding cases; session-id filtering; the supported-version list.
- `tests/meet_service.rs`:
  - `prepare_start` validation order, runtime resolved once, and `DeviceNote` text.
  - `transcript` selection: an active recording alongside an older stopped meeting; a newer transcribing or failed meeting; explicit recording, transcribing and failed ids; none; unknown; missing or invalid JSON export; retention off; JSON content is an object.
  - `mcp_privacy`.
  - The finalizer reaper launched from tokio `spawn_blocking`, with the `ps` poll extracted into `common::assert_reaped`.
  - Recorders started from a long-lived tokio parent are in their own process group and are reaped. Reverting either change fails this test: the pgid check, or `ps state "Z"`.
- `tests/mcp_protocol.rs`:
  - version negotiation;
  - `tools/list` names, required fields and enums, and every annotation;
  - both resource templates and `resources/list`;
  - reading `.md` and `.json`;
  - the resource error matrix (JSON-RPC code and `data.error_code`);
  - schema violations and unsafe ids;
  - every server line is a JSON-RPC 2.0 frame.
- `tests/mcp_lifecycle.rs`:
  - the `allow_start` gate, toggled without a restart;
  - the full cycle with polling;
  - the near-silent warning in status and transcript;
  - the `isError` + `error_code` matrix (none, unknown id, bad mode, already active, not stopped, still transcribing, not recording, failed with remedy);
  - retention off.
- `tests/mcp_cli_interop.rs` (real binary):
  - a CLI start is stopped through a spawned `comlink mcp`, then exported by the CLI;
  - an MCP start after `config set` with no restart, stopped by the CLI, then read through MCP;
  - stdout is JSON-RPC only and stderr is empty;
  - invalid-mode errors and exit codes are unchanged for `transcribe`, `record` and `meet start`;
  - `config set` / `privacy audit` / `config show` MCP output.
- `tests/meet_service_stdout.rs` also runs `prepare_start`, `transcript`, `mcp_privacy` and a full in-process MCP tool cycle, and still asserts 0 bytes on stdout and stderr.

## Fix Round (review round 2)

The first cross-review's Codex pass (95 s, no findings) was treated as no
adversary. This round fixed the Claude reviewers' mediums, ran a real Codex
review of the whole branch diff with a checklist, fixed everything it
confirmed, and ran a confirming Codex re-review of the fix commits.

Claude review mediums:
- `transcript()` for a `failed` session carries the recorded error, or points
  at `finalize.log` when none was recorded; it never says "unknown error".
- A corrupt `session.json` read by `transcript()` is
  `meeting_session_unreadable` naming the session and file.
- New MCP tests: `system-only` and `mic-plus-system` full cycles, start
  without a `device` (the result names the microphone for every selection:
  `input_device` + a text line), and a failing / panicking context factory
  mid-session (tools `isError` with `config_parse` / `internal_panic`,
  resources `-32603` with `data.error_code`, the server keeps serving).

Codex adversary (checklist: stdout purity, child stdio, transcript in logs
were checked clean with file:line evidence), fixed:
- **high — concurrent starts:** two `meeting_start` calls (rmcp dispatches
  concurrently) could both record. Starts are now serialized by
  `<meetings>/start.lock`, held from the active-session check through
  recorder startup and the active-pointer write.
- **medium — config race:** every persistent config read-modify-write now
  runs under `config.json.lock` (`config::update_persistent`), so a
  concurrent `vocab add` cannot restore `mcp.allow_start=true` after a
  successful `config set mcp.allow_start false`.
- **medium — transcript confinement:** transcript reads (tools and
  resources) require `session.json` to name the requested id, reject a
  symlinked session directory, and reject export paths resolving outside it.
- **medium — group stop:** recorder stop signals the recorder's process
  group (falling back to the pid for pre-10b sessions) and waits until the
  group is empty, so a wrapper's descendants stop too.
- low: reaper-thread spawn failures are reported (recorder: stderr;
  finalizer: `finalize.log`).

Lows also fixed: `JoinError` split into `internal_panic` (with panic text)
and `internal_cancelled`; doctor shows why the binary path is unknown;
`config set` flags an invalid `COMLINK_MCP_ALLOW_START` rather than saying it
wins; the E2E fails when artifact redaction or the `lsof` probe fails;
atomic config writes (symlinked `config.json`) documented; a brittle
serde-wording assertion loosened.

Confirming Codex re-review of the fix commits (229 s): P1 (start race), P2
(config race) and P5 (reaper diagnostics) fixed; P3 and P4 partially fixed,
with two new mediums, both fixed in a further commit:
- **TOCTOU symlink swap on transcript reads:** transcript bytes are now read
  with `openat` + `O_NOFOLLOW` (store root → session dir → fixed file name,
  via `rustix`, already in the dependency tree) from the session's own
  directory, never from the absolute paths stored in `session.json`, and the
  validated JSON bytes are the bytes returned. FIFOs and non-regular files
  are refused without blocking.
- **Capture outliving its recorder leader:** a capture is live while its
  verified leader runs or any process in the recorder's group carries this
  session's chunk output pattern (`pgrep -g` + command check, so a reused
  group id never matches). Status, reclaim and stop use that check; stop
  signals the group and waits until it is empty.

A second, short confirming Codex re-review then covered that commit (see the
PR body for its result).

Every fix has a regression test that fails with the fix reverted (checked
by reverting each fix locally): the concurrent-start test failed 3/3 without
the lock; the config race test reported "a stale snapshot re-enabled start";
the group test left the descendant in state `S`; the leader-exit test
reported the session stale; the in-directory symlink test read
`segments.jsonl` as the transcript.

Gate after the round: `cargo fmt --check`, `cargo clippy --all-targets -- -D
warnings`, `cargo test --all` three times (0 failures; see the PR body for the final counts),
`cargo run -- doctor` (exit 0), `cargo run -- privacy audit --format json`,
`scripts/e2e/phase-10b-mcp.sh` and `scripts/e2e/phase-10a-meet-service.sh`
all passed.

## Known Gaps

- `transcribe_file` is not exposed (optional in the issue; deferred).
- There is no `outputSchema`. Structured shapes are documented rather than advertised.
- Protocol `2026-07-28` (stateless, no `initialize`) is not supported. Such clients are offered `2025-11-25`.
- Tool-argument schema errors come from rmcp as text-only `isError` results without `error_code`.
- `meeting_start` cannot set the chunk length.
- The TCC permission prompt and the Claude Desktop flow cannot be automated here. They are covered by the manual gate.
- The `lsof` socket check is macOS/BSD-specific, and the E2E requires `lsof`.
- Protocol `2024-11-05` is not accepted (older MCP Inspector / Desktop builds are offered `2025-11-25`).
- With no sessions at all, `meeting_get_transcript` returns `meeting_no_active_session` ("no active meeting session"), which is worded for recording.
- `meet export` (CLI) keeps trusting the export paths in `session.json`; only the MCP transcript reads are confined to the session directory.
- Concurrent `meeting_start` is tested at the service level (the MCP handlers call the same `start`), not with two in-flight MCP requests.
- `resources/read` on a still-recording session (`-32600`) has no dedicated test.

## Manual Test Instructions (pause gate)

Build and install first, then confirm doctor sees the server:

```bash
cargo install --path .
comlink doctor            # expect [ok] mcp-server <path> and [info] mcp-allow-start
comlink models select base --path /path/to/ggml-base.en.bin   # MCP clients do not read your shell profile
```

1. **Claude Code: record, check, stop, summarise.**
   1. Run `claude mcp add comlink -e COMLINK_FFMPEG=$(command -v ffmpeg) -e COMLINK_WHISPER_CPP=$(command -v whisper-cli) -- "$(command -v comlink)" mcp`.
   2. Run `comlink config set mcp.allow_start true`.
   3. In Claude Code, ask: "Start recording this meeting with comlink (mic-only, clean mode)."
   4. Confirm Claude relays the consent reminder.
   5. Talk for about a minute, then ask for the status. Expect `recording`, recorder alive, and no warnings.
   6. Ask Claude to stop and summarise. Confirm it polls `meeting_status` until `stopped`, then calls `meeting_get_transcript`, and the summary matches what was said.
   7. Run `comlink meet export --format md` in a terminal and confirm it is the same meeting.
2. **Claude Desktop: permission path.**
   1. Add the `claude_desktop_config.json` snippet from `docs/local-dev.md` and restart Claude Desktop.
   2. Ask it to start a recording. Confirm the macOS microphone prompt names Claude (or, if it was already denied, that the doctor guidance in `docs/local-dev.md` makes sense).
   3. With the permission denied, record a few seconds and stop. Confirm the agent reports the near-silent warning from `meeting_status` / `meeting_get_transcript`.
   4. Grant the permission and repeat to confirm real text comes back.
3. **Consent gate and client permission.**
   1. Run `comlink config set mcp.allow_start false`.
   2. Ask the agent to start a recording. Confirm it is refused with a message naming `comlink config set mcp.allow_start true`, and that status, list and transcript still work.
   3. Re-enable the setting without restarting the client, and confirm start now works.
   4. Confirm the client asks for permission before calling `meeting_start` and `meeting_stop`. `meeting_stop` is marked destructive.
4. **Privacy audit.**
   1. Run `comlink privacy audit` and `comlink privacy audit --format json`.
   2. Confirm the `mcp:` line or object is understandable: transport stdio, no network listener, the `allow_start` value, and that transcripts are sent to the calling model.

Resume next: after the gate passes, the next Phase 10 sub-mission (or `transcribe_file` as a follow-up) starts from `docs/comlink-agent-implementation-plan.md`.
