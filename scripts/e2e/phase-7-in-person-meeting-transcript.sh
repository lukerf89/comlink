#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fixture="$repo_root/tests/fixtures/audio/short.wav"
artifact_dir="$repo_root/docs/validation/artifacts/phase-7"
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
  echo "cargo is required for the Phase 7 E2E suite" >&2
  exit 1
fi

if [ ! -f "$fixture" ]; then
  "$repo_root/scripts/dev/generate-short-fixture.sh"
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

out="${@: -1}"
chunks="${COMLINK_MOCK_MEETING_CHUNKS:-6}"
fixture="${COMLINK_MOCK_FIXTURE:?fixture required}"

mkdir -p "$(dirname "$out")"
for index in $(seq 0 $((chunks - 1))); do
  chunk="$(printf "$out" "$index")"
  cp "$fixture" "$chunk"
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

name="$(basename "$wav" .wav)"
index="${name#chunk-}"
printf 'Meeting segment %s.\n' "$index" > "$out.txt"
SH
chmod +x "$mock_whisper"

printf 'mock model\n' > "$mock_model"

echo "binary: $binary"
echo "config: $comlink_home/config.json"
echo "data: $comlink_data"
echo "fixture: $fixture"

run_comlink() {
  COMLINK_HOME="$comlink_home" \
  COMLINK_DATA_DIR="$comlink_data" \
  COMLINK_FFMPEG="$mock_ffmpeg" \
  COMLINK_FFPROBE="$mock_ffprobe" \
  COMLINK_WHISPER_CPP="$mock_whisper" \
  COMLINK_WHISPER_MODEL="$mock_model" \
  COMLINK_LLM_ENABLED=false \
  COMLINK_MOCK_FIXTURE="$fixture" \
  "$binary" "$@"
}

session_id_from() {
  python3 - "$1" <<'PY'
import json
import sys

print(json.load(open(sys.argv[1]))["session_id"])
PY
}

chunks_dir_from() {
  python3 - "$1" <<'PY'
import json
import sys

print(json.load(open(sys.argv[1]))["chunks_dir"])
PY
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
  find "$dir" -maxdepth 1 -type f -print >&2 || true
  return 1
}

