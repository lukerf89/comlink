#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fixture="$repo_root/tests/fixtures/audio/short.wav"
artifact_dir="$repo_root/docs/validation/artifacts/phase-0"
tmp_dir="$(mktemp -d)"

cleanup() {
  python3 - "$tmp_dir" "$artifact_dir" <<'PY' || true
import pathlib
import sys

tmp_dir = sys.argv[1]
artifact_dir = pathlib.Path(sys.argv[2])
for path in artifact_dir.glob("*"):
    if path.is_file():
        text = path.read_text(errors="ignore")
        path.write_text(text.replace(tmp_dir, "<tmp>"))
PY
  rm -rf "$tmp_dir"
}

trap cleanup EXIT

mkdir -p "$artifact_dir"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required for the Phase 0 E2E suite" >&2
  exit 1
fi

if [ ! -f "$fixture" ]; then
  "$repo_root/scripts/dev/generate-short-fixture.sh"
fi

mock_whisper="$tmp_dir/mock-whisper"
mock_model="$tmp_dir/mock-model.bin"
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
printf 'Comlink phase zero fixture.\n' > "$out.txt"
SH
chmod +x "$mock_whisper"
printf 'mock model\n' > "$mock_model"

echo "binary: cargo run --"
echo "fixture: $fixture"
echo "mock whisper: $mock_whisper"

COMLINK_WHISPER_CPP="$mock_whisper" \
COMLINK_WHISPER_MODEL="$mock_model" \
cargo run --quiet -- transcribe "$fixture" --format text \
  > "$artifact_dir/text.out"

grep -q "Comlink phase zero fixture." "$artifact_dir/text.out"

COMLINK_WHISPER_CPP="$mock_whisper" \
COMLINK_WHISPER_MODEL="$mock_model" \
cargo run --quiet -- transcribe "$fixture" --format json \
  > "$artifact_dir/transcript.json"

python3 - "$artifact_dir/transcript.json" <<'PY'
import json
import sys

path = sys.argv[1]
data = json.load(open(path))
required = {"text", "engine", "model", "duration_ms", "segments", "source"}
missing = required - set(data)
if missing:
    raise SystemExit(f"missing keys: {sorted(missing)}")
if data["engine"] != "whisper.cpp":
    raise SystemExit(f"unexpected engine: {data['engine']}")
if not data["segments"]:
    raise SystemExit("segments must not be empty")
PY

set +e
COMLINK_WHISPER_CPP="$mock_whisper" \
COMLINK_WHISPER_MODEL="$mock_model" \
cargo run --quiet -- transcribe "$tmp_dir/missing.wav" --format json \
  > "$artifact_dir/missing-file.out" 2> "$artifact_dir/missing-file.err"
missing_status=$?
set -e
if [ "$missing_status" -eq 0 ]; then
  echo "missing-file check unexpectedly succeeded" >&2
  exit 1
fi

set +e
COMLINK_WHISPER_CPP="$tmp_dir/not-a-binary" \
COMLINK_WHISPER_MODEL="$mock_model" \
cargo run --quiet -- doctor \
  > "$artifact_dir/invalid-doctor.out" 2> "$artifact_dir/invalid-doctor.err"
doctor_status=$?
set -e
if [ "$doctor_status" -eq 0 ]; then
  echo "doctor should fail for invalid COMLINK_WHISPER_CPP" >&2
  exit 1
fi
grep -q "whisper.cpp" "$artifact_dir/invalid-doctor.err"

echo "Phase 0 E2E passed. Artifacts: $artifact_dir"
