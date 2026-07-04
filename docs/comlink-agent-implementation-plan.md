# Comlink Agent Implementation Plan

Date: 2026-07-04

This plan turns the research/spec documents in `docs/research/` into implementation phases that a long-running coding agent can execute one at a time. Each phase is intended to be complete enough for automated validation and agent-run E2E testing, then stop at a manual user testing gate before the next phase begins.

Primary source documents:

- `docs/research/local-cli-transcription-app-spec.md`
- `docs/research/comlink-implementation-outline.html`

## Planning Baseline

Comlink is a macOS-first, local-first CLI for turning speech and audio files into useful work text. The first durable product loop is:

```text
record or transcribe
  -> normalize audio
  -> reject silence/invalid input
  -> run local ASR
  -> preserve raw transcript
  -> produce clean/work-mode text
  -> emit text or structured output
  -> optionally copy and retain transcript metadata
```

Default technical stance:

- CLI language: Rust with Clap.
- ASR runtime: `whisper.cpp` subprocess first.
- File conversion: FFmpeg subprocess.
- Audio capture: Rust `cpal` or a similarly stable local audio adapter.
- Storage: SQLite.
- Clipboard: macOS copy-first via an adapter.
- Platform priority: macOS first, with Linux/Windows kept architecturally possible.
- Cloud policy: core flows work offline; no silent cloud fallback.
- Retention policy during build-out: retain transcripts for benchmarking by default, retain audio only behind explicit benchmark settings.

## Agent Operating Protocol

Every implementation phase should be run as its own agent mission. The agent should not continue into the next phase until manual testing has completed and the next phase is explicitly approved.

For each phase, the agent should:

1. Re-read this plan and the relevant source spec sections.
2. Inspect the current repo state and previous validation logs.
3. Create or update a short phase plan before editing code.
4. Implement only the scoped phase.
5. Add automated tests and fixture coverage as part of the implementation.
6. Run the phase validation suite.
7. Run the phase E2E suite against the built CLI.
8. If validation fails, convert the failure into a regression test or fixture, fix it, and rerun.
9. Write `docs/validation/phase-N.md` with commands run, results, known gaps, and manual test instructions.
10. Stop and wait for manual user testing.

The recursive improvement rule is mandatory: any E2E bug found by the agent should become either a test, fixture, diagnostic check, or documented known limitation before the phase is considered ready for human testing.

## Cross-Phase Validation Harness

The first implementation phases should create this validation structure and keep it healthy:

```text
tests/
  cli/
  audio/
  fixtures/
scripts/
  dev/
  e2e/
docs/
  validation/
```

Recommended automated checks:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo run -- doctor
```

Recommended CLI contract checks:

- Command help snapshots for major commands.
- Exit code checks for success, missing model, bad input, no speech, and delivery failure.
- JSON schema checks for `--format json` and `--format jsonl`.
- Golden-output checks for deterministic text cleanup.
- Redacted logging checks to ensure transcripts are not emitted in normal logs.

Recommended audio fixture strategy:

- Use short, deterministic fixture files checked into the repo when license-safe.
- Add a macOS-only fixture generator using `say` plus FFmpeg for local test audio.
- Avoid exact transcript equality for ASR output unless the fixture/model pairing is known stable.
- Prefer normalized contains checks, word error rate thresholds, segment schema checks, and latency/timing assertions.

Recommended E2E command shape:

```bash
scripts/e2e/phase-0-file-transcribe.sh
scripts/e2e/phase-1-record-memo.sh
scripts/e2e/phase-3-work-modes.sh
scripts/e2e/phase-7-meeting-mic.sh
```

Each E2E script should:

- Build the release or dev binary from source.
- Use an isolated temp config/data/cache directory.
- Print the exact binary path and config path.
- Fail loudly on unexpected stderr, invalid JSON, missing files, or unexpected network/LLM behavior.
- Leave a small validation bundle under `docs/validation/artifacts/` or a temp path referenced from the phase log.

## Manual Testing Contract

Each phase ends with a human pause gate. The agent should provide:

- What changed.
- What automated checks passed.
- What E2E checks passed.
- Known limitations and deferred items.
- A numbered manual test script.
- Clear pass/fail criteria.

The next phase should not start until manual testing either passes or produces issues that are folded into the current phase.

## Phase 0: Repo Scaffold and Local ASR Spike

Goal: prove a Rust CLI can call local ASR on a file and emit structured output without building the full product.

Scope:

- Create Rust workspace or single crate.
- Add Clap command router.
- Add `comlink doctor`.
- Add `comlink transcribe FILE`.
- Shell out to locally installed `whisper.cpp` binary.
- Detect FFmpeg and `whisper.cpp` paths.
- Normalize common input files to 16 kHz mono WAV when needed.
- Emit plain text and a minimal JSON envelope.
- Clean temp files on success and failure.

Out of scope:

- Microphone capture.
- Model download/install.
- SQLite.
- Clipboard.
- VAD beyond basic duration/silence checks if cheap.

Implementation notes:

- Allow early dependency configuration through env vars and config defaults, for example `COMLINK_WHISPER_CPP` and `COMLINK_FFMPEG`.
- Keep the ASR adapter behind a trait/interface so future engines do not change the CLI contract.
- Keep stdout reserved for command output; diagnostics go to stderr.

Automated validation:

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo run -- doctor
cargo run -- transcribe tests/fixtures/audio/short.wav --format json
```

