# Phase 6b Validation (LF-80: doctor false-OK, diagnostics, editor new line)

Date: 2026-09-23

Fast-follow to Phase 6 (see PRs #7, #12, #13, #14). Scope:

1. `doctor` no longer reports a test-stub whisper model as `ok`.
2. `doctor --probe-mic` (opt-in) runs a short live capture and reports whether
   the record input device has signal.
3. `record` detects near-silent captures, warns with an actionable device hint,
   and turns an empty transcript from near-silent audio into a specific error.
4. `record`, `meet start` and `doctor` share one microphone device resolver.
5. Dictated `new line` / `new paragraph` directives absorb the punctuation ASR
   adds around them in `editor`, `outlook` and `terminal` modes.

## Commands run

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --all
cargo run -- doctor
scripts/e2e/phase-6b-doctor-diagnostics.sh
scripts/e2e/phase-{0,1,3,4,6,8,9}-*.sh   # regression re-runs
```

Results (observed on macOS, worktree off `main` 97988d2):

- `cargo fmt --check` passed.
- `cargo clippy --all-targets -- -D warnings` passed.
- `cargo test --all` passed: 117 lib unit tests, 12 `tests/doctor_diagnostics.rs`,
  7 `tests/meet_lifecycle.rs`, 2 `tests/audio_levels.rs`.
- `cargo run -- doctor` exited 0. A real 141 MB `ggml-base.en.bin` reports `[ok]`.
- `scripts/e2e/phase-6b-doctor-diagnostics.sh` passed. It also re-runs Phase 6.
  Artifacts are in `docs/validation/artifacts/phase-6b/`.
- Phase 0, 1, 3, 4, 8 and 9 E2E scripts still pass.

## Decisions

### Stub model detection

- A model file is a stub when its size is `< 10,000,000` bytes (strict less-than,
  so exactly 10,000,000 bytes is `ok`), or when its **file name** contains
  `for-tests-`. The whisper.cpp `for-tests-*` models are about 562 KB and the
  smallest real ggml model (tiny q5_1) is about 31 MB, so the threshold has wide
  margin on both sides. A `for-tests-` segment in a parent directory is ignored.
- A stub is reported as `status: "warn"` with `required: true`, a detail such
  as `looks like a test stub / not a real model (N bytes)`, and the remediation
  `comlink models select <name> --path <real ggml model>`.
- If the file's metadata can't be read, the check is `warn` with
  `could not read model file size`.
- A smoke transcription was rejected: it is too slow for `doctor` and not
  hermetic.

### `warn` status semantics (additive to `comlink.doctor.v1`)

- `warn` is a new, additive status value. Top-level `ok` is now
  `every required check is ok or warn`, so a `warn` never changes the exit code.
- Consumers, including the LF-162 MCP, should treat `warn` as
  **degraded-but-usable**.
- The text report prints a `[warn]` marker and the check's detail line.

### Mic probe (`doctor --probe-mic`)

- Opt-in only. Plain `doctor`, the CI gate and every E2E suite never open the
  microphone. Plain doctor still runs `ffmpeg -list_devices` to resolve names,
  as it did before; `tests/doctor_diagnostics.rs` asserts that it never starts
  a capture.
- The probe is a `MicProbe` trait with an `FfmpegMicProbe` adapter, so unit
  tests use fakes.
- Capture: 1.5 s of 16 kHz mono `pcm_s16le` WAV into a `TempDir`, which is
  removed on every path.
- Deadline: 5 s using `try_wait` polling, then `kill()` + `wait()`, so the child
  is always reaped and never left as a zombie.
- Stderr is kept up to its last 4 KB. The reader thread is joined once it
  reports. If an orphaned grandchild keeps the pipe open, the probe waits only
  250 ms for the reader, so a hang is bounded at about 5.3 s.
- Result mapping (the check is always `required: false`):

  | Probe result | Status | Detail |
  | --- | --- | --- |
  | signal | `ok` | mean/peak dBFS |
  | near-silent | `warn` | `no signal from device <name> (<selector>) (mean N dBFS…)`, plus the device hint |
  | capture failed | `warn` | reason and stderr tail |
  | timed out | `warn` | reason |
  | unmeasurable | `warn` | reason |

### Shared device resolution

- `record::resolve_record_device` now owns the precedence:
  1. `--device`
  2. `COMLINK_RECORD_DEVICE`
  3. the CoreAudio default input, mapped through the AVFoundation list
  4. `:0`
- It prints nothing. The `cli.rs` wrapper keeps the existing
  `Using system default input device ...` stderr line, so the `meet start` call
  site is unchanged.
- This fixes a divergence: `doctor` used to echo the raw
  `COMLINK_RECORD_DEVICE` value. It now resolves it the way `record` does, and a
  name becomes `Name (:N)`.
- If resolution fails, the microphone check becomes `warn`, never a doctor
  failure.

### Record near-silent handling

- After the minimum-duration check, the captured WAV is measured with the
  meet-path helpers. The threshold is the mean at or below -60 dBFS.
- If the capture is near-silent:
  - stderr gets the warning plus a device hint. The hint names the device,
    lists the AVFoundation devices (best effort), and suggests
    `COMLINK_RECORD_DEVICE=":N"`, `--device`, and System Settings > Privacy &
    Security > Microphone.
  - A transcript that succeeds carries the warning in `warnings`, so it appears
    in JSON, JSONL and Markdown. stdout stays data-only.
  - An `EmptyTranscript` becomes `NoSpeechNearSilent`. It keeps **exit code 4**
    and names `COMLINK_RECORD_DEVICE` and `doctor --probe-mic`.
- If the WAV can't be measured (not 16-bit PCM), behavior is exactly as before.

### Layout directives

- Directives are first replaced by private-use sentinels. Any U+E000/U+E001
  already in the input is stripped first.
- Before a directive: whitespace and `, ; :` are dropped, and a terminal
  `. ? !` is kept.
- After a directive: whitespace and `, ; : .` are dropped unless the mark
  starts a token such as `.gitignore`. `? !` are kept.
- Adjacent directives collapse to the strongest one (paragraph beats line).
- Sentinels become `\n` / `\n\n` before terminal mode splits lines with `&&`.

### Phase 6 script compatibility

`scripts/e2e/phase-6-workday-surface-hardening.sh` now writes a sparse 16 MiB
mock model, so its `model-path == ok` assertion keeps its meaning.

## Known gaps

- No ggml header or magic validation: the model check uses only size and file
  name. A heavily quantized custom model under 10 MB would `warn`. This is
  acceptable because `warn` is non-fatal and the threshold is documented.
- The probe cannot tell a macOS TCC denial apart from a dead or muted device.
  All of them read as near-silent or failed capture.
- The first real `--probe-mic` run may trigger the macOS microphone permission
  prompt. The 5 s deadline turns an unanswered prompt into `timed out`.
- `new line new line` collapses to a single line break by design.
- Running the Phase 6b script also re-runs Phase 6, which rewrites the Phase 6
  artifacts.

## Manual test instructions (human gate)

1. **Stub model:**
   - Run `curl -L -o /tmp/ggml-for-tests-tiny.bin https://huggingface.co/ggerganov/whisper.cpp/resolve/main/for-tests-ggml-tiny.bin`,
     or use any file under 10 MB.
   - Run `COMLINK_WHISPER_MODEL=/tmp/ggml-for-tests-tiny.bin cargo run -- doctor`.
   - Expect `[warn] model-path` followed by a `test stub` detail line, and exit
     code 0 (`echo $?`).
2. **Real model:** run `cargo run -- doctor`. Expect `[ok] model-path` and a
   microphone `[info]` line that mentions `--probe-mic`.
3. **Live mic, unmuted:**
   - Run `cargo run -- doctor --probe-mic` while speaking.
   - Expect `[ok] microphone` or the JSON detail `live probe heard signal`.
   - Approve the macOS permission prompt if it appears.
4. **Live mic, muted:**
   - Mute the input, or set the input volume to 0 in Sound settings.
   - Run `cargo run -- doctor --probe-mic --format json`.
   - Expect the microphone check to be `warn` with `no signal from device`, and
     `ok: true`.
5. **Wrong record device:**
   - Pick a silent input such as BlackHole: `COMLINK_RECORD_DEVICE="BlackHole 2ch" cargo run -- record --format json`.
   - Speak, then press Enter.
   - Expect a stderr near-silent warning plus the device list and
     `COMLINK_RECORD_DEVICE` hint.
   - Expect either JSON `warnings` containing `near-silent`, or exit 4 with
     `no speech transcribed: ... near-silent`.
6. **Editor dictation:**
   - Run `cargo run -- record --mode editor` and say "first line, new line,
     second line, new paragraph, third line".
   - Expect three blocks, with no stray commas at line starts or ends.
