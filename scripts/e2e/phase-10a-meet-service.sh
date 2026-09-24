#!/usr/bin/env bash
set -euo pipefail

# Phase 10a E2E: meeting service core, `meet status`, and detached finalize.
# Runs start -> status -> stop --detach (whisper held on a barrier) -> status
# (transcribing) -> release -> poll status until stopped -> export md/json,
# then a plain synchronous stop. Isolated temp config/data dir, mock
# recorder/whisper, no network.

export COMLINK_RECORD_DEVICE="${COMLINK_RECORD_DEVICE:-:0}"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
artifact_dir="$repo_root/docs/validation/artifacts/phase-10a"
tmp_dir="$(mktemp -d)"

cleanup() {
  # Release any barrier so a stray finalizer cannot linger, then scrub temp
  # paths from the artifacts.
  touch "$tmp_dir/whisper-barrier" 2>/dev/null || true
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

fail() {
  echo "phase-10a E2E failed: $*" >&2
  exit 1
}

for tool in cargo jq python3; do
  command -v "$tool" >/dev/null 2>&1 || fail "$tool is required"
done

mkdir -p "$artifact_dir"
rm -f "$artifact_dir"/*

cargo build --quiet
binary="$repo_root/target/debug/comlink"

mock_ffmpeg="$tmp_dir/mock-ffmpeg"
mock_ffprobe="$tmp_dir/mock-ffprobe"
mock_whisper="$tmp_dir/mock-whisper"
mock_model="$tmp_dir/mock-model.bin"
comlink_home="$tmp_dir/comlink-home"
comlink_data="$tmp_dir/comlink-data"
barrier="$tmp_dir/whisper-barrier"
started_flag="$tmp_dir/whisper-started"

cat > "$mock_ffmpeg" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
out="${@: -1}"
mkdir -p "$(dirname "$out")"
for index in 0 1; do
  printf 'mock wav %s\n' "$index" > "$(printf "$out" "$index")"
done
trap 'exit 0' INT TERM
while true; do
  sleep 0.1
done
SH

cat > "$mock_ffprobe" <<'SH'
#!/usr/bin/env bash
printf '30\n'
SH

cat > "$mock_whisper" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
out=""
wav=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -of) shift; out="$1" ;;
    -f) shift; wav="$1" ;;
  esac
  shift || true
done
if [ -n "${COMLINK_MOCK_WHISPER_STARTED:-}" ]; then
  touch "$COMLINK_MOCK_WHISPER_STARTED"
fi
if [ -n "${COMLINK_MOCK_WHISPER_BARRIER:-}" ]; then
  for _ in $(seq 1 300); do
    [ -f "$COMLINK_MOCK_WHISPER_BARRIER" ] && break
    sleep 0.1
  done
fi
index="$(basename "$wav" .wav)"
printf 'Phase 10a fixture segment %s.\n' "${index#chunk-}" > "$out.txt"
SH

chmod +x "$mock_ffmpeg" "$mock_ffprobe" "$mock_whisper"
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

# Fail on any stderr line outside the allow-list.
check_stderr() {
  local file="$1"
  local unexpected
  unexpected="$(grep -Ev '^(Consent reminder: |Stopping meeting recording: |Transcribing in the background )' "$file" || true)"
  if [ -n "$unexpected" ]; then
    fail "unexpected stderr in $(basename "$file"): $unexpected"
  fi
}

check_json() {
  local file="$1"
  local filter="$2"
  jq -e . "$file" >/dev/null || fail "invalid JSON: $(basename "$file")"
  jq -e "$filter" "$file" >/dev/null || fail "$(basename "$file") failed check: $filter"
}

wait_for_chunks() {
  local dir="$1"
  for _ in $(seq 1 100); do
    [ "$(find "$dir" -maxdepth 1 -name 'chunk-*.wav' 2>/dev/null | wc -l | tr -d ' ')" = "2" ] && return 0
    sleep 0.1
  done
  fail "timed out waiting for chunks in $dir"
}

# 0. No session yet: status exits 0 with status none.
run_comlink meet status --format json > "$artifact_dir/status-none.json" 2> "$artifact_dir/status-none.err"
check_json "$artifact_dir/status-none.json" '.status == "none" and .session_id == null and .schema_version == "comlink.meeting.v1"'
check_stderr "$artifact_dir/status-none.err"

# 1. start
run_comlink meet start --format json --chunk-seconds 30 --no-llm \
  > "$artifact_dir/start.json" 2> "$artifact_dir/start.err"
check_json "$artifact_dir/start.json" '.status == "recording"'
check_stderr "$artifact_dir/start.err"
session_id="$(jq -r .session_id "$artifact_dir/start.json")"
wait_for_chunks "$(jq -r .chunks_dir "$artifact_dir/start.json")"

# 2. status while recording
run_comlink meet status --format json > "$artifact_dir/status-recording.json" 2> "$artifact_dir/status-recording.err"
check_json "$artifact_dir/status-recording.json" \
  ".session_id == \"$session_id\" and .status == \"recording\" and .stale == false and .chunk_count == 2 and (.recorders | length) == 1 and .recorders[0].alive == true"
check_stderr "$artifact_dir/status-recording.err"

# 3. stop --detach with whisper held on the barrier: must return in < 2s.
detach_started="$(python3 -c 'import time; print(time.time())')"
COMLINK_MOCK_WHISPER_BARRIER="$barrier" COMLINK_MOCK_WHISPER_STARTED="$started_flag" \
  run_comlink meet stop --detach --format json \
  > "$artifact_dir/stop-detach.json" 2> "$artifact_dir/stop-detach.err"
detach_seconds="$(python3 -c "import time; print(round(time.time() - $detach_started, 3))")"
echo "stop --detach wall time: ${detach_seconds}s" | tee "$artifact_dir/stop-detach-timing.txt"
python3 -c "import sys; sys.exit(0 if $detach_seconds < 2 else 1)" || fail "stop --detach took ${detach_seconds}s"
check_json "$artifact_dir/stop-detach.json" ".status == \"transcribing\" and .session_id == \"$session_id\" and .finalizer_pid > 0"
check_stderr "$artifact_dir/stop-detach.err"

# 4. status shows transcribing with a live finalizer.
for _ in $(seq 1 100); do [ -f "$started_flag" ] && break; sleep 0.1; done
[ -f "$started_flag" ] || fail "finalizer never invoked whisper"
run_comlink meet status --format json > "$artifact_dir/status-transcribing.json" 2> "$artifact_dir/status-transcribing.err"
check_json "$artifact_dir/status-transcribing.json" \
  ".session_id == \"$session_id\" and .status == \"transcribing\" and .finalizer.alive == true and .stale == false"
check_stderr "$artifact_dir/status-transcribing.err"

# 5. Release the barrier and poll until stopped (bounded).
touch "$barrier"
final_status=""
for _ in $(seq 1 150); do
  run_comlink meet status "$session_id" --format json > "$artifact_dir/status-final.json" 2> "$artifact_dir/status-final.err"
  final_status="$(jq -r .status "$artifact_dir/status-final.json")"
  [ "$final_status" = "stopped" ] && break
  sleep 0.2
done
if [ "$final_status" != "stopped" ]; then
  cat "$artifact_dir/status-final.json" >&2
  cat "$comlink_data/meetings/$session_id/finalize.log" >&2 || true
  fail "session never reached stopped (last: $final_status)"
fi
check_json "$artifact_dir/status-final.json" '.stale == false and .finalizer == null'
check_stderr "$artifact_dir/status-final.err"
cp "$comlink_data/meetings/$session_id/finalize.log" "$artifact_dir/finalize.log"

# 6. Exports
run_comlink meet export "$session_id" --format json > "$artifact_dir/export.json" 2> "$artifact_dir/export-json.err"
run_comlink meet export "$session_id" --format md > "$artifact_dir/export.md" 2> "$artifact_dir/export-md.err"
check_json "$artifact_dir/export.json" \
  '.schema_version == "comlink.meeting.v1" and .session.status == "stopped" and .session.segment_count == 2 and (.segments | length) == 2'
grep -q "# Comlink Meeting Transcript" "$artifact_dir/export.md" || fail "markdown export missing header"
grep -q "Phase 10a fixture segment 00001." "$artifact_dir/export.md" || fail "markdown export missing transcript"
for path in segments_jsonl json_export markdown_export; do
  file="$(jq -r ".artifacts.$path" "$artifact_dir/export.json")"
  [ -f "$file" ] || fail "missing artifact $path: $file"
done
check_stderr "$artifact_dir/export-json.err"
check_stderr "$artifact_dir/export-md.err"

# 7. finalize again is an idempotent no-op with the sync stop shape.
run_comlink meet finalize "$session_id" --format json > "$artifact_dir/finalize-rerun.json" 2> "$artifact_dir/finalize-rerun.err"
check_json "$artifact_dir/finalize-rerun.json" ".status == \"stopped\" and .chunks_processed == 2 and .segment_count == 2"
check_stderr "$artifact_dir/finalize-rerun.err"

# 8. Plain synchronous stop still works as before.
run_comlink meet start --format json --chunk-seconds 30 --no-llm \
  > "$artifact_dir/sync-start.json" 2> "$artifact_dir/sync-start.err"
check_stderr "$artifact_dir/sync-start.err"
wait_for_chunks "$(jq -r .chunks_dir "$artifact_dir/sync-start.json")"
run_comlink meet stop --format json > "$artifact_dir/sync-stop.json" 2> "$artifact_dir/sync-stop.err"
check_json "$artifact_dir/sync-stop.json" '.status == "stopped" and .chunks_processed == 2 and .segment_count == 2'
check_stderr "$artifact_dir/sync-stop.err"
sync_id="$(jq -r .session_id "$artifact_dir/sync-stop.json")"
[ ! -f "$comlink_data/meetings/$sync_id/finalize.log" ] || fail "sync stop launched a finalizer"

# 9. Nothing active or transcribing -> none again.
run_comlink meet status --format json > "$artifact_dir/status-none-after.json" 2> "$artifact_dir/status-none-after.err"
check_json "$artifact_dir/status-none-after.json" '.status == "none"'

echo "Phase 10a meeting service E2E passed"