Agent E2E:

- Generate or use a short local audio fixture.
- Run `transcribe` in text mode.
- Run `transcribe` in JSON mode.
- Validate JSON contains `text`, `engine`, `model`, `duration_ms`, `segments`, and `source`.
- Run against a missing file and assert the expected non-zero exit code.
- Run with an invalid `COMLINK_WHISPER_CPP` and assert `doctor` explains the issue.

Manual pause gate:

- User runs `comlink doctor` on their Mac.
- User transcribes one known audio file.
- User confirms temp files are not left behind in normal operation.
- User approves the minimal CLI shape before microphone work begins.

Phase completion artifact:

- `docs/validation/phase-0.md`

## Phase 1: Basic Voice Memo Loop

Goal: record a short voice memo from the MacBook microphone, transcribe it locally, copy clean text optionally, and emit JSON.

Scope:

- Add `comlink record`.
- Start recording feedback in under 100 ms.
- Stop on Enter.
- Normalize captured audio to 16 kHz mono WAV.
- Add pre-roll if the capture design supports it cleanly.
- Add minimum duration and no-speech guard.
- Add `--format text|json`.
- Add `--copy` using a clipboard adapter.
- Add `--mode memo` with minimal deterministic cleanup.
- Retain transcript records in a simple local file or SQLite if Phase 2 storage is pulled forward.

Out of scope:

- Global hotkeys.
- Active paste.
- Long meetings.
- Style-personalized modes.
- Local LLM rewrite.

Automated validation:

```bash
cargo test --all
cargo run -- record --help
cargo run -- transcribe tests/fixtures/audio/short.wav --mode memo --format json
```

Agent E2E:

- Run a guided microphone smoke test with a timeout and clear prompt.
- Validate that stopping on Enter produces JSON with raw and final text.
- Validate `--copy` reports success and clipboard contents match final text.
- Run a silence/no-input test if possible and assert exit code `4`.
- Measure stop-to-final-text latency and record it in the validation log.

Manual pause gate:

- User records this phrase: "Comlink memo test for July fourth. Supabase API should stay readable."
- User verifies text appears in stdout.
- User runs `record --copy` and pastes into a text editor.
- User confirms microphone permission behavior is understandable.
- User confirms no unexpected audio retention.

Phase completion artifact:

- `docs/validation/phase-1.md`

## Phase 2: Config, Storage, Models, and Privacy Core

Goal: make the local CLI durable across sessions with inspectable config, model state, history, and privacy controls.

Scope:

- Add OS-appropriate config path.
- Add `comlink config show`.
- Add safe config parsing and precedence: defaults, config file, env vars, CLI flags.
- Add SQLite storage.
- Add history tables for sessions and segments.
- Add retention toggles for metadata, transcripts, and audio.
- Add `comlink history list`, `history show`, and `history prune`.
- Add `comlink models list`, `models select`, and registry entries for local model paths.
- Add `comlink privacy audit`.

Out of scope:

- Full model download UX unless trivial.
- Strict network sandboxing.
- Meeting retention.
- Cloud endpoints.

Automated validation:

```bash
cargo test --all
cargo run -- config show --format json
cargo run -- history list --format json
cargo run -- privacy audit --format json
```

Agent E2E:

- Use a temp `COMLINK_HOME` or equivalent isolated data directory.
- Run `record` or `transcribe --save`.
- Verify the session appears in history.
- Toggle transcript retention off and verify future history omits transcript text while preserving metadata.
- Run `history prune --all` and verify records are removed.
- Run `privacy audit` and assert retention/model/LLM status is visible.

Manual pause gate:

- User inspects the config path and history behavior.
- User verifies `privacy audit` is understandable.
- User confirms the default retention posture is acceptable for build-out.
- User approves the data model before work modes depend on it.

Phase completion artifact:

- `docs/validation/phase-2.md`

