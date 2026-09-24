#!/usr/bin/env bash
# LF-80 (Phase 6 fast-follow): doctor stub-model warning, opt-in
# `doctor --probe-mic`, record near-silent diagnostics, and editor/outlook/
# terminal layout-directive punctuation. Hermetic: every external tool is a
# mock, no network, isolated COMLINK_HOME/COMLINK_DATA_DIR.
set -euo pipefail

export COMLINK_RECORD_DEVICE="${COMLINK_RECORD_DEVICE:-:0}"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
silence="$repo_root/tests/fixtures/audio/silence.wav"
speech="$repo_root/tests/fixtures/audio/short.wav"
artifact_dir="$repo_root/docs/validation/artifacts/phase-6b"
tmp_dir="$(mktemp -d)"

cleanup() {
  python3 - "$tmp_dir" "$repo_root" "$artifact_dir" <<'PY' || true
import pathlib
import sys

tmp_dir, repo_root, artifact_dir = sys.argv[1], sys.argv[2], pathlib.Path(sys.argv[3])
for path in artifact_dir.glob("*"):
    if path.is_file():
        text = path.read_text(errors="ignore")
        path.write_text(text.replace(tmp_dir, "<tmp>").replace(repo_root, "<repo>"))
PY
  rm -rf "$tmp_dir"
}
trap cleanup EXIT

rm -rf "$artifact_dir"
mkdir -p "$artifact_dir"

for required in cargo python3; do
  if ! command -v "$required" >/dev/null 2>&1; then
    echo "$required is required for the Phase 6b E2E suite" >&2
    exit 1
  fi
done
for fixture in "$silence" "$speech"; do
  [ -f "$fixture" ] || { echo "missing fixture $fixture" >&2; exit 1; }
done

(cd "$repo_root" && cargo build --quiet)
binary="$repo_root/target/debug/comlink"

mock_ffmpeg="$tmp_dir/mock-ffmpeg"
mock_ffprobe="$tmp_dir/mock-ffprobe"
mock_whisper="$tmp_dir/mock-whisper"
mock_pbcopy="$tmp_dir/mock-pbcopy"
mock_pbpaste="$tmp_dir/mock-pbpaste"
real_model="$tmp_dir/ggml-base.bin"
stub_model="$tmp_dir/ggml-stub.bin"
argv_log="$tmp_dir/ffmpeg-argv.log"
comlink_home="$tmp_dir/comlink-home"
comlink_data="$tmp_dir/comlink-data"

cat > "$mock_ffmpeg" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "$*" >> "$COMLINK_MOCK_ARGV_LOG"
case " $* " in
  *" -list_devices true "*)
    {
      echo "[AVFoundation indev @ 0x1] AVFoundation video devices:"
      echo "[AVFoundation indev @ 0x1] [0] FaceTime HD Camera"
      echo "[AVFoundation indev @ 0x1] AVFoundation audio devices:"
      echo "[AVFoundation indev @ 0x1] [0] BlackHole 2ch"
      echo "[AVFoundation indev @ 0x1] [1] MacBook Pro Microphone"
    } >&2
    exit 1
    ;;
esac
out="${@: -1}"
case "${COMLINK_MOCK_CAPTURE_MODE:-copy}" in
  hang) sleep 30 ;;
  fail) echo "mock avfoundation: Input/output error opening device" >&2; exit 5 ;;
  *) cp "$COMLINK_MOCK_CAPTURE_WAV" "$out" ;;
esac
read -r _ || true
exit 0
SH
cat > "$mock_ffprobe" <<'SH'
#!/usr/bin/env bash
printf '2.0\n'
SH
cat > "$mock_whisper" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
out=""
while [ "$#" -gt 0 ]; do
  case "$1" in -of) shift; out="$1" ;; esac
  shift || true
done
printf '%s' "${COMLINK_MOCK_TRANSCRIPT:-}" > "$out.txt"
SH
printf '#!/usr/bin/env bash\ncat > /dev/null\n' > "$mock_pbcopy"
printf '#!/usr/bin/env bash\nprintf ""\n' > "$mock_pbpaste"
chmod +x "$mock_ffmpeg" "$mock_ffprobe" "$mock_whisper" "$mock_pbcopy" "$mock_pbpaste"
python3 -c 'import sys; open(sys.argv[1], "wb").truncate(16 << 20)' "$real_model"
head -c 1024 /dev/zero > "$stub_model"
: > "$argv_log"

