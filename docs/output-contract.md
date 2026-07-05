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

For saved sessions where transcript retention was disabled, `history show --format json|jsonl|md` keeps the v1 metadata fields and emits retained text fields as `null` or `<not retained>`.

## JSONL Records

Live transcript JSONL emits records in this order:

```text
session
segment
transcript
```

The final `transcript` record contains the same fields as JSON output, plus `record_type`. Saved-session JSONL emits a single `transcript` record.

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
