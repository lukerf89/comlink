# Phase 3 Validation

Date: 2026-07-04

## Scope

Implemented deterministic text processing and work modes:

- Built-in modes: `raw`, `clean`, `memo`, `coding-prompt`, `email-reply`, and `slack-reply`.
- `comlink modes list --format text|json`.
- `comlink modes apply --mode <mode> --text <text> --format text|json` for no-ASR fixture checks.
- Deterministic cleanup for whitespace, safe fillers, and punctuation spacing.
- Local vocabulary replacements with `comlink vocab add/list/remove`.
- Local snippets with `comlink snippets add/list/remove`.
- Longest-trigger-wins snippet expansion.
- Transcript output still preserves `raw_text` while `text` and `final_text` contain processed output.
- Coding prompt cleanup preserves filenames, commands, flags, camelCase, snake_case, and URLs.

## Commands Run

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo test --all text
cargo test --all modes
cargo test --all vocab
cargo test --all snippets
COMLINK_HOME=/tmp/comlink-phase3 cargo run --quiet -- modes list --format json
COMLINK_HOME=/tmp/comlink-phase3 cargo run --quiet -- vocab add "super base" "Supabase"
COMLINK_HOME=/tmp/comlink-phase3 cargo run --quiet -- snippets add "my signature" "Best,\nLuke"
scripts/e2e/phase-3-work-modes.sh
```

Results:

- `cargo fmt --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test --all` passed: 22 tests.
- The phase-plan test command was split into valid Cargo filters because Cargo accepts a single test filter per invocation.
- `modes list --format json` passed and returned the six built-in deterministic modes.
- `vocab add "super base" "Supabase"` passed.
- `snippets add "my signature" "Best,\nLuke"` passed and persisted the snippet body with a newline.
- `scripts/e2e/phase-3-work-modes.sh` passed.

## Agent E2E

Ran `scripts/e2e/phase-3-work-modes.sh`. It:

- Uses isolated `COMLINK_HOME`.
- Adds vocabulary and snippet config.
- Verifies `modes list --format json`.
- Runs transcript text fixtures through `raw`, `clean`, `memo`, `coding-prompt`, `email-reply`, and `slack-reply` without ASR.
- Verifies `raw_text` is preserved in JSON output.
- Verifies final text applies vocab and snippets.
- Verifies coding prompt output preserves `src/main.rs`, `cargo test --all`, `camelCase`, `snake_case`, and `https://example.com/api`.
- Runs the audio fixture through mocked `ffmpeg` and `whisper.cpp` with `transcribe --mode coding-prompt --format json`.
- Verifies transcript JSON preserves raw text and records coding-prompt processing steps.
- Verifies vocab and snippets can be removed.
- Writes artifacts under `docs/validation/artifacts/phase-3/`.

## Known Gaps

- Modes are deterministic only; local LLM rewriting remains Phase 5.
- Vocab and snippets are global config entries; mode-specific vocab/snippet scopes are not implemented.
- Cleanup intentionally removes only conservative fillers: `um`, `uh`, `umm`, and `uhh`.
- The mode copy is deliberately light. It does not infer salutations, paragraph structure, or rich Markdown.
- Non-ASCII phrase matching is not a primary Phase 3 target.

## Manual Test Script

1. Inspect the built-in mode registry:

```bash
cargo run -- modes list --format json
```

2. Add local deterministic rules:

```bash
cargo run -- vocab add "super base" "Supabase"
cargo run -- snippets add "my signature" "Best,\nLuke"
```

3. Try a coding prompt without ASR:

```bash
cargo run -- modes apply --mode coding-prompt --text "uh update src/main.rs then run cargo test --all and check https://example.com/api with super base ." --format json
```

Pass criteria:

- `raw_text` still contains the original wording.
- `final_text` removes safe fillers and fixes punctuation.
- `src/main.rs`, `cargo test --all`, and the URL survive unchanged.
- `super base` becomes `Supabase`.

4. Dictate or transcribe real audio with the coding prompt mode:

```bash
cargo run -- transcribe tests/fixtures/audio/short.wav --mode coding-prompt --format json
```

5. Try one email-style reply and one Slack-style reply:

```bash
cargo run -- modes apply --mode email-reply --text "um thanks for sending this over , I can take a look tomorrow ." --format text
cargo run -- modes apply --mode slack-reply --text "uh sounds good , I will check it after standup" --format text
```

Pass criteria:

- Output is useful without feeling over-polished.
- Email and Slack modes keep the user's wording.
- Built-in mode names feel right enough to carry into later phases.