## Phase 3: Deterministic Text Processing and Work Modes

Goal: make Comlink useful for daily work without requiring an LLM.

Scope:

- Add deterministic cleanup pipeline:
  - Trim and collapse whitespace.
  - Remove only safe fillers such as `um`, `uh`, `umm`, and `uhh`.
  - Clean punctuation spacing.
  - Preserve raw text.
- Add vocabulary replacement.
- Add snippets with longest-trigger-wins behavior.
- Add `comlink vocab add/list/remove`.
- Add `comlink snippets add/list/remove`.
- Add mode registry.
- Add built-in modes:
  - `raw`
  - `clean`
  - `memo`
  - `coding-prompt`
  - `email-reply`
  - `slack-reply`
- Add terminal/coding rules that preserve filenames, URLs, camelCase, snake_case, flags, and shell-ish syntax.

Out of scope:

- Local LLM rewrite.
- Active app context.
- Selected text transforms.

Automated validation:

```bash
cargo test --all text modes vocab snippets
cargo run -- modes list --format json
cargo run -- vocab add "super base" "Supabase"
cargo run -- snippets add "my signature" "Best,\nLuke"
```

Agent E2E:

- Run fixture transcripts through each mode without ASR.
- Run audio fixtures through `transcribe --mode coding-prompt`.
- Verify raw text is preserved in JSON.
- Verify clean/final text applies replacements and snippets.
- Verify coding fixtures preserve `src/main.rs`, `cargo test --all`, `camelCase`, `snake_case`, and URLs.
- Add a regression fixture for every cleanup bug discovered.

Manual pause gate:

- User dictates a coding prompt containing a filename, a command, and a URL.
- User dictates one email-style reply and one Slack-style reply.
- User verifies output is useful without feeling over-polished.
- User approves the built-in mode names and deterministic behavior.

Phase completion artifact:

- `docs/validation/phase-3.md`

## Phase 4: Stable Agent-Friendly Output Contract

Goal: make Comlink output reliable for other agents, scripts, and future automation.

Scope:

- Finalize v1 JSON schema for sessions and segments.
- Add JSONL output for streaming-like or batch consumption.
- Document exit codes.
- Ensure stderr/stdout behavior is strict and test-covered.
- Add `--format text|json|jsonl|md`.
- Add Markdown transcript export for files and saved sessions.
- Add source metadata and context metadata fields, even if context is empty.
- Add schema versioning.
- Add CLI examples for agent consumption.

Out of scope:

- MCP/local HTTP server.
- Downstream action extraction.
- Meeting-specific source labels beyond schema placeholders.

Automated validation:

```bash
cargo test --all
cargo run -- transcribe tests/fixtures/audio/short.wav --format json
cargo run -- transcribe tests/fixtures/audio/short.wav --format jsonl
cargo run -- transcribe tests/fixtures/audio/short.wav --format md
```

Agent E2E:

- Run a parser script against JSON and JSONL.
- Assert schema version, session ID, raw text, final text, mode, engine, model, duration, segments, source metadata, and context policy are present.
- Assert no diagnostic text appears on stdout for JSON/JSONL modes.
- Assert exit codes match the spec for missing file, missing model, no speech, and delivery failure.

Manual pause gate:

- User reviews sample JSON, JSONL, and Markdown outputs.
- User confirms the output is sufficient for an external agent to consume.
- User approves schema v1 or requests changes before meetings and style modes build on it.

Phase completion artifact:

- `docs/validation/phase-4.md`

## Phase 5: Local Style Profiles and Optional LLM Rewrite

Goal: add personalized work modes while keeping deterministic output as the safe fallback.

Scope:

- Add local style/profile setup from structured files.
- Store raw examples and distilled local profiles.
- Add optional local LLM rewrite through Ollama first.
- Add OpenAI-compatible local endpoint support if the abstraction is small.
- Add mode-level `llm_instruction`.
- Add `--no-llm`.
- On LLM failure, fall back to deterministic clean text and mark the session warning.
- Ensure LLM receives text only, never audio.
- Include explicit context policy in every LLM request record.

Out of scope:

- Cloud LLM providers.
- Automatic training on corrections.
- Screen or textbox context.

Automated validation:

```bash
cargo test --all
cargo run -- modes add prompt --instruction "Convert rough dictation into a concise coding prompt."
cargo run -- transcribe tests/fixtures/audio/short.wav --mode prompt --no-llm --format json
```

Agent E2E:

- Run deterministic fallback with `--no-llm`.
- Run against a local fake Ollama-compatible test server and assert request/response handling.
- If Ollama is installed and a local model is configured, run a real local rewrite smoke test.
- Kill or misconfigure the LLM endpoint and verify fallback behavior.
- Verify privacy audit reports configured local LLM status.

