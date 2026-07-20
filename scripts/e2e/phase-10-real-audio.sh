#!/usr/bin/env bash
set -euo pipefail

# Phase 10 (supplemental): real-audio end-to-end.
#
# Unlike phases 0-9, this suite exercises the REAL local stack — macOS `say`
# generates a deterministic pangram utterance, real FFmpeg normalizes it, and
# the real whisper.cpp model transcribes it — then asserts the core output
# contract across every format plus a healthy `doctor`.
#
# It requires a real ggml model + whisper-cli + macOS `say`, which are not
# present on every machine (e.g. CI without a model). When any real dependency
# is missing the suite SKIPS (exit 0) with a clear reason instead of failing,
# so it is safe to wire into CI as an opt-in gate.

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
artifact_dir="$repo_root/docs/validation/artifacts/phase-10"
tmp_dir="$(mktemp -d)"

# Resolve a binary the way src/deps.rs does: an env override may be an absolute
# path OR a bare command name resolved through PATH (via `which::which`); with no
# override, fall back to the candidate command names. Prints the resolved
# executable path on success, nothing on failure.
resolve_binary() {
  local override="$1"
  shift
  if [ -n "$override" ]; then
    if [ -f "$override" ] && [ -x "$override" ]; then
      printf '%s\n' "$override"
      return 0
    fi
    case "$override" in
      */*) return 1 ;; # path-like but not an executable file → treated as missing
      *) command -v "$override" 2>/dev/null || return 1 ;;
    esac
    return 0
  fi
  local cand resolved
  for cand in "$@"; do
    if resolved="$(command -v "$cand" 2>/dev/null)"; then
      printf '%s\n' "$resolved"
      return 0
    fi
  done
  return 1
}

# whisper candidates mirror src/deps.rs WHISPER_CPP_CANDIDATES (whisper-cli, whisper.cpp).
ffmpeg_path="$(resolve_binary "${COMLINK_FFMPEG:-}" ffmpeg || true)"
ffprobe_path="$(resolve_binary "${COMLINK_FFPROBE:-}" ffprobe || true)"
whisper_path="$(resolve_binary "${COMLINK_WHISPER_CPP:-}" whisper-cli whisper.cpp || true)"

# Resolve a real model: explicit env first, then common local locations.
model_path="${COMLINK_WHISPER_MODEL:-}"
if [ -z "$model_path" ]; then
  for cand in \
    "$HOME/.local/share/whisper/ggml-medium.en.bin" \
    "$HOME/.local/share/whisper"/ggml-*.bin; do
    if [ -f "$cand" ]; then
      model_path="$cand"
      break
    fi
  done
fi

cleanup() {
  python3 - "$tmp_dir" "$repo_root" "$HOME" "$ffmpeg_path" "$ffprobe_path" "$whisper_path" "$model_path" "$artifact_dir" <<'PY' || true
import pathlib
import sys

tmp_dir, repo_root, home = sys.argv[1], sys.argv[2], sys.argv[3]
ffmpeg_path, ffprobe_path, whisper_path, model_path = sys.argv[4:8]
artifact_dir = pathlib.Path(sys.argv[8])
subs = [
    (tmp_dir, "<tmp>"),
    (repo_root, "<repo>"),
    (model_path, "<model>"),
    (whisper_path, "<whisper>"),
    (ffmpeg_path, "<ffmpeg-path>"),
    (ffprobe_path, "<ffprobe-path>"),
    (home, "<home>"),  # last: broadest, catches any other home-rooted paths
]
for path in artifact_dir.glob("*"):
    if not path.is_file():
        continue
    text = path.read_text(errors="ignore")
    for needle, replacement in subs:
        if needle:
            text = text.replace(needle, replacement)
    path.write_text(text)
PY
  rm -rf "$tmp_dir"
}

trap cleanup EXIT

mkdir -p "$artifact_dir"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required for the Phase 10 real-audio E2E suite" >&2
  exit 1
fi

# Skip (not fail) when the real local stack is unavailable.
skip() {
  echo "SKIP: Phase 10 real-audio E2E — $1" >&2
  exit 0
}
[ -n "$ffmpeg_path" ] || skip "ffmpeg not found (set COMLINK_FFMPEG)"
[ -n "$whisper_path" ] || skip "whisper-cli/whisper.cpp not found (set COMLINK_WHISPER_CPP)"
[ -n "$model_path" ] && [ -f "$model_path" ] || skip "no whisper ggml model found (set COMLINK_WHISPER_MODEL)"
command -v say >/dev/null 2>&1 || skip "macOS 'say' not available for fixture generation"
command -v python3 >/dev/null 2>&1 || skip "python3 not available for validation"

cargo build --quiet
binary="$repo_root/target/debug/comlink"

comlink_home="$tmp_dir/comlink-home"
comlink_data="$tmp_dir/comlink-data"
fixture="$tmp_dir/fixture.wav"

# Deterministic, ASR-friendly utterance. Assertions match on the pangram only
# (brand words like "comlink" are prone to ASR variation and are not asserted).
sentence="Testing comlink transcription. The quick brown fox jumps over the lazy dog."
pangram="quick brown fox jumps over the lazy dog"

echo "binary: $binary"
echo "config: $comlink_home/config.json"
echo "data: $comlink_data"
echo "model: $model_path"
echo "artifacts: $artifact_dir"

say -o "$tmp_dir/src.aiff" "$sentence"
"$ffmpeg_path" -hide_banner -loglevel error -y -i "$tmp_dir/src.aiff" -ac 1 -ar 16000 "$fixture"
if [ ! -s "$fixture" ]; then
  echo "fixture generation produced no audio: $fixture" >&2
  exit 1
fi

run_comlink() {
  COMLINK_HOME="$comlink_home" \
  COMLINK_DATA_DIR="$comlink_data" \
  COMLINK_FFMPEG="$ffmpeg_path" \
  COMLINK_FFPROBE="$ffprobe_path" \
  COMLINK_WHISPER_CPP="$whisper_path" \
  COMLINK_WHISPER_MODEL="$model_path" \
  COMLINK_LLM_ENABLED=false \
  "$binary" "$@"
}

# A healthy real run of `doctor`/`transcribe --no-llm` has no expected stderr
# diagnostics, so any stderr output is a contract regression.
assert_empty_stderr() {
  if [ -s "$1" ]; then
    echo "unexpected stderr from $2:" >&2
    cat "$1" >&2
    exit 1
  fi
}

# 1) doctor must be fully healthy with the real stack configured.
run_comlink doctor --format json > "$artifact_dir/doctor.json" 2> "$artifact_dir/doctor.err"
assert_empty_stderr "$artifact_dir/doctor.err" "doctor --format json"

python3 - "$artifact_dir/doctor.json" <<'PY'
import json
import sys

doctor = json.load(open(sys.argv[1]))
if doctor.get("ok") is not True:
    raise SystemExit(f"doctor not ok: {doctor.get('ok')}")
checks = {c["name"]: c for c in doctor.get("checks", [])}
for name in ("ffmpeg", "asr", "model-path"):
    status = checks.get(name, {}).get("status")
    if status != "ok":
        raise SystemExit(f"required doctor check {name!r} not ok: {status!r}")
PY

# 2) Real transcribe across every output format.
for fmt in text json jsonl md; do
  run_comlink transcribe "$fixture" --mode clean --no-llm --format "$fmt" \
    > "$artifact_dir/transcribe.$fmt" \
    2> "$artifact_dir/transcribe.$fmt.err"
  if [ ! -s "$artifact_dir/transcribe.$fmt" ]; then
    echo "transcribe --format $fmt produced empty stdout" >&2
    exit 1
  fi
  assert_empty_stderr "$artifact_dir/transcribe.$fmt.err" "transcribe --format $fmt"
done

# 3) Assert schema validity + normalized-contains the pangram in each format.
python3 - "$artifact_dir" "$pangram" <<'PY'
import json
import pathlib
import re
import sys

artifact_dir = pathlib.Path(sys.argv[1])
pangram = sys.argv[2]


def norm(text):
    return re.sub(r"\s+", " ", re.sub(r"[^a-z0-9 ]", "", text.lower())).strip()


def require_pangram(label, text):
    if pangram not in norm(text):
        raise SystemExit(f"{label} missing expected pangram; got: {text!r}")


SCHEMA_VERSION = "comlink.session.v1"


def require_int(label, value, *, minimum=None):
    # bool is a subclass of int; reject it explicitly.
    if isinstance(value, bool) or not isinstance(value, int):
        raise SystemExit(f"{label} must be an int, got {value!r}")
    if minimum is not None and value < minimum:
        raise SystemExit(f"{label} must be >= {minimum}, got {value!r}")


def require_segments(label, segments):
    if not isinstance(segments, list) or not segments:
        raise SystemExit(f"{label} must be a non-empty list")
    for i, seg in enumerate(segments):
        for key in ("start_ms", "end_ms", "text"):
            if key not in seg:
                raise SystemExit(f"{label}[{i}] missing {key!r}")
        require_int(f"{label}[{i}].start_ms", seg["start_ms"], minimum=0)
        require_int(f"{label}[{i}].end_ms", seg["end_ms"], minimum=0)
        if seg["end_ms"] < seg["start_ms"]:
            raise SystemExit(f"{label}[{i}] end_ms < start_ms")
        if not isinstance(seg["text"], str):
            raise SystemExit(f"{label}[{i}].text must be a string")


def require_source(label, source):
    for key in ("path", "normalized_sample_rate_hz", "normalized_channels"):
        if key not in source:
            raise SystemExit(f"{label} missing {key!r}")
    require_int(f"{label}.normalized_sample_rate_hz", source["normalized_sample_rate_hz"], minimum=1)
    require_int(f"{label}.normalized_channels", source["normalized_channels"], minimum=1)


def require_context(label, context):
    if context.get("policy") is None:
        raise SystemExit(f"{label} missing 'policy'")
    if not isinstance(context.get("items"), list):
        raise SystemExit(f"{label}.items must be a list")


def require_processing_steps(label, steps):
    if not isinstance(steps, list) or not steps:
        raise SystemExit(f"{label} must be a non-empty list")
    for i, step in enumerate(steps):
        if not isinstance(step.get("name"), str) or not step["name"]:
            raise SystemExit(f"{label}[{i}] missing string 'name'")


# --- json: full comlink.session.v1 session document ---
data = json.loads((artifact_dir / "transcribe.json").read_text())
required = {
    "schema_version", "session_id", "text", "raw_text", "final_text", "mode",
    "copied", "engine", "model", "duration_ms", "segments", "source", "context",
    "processing_steps", "warnings",
}
missing = required - set(data)
if missing:
    raise SystemExit(f"json missing keys: {sorted(missing)}")
if data["schema_version"] != SCHEMA_VERSION:
    raise SystemExit(f"unexpected schema_version: {data['schema_version']!r}")
if data["engine"] != "whisper.cpp":
    raise SystemExit(f"unexpected engine: {data['engine']!r}")
if data["mode"] != "clean":
    raise SystemExit(f"unexpected mode: {data['mode']!r}")
if not isinstance(data["copied"], bool):
    raise SystemExit("json.copied must be a bool")
if not isinstance(data["model"], str) or not data["model"]:
    raise SystemExit("json.model must be a non-empty string")
if not isinstance(data["warnings"], list):
    raise SystemExit("json.warnings must be a list")
# `--no-llm` with a non-history transcribe: history_session_id must be absent/null.
if data.get("history_session_id") is not None:
    raise SystemExit(f"json.history_session_id must be null, got {data['history_session_id']!r}")
require_int("json.duration_ms", data["duration_ms"], minimum=0)
require_segments("json.segments", data["segments"])
require_source("json.source", data["source"])
require_context("json.context", data["context"])
require_processing_steps("json.processing_steps", data["processing_steps"])
require_pangram("json.final_text", data["final_text"])
require_pangram("json.text", data["text"])
require_pangram("json.raw_text", data["raw_text"])

# --- jsonl: every line parses; record types appear in the documented order ---
lines = [l for l in (artifact_dir / "transcribe.jsonl").read_text().splitlines() if l.strip()]
records = [json.loads(l) for l in lines]
if len(records) < 3:
    raise SystemExit(f"jsonl expected session + >=1 segment + transcript, got {len(records)} records")
types = [r.get("record_type") for r in records]
if types[0] != "session":
    raise SystemExit(f"jsonl first record must be 'session', got {types[0]!r}")
if types[-1] != "transcript":
    raise SystemExit(f"jsonl last record must be 'transcript', got {types[-1]!r}")
middle = types[1:-1]
if not middle or any(t != "segment" for t in middle):
    raise SystemExit(f"jsonl middle records must all be 'segment', got {middle!r}")

session_ids = {r.get("session_id") for r in records}
if len(session_ids) != 1 or None in session_ids:
    raise SystemExit(f"jsonl records must share one session_id, got {session_ids!r}")
if any(r.get("schema_version") != SCHEMA_VERSION for r in records):
    raise SystemExit("jsonl records must all carry the session schema_version")

session_rec = records[0]
for key in ("mode", "engine", "model", "duration_ms", "source", "context", "processing_steps", "warnings"):
    if key not in session_rec:
        raise SystemExit(f"jsonl session record missing {key!r}")
require_source("jsonl.session.source", session_rec["source"])
require_processing_steps("jsonl.session.processing_steps", session_rec["processing_steps"])

# Segment records must carry sequential indexes and valid timing.
seg_records = records[1:-1]
for i, rec in enumerate(seg_records):
    if rec.get("segment_index") != i:
        raise SystemExit(f"jsonl segment_index out of order at {i}: {rec.get('segment_index')!r}")
    require_segments(f"jsonl.segment[{i}]", [rec["segment"]])

transcript_rec = records[-1]
for key in ("text", "raw_text", "final_text", "mode", "engine", "model", "duration_ms", "source"):
    if key not in transcript_rec:
        raise SystemExit(f"jsonl transcript record missing {key!r}")
if transcript_rec.get("history_session_id") is not None:
    raise SystemExit("jsonl transcript history_session_id must be null")
require_pangram("jsonl.transcript.final_text", transcript_rec["final_text"])

# --- md ---
md = (artifact_dir / "transcribe.md").read_text()
for marker in ("# Comlink Transcript", "## Metadata", "## Final Text", "## Raw Text", "## Segments"):
    if marker not in md:
        raise SystemExit(f"md missing section: {marker!r}")
if SCHEMA_VERSION not in md:
    raise SystemExit("md missing schema version in metadata")
require_pangram("md", md)

# --- text ---
require_pangram("text", (artifact_dir / "transcribe.text").read_text())

print("real-audio transcription verified across text/json/jsonl/md")
PY

echo "Phase 10 real-audio E2E passed. Artifacts: $artifact_dir"
