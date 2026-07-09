#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
artifact_dir="$repo_root/docs/validation/artifacts/phase-9"
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
  echo "cargo is required for the Phase 9 E2E suite" >&2
  exit 1
fi

cargo build --quiet
binary="$repo_root/target/debug/comlink"

mock_ffmpeg="$tmp_dir/mock-ffmpeg"
mock_ffprobe="$tmp_dir/mock-ffprobe"
mock_whisper="$tmp_dir/mock-whisper"
mock_model="$tmp_dir/mock-model.bin"
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
      if [ "${COMLINK_MOCK_BLACKHOLE:-1}" = "1" ]; then
        echo "[AVFoundation indev @ 0x1] [0] BlackHole 2ch"
        echo "[AVFoundation indev @ 0x1] [1] MacBook Pro Microphone"
      else
        echo "[AVFoundation indev @ 0x1] [0] MacBook Pro Microphone"
      fi
    } >&2
    exit 1
    ;;
esac

out="${@: -1}"
input=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -i)
      shift
      input="${1:-}"
      ;;
  esac
  shift || true
done

if [ "${COMLINK_MOCK_CAPTURE_FAIL:-0}" = "1" ]; then
  echo "mock FFmpeg capture failed for input $input" >&2
  exit 8
fi

chunks="${COMLINK_MOCK_MEETING_CHUNKS:-2}"
mkdir -p "$(dirname "$out")"
for index in $(seq 0 $((chunks - 1))); do
  chunk="$(printf "$out" "$index")"
  printf 'mock wav %s %s\n' "$input" "$index" > "$chunk"
done

trap 'exit 0' INT TERM
while true; do
  sleep 1
done
SH
chmod +x "$mock_ffmpeg"

cat > "$mock_ffprobe" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
printf '%s\n' "${COMLINK_MOCK_DURATION_SECONDS:-30}"
SH
chmod +x "$mock_ffprobe"

cat > "$mock_whisper" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

out=""
wav=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -of)
      shift
      out="$1"
      ;;
    -f)
      shift
      wav="$1"
      ;;
  esac
  shift || true
done

if [ -z "$out" ]; then
  echo "missing -of" >&2
  exit 2
fi

source_label="$(basename "$(dirname "$wav")")"
if [ "$source_label" = "chunks" ]; then
  source_label="user_mic"
fi
index="$(basename "$wav" .wav)"
index="${index#chunk-}"
printf '%s Teams fixture segment %s.\n' "$source_label" "$index" > "$out.txt"
SH
chmod +x "$mock_whisper"

printf 'mock model\n' > "$mock_model"

echo "binary: $binary"
echo "config: $comlink_home/config.json"
echo "data: $comlink_data"
echo "artifacts: $artifact_dir"

run_comlink() {
  COMLINK_HOME="$comlink_home" \
  COMLINK_DATA_DIR="$comlink_data" \
  COMLINK_FFMPEG="$mock_ffmpeg" \
  COMLINK_FFPROBE="$mock_ffprobe" \
  COMLINK_WHISPER_CPP="$mock_whisper" \
  COMLINK_WHISPER_MODEL="$mock_model" \
  COMLINK_LLM_ENABLED=false \
  "$binary" "$@"
}

wait_for_chunks() {
  local dir="$1"
  local expected="$2"
  for _ in $(seq 1 100); do
    count="$(find "$dir" -maxdepth 1 -name 'chunk-*.wav' 2>/dev/null | wc -l | tr -d ' ')"
    if [ "$count" = "$expected" ]; then
      return 0
    fi
    sleep 0.1
  done
  echo "timed out waiting for $expected chunks in $dir" >&2
  find "$dir" -maxdepth 2 -type f -print >&2 || true
  return 1
}

