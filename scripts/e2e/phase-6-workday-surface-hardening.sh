#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fixture="$repo_root/tests/fixtures/audio/short.wav"
artifact_dir="$repo_root/docs/validation/artifacts/phase-6"
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
  echo "cargo is required for the Phase 6 E2E suite" >&2
  exit 1
fi

if [ ! -f "$fixture" ]; then
  "$repo_root/scripts/dev/generate-short-fixture.sh"
fi

mock_ffmpeg="$tmp_dir/mock-ffmpeg"
mock_whisper="$tmp_dir/mock-whisper"
mock_pbcopy="$tmp_dir/mock-pbcopy"
mock_pbpaste="$tmp_dir/mock-pbpaste"
mock_model="$tmp_dir/mock-model.bin"
clipboard_file="$tmp_dir/clipboard.txt"
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

if [ "${COMLINK_MOCK_DOCTOR_FAIL:-0}" = "1" ]; then
  echo "mock ffmpeg unavailable" >&2
  exit 9
fi

cp "$COMLINK_MOCK_FIXTURE" "$out"

case " $* " in
  *" avfoundation "*)
    while IFS= read -r line; do
      [ "$line" = "q" ] && break
    done
    ;;
esac
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

printf 'Phase six copy target memo .\n' > "$out.txt"
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

echo "binary: cargo run --"
echo "fixture: $fixture"
echo "isolated COMLINK_HOME: $comlink_home"

COMLINK_HOME="$comlink_home" \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$fixture" \
COMLINK_WHISPER_CPP="$mock_whisper" \
COMLINK_WHISPER_MODEL="$mock_model" \
COMLINK_PBCOPY="$mock_pbcopy" \
COMLINK_PBPASTE="$mock_pbpaste" \
COMLINK_MOCK_CLIPBOARD="$clipboard_file" \
cargo run --quiet -- doctor --format json \
  > "$artifact_dir/doctor.json"

python3 - "$artifact_dir/doctor.json" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
if data["schema_version"] != "comlink.doctor.v1":
    raise SystemExit(f"unexpected doctor schema: {data['schema_version']}")
if not data["ok"]:
    raise SystemExit(f"doctor should be healthy with mocks: {data}")
checks = {check["name"]: check for check in data["checks"]}
for name in ["ffmpeg", "asr", "model-path", "clipboard-copy", "data-path", "microphone"]:
    if name not in checks:
        raise SystemExit(f"doctor missing {name}")
for name in ["ffmpeg", "asr", "model-path", "clipboard-copy", "data-path"]:
    if checks[name]["status"] != "ok":
        raise SystemExit(f"{name} should be ok: {checks[name]}")
if "local-first" not in data["privacy"]["posture"]:
    raise SystemExit("doctor should report privacy posture")
PY

surface_text="um cargo test --all . new line git status --short ."
COMLINK_HOME="$comlink_home" \
cargo run --quiet -- modes apply --mode terminal --text "$surface_text" --format json \
  > "$artifact_dir/mode-terminal.json"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- modes apply --mode editor --text "first line new line second line" --format json \
  > "$artifact_dir/mode-editor.json"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- modes apply --mode outlook --text "thanks for sending this new paragraph I can review today" --format json \
  > "$artifact_dir/mode-outlook.json"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- modes apply --mode slack --text "uh sounds good , I will check after standup" --format json \
  > "$artifact_dir/mode-slack.json"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- modes apply --mode memo --text "phase six memo" --format json \
  > "$artifact_dir/mode-memo.json"

python3 - "$artifact_dir" <<'PY'
import json
import pathlib
import sys

artifact_dir = pathlib.Path(sys.argv[1])
terminal = json.load(open(artifact_dir / "mode-terminal.json"))
editor = json.load(open(artifact_dir / "mode-editor.json"))
outlook = json.load(open(artifact_dir / "mode-outlook.json"))
slack = json.load(open(artifact_dir / "mode-slack.json"))
memo = json.load(open(artifact_dir / "mode-memo.json"))