Manual pause gate:

- User supplies or approves a small local style profile.
- User runs a coding prompt and a memo with local rewrite enabled, if available.
- User compares raw, deterministic, and rewritten text.
- User confirms the rewrite does not invent facts or erase technical details.

Phase completion artifact:

- `docs/validation/phase-5.md`

## Phase 6: Workday Surface Hardening

Goal: make the copy-first workflow reliable across the real places the user works.

Scope:

- Harden clipboard adapter and error reporting.
- Add optional clipboard restore if implemented safely.
- Add formatting presets for terminal, editor, Outlook, Slack, and memo targets.
- Add local install/dev-run documentation.
- Add diagnostics for microphone, clipboard, FFmpeg, ASR, model path, data path, and privacy posture.
- Spike but do not commit to global hotkey or active paste unless the path is clearly low-risk.

Out of scope:

- Full daemon.
- System-wide paste.
- App-specific automation.

Automated validation:

```bash
cargo test --all
cargo run -- doctor --format json
cargo run -- record --mode memo --copy --format json
```

Agent E2E:

- Copy deterministic fixture output and verify clipboard contents.
- Run through terminal/editor-safe formatting fixtures.
- Run failure simulation for clipboard command unavailable.
- Validate `doctor` gives actionable remediation for missing dependencies.
- Produce a manual work-surface checklist with exact phrases to dictate.

Manual pause gate:

- User dictates and copies into:
  - Terminal or agent prompt.
  - Editor text field.
  - Outlook or email composer.
  - Slack or chat composer.
  - Memo/note file.
- User confirms formatting survives paste.
- User decides whether active paste/global hotkey should enter the next roadmap slice.

Phase completion artifact:

- `docs/validation/phase-6.md`

## Phase 7: In-Person Meeting Transcript v0

Goal: capture a 30-60 minute in-person meeting from the MacBook microphone and export timestamped transcript artifacts after stop.

Scope:

- Add `comlink meet start`.
- Add `comlink meet stop`.
- Add visible recording state and elapsed time.
- Add long-form mic capture with chunking.
- Add VAD-aware segmenting where available.
- Add rolling JSONL segment output or saved segment records.
- Add final Markdown and JSON export.
- Add consent reminder.
- Add inactivity auto-stop if reliable enough.

Out of scope:

- Zoom/Teams system audio.
- Diarization.
- Live transcript UI.
- Meeting notes or summaries.

Automated validation:

```bash
cargo test --all
cargo run -- meet --help
cargo run -- meet export --help
```

Agent E2E:

- Run a short 2-5 minute simulated meeting from a fixture file through the meeting pipeline.
- Verify chunks stitch into ordered segments.
- Verify timestamps are monotonic.
- Verify Markdown and JSON exports include session metadata and retention policy.
- Run a longer synthetic fixture if practical to catch memory/file-handle issues.

Manual pause gate:

- User records a 10-15 minute real-world meeting or monologue first.
- If that passes, user records a 30-60 minute in-person meeting.
- User verifies recording state is visible and stop behavior is safe.
- User reviews Markdown and JSONL transcript artifacts.
- User approves the meeting lifecycle before system audio work begins.

Phase completion artifact:

- `docs/validation/phase-7.md`

## Phase 8: Zoom/Teams System Audio Spike

Goal: decide how online meeting audio can be captured locally on macOS before committing to a full feature build.

Scope:

- Research and prototype macOS system audio capture paths.
- Test Zoom and Teams permission behavior.
- Probe mic-only, system-only, and mic-plus-system source handling.
- Decide whether a virtual audio driver or documented local dependency is required.
- Add diagnostics that can detect the chosen dependency or explain why capture is unavailable.
- Produce a decision record.

Out of scope:

- Production online meeting capture.
- Diarization.
- Speaker identification.

Automated validation:

```bash
cargo test --all
cargo run -- doctor --format json
```

Agent E2E:

- Run a short local system-audio capture proof if possible.
- Run a short Zoom or Teams call capture proof if possible.
- Verify source metadata can distinguish `user_mic` and `system_audio` when both are available.
- Verify failure modes produce actionable diagnostics.

Manual pause gate:

- User reviews the system audio decision record.
- User confirms whether requiring a local audio dependency is acceptable.
- User confirms which app matters first: Zoom or Teams.
- User approves or rejects moving to online meeting v0.

Phase completion artifact:

- `docs/validation/phase-8.md`
- `docs/decisions/system-audio-macos.md`

## Phase 9: Online Meeting Capture v0

