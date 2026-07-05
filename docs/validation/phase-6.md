# Phase 6 Validation

Date: 2026-07-05

## Scope

Implemented workday surface hardening for the copy-first workflow:

- Hardened clipboard delivery errors with explicit command/path reporting.
- Added clipboard diagnostics for `pbcopy`/`pbpaste` and test overrides.
- Added opt-in `record --copy --restore-clipboard` using `pbpaste`/`COMLINK_PBPASTE`.
- Added built-in deterministic work-surface modes: `terminal`, `editor`, `outlook`, and `slack`.
- Added `doctor --format json` with microphone, clipboard, FFmpeg, ASR, model path, data path, and privacy posture checks.
- Added local install/dev-run docs in `docs/local-dev.md`.
- Kept active paste and global hotkey out of committed scope.

## Commands Run

```bash
cargo fmt --check
cargo test --all
cargo clippy --all-targets -- -D warnings
scripts/e2e/phase-6-workday-surface-hardening.sh
```

Results:

- `cargo fmt --check` passed.
- `cargo test --all` passed: 47 tests.
- `cargo clippy --all-targets -- -D warnings` passed.
- Phase 6 E2E passed and wrote artifacts under `docs/validation/artifacts/phase-6/`.

## Agent E2E

Ran `scripts/e2e/phase-6-workday-surface-hardening.sh`. It:

- Uses isolated `COMLINK_HOME`.
- Uses mocked `ffmpeg`, `whisper.cpp`, model, `pbcopy`, and `pbpaste`.
- Runs `doctor --format json` and verifies required checks are healthy.
- Runs terminal, editor, Outlook, Slack, and memo formatting fixtures.
- Runs `record --mode memo --copy --format json` and verifies clipboard contents match `final_text`.
- Runs `record --copy --restore-clipboard` and verifies the previous clipboard is restored.
- Simulates an unavailable clipboard command and verifies exit code 5 plus actionable error text.

## Known Gaps

- Real microphone and real clipboard testing still require the manual work-surface checklist below.
- `--restore-clipboard` restores immediately after copy; it is primarily for adapter testing and future active-paste work, not the default copy-first flow.
- Global hotkey and active paste remain deferred because they require app-specific OS behavior and manual risk review.

## Manual Work-Surface Checklist

Use a real model and microphone, then dictate and paste:

```bash
cargo run -- doctor --format json
cargo run -- record --mode terminal --copy --format json
cargo run -- record --mode editor --copy --format json
cargo run -- record --mode outlook --copy --format json
cargo run -- record --mode slack --copy --format json
cargo run -- record --mode memo --copy --format json
```

Exact phrases to dictate:

- Terminal or agent prompt: `cargo test --all new line git status --short`.
- Editor text field: `first line new line second line`.
- Outlook or email composer: `thanks for sending this new paragraph I can review today`.
- Slack or chat composer: `sounds good comma I will check after standup`.
- Memo/note file: `phase six memo`.

Pass criteria:

- Clipboard contents paste into the target field without extra diagnostic text.
- Terminal output is single-line command text with `&&` between dictated lines.
- Editor output preserves line breaks.
- Outlook output preserves paragraph breaks.
- Slack output stays concise without added punctuation.
- Memo output reads as sentence-ended note text.