validate_online_capture() {
  local label="$1"
  local expected_audio_retained="$2"
  local expected_labels_csv="$3"

  python3 - "$artifact_dir" "$label" "$expected_audio_retained" "$expected_labels_csv" <<'PY'
import json
import pathlib
import sys

artifact_dir = pathlib.Path(sys.argv[1])
label = sys.argv[2]
expected_audio_retained = sys.argv[3] == "true"
expected_labels = sys.argv[4].split(",")

start = json.load(open(artifact_dir / f"{label}-start.json"))
stop = json.load(open(artifact_dir / f"{label}-stop.json"))
export = json.load(open(artifact_dir / f"{label}-export.json"))
markdown = (artifact_dir / f"{label}-export.md").read_text()
segments_jsonl_path = pathlib.Path(stop["artifacts"]["segments_jsonl"])
records = [json.loads(line) for line in segments_jsonl_path.read_text().splitlines()]

if start["schema_version"] != "comlink.meeting.v1":
    raise SystemExit("bad start schema")
if start["status"] != "recording" or stop["status"] != "stopped":
    raise SystemExit("bad lifecycle status")
if start["source"]["mode"] != "mic-plus-system":
    raise SystemExit(f"unexpected source mode: {start['source']}")
if export["schema_version"] != "comlink.meeting.v1":
    raise SystemExit("bad export schema")
if "session" not in export or "retention" not in export or "source" not in export:
    raise SystemExit("export missing session, retention, or source metadata")
if export["retention"]["audio"] is not expected_audio_retained:
    raise SystemExit(f"unexpected audio retention: {export['retention']}")
if export["session"]["source_mode"] != "mic-plus-system":
    raise SystemExit("session missing source mode")
if records[0]["record_type"] != "session" or records[0]["source"]["mode"] != "mic-plus-system":
    raise SystemExit("jsonl missing source session record")

segments = export["segments"]
labels = [segment["source_label"] for segment in segments]
for expected in expected_labels:
    if expected not in labels:
        raise SystemExit(f"missing source label {expected}: {labels}")

previous_start = -1
for segment in segments:
    if segment["start_ms"] < previous_start:
        raise SystemExit("segment starts are not monotonic")
    if segment["end_ms"] <= segment["start_ms"]:
        raise SystemExit(f"bad segment duration: {segment}")
    if expected_audio_retained:
        if not segment["chunk_path"]:
            raise SystemExit("retained audio export should include chunk paths")
    else:
        if segment["chunk_path"] is not None:
            raise SystemExit("default non-retained audio export should omit chunk paths")
    previous_start = segment["start_ms"]

jsonl_segments = [record for record in records if record["record_type"] == "segment"]
if len(jsonl_segments) != len(segments):
    raise SystemExit("jsonl segment count mismatch")
if [record["source_label"] for record in jsonl_segments] != labels:
    raise SystemExit("jsonl source labels differ from export")

for text in [
    "# Comlink Meeting Transcript",
    "## Session Metadata",
    "## Retention Policy",
    "Source mode",
]:
    if text not in markdown:
        raise SystemExit(f"markdown missing {text}")
for expected in expected_labels:
    if expected not in markdown:
        raise SystemExit(f"markdown missing label {expected}")

chunks_dir = pathlib.Path(start["chunks_dir"])
if expected_audio_retained:
    if not chunks_dir.exists():
        raise SystemExit(f"retained chunks dir missing: {chunks_dir}")
else:
    if chunks_dir.exists():
        raise SystemExit(f"raw audio chunks should have been deleted: {chunks_dir}")
PY

  cp "$(python3 - "$artifact_dir/$label-stop.json" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1]))["artifacts"]["segments_jsonl"])
PY
)" "$artifact_dir/$label-segments.jsonl"
}

