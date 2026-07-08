#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
artifact_dir="$repo_root/docs/validation/artifacts/phase-8"
tmp_dir="$(mktemp -d)"

cleanup() {
  python3 - "$tmp_dir" "$repo_root" "$artifact_dir" <<'PY' || true
import pathlib
import sys

tmp_dir = sys.argv[1]
repo_root = sys.argv[2]
artifact_dir = pathlib.Path(sys.argv[3])
for path in artifact_dir.glob("*"):
    if path.is_file():
        text = path.read_text(errors="ignore")
        path.write_text(text.replace(tmp_dir, "<tmp>").replace(repo_root, "<repo>"))
PY
  rm -rf "$tmp_dir"
}

trap cleanup EXIT

mkdir -p "$artifact_dir"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required for the Phase 8 E2E suite" >&2
  exit 1
fi

cargo build --quiet
binary="$repo_root/target/debug/comlink"

mock_ffmpeg="$tmp_dir/mock-ffmpeg"
mock_ffprobe="$tmp_dir/mock-ffprobe"
mock_whisper="$tmp_dir/mock-whisper"
mock_pbcopy="$tmp_dir/mock-pbcopy"
mock_pbpaste="$tmp_dir/mock-pbpaste"
mock_model="$tmp_dir/mock-model.bin"
clipboard_file="$tmp_dir/clipboard.txt"
comlink_home="$tmp_dir/comlink-home"
comlink_data="$tmp_dir/comlink-data"

cat > "$mock_ffmpeg" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

case " $* " in
  *" -list_devices true "*)
    {
      echo "[AVFoundation indev @ 0x1] AVFoundation video devices:"
      echo "[AVFoundation indev @ 0x1] [0] FaceTime HD Camera"
      echo "[AVFoundation indev @ 0x1] AVFoundation audio devices:"
      if [ "${COMLINK_MOCK_BLACKHOLE:-0}" = "1" ]; then
        echo "[AVFoundation indev @ 0x1] [0] BlackHole 2ch"
        echo "[AVFoundation indev @ 0x1] [1] MacBook Pro Microphone"
      else
        echo "[AVFoundation indev @ 0x1] [0] MacBook Pro Microphone"
      fi
    } >&2
    exit 1
    ;;
esac

exit 0
SH
chmod +x "$mock_ffmpeg"

cat > "$mock_ffprobe" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '0\n'
SH
chmod +x "$mock_ffprobe"

cat > "$mock_whisper" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
exit 0
SH
chmod +x "$mock_whisper"

cat > "$mock_pbcopy" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
cat > "$COMLINK_MOCK_CLIPBOARD"
SH
chmod +x "$mock_pbcopy"

cat > "$mock_pbpaste" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
cat "$COMLINK_MOCK_CLIPBOARD"
SH
chmod +x "$mock_pbpaste"

printf 'mock model\n' > "$mock_model"
printf 'previous clipboard\n' > "$clipboard_file"

echo "binary: $binary"
echo "config: $comlink_home/config.json"
echo "data: $comlink_data"
echo "artifacts: $artifact_dir"

run_doctor() {
  local label="$1"
  local expected_status="$2"
  local expected_available="$3"
  shift 3

  env \
    COMLINK_HOME="$comlink_home" \
    COMLINK_DATA_DIR="$comlink_data" \
    COMLINK_FFMPEG="$mock_ffmpeg" \
    COMLINK_FFPROBE="$mock_ffprobe" \
    COMLINK_WHISPER_CPP="$mock_whisper" \
    COMLINK_WHISPER_MODEL="$mock_model" \
    COMLINK_PBCOPY="$mock_pbcopy" \
    COMLINK_PBPASTE="$mock_pbpaste" \
    COMLINK_MOCK_CLIPBOARD="$clipboard_file" \
    COMLINK_LLM_ENABLED=false \
    "$@" \
    "$binary" doctor --format json \
    > "$artifact_dir/$label-doctor.json" \
    2> "$artifact_dir/$label-doctor.err"

  if [ -s "$artifact_dir/$label-doctor.err" ]; then
    echo "doctor emitted unexpected stderr for $label" >&2
    cat "$artifact_dir/$label-doctor.err" >&2
    exit 1
  fi

  python3 - "$artifact_dir/$label-doctor.json" "$expected_status" "$expected_available" <<'PY'
import json
import sys

path, expected_status, expected_available = sys.argv[1:4]
data = json.load(open(path))

if data["schema_version"] != "comlink.doctor.v1":
    raise SystemExit(f"unexpected doctor schema: {data['schema_version']}")
if "system_audio" not in data:
    raise SystemExit("doctor JSON missing additive system_audio field")

system_audio = data["system_audio"]
expected_available_bool = expected_available == "true"
if system_audio["status"] != expected_status:
    raise SystemExit(f"unexpected system audio status: {system_audio}")
if system_audio["available"] is not expected_available_bool:
    raise SystemExit(f"unexpected system audio availability: {system_audio}")
if system_audio["strategy"] != "blackhole-virtual-audio-device":
    raise SystemExit(f"unexpected strategy: {system_audio['strategy']}")
if not system_audio["remediation"]:
    raise SystemExit("system audio remediation must be actionable")

checks = {check["name"]: check for check in data["checks"]}
if "system-audio" not in checks:
    raise SystemExit("doctor checks missing system-audio")
if checks["system-audio"]["required"] is not False:
    raise SystemExit("system-audio check must remain non-required in Phase 8")
if checks["system-audio"]["status"] != expected_status:
    raise SystemExit(f"system-audio check disagrees with report: {checks['system-audio']}")

labels = set(system_audio["source_metadata"]["labels"])
if {"user_mic", "system_audio", "mixed"} - labels:
    raise SystemExit(f"source metadata labels incomplete: {labels}")
if system_audio["dependency"]["name"] != "BlackHole virtual audio device":
    raise SystemExit(f"unexpected dependency: {system_audio['dependency']}")
PY
}

run_doctor "missing" "missing-dependency" "false"
run_doctor "available" "ok" "true" COMLINK_MOCK_BLACKHOLE=1
run_doctor "fake-wrong-os" "unsupported-os" "false" COMLINK_SYSTEM_AUDIO_FAKE=wrong-os
run_doctor "fake-probe-error" "probe-error" "false" COMLINK_SYSTEM_AUDIO_FAKE=probe-error

echo "Phase 8 system-audio spike E2E passed"