echo "binary: $binary"
echo "isolated COMLINK_HOME: $comlink_home"
echo "isolated COMLINK_DATA_DIR: $comlink_data"
echo "artifacts: $artifact_dir"

run_comlink() {
  env \
    COMLINK_HOME="$comlink_home" \
    COMLINK_DATA_DIR="$comlink_data" \
    COMLINK_FFMPEG="$mock_ffmpeg" \
    COMLINK_FFPROBE="$mock_ffprobe" \
    COMLINK_WHISPER_CPP="$mock_whisper" \
    COMLINK_WHISPER_MODEL="${MODEL:-$real_model}" \
    COMLINK_PBCOPY="$mock_pbcopy" \
    COMLINK_PBPASTE="$mock_pbpaste" \
    COMLINK_LLM_ENABLED=false \
    COMLINK_MOCK_ARGV_LOG="$argv_log" \
    COMLINK_MOCK_CAPTURE_WAV="${CAPTURE:-$silence}" \
    COMLINK_MOCK_CAPTURE_MODE="${CAPTURE_MODE:-copy}" \
    COMLINK_MOCK_TRANSCRIPT="${TRANSCRIPT:-}" \
    "$binary" "$@"
}

assert_empty_stderr() {
  if [ -s "$1" ]; then
    echo "unexpected stderr for $2:" >&2
    cat "$1" >&2
    exit 1
  fi
}

# assert_json FILE PYTHON_EXPR... (each expression must be truthy on `d`).
# The expressions are literals written in this script (never external input),
# so eval is used deliberately as a tiny assertion DSL.
assert_json() {
  local file="$1"
  shift
  python3 - "$file" "$@" <<'PY'
import json
import sys

d = json.load(open(sys.argv[1]))
checks = {c["name"]: c for c in d.get("checks", [])}
for expr in sys.argv[2:]:
    if not eval(expr, {"d": d, "checks": checks}):
        raise SystemExit(f"{sys.argv[1]}: assertion failed: {expr}\n{json.dumps(d, indent=2)[:4000]}")
PY
}

# 1) Stub model: warn, still ok=true, exit 0.
MODEL="$stub_model" run_comlink doctor --format json \
  > "$artifact_dir/doctor-stub.json" 2> "$artifact_dir/doctor-stub.err"
assert_empty_stderr "$artifact_dir/doctor-stub.err" "doctor stub json"
assert_json "$artifact_dir/doctor-stub.json" \
  'd["ok"] is True' \
  'checks["model-path"]["status"] == "warn"' \
  'checks["model-path"]["required"] is True' \
  '"test stub" in checks["model-path"]["detail"]'
MODEL="$stub_model" run_comlink doctor \
  > "$artifact_dir/doctor-stub-text.out" 2> "$artifact_dir/doctor-stub-text.err"
grep -q "\[warn\] model-path" "$artifact_dir/doctor-stub-text.err"
grep -q "test stub" "$artifact_dir/doctor-stub-text.err"

# 2) Real-sized model + plain doctor: model ok, microphone info, no capture.
: > "$argv_log"
run_comlink doctor --format json > "$artifact_dir/doctor-plain.json" 2> "$artifact_dir/doctor-plain.err"
assert_empty_stderr "$artifact_dir/doctor-plain.err" "doctor plain json"
assert_json "$artifact_dir/doctor-plain.json" \
  'd["ok"] is True' \
  'checks["model-path"]["status"] == "ok"' \
  'checks["microphone"]["status"] == "info"' \
  '"--probe-mic" in checks["microphone"]["remediation"]'
if grep -q "pcm_s16le" "$argv_log"; then
  echo "plain doctor must not capture audio" >&2
  cat "$argv_log" >&2
  exit 1
fi
cp "$argv_log" "$artifact_dir/doctor-plain-ffmpeg-argv.log"

# 3) --probe-mic: silence -> warn, signal -> ok, hang -> bounded warn, fail -> warn.
run_comlink doctor --format json --probe-mic \
  > "$artifact_dir/probe-silence.json" 2> "$artifact_dir/probe-silence.err"
assert_empty_stderr "$artifact_dir/probe-silence.err" "probe silence"
assert_json "$artifact_dir/probe-silence.json" \
  'd["ok"] is True' \
  'checks["microphone"]["status"] == "warn"' \
  '"no signal from device" in checks["microphone"]["detail"]'