validate_session() {
  local label="$1"
  local expected_chunks="$2"
  local expected_duration_ms="$3"

  python3 - "$artifact_dir" "$label" "$expected_chunks" "$expected_duration_ms" <<'PY'
import json
import pathlib
import sys

artifact_dir = pathlib.Path(sys.argv[1])
label = sys.argv[2]
expected_chunks = int(sys.argv[3])
expected_duration_ms = int(sys.argv[4])

start = json.load(open(artifact_dir / f"{label}-start.json"))
stop = json.load(open(artifact_dir / f"{label}-stop.json"))
export = json.load(open(artifact_dir / f"{label}-export.json"))
markdown = (artifact_dir / f"{label}-export.md").read_text()

if start["schema_version"] != "comlink.meeting.v1":
    raise SystemExit(f"bad start schema: {start['schema_version']}")
if start["status"] != "recording":
    raise SystemExit(f"start should report recording: {start}")
if stop["status"] != "stopped":
    raise SystemExit(f"stop should report stopped: {stop}")
if stop["chunks_processed"] != expected_chunks:
    raise SystemExit(f"unexpected chunks: {stop['chunks_processed']}")
if stop["duration_ms"] != expected_duration_ms:
    raise SystemExit(f"unexpected duration: {stop['duration_ms']}")
if stop["segment_count"] != expected_chunks:
    raise SystemExit(f"unexpected segment count: {stop['segment_count']}")

if export["schema_version"] != "comlink.meeting.v1":
    raise SystemExit(f"bad export schema: {export['schema_version']}")
if export["session"]["session_id"] != start["session_id"]:
    raise SystemExit("export session id mismatch")
if export["session"]["duration_ms"] != expected_duration_ms:
    raise SystemExit("export duration mismatch")
if "retention" not in export:
    raise SystemExit("export missing retention policy")
if export["retention"] != {"metadata": True, "transcripts": True, "audio": False}:
    raise SystemExit(f"unexpected retention: {export['retention']}")
if export["session"]["inactivity_auto_stop"]["enabled"] is not False:
    raise SystemExit("inactivity auto-stop should be documented as disabled")

segments = export["segments"]
if len(segments) != expected_chunks:
    raise SystemExit(f"expected {expected_chunks} segments, got {len(segments)}")
previous_end = -1
for index, segment in enumerate(segments):
    if segment["segment_index"] != index:
        raise SystemExit(f"segment index mismatch: {segment}")
    if segment["start_ms"] < previous_end:
        raise SystemExit("segment timestamps are not monotonic")
    if segment["end_ms"] <= segment["start_ms"]:
        raise SystemExit(f"segment has non-positive duration: {segment}")
    expected_text = f"Meeting segment {index:05d}."
    if segment["text"] != expected_text:
        raise SystemExit(f"unexpected segment text: {segment['text']!r}")
    previous_end = segment["end_ms"]

jsonl_path = pathlib.Path(stop["artifacts"]["segments_jsonl"])
if not jsonl_path.is_file():
    raise SystemExit(f"missing segments jsonl: {jsonl_path}")
records = [json.loads(line) for line in jsonl_path.read_text().splitlines()]
if records[0]["record_type"] != "session":
    raise SystemExit("jsonl should start with a session record")
if len([record for record in records if record["record_type"] == "segment"]) != expected_chunks:
    raise SystemExit("jsonl segment record count mismatch")

for key in ["json_export", "markdown_export"]:
    if not pathlib.Path(stop["artifacts"][key]).is_file():
        raise SystemExit(f"missing export artifact: {stop['artifacts'][key]}")

if "# Comlink Meeting Transcript" not in markdown:
    raise SystemExit("markdown missing title")
if "## Retention Policy" not in markdown:
    raise SystemExit("markdown missing retention policy")
if "Meeting segment 00000." not in markdown:
    raise SystemExit("markdown missing transcript segment")
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
  local chunks="$2"
  local duration_seconds="$3"
  local expected_duration_ms=$((chunks * duration_seconds * 1000))

  COMLINK_MOCK_MEETING_CHUNKS="$chunks" \
  COMLINK_MOCK_DURATION_SECONDS="$duration_seconds" \
  run_comlink meet start --format json --chunk-seconds "$duration_seconds" --no-llm \
    > "$artifact_dir/$label-start.json" \
    2> "$artifact_dir/$label-start.err"

  grep -q "Consent reminder" "$artifact_dir/$label-start.err"
  if grep -q "Meeting segment" "$artifact_dir/$label-start.err"; then
    echo "start stderr leaked transcript text" >&2
    exit 1
  fi

  local session_id
  session_id="$(session_id_from "$artifact_dir/$label-start.json")"
  wait_for_chunks "$(chunks_dir_from "$artifact_dir/$label-start.json")" "$chunks"

  COMLINK_MOCK_DURATION_SECONDS="$duration_seconds" \
  run_comlink meet stop "$session_id" --format json \
    > "$artifact_dir/$label-stop.json" \
    2> "$artifact_dir/$label-stop.err"

  if grep -q "Meeting segment" "$artifact_dir/$label-stop.err"; then
    echo "stop stderr leaked transcript text" >&2
    exit 1
  fi

  run_comlink meet export "$session_id" --format json > "$artifact_dir/$label-export.json"
  run_comlink meet export "$session_id" --format md > "$artifact_dir/$label-export.md"
  validate_session "$label" "$chunks" "$expected_duration_ms"
}

run_meeting_case "short" 6 30
run_meeting_case "long" 72 30

echo "Phase 7 E2E passed. Artifacts: $artifact_dir"
