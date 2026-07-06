# AGENTS.md — Comlink

Durable operating context for any agent (Codex, Claude) working in this repo.
Read this first, then the section of the plan named in your mission.

## What Comlink is

macOS-first, local-first Rust CLI that turns speech and audio files into useful
work text: `record` / `transcribe` → normalize audio → reject silence → local
ASR (`whisper.cpp` subprocess) → preserve raw transcript → deterministic clean
/ work-mode text → emit text or structured output → optionally copy.

## Source of truth

`docs/comlink-agent-implementation-plan.md` is the spec. It carries the phase
definitions, the **Agent Operating Protocol** (10-step mission), the
**Cross-Phase Validation Harness**, and the **Definition of Done**. Your Linear
issue names one phase; implement **only** that phase's scope. Do not start the
next phase — every phase ends on a human manual-test gate.

## Architecture invariants

- **Core-first.** Product logic lives in `src/` modules (`asr`, `audio`, `text`,
  `output`, `record`, `config`, `storage`, `llm`, …). Side-effecting surfaces
  (clipboard, subprocesses, mic, HTTP) go through **adapters** so the core stays
  testable. New I/O = new adapter, not inline calls.
- **Local-first, offline-capable.** Core flows work with no network. **No silent
  cloud fallback.** LLM rewrite is optional and must degrade to deterministic
  output on any failure (`--no-llm` always works).
- **Privacy.** Transcripts are sensitive: never emit them in normal logs;
  respect retention toggles; `privacy audit` must reflect real state.
- **Stable output contract.** `--format text|json|jsonl|md` is a public API.
  stdout carries data, stderr carries diagnostics. Don't break the v1 schema or
  documented exit codes without the plan saying so.

## The gate (run before you report done)

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo run -- doctor
```

All four must pass. This is the exact gate the orchestrator re-runs
independently — "green on my machine" is verified, not trusted.

## Test-first + recursive-improvement rule

Add automated tests and fixtures **as part of** the implementation, not after.
Any bug you find while running the phase E2E must become a test, fixture,
diagnostic check, or a documented known-limitation **before** you report done.
Prefer normalized/contains assertions and schema checks over exact ASR equality.

## Per-phase deliverables (match existing conventions)

- Code + tests for the scoped phase only.
- `scripts/e2e/phase-N-<slug>.sh` — builds the binary, uses an isolated temp
  config/data dir, prints the binary + config paths, fails loudly on unexpected
  stderr / invalid JSON / missing files / unexpected network.
- `docs/validation/phase-N.md` — commands run, results, known gaps, and the
  exact **manual test instructions** for the human gate.
- Leave any validation artifacts under `docs/validation/artifacts/phase-N/`.

## Commit + self-report

- Commit to the mission's branch (`codex/phase-N-<slug>`) with focused messages.
- End your run with a structured report (written to the file the orchestrator
  passes via `--output-last-message`) covering: what changed (files), the exact
  gate commands run + their results, tests added, known gaps / risks, and the
  manual-test checklist for the human. State failures plainly — do not claim
  green you did not observe.
