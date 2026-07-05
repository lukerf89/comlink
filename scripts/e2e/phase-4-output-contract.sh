#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
audio_fixture="$repo_root/tests/fixtures/audio/short.wav"
artifact_dir="$repo_root/docs/validation/artifacts/phase-4"
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
  echo "cargo is required for the Phase 4 E2E suite" >&2
  exit 1
fi

if [ ! -f "$audio_fixture" ]; then
  "$repo_root/scripts/dev/generate-short-fixture.sh"
fi

mock_ffmpeg="$tmp_dir/mock-ffmpeg"
mock_whisper="$tmp_dir/mock-whisper"
mock_empty_whisper="$tmp_dir/mock-empty-whisper"
mock_model="$tmp_dir/mock-model.bin"
comlink_home="$tmp_dir/comlink-home"

cat > "$mock_ffmpeg" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

out=""
for arg in "$@"; do
  out="$arg"
done

if [ -z "$out" ]; then
  echo "missing output path" >&2
  exit 2
fi

cp "$COMLINK_MOCK_FIXTURE" "$out"
SH
chmod +x "$mock_ffmpeg"

cat > "$mock_whisper" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

out=""
while [ "$#" -gt 0 ]; do
  case "$1" in
    -of)
      shift
      out="$1"
      ;;
  esac
  shift || true
done

if [ -z "$out" ]; then
  echo "missing -of" >&2
  exit 2
fi

printf 'Phase four agent output for Supabase .\n' > "$out.txt"
SH
chmod +x "$mock_whisper"

cat > "$mock_empty_whisper" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

out=""
while [ "$#" -gt 0 ]; do
  if [ "$1" = "-of" ]; then
    shift
    out="$1"
  fi
  shift || true
done

: > "$out.txt"
SH
chmod +x "$mock_empty_whisper"
printf 'mock model\n' > "$mock_model"

echo "binary: cargo run --"
echo "audio fixture: $audio_fixture"
echo "isolated COMLINK_HOME: $comlink_home"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- models select tiny --path "$mock_model" \
  > "$artifact_dir/models-select.out"

run_transcribe() {
  local format="$1"
  local stdout_path="$2"
  local stderr_path="$3"
  COMLINK_HOME="$comlink_home" \
  COMLINK_FFMPEG="$mock_ffmpeg" \
  COMLINK_MOCK_FIXTURE="$audio_fixture" \
  COMLINK_WHISPER_CPP="$mock_whisper" \
  cargo run --quiet -- transcribe "$audio_fixture" --mode memo --format "$format" --save \
    > "$stdout_path" \
    2> "$stderr_path"
}

run_transcribe json "$artifact_dir/transcribe.json" "$artifact_dir/transcribe-json.err"
run_transcribe jsonl "$artifact_dir/transcribe.jsonl" "$artifact_dir/transcribe-jsonl.err"
run_transcribe md "$artifact_dir/transcribe.md" "$artifact_dir/transcribe-md.err"

python3 - "$artifact_dir/transcribe.json" "$artifact_dir/transcribe.jsonl" "$artifact_dir/transcribe.md" <<'PY'
import json
import sys

json_path, jsonl_path, md_path = sys.argv[1:4]

data = json.load(open(json_path))
json_required = [
    "schema_version",
    "session_id",
    "raw_text",
    "final_text",
    "mode",
    "engine",
    "model",
    "duration_ms",
    "segments",
    "source",
    "context",
]
missing = [key for key in json_required if key not in data]
if missing:
    raise SystemExit(f"JSON output missing contract fields: {missing}")
if data["schema_version"] != "comlink.session.v1":
    raise SystemExit(f"unexpected schema version: {data['schema_version']}")
if data["raw_text"] != "Phase four agent output for Supabase .":
    raise SystemExit(f"raw text was not preserved: {data['raw_text']!r}")
if data["final_text"] != "Phase four agent output for Supabase.":
    raise SystemExit(f"unexpected final text: {data['final_text']!r}")
if data["context"]["policy"] != "none":
    raise SystemExit("context policy must be explicit")
if not data["segments"] or data["segments"][0]["text"] != data["raw_text"]:
    raise SystemExit("segments must preserve raw ASR text")
if data["history_session_id"] != data["session_id"]:
    raise SystemExit("saved output should align history_session_id and session_id")

lines = [json.loads(line) for line in open(jsonl_path) if line.strip()]
if [line["record_type"] for line in lines] != ["session", "segment", "transcript"]:
    raise SystemExit(f"unexpected JSONL record sequence: {lines}")
