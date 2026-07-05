# Phase 5 Validation

Date: 2026-07-05

## Scope

Implemented local style profiles and optional local LLM rewrite:

- Added configured modes via `comlink modes add <name> --instruction ...`.
- Kept built-in deterministic modes and custom-mode deterministic fallback.
- Added `--no-llm` for `transcribe` and `record`.
- Added local style profile import/list commands.
- Added local LLM config under `llm` with env overrides for enabled/provider/endpoint/model.
- Added Ollama and OpenAI-compatible HTTP request support for local endpoints.
- Ensured local LLM requests receive text only, with an explicit context policy.
- Added transcript LLM metadata and warning records.
- Preserved deterministic final text when LLM rewrite is skipped or unavailable.
- Updated privacy audit to report local LLM posture.

## Commands Run

```bash
cargo fmt --check
cargo test --all
cargo clippy --all-targets -- -D warnings
scripts/e2e/phase-3-work-modes.sh
scripts/e2e/phase-4-output-contract.sh
scripts/e2e/phase-5-local-llm-modes.sh
```

Results:

- `cargo fmt --check` passed.
- `cargo test --all` passed: 30 tests.
- `cargo clippy --all-targets -- -D warnings` passed.
- Phase 3, Phase 4, and Phase 5 E2E scripts passed.

## Agent E2E

Ran `scripts/e2e/phase-5-local-llm-modes.sh`. It:

- Uses isolated `COMLINK_HOME`.
- Uses mocked `ffmpeg`, `whisper.cpp`, and local model files.
- Adds a local vocabulary entry, style profile, and custom prompt mode.
- Runs deterministic fallback with `--no-llm`.
- Runs against a fake Ollama-compatible local HTTP server.
- Verifies the fake LLM rewrite updates `final_text`.
- Verifies the LLM request record is text-only and includes the explicit context policy.
- Kills the fake endpoint and verifies fallback preserves deterministic text and adds a warning.
- Verifies `privacy audit --format json` reports enabled local LLM status and text-only policy.
- Writes artifacts under `docs/validation/artifacts/phase-5/`.

## Known Gaps

- The HTTP client intentionally supports local `http://` endpoints only; HTTPS/cloud providers remain out of scope.
- Local LLM rewrite is not streamed.
- The request record stores instruction/profile metadata and byte counts, not full request/response text beyond generated transcript output.
- Real Ollama smoke testing depends on a user-installed model and was not run in this automated pass.

## Manual Test Script

1. Configure the ASR model and prompt mode:

```bash
export COMLINK_WHISPER_MODEL="/path/to/ggml-tiny.en.bin"

cargo run -- modes add prompt \
  --instruction "Convert rough dictation into a concise coding prompt. Preserve facts and technical terms." \
  --deterministic-mode memo
```

Pass criteria:

- The command prints `saved mode: prompt`.
- `cargo run -- modes list --format json` includes the prompt mode and its `llm_instruction`.
- `COMLINK_WHISPER_MODEL` points to an existing whisper.cpp ggml model, or `cargo run -- models select tiny --path <model>` has selected one.

2. Run deterministic fallback:

```bash
cargo run -- transcribe tests/fixtures/audio/short.wav --mode prompt --no-llm --format json
```

Pass criteria:

- `mode` is `prompt`.
- `final_text` is deterministic fallback text.
- `llm.status` is `skipped`.

3. Run with a local Ollama model, if available:

```bash
export LOCAL_OLLAMA_MODEL="llama3.2"
ollama pull "$LOCAL_OLLAMA_MODEL"
ollama serve
```

In another shell with the same `COMLINK_WHISPER_MODEL` and `LOCAL_OLLAMA_MODEL` values:

```bash
COMLINK_LLM_ENABLED=true \
COMLINK_LLM_PROVIDER=ollama \
COMLINK_LLM_ENDPOINT="http://127.0.0.1:11434/api/generate" \
COMLINK_LLM_MODEL="$LOCAL_OLLAMA_MODEL" \
cargo run -- transcribe tests/fixtures/audio/short.wav --mode prompt --format json
```

Pass criteria:

- `warnings` is empty.
- `llm.status` is `rewritten`; `fallback` means deterministic text was used instead of the LLM rewrite.
- `llm.request.context_policy` is `text-only; no audio; no external context`.
- Audio paths or audio bytes are not sent to the LLM endpoint.

If Ollama is not installed, run `scripts/e2e/phase-5-local-llm-modes.sh` instead; it starts a fake Ollama-compatible local server and verifies the `rewritten` path.