if terminal["final_text"] != "cargo test --all && git status --short":
    raise SystemExit(f"unexpected terminal text: {terminal['final_text']!r}")
if editor["final_text"] != "first line\nsecond line.":
    raise SystemExit(f"unexpected editor text: {editor['final_text']!r}")
if outlook["final_text"] != "thanks for sending this\n\nI can review today.":
    raise SystemExit(f"unexpected outlook text: {outlook['final_text']!r}")
if slack["final_text"] != "sounds good, I will check after standup":
    raise SystemExit(f"unexpected slack text: {slack['final_text']!r}")
if memo["final_text"] != "phase six memo.":
    raise SystemExit(f"unexpected memo text: {memo['final_text']!r}")
PY

printf '\n' | \
COMLINK_HOME="$comlink_home" \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$fixture" \
COMLINK_WHISPER_CPP="$mock_whisper" \
COMLINK_WHISPER_MODEL="$mock_model" \
COMLINK_PBCOPY="$mock_pbcopy" \
COMLINK_PBPASTE="$mock_pbpaste" \
COMLINK_MOCK_CLIPBOARD="$clipboard_file" \
cargo run --quiet -- record --mode memo --copy --format json \
  > "$artifact_dir/record-copy.json" \
  2> "$artifact_dir/record-copy.err"

python3 - "$artifact_dir/record-copy.json" "$clipboard_file" <<'PY'
import json
import pathlib
import sys

data = json.load(open(sys.argv[1]))
clipboard = pathlib.Path(sys.argv[2]).read_text()
if data["final_text"] != "Phase six copy target memo.":
    raise SystemExit(f"unexpected final text: {data['final_text']!r}")
if data["copied"] is not True:
    raise SystemExit("record --copy should mark copied=true")
if clipboard != data["final_text"]:
    raise SystemExit(f"clipboard mismatch: {clipboard!r}")
PY

printf 'previous clipboard\n' > "$clipboard_file"
printf '\n' | \
COMLINK_HOME="$comlink_home" \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$fixture" \
COMLINK_WHISPER_CPP="$mock_whisper" \
COMLINK_WHISPER_MODEL="$mock_model" \
COMLINK_PBCOPY="$mock_pbcopy" \
COMLINK_PBPASTE="$mock_pbpaste" \
COMLINK_MOCK_CLIPBOARD="$clipboard_file" \
cargo run --quiet -- record --mode memo --copy --restore-clipboard --format json \
  > "$artifact_dir/record-restore-clipboard.json" \
  2> "$artifact_dir/record-restore-clipboard.err"

python3 - "$artifact_dir/record-restore-clipboard.json" "$clipboard_file" <<'PY'
import json
import pathlib
import sys

data = json.load(open(sys.argv[1]))
clipboard = pathlib.Path(sys.argv[2]).read_text()
if data["copied"] is not True:
    raise SystemExit("restore path should still mark copied=true")
if clipboard != "previous clipboard\n":
    raise SystemExit(f"previous clipboard was not restored: {clipboard!r}")
PY

grep -q "restored previous clipboard" "$artifact_dir/record-restore-clipboard.err"

set +e
printf '\n' | \
COMLINK_HOME="$comlink_home" \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$fixture" \
COMLINK_WHISPER_CPP="$mock_whisper" \
COMLINK_WHISPER_MODEL="$mock_model" \
COMLINK_PBCOPY="$tmp_dir/missing-pbcopy" \
COMLINK_MOCK_CLIPBOARD="$clipboard_file" \
cargo run --quiet -- record --mode memo --copy --format json \
  > "$artifact_dir/record-copy-missing-command.out" \
  2> "$artifact_dir/record-copy-missing-command.err"
copy_missing_status=$?
set -e

if [ "$copy_missing_status" -ne 5 ]; then
  echo "missing clipboard command should exit 5, got $copy_missing_status" >&2
  exit 1
fi

grep -q "failed to start" "$artifact_dir/record-copy-missing-command.err"
grep -q "missing-pbcopy" "$artifact_dir/record-copy-missing-command.err"

echo "Phase 6 E2E passed. Artifacts: $artifact_dir"