CAPTURE="$speech" run_comlink doctor --format json --probe-mic \
  > "$artifact_dir/probe-signal.json" 2> "$artifact_dir/probe-signal.err"
assert_json "$artifact_dir/probe-signal.json" 'checks["microphone"]["status"] == "ok"'

started=$(date +%s)
CAPTURE_MODE=hang run_comlink doctor --format json --probe-mic \
  > "$artifact_dir/probe-hang.json" 2> "$artifact_dir/probe-hang.err"
elapsed=$(( $(date +%s) - started ))
if [ "$elapsed" -gt 8 ]; then
  echo "hanging probe took ${elapsed}s (expected <= 8s)" >&2
  exit 1
fi
assert_json "$artifact_dir/probe-hang.json" \
  'd["ok"] is True' \
  '"timed out" in checks["microphone"]["detail"]'

CAPTURE_MODE=fail run_comlink doctor --format json --probe-mic \
  > "$artifact_dir/probe-fail.json" 2> "$artifact_dir/probe-fail.err"
assert_json "$artifact_dir/probe-fail.json" \
  'checks["microphone"]["status"] == "warn"' \
  '"Input/output error" in checks["microphone"]["detail"]'

# 4) record near-silent with text: exit 0, JSON warning, hint on stderr.
printf '\n' | TRANSCRIPT="you you you" run_comlink record --format json --mode raw \
  > "$artifact_dir/record-near-silent.json" 2> "$artifact_dir/record-near-silent.err"
assert_json "$artifact_dir/record-near-silent.json" \
  'any("near-silent" in w for w in d["warnings"])'
grep -q "COMLINK_RECORD_DEVICE" "$artifact_dir/record-near-silent.err"
grep -q "MacBook Pro Microphone" "$artifact_dir/record-near-silent.err"

# 5) record near-silent with empty transcript: exit 4, hint, empty stdout.
set +e
printf '\n' | run_comlink record --format json \
  > "$artifact_dir/record-near-silent-empty.out" 2> "$artifact_dir/record-near-silent-empty.err"
status=$?
set -e
if [ "$status" -ne 4 ]; then
  echo "expected exit 4 for near-silent empty transcript, got $status" >&2
  cat "$artifact_dir/record-near-silent-empty.err" >&2
  exit 1
fi
if [ -s "$artifact_dir/record-near-silent-empty.out" ]; then
  echo "stdout must stay empty on error" >&2
  exit 1
fi
grep -q "no speech transcribed" "$artifact_dir/record-near-silent-empty.err"
grep -q "doctor --probe-mic" "$artifact_dir/record-near-silent-empty.err"

# 6) record normal speech: no near-silent warning anywhere.
printf '\n' | CAPTURE="$speech" TRANSCRIPT="Comlink phase zero fixture." \
  run_comlink record --format json --mode raw \
  > "$artifact_dir/record-speech.json" 2> "$artifact_dir/record-speech.err"
assert_json "$artifact_dir/record-speech.json" \
  'not any("near-silent" in w for w in d["warnings"])'
if grep -q "near-silent" "$artifact_dir/record-speech.err"; then
  echo "normal speech must not warn near-silent" >&2
  exit 1
fi

# 7) Layout directives absorb ASR punctuation.
run_comlink modes apply --mode editor --text "first line, new line, second line." --format json \
  > "$artifact_dir/mode-editor.json" 2> "$artifact_dir/mode-editor.err"
assert_empty_stderr "$artifact_dir/mode-editor.err" "modes apply editor"
assert_json "$artifact_dir/mode-editor.json" 'd["final_text"] == "first line\nsecond line."'
run_comlink modes apply --mode outlook --text "thanks, new paragraph, I can review." --format json \
  > "$artifact_dir/mode-outlook.json" 2> "$artifact_dir/mode-outlook.err"
assert_json "$artifact_dir/mode-outlook.json" 'd["final_text"] == "thanks\n\nI can review."'
run_comlink modes apply --mode terminal --text "cargo test, new line, git status." --format json \
  > "$artifact_dir/mode-terminal.json" 2> "$artifact_dir/mode-terminal.err"
assert_json "$artifact_dir/mode-terminal.json" 'd["final_text"] == "cargo test && git status"'

# 8) Phase 6 compatibility.
"$repo_root/scripts/e2e/phase-6-workday-surface-hardening.sh" > "$artifact_dir/phase-6-rerun.log" 2>&1

echo "Phase 6b E2E passed"
