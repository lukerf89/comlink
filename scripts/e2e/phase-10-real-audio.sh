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

ffmpeg_path="${COMLINK_FFMPEG:-$(command -v ffmpeg || true)}"
ffprobe_path="${COMLINK_FFPROBE:-$(command -v ffprobe || true)}"
whisper_path="${COMLINK_WHISPER_CPP:-$(command -v whisper-cli || true)}"

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
[ -n "$ffmpeg_path" ] && [ -x "$ffmpeg_path" ] || skip "ffmpeg not found (set COMLINK_FFMPEG)"
[ -n "$whisper_path" ] && [ -x "$whisper_path" ] || skip "whisper-cli not found (set COMLINK_WHISPER_CPP)"
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

# 1) doctor must be fully healthy with the real stack configured.
run_comlink doctor --format json > "$artifact_dir/doctor.json" 2> "$artifact_dir/doctor.err"

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


# --- json ---
data = json.loads((artifact_dir / "transcribe.json").read_text())
required = {"text", "final_text", "engine", "model", "duration_ms", "segments", "source"}
missing = required - set(data)
if missing:
    raise SystemExit(f"json missing keys: {sorted(missing)}")
if data["engine"] != "whisper.cpp":
    raise SystemExit(f"unexpected engine: {data['engine']}")
if not data["segments"]:
    raise SystemExit("json segments must not be empty")
for seg in data["segments"]:
    if not {"start_ms", "end_ms", "text"} <= set(seg):
        raise SystemExit(f"segment missing timing/text keys: {sorted(seg)}")
require_pangram("json.final_text", data["final_text"])
require_pangram("json.text", data["text"])

# --- jsonl: every line must parse; concatenation must contain the pangram ---
lines = [l for l in (artifact_dir / "transcribe.jsonl").read_text().splitlines() if l.strip()]
records = [json.loads(l) for l in lines]
if not records:
    raise SystemExit("jsonl produced no records")
require_pangram("jsonl", " ".join(json.dumps(r) for r in records))

# --- md ---
md = (artifact_dir / "transcribe.md").read_text()
for marker in ("# Comlink Transcript", "## Metadata"):
    if marker not in md:
        raise SystemExit(f"md missing section: {marker!r}")
require_pangram("md", md)

# --- text ---
require_pangram("text", (artifact_dir / "transcribe.text").read_text())

print("real-audio transcription verified across text/json/jsonl/md")
PY

echo "Phase 10 real-audio E2E passed. Artifacts: $artifact_dir"