if lines[1]["segment"]["text"] != data["raw_text"]:
    raise SystemExit("JSONL segment record should carry raw segment text")
final = lines[-1]
jsonl_final_required = [
    "schema_version",
    "session_id",
    "raw_text",
    "final_text",
    "mode",
    "engine",
    "model",
    "duration_ms",
    "source",
    "context",
]
for key in jsonl_final_required:
    if key not in final:
        raise SystemExit(f"JSONL final record missing {key}")
if "segments" in final:
    raise SystemExit("JSONL final record should not duplicate segment records")
if final["schema_version"] != "comlink.session.v1":
    raise SystemExit("JSONL final record schema mismatch")
if final["final_text"] != "Phase four agent output for Supabase.":
    raise SystemExit("JSONL final text mismatch")

markdown = open(md_path).read()
for token in [
    "# Comlink Transcript",
    "**Schema:** comlink.session.v1",
    "## Final Text",
    "Phase four agent output for Supabase.",
    "## Raw Text",
]:
    if token not in markdown:
        raise SystemExit(f"Markdown output missing {token!r}")
PY

saved_id="$(python3 - "$artifact_dir/transcribe.json" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1]))["session_id"])
PY
)"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- history show "$saved_id" --format md \
  > "$artifact_dir/history-show.md"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- history show "$saved_id" --format jsonl \
  > "$artifact_dir/history-show.jsonl"

python3 - "$artifact_dir/history-show.md" "$artifact_dir/history-show.jsonl" <<'PY'
import json
import sys

markdown = open(sys.argv[1]).read()
if "Phase four agent output for Supabase." not in markdown:
    raise SystemExit("saved-session Markdown should include final text")
if "**Context policy:** none" not in markdown:
    raise SystemExit("saved-session Markdown should include context policy")

records = [json.loads(line) for line in open(sys.argv[2]) if line.strip()]
if len(records) != 1 or records[0]["record_type"] != "transcript":
    raise SystemExit(f"unexpected saved-session JSONL: {records}")
if records[0]["schema_version"] != "comlink.session.v1":
    raise SystemExit("saved-session JSONL should include schema version")
if records[0]["copied"] is not False:
    raise SystemExit("saved-session JSONL should preserve copied flag")
if records[0]["context"]["policy"] != "none":
    raise SystemExit("saved-session JSONL should include context policy")
if not records[0]["segments"]:
    raise SystemExit("saved-session JSONL should include retained segments")
PY

python3 - "$artifact_dir/transcribe.json" "$artifact_dir/transcribe.jsonl" <<'PY'
import sys

for path in sys.argv[1:]:
    text = open(path).read()
    if "Saved history session" in text or "Stop-to-final" in text or "Recording..." in text:
        raise SystemExit(f"diagnostic text leaked to stdout: {path}")
PY

set +e
COMLINK_HOME="$comlink_home" \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$audio_fixture" \
COMLINK_WHISPER_CPP="$mock_whisper" \
cargo run --quiet -- transcribe "$tmp_dir/missing.wav" --format json \
  > "$artifact_dir/missing-file.out" \
  2> "$artifact_dir/missing-file.err"
missing_file_status=$?

COMLINK_HOME="$tmp_dir/no-model-home" \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$audio_fixture" \
COMLINK_WHISPER_CPP="$mock_whisper" \
cargo run --quiet -- transcribe "$audio_fixture" --format json \
  > "$artifact_dir/missing-model.out" \
  2> "$artifact_dir/missing-model.err"
missing_model_status=$?

COMLINK_HOME="$comlink_home" \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$audio_fixture" \
COMLINK_WHISPER_CPP="$mock_empty_whisper" \
cargo run --quiet -- transcribe "$audio_fixture" --format json \
  > "$artifact_dir/no-speech.out" \
  2> "$artifact_dir/no-speech.err"
no_speech_status=$?
set -e

if [ "$missing_file_status" -ne 1 ]; then
  echo "missing file should exit 1, got $missing_file_status" >&2
  exit 1
fi
if [ "$missing_model_status" -ne 3 ]; then
  echo "missing model should exit 3, got $missing_model_status" >&2
  exit 1
fi
if [ "$no_speech_status" -ne 4 ]; then
  echo "no speech should exit 4, got $no_speech_status" >&2
  exit 1
fi

echo "Phase 4 E2E passed."