run_meeting_case() {
  local label="$1"
  shift

  COMLINK_MOCK_MEETING_CHUNKS=2 \
  COMLINK_MOCK_DURATION_SECONDS=30 \
  run_comlink meet start "$@" --format json --chunk-seconds 30 --no-llm \
    > "$artifact_dir/$label-start.json" \
    2> "$artifact_dir/$label-start.err"

  python3 - "$artifact_dir/$label-start.json" <<'PY' | while read -r dir; do
import json
import sys
start = json.load(open(sys.argv[1]))
for recorder in start["recorders"]:
    print(recorder["chunks_dir"])
PY
    wait_for_chunks "$dir" 2
  done

  session_id="$(python3 - "$artifact_dir/$label-start.json" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1]))["session_id"])
PY
)"

  COMLINK_MOCK_MEETING_CHUNKS=2 \
  COMLINK_MOCK_DURATION_SECONDS=30 \
  run_comlink meet stop "$session_id" --format json \
    > "$artifact_dir/$label-stop.json" \
    2> "$artifact_dir/$label-stop.err"

  run_comlink meet export "$session_id" --format json > "$artifact_dir/$label-export.json"
  run_comlink meet export "$session_id" --format md > "$artifact_dir/$label-export.md"

  if ! grep -q "Consent reminder" "$artifact_dir/$label-start.err"; then
    echo "start stderr did not include consent reminder for $label" >&2
    cat "$artifact_dir/$label-start.err" >&2
    exit 1
  fi
  if ! grep -q "Stopping meeting recording" "$artifact_dir/$label-stop.err"; then
    echo "stop stderr did not include stop diagnostic for $label" >&2
    cat "$artifact_dir/$label-stop.err" >&2
    exit 1
  fi
}

run_comlink doctor --format json > "$artifact_dir/doctor.json" 2> "$artifact_dir/doctor.err"
run_comlink privacy audit --format json > "$artifact_dir/privacy-audit.json"

python3 - "$artifact_dir/doctor.json" "$artifact_dir/privacy-audit.json" <<'PY'
import json
import sys
doctor = json.load(open(sys.argv[1]))
privacy = json.load(open(sys.argv[2]))
if doctor["system_audio"]["status"] != "ok":
    raise SystemExit(f"doctor did not see BlackHole: {doctor['system_audio']}")
if "permissions" not in doctor["system_audio"]:
    raise SystemExit("doctor missing system_audio permissions")
if privacy["system_audio"]["raw_audio_retained"] is not False:
    raise SystemExit("privacy audit should report default raw audio retention as false")
if privacy["system_audio"]["status"] != "ok":
    raise SystemExit("privacy audit should include system audio diagnostic")
PY

run_meeting_case "mic-plus-system" --source mic-plus-system
validate_online_capture "mic-plus-system" false "user_mic,system_audio"

COMLINK_RETAIN_AUDIO=true run_meeting_case "retained-audio" --source mic-plus-system
COMLINK_RETAIN_AUDIO=true validate_online_capture "retained-audio" true "user_mic,system_audio"

run_meeting_case "mixed" --source mic-plus-system --device "BlackHole 2ch" --system-device "BlackHole 2ch"
validate_online_capture "mixed" false "mixed"

if COMLINK_MOCK_BLACKHOLE=0 run_comlink meet start --source system-only --format json \
  > "$artifact_dir/missing-blackhole-start.json" \
  2> "$artifact_dir/missing-blackhole-start.err"; then
  echo "system-only start unexpectedly succeeded without BlackHole" >&2
  exit 1
fi
if ! grep -Eq "BlackHole|Install" "$artifact_dir/missing-blackhole-start.err"; then
  echo "missing BlackHole diagnostic was not actionable" >&2
  cat "$artifact_dir/missing-blackhole-start.err" >&2
  exit 1
fi

if COMLINK_SYSTEM_AUDIO_DEVICE="MacBook Pro Microphone" run_comlink meet start --source system-only --format json \
  > "$artifact_dir/bad-system-device-start.json" \
  2> "$artifact_dir/bad-system-device-start.err"; then
  echo "system-only start unexpectedly accepted a microphone as system audio" >&2
  exit 1
fi
if ! grep -q "requires a BlackHole input device" "$artifact_dir/bad-system-device-start.err"; then
  echo "bad system device diagnostic was not actionable" >&2
  cat "$artifact_dir/bad-system-device-start.err" >&2
  exit 1
fi

echo "Phase 9 online meeting capture E2E passed"