Goal: capture local Zoom or Teams meeting audio into agent-readable transcript segments with source metadata.

Scope:

- Implement the chosen system audio adapter.
- Capture mic plus system audio where available.
- Add source labels: `user_mic`, `system_audio`, or `mixed`.
- Add permission diagnostics.
- Add timestamped JSONL and Markdown export.
- Add acceptance tests for the first approved app, then the second if feasible.
- Keep diarization out of scope unless source labels are insufficient and the dependency is approved separately.

Out of scope:

- Bot-based meeting attendance.
- Cloud transcription.
- Automatic summaries.
- Speaker diarization.

Automated validation:

```bash
cargo test --all
cargo run -- doctor --format json
cargo run -- meet start --help
```

Agent E2E:

- Run local system-audio fixture tests.
- Run short controlled online call capture where possible.
- Verify source metadata in JSONL.
- Verify audio is not retained unless explicitly enabled.
- Verify privacy audit reflects system audio permissions and retention.

Manual pause gate:

- User joins a short Zoom or Teams call and captures a transcript.
- User verifies local participant and remote participant audio are represented.
- User reviews source labels and timestamp usefulness.
- User approves the online meeting v0 behavior before enrichment work starts.

Phase completion artifact:

- `docs/validation/phase-9.md`

## Phase 10: Context, Agent Integration, and Pro Polish

Goal: add richer work context and integrations without compromising local inspectability.

Scope options, to be split into smaller agent missions before implementation:

- Git-aware context: repo name, branch, changed files, optional current file.
- Per-directory/project vocabulary.
- Selected text transform mode with explicit opt-in.
- Local HTTP or MCP server for agent integration.
- Searchable transcript library.
- Meeting context bundles with title, agenda, participant list, and project tags.
- Optional diarization research via WhisperX/pyannote.
- SRT/VTT/DOCX exports.
- Global hotkey/listen daemon if manual testing proves copy-first is not enough.

Out of scope unless separately approved:

- Cloud sync.
- Team accounts.
- Mobile app.
- Silent screenshot or textbox capture.

Automated validation:

```bash
cargo test --all
cargo run -- privacy audit --format json
```

Agent E2E:

- For each approved sub-feature, add a dedicated E2E script.
- Verify context metadata is explicit, opt-in, and visible in JSON output.
- Verify disabling a context source removes it from LLM requests and output metadata.
- Verify integrations can consume Comlink output without parsing human text.

Manual pause gate:

- User reviews each new context source before it is enabled by default.
- User tests agent integration against a real workflow.
- User confirms the privacy audit remains understandable.

Phase completion artifact:

- `docs/validation/phase-10.md`

## Phase Acceptance Summary

| Phase | Manual test checkpoint | Must pass before next phase |
| --- | --- | --- |
| 0 | File transcription works locally | `doctor`, text output, JSON output, temp cleanup |
| 1 | User records and copies a memo | Mic capture, no-speech guard, JSON, clipboard |
| 2 | User approves config/history/privacy | Retention toggles, history, privacy audit |
| 3 | User approves deterministic work modes | Raw preserved, vocab/snippets, coding syntax preserved |
| 4 | User approves schema v1 | JSON/JSONL/MD, exit codes, stdout/stderr contract |
| 5 | User approves local style/rewrite behavior | LLM optional, fallback safe, no invented facts |
| 6 | User tests work surfaces | Copy works in terminal, editor, email, chat, memo |
| 7 | User records in-person meeting | Long capture, chunks, timestamps, Markdown/JSONL |
| 8 | User approves system audio path | Zoom/Teams capture dependency decision |
| 9 | User captures online meeting | Mic/system audio, source metadata, privacy audit |
| 10 | User approves each pro/context feature | Explicit context, integration E2E, privacy clarity |

## Definition of Done for Any Phase

A phase is done only when:

- The implementation matches the scoped goal.
- All new commands have help text.
- Unit and integration tests cover the changed behavior.
- Agent-run E2E has been attempted and either passed or produced documented limitations.
- E2E failures were converted into regression coverage where practical.
- Privacy and retention behavior is explicit.
- `docs/validation/phase-N.md` exists.
- The final agent response includes manual test instructions and says where to resume next.

## Recommended First Agent Prompt

Use this prompt to start Phase 0:

```text
Implement Phase 0 from docs/comlink-agent-implementation-plan.md. Keep the scope limited to repo scaffold, Rust CLI, doctor, file transcription through a local whisper.cpp subprocess, FFmpeg normalization, minimal text/JSON output, tests, and docs/validation/phase-0.md. Run the phase validation and E2E checks, recursively fix failures, then stop at the manual testing gate.
```

