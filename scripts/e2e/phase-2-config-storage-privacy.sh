#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fixture="$repo_root/tests/fixtures/audio/short.wav"
artifact_dir="$repo_root/docs/validation/artifacts/phase-2"
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
  echo "cargo is required for the Phase 2 E2E suite" >&2
  exit 1
fi

if [ ! -f "$fixture" ]; then
  "$repo_root/scripts/dev/generate-short-fixture.sh"
fi

mock_ffmpeg="$tmp_dir/mock-ffmpeg"
mock_whisper="$tmp_dir/mock-whisper"
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

printf 'Phase two saved transcript .\n' > "$out.txt"
SH
chmod +x "$mock_whisper"
printf 'mock model\n' > "$mock_model"

echo "binary: cargo run --"
echo "fixture: $fixture"
echo "isolated COMLINK_HOME: $comlink_home"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- config show --format json \
  > "$artifact_dir/config-default.json"

python3 - "$artifact_dir/config-default.json" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
if data["config"]["history_enabled"] is not True:
    raise SystemExit("history should default on")
if data["config"]["retention"]["metadata"] is not True:
    raise SystemExit("metadata retention should default on")
if data["config"]["retention"]["transcripts"] is not True:
    raise SystemExit("transcript retention should default on")
if data["config"]["retention"]["audio"] is not False:
    raise SystemExit("audio retention should default off")
if "defaults" not in data["sources"]:
    raise SystemExit("config show must include source precedence")
PY

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- models select tiny --path "$mock_model" \
  > "$artifact_dir/models-select.out"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- models list --format json \
  > "$artifact_dir/models-list.json"

python3 - "$artifact_dir/models-list.json" <<'PY'
import json
import sys

models = json.load(open(sys.argv[1]))
if len(models) != 1:
    raise SystemExit(f"expected one model, got {len(models)}")
if models[0]["name"] != "tiny" or not models[0]["selected"]:
    raise SystemExit(f"unexpected selected model: {models}")
PY

COMLINK_HOME="$comlink_home" \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$fixture" \
COMLINK_WHISPER_CPP="$mock_whisper" \
cargo run --quiet -- transcribe "$fixture" --mode memo --format json --save \
  > "$artifact_dir/transcribe-saved.json" \
  2> "$artifact_dir/transcribe-saved.err"

python3 - "$artifact_dir/transcribe-saved.json" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
if not data.get("history_session_id"):
    raise SystemExit("saved transcript should include history_session_id")
if data["final_text"] != "Phase two saved transcript.":
    raise SystemExit(f"unexpected final text: {data['final_text']!r}")
PY

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- history list --format json \
  > "$artifact_dir/history-list.json"

python3 - "$artifact_dir/history-list.json" "$artifact_dir/transcribe-saved.json" <<'PY'
import json
import sys

sessions = json.load(open(sys.argv[1]))
saved = json.load(open(sys.argv[2]))
if len(sessions) != 1:
    raise SystemExit(f"expected one saved session, got {len(sessions)}")
if sessions[0]["id"] != saved["history_session_id"]:
    raise SystemExit("history list id should match saved transcript")
if not sessions[0]["has_transcript"]:
    raise SystemExit("first saved session should retain transcript text")
PY

saved_id="$(python3 - "$artifact_dir/transcribe-saved.json" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1]))["history_session_id"])
PY
)"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- history show "$saved_id" --format json \
  > "$artifact_dir/history-show.json"

python3 - "$artifact_dir/history-show.json" <<'PY'
import json
import sys

session = json.load(open(sys.argv[1]))
if session["final_text"] != "Phase two saved transcript.":
    raise SystemExit("retained session should include final_text")
if not session["segments"] or session["segments"][0]["text"] is None:
    raise SystemExit("retained session should include segment text")
PY

COMLINK_HOME="$comlink_home" \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$fixture" \
COMLINK_WHISPER_CPP="$mock_whisper" \
COMLINK_RETAIN_TRANSCRIPTS=false \
cargo run --quiet -- transcribe "$fixture" --mode memo --format json --save \
  > "$artifact_dir/transcribe-no-transcripts.json" \
  2> "$artifact_dir/transcribe-no-transcripts.err"

private_id="$(python3 - "$artifact_dir/transcribe-no-transcripts.json" <<'PY'
import json
import sys
print(json.load(open(sys.argv[1]))["history_session_id"])
PY
)"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- history show "$private_id" --format json \
  > "$artifact_dir/history-show-no-transcripts.json"

python3 - "$artifact_dir/history-show-no-transcripts.json" <<'PY'
import json
import sys

session = json.load(open(sys.argv[1]))
if session["final_text"] is not None or session["raw_text"] is not None:
    raise SystemExit("transcript retention off should omit stored transcript text")
if not session["segments"] or session["segments"][0]["text"] is not None:
    raise SystemExit("transcript retention off should omit stored segment text")
if session["source"]["path"] == "<redacted>":
    raise SystemExit("metadata should still be retained when only transcripts are disabled")
PY

COMLINK_HOME="$comlink_home" \
COMLINK_RETAIN_TRANSCRIPTS=false \
cargo run --quiet -- privacy audit --format json \
  > "$artifact_dir/privacy-audit.json"

python3 - "$artifact_dir/privacy-audit.json" <<'PY'
import json
import sys

audit = json.load(open(sys.argv[1]))
if audit["history_enabled"] is not True:
    raise SystemExit("privacy audit should expose history_enabled")
if audit["retention"]["transcripts"] is not False:
    raise SystemExit("privacy audit should expose transcript retention override")
if "local" not in audit["asr"]:
    raise SystemExit("privacy audit should expose local ASR posture")
if "disabled" not in audit["llm"]:
    raise SystemExit("privacy audit should expose LLM status")
PY

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- history prune --all --format json \
  > "$artifact_dir/history-prune.json"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- history list --format json \
  > "$artifact_dir/history-list-after-prune.json"

python3 - "$artifact_dir/history-prune.json" "$artifact_dir/history-list-after-prune.json" <<'PY'
import json
import sys

prune = json.load(open(sys.argv[1]))
after = json.load(open(sys.argv[2]))
if prune["sessions_deleted"] != 2:
    raise SystemExit(f"expected two pruned sessions, got {prune}")
if after != []:
    raise SystemExit(f"history should be empty after prune: {after}")
PY

echo "Phase 2 E2E passed."
