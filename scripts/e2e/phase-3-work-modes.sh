#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
audio_fixture="$repo_root/tests/fixtures/audio/short.wav"
work_modes_fixture="$repo_root/tests/fixtures/text/phase-3-work-modes.txt"
coding_fixture="$repo_root/tests/fixtures/text/phase-3-coding-prompt.txt"
artifact_dir="$repo_root/docs/validation/artifacts/phase-3"
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
  echo "cargo is required for the Phase 3 E2E suite" >&2
  exit 1
fi

if [ ! -f "$audio_fixture" ]; then
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

cat "$COMLINK_MOCK_TRANSCRIPT" > "$out.txt"
SH
chmod +x "$mock_whisper"
printf 'mock model\n' > "$mock_model"

echo "binary: cargo run --"
echo "audio fixture: $audio_fixture"
echo "text fixtures: $work_modes_fixture, $coding_fixture"
echo "isolated COMLINK_HOME: $comlink_home"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- modes list --format json \
  > "$artifact_dir/modes-list.json"

python3 - "$artifact_dir/modes-list.json" <<'PY'
import json
import sys

modes = json.load(open(sys.argv[1]))
names = [mode["name"] for mode in modes]
expected = ["raw", "clean", "memo", "coding-prompt", "email-reply", "slack-reply"]
if names != expected:
    raise SystemExit(f"unexpected mode registry: {names}")
if not all(mode["deterministic"] for mode in modes):
    raise SystemExit("all phase 3 modes should be deterministic")
PY

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- vocab add "super base" "Supabase" \
  > "$artifact_dir/vocab-add.out"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- snippets add "my signature" "Best,\nLuke" \
  > "$artifact_dir/snippets-add.out"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- vocab list --format json \
  > "$artifact_dir/vocab-list.json"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- snippets list --format json \
  > "$artifact_dir/snippets-list.json"

python3 - "$artifact_dir/vocab-list.json" "$artifact_dir/snippets-list.json" <<'PY'
import json
import sys

vocab = json.load(open(sys.argv[1]))
snippets = json.load(open(sys.argv[2]))
if vocab != [{"phrase": "super base", "replacement": "Supabase"}]:
    raise SystemExit(f"unexpected vocabulary: {vocab}")
if snippets != [{"trigger": "my signature", "body": "Best,\nLuke"}]:
    raise SystemExit(f"unexpected snippets: {snippets}")
PY

work_modes_text="$(cat "$work_modes_fixture")"
for mode in raw clean memo email-reply slack-reply; do
  COMLINK_HOME="$comlink_home" \
  cargo run --quiet -- modes apply --mode "$mode" --text "$work_modes_text" --format json \
    > "$artifact_dir/mode-$mode.json"
done

coding_text="$(cat "$coding_fixture")"
COMLINK_HOME="$comlink_home" \
cargo run --quiet -- modes apply --mode coding-prompt --text "$coding_text" --format json \
  > "$artifact_dir/mode-coding-prompt.json"

python3 - "$artifact_dir" <<'PY'
import json
import pathlib
import sys

artifact_dir = pathlib.Path(sys.argv[1])

clean = json.load(open(artifact_dir / "mode-clean.json"))
if clean["raw_text"] != "um please send this to super base . my signature":
    raise SystemExit("mode apply should preserve raw_text")
if clean["final_text"] != "please send this to Supabase. Best, Luke":
    raise SystemExit(f"unexpected clean text: {clean['final_text']!r}")

raw = json.load(open(artifact_dir / "mode-raw.json"))
if raw["final_text"] != raw["raw_text"]:
    raise SystemExit("raw mode should not apply deterministic rewrites")

memo = json.load(open(artifact_dir / "mode-memo.json"))
if memo["final_text"] != "please send this to Supabase. Best, Luke.":
    raise SystemExit(f"unexpected memo text: {memo['final_text']!r}")

email = json.load(open(artifact_dir / "mode-email-reply.json"))
if email["final_text"] != memo["final_text"]:
    raise SystemExit("email-reply should use memo-like deterministic cleanup")

slack = json.load(open(artifact_dir / "mode-slack-reply.json"))
if slack["final_text"] != clean["final_text"]:
    raise SystemExit("slack-reply should stay concise without adding punctuation")

coding = json.load(open(artifact_dir / "mode-coding-prompt.json"))
required = [
    "src/main.rs",
    "cargo test --all",
    "camelCase",
    "snake_case",
    "https://example.com/api",
    "Supabase",
]
missing = [token for token in required if token not in coding["final_text"]]
if missing:
    raise SystemExit(f"coding prompt lost technical tokens: {missing}")
if coding["raw_text"] == coding["final_text"]:
    raise SystemExit("coding prompt should still clean safe fillers and vocabulary")
PY

COMLINK_HOME="$comlink_home" \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$audio_fixture" \
COMLINK_MOCK_TRANSCRIPT="$coding_fixture" \
COMLINK_WHISPER_CPP="$mock_whisper" \
COMLINK_WHISPER_MODEL="$mock_model" \
cargo run --quiet -- transcribe "$audio_fixture" --mode coding-prompt --format json \
  > "$artifact_dir/transcribe-coding-prompt.json" \
  2> "$artifact_dir/transcribe-coding-prompt.err"

python3 - "$artifact_dir/transcribe-coding-prompt.json" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
if data["mode"] != "coding-prompt":
    raise SystemExit(f"unexpected mode: {data['mode']}")
if not data["raw_text"].startswith("uh update src/main.rs"):
    raise SystemExit(f"raw text was not preserved: {data['raw_text']!r}")
for token in ["src/main.rs", "cargo test --all", "camelCase", "snake_case", "https://example.com/api", "Supabase"]:
    if token not in data["final_text"]:
        raise SystemExit(f"final text lost token {token!r}: {data['final_text']!r}")
if data["processing_steps"][-1]["name"] != "coding-prompt-mode":
    raise SystemExit("coding prompt processing step should be recorded")
PY

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- snippets remove "my signature" \
  > "$artifact_dir/snippets-remove.out"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- vocab remove "super base" \
  > "$artifact_dir/vocab-remove.out"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- snippets list --format json \
  > "$artifact_dir/snippets-list-after-remove.json"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- vocab list --format json \
  > "$artifact_dir/vocab-list-after-remove.json"

python3 - "$artifact_dir/snippets-list-after-remove.json" "$artifact_dir/vocab-list-after-remove.json" <<'PY'
import json
import sys

if json.load(open(sys.argv[1])) != []:
    raise SystemExit("snippet removal should leave an empty registry")
if json.load(open(sys.argv[2])) != []:
    raise SystemExit("vocabulary removal should leave an empty registry")
PY

echo "Phase 3 E2E passed."
