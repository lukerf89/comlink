#!/usr/bin/env bash
set -euo pipefail

# LF-38: pin the record/meet input device so these hermetic suites do not
# exercise the platform-dependent system-default-input resolution (that path
# is covered by unit tests). Callers may still override.
export COMLINK_RECORD_DEVICE="${COMLINK_RECORD_DEVICE:-:0}"

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
fixture="$repo_root/tests/fixtures/audio/short.wav"
artifact_dir="$repo_root/docs/validation/artifacts/phase-1"
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
  echo "cargo is required for the Phase 1 E2E suite" >&2
  exit 1
fi

if [ ! -f "$fixture" ]; then
  "$repo_root/scripts/dev/generate-short-fixture.sh"
fi

mock_ffmpeg="$tmp_dir/mock-ffmpeg"
mock_whisper="$tmp_dir/mock-whisper"
mock_pbcopy="$tmp_dir/mock-pbcopy"
mock_model="$tmp_dir/mock-model.bin"
clipboard_file="$tmp_dir/clipboard.txt"
ffmpeg_log="$tmp_dir/ffmpeg.log"

cat > "$mock_ffmpeg" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

if [ -n "${COMLINK_MOCK_FFMPEG_LOG:-}" ]; then
  printf '%s\n' "$*" >> "$COMLINK_MOCK_FFMPEG_LOG"
fi

out=""
for arg in "$@"; do
  out="$arg"
done

if [ -z "$out" ]; then
  echo "missing output path" >&2
  exit 2
fi

case " $* " in
  *" avfoundation "*)
    if [ "${COMLINK_MOCK_RECORDER_FAIL:-0}" = "1" ]; then
      echo "mock microphone permission denied" >&2
      exit 7
    fi
    ;;
esac

if [ "${COMLINK_MOCK_NO_AUDIO_FILE:-0}" = "1" ]; then
  exit 255
elif [ "${COMLINK_MOCK_SHORT_AUDIO:-0}" = "1" ]; then
  : > "$out"
else
  cp "$COMLINK_MOCK_FIXTURE" "$out"
fi

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

printf 'Comlink phase one memo .\n' > "$out.txt"
SH
chmod +x "$mock_whisper"

cat > "$mock_pbcopy" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
cat > "$COMLINK_MOCK_CLIPBOARD"
SH
chmod +x "$mock_pbcopy"
printf 'mock model\n' > "$mock_model"

echo "binary: cargo run --"
echo "fixture: $fixture"
echo "mock ffmpeg: $mock_ffmpeg"
echo "mock whisper: $mock_whisper"

COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$fixture" \
COMLINK_WHISPER_CPP="$mock_whisper" \
COMLINK_WHISPER_MODEL="$mock_model" \
cargo run --quiet -- transcribe "$fixture" --mode memo --format json \
  > "$artifact_dir/transcribe-memo.json"

python3 - "$artifact_dir/transcribe-memo.json" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
if data["raw_text"] != "Comlink phase one memo .":
    raise SystemExit(f"unexpected raw text: {data['raw_text']!r}")
if data["final_text"] != "Comlink phase one memo.":
    raise SystemExit(f"unexpected final text: {data['final_text']!r}")
if data["mode"] != "memo":
    raise SystemExit(f"unexpected mode: {data['mode']}")
if data["copied"]:
    raise SystemExit("file transcription should not report copied=true")
PY

printf '\n' | \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FFMPEG_LOG="$ffmpeg_log" \
COMLINK_MOCK_FIXTURE="$fixture" \
COMLINK_WHISPER_CPP="$mock_whisper" \
COMLINK_WHISPER_MODEL="$mock_model" \
COMLINK_PBCOPY="$mock_pbcopy" \
COMLINK_MOCK_CLIPBOARD="$clipboard_file" \
cargo run --quiet -- record --copy --format json \
  > "$artifact_dir/record-copy.json" \
  2> "$artifact_dir/record-copy.err"

python3 - "$artifact_dir/record-copy.json" "$clipboard_file" <<'PY'
import json
import pathlib
import sys

data = json.load(open(sys.argv[1]))
clipboard = pathlib.Path(sys.argv[2]).read_text()
if data["raw_text"] != "Comlink phase one memo .":
    raise SystemExit(f"unexpected raw text: {data['raw_text']!r}")
if data["final_text"] != "Comlink phase one memo.":
    raise SystemExit(f"unexpected final text: {data['final_text']!r}")
if data["text"] != data["final_text"]:
    raise SystemExit("text must match final_text")
if data["mode"] != "memo":
    raise SystemExit(f"unexpected mode: {data['mode']}")
if data["copied"] is not True:
    raise SystemExit("record --copy should report copied=true")
if clipboard != data["final_text"]:
    raise SystemExit(f"clipboard mismatch: {clipboard!r}")
if data["source"]["path"] != "microphone":
    raise SystemExit(f"unexpected source path: {data['source']['path']}")
if not data["segments"]:
    raise SystemExit("segments must not be empty")
PY

grep -q "Recording... press Enter to stop." "$artifact_dir/record-copy.err"
grep -q "Copied final text to clipboard." "$artifact_dir/record-copy.err"
grep -q "Stop-to-final latency:" "$artifact_dir/record-copy.err"
if [ "$(wc -l < "$ffmpeg_log")" -ne 1 ]; then
  echo "record should invoke ffmpeg exactly once" >&2
  exit 1
fi
grep -q "avfoundation" "$ffmpeg_log"

set +e
printf '\n' | \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$fixture" \
COMLINK_MOCK_SHORT_AUDIO=1 \
COMLINK_WHISPER_CPP="$mock_whisper" \
COMLINK_WHISPER_MODEL="$mock_model" \
cargo run --quiet -- record --format json --min-duration-ms 300 \
  > "$artifact_dir/no-speech.out" \
  2> "$artifact_dir/no-speech.err"
no_speech_status=$?
set -e

if [ "$no_speech_status" -ne 4 ]; then
  echo "expected no-speech exit code 4, got $no_speech_status" >&2
  exit 1
fi

grep -q "recording too short or no speech detected" "$artifact_dir/no-speech.err"

set +e
printf '\n' | \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$fixture" \
COMLINK_MOCK_NO_AUDIO_FILE=1 \
COMLINK_WHISPER_CPP="$mock_whisper" \
COMLINK_WHISPER_MODEL="$mock_model" \
cargo run --quiet -- record --format json --min-duration-ms 300 \
  > "$artifact_dir/no-audio-file.out" \
  2> "$artifact_dir/no-audio-file.err"
no_audio_file_status=$?
set -e

if [ "$no_audio_file_status" -ne 4 ]; then
  echo "expected missing recorder output to be treated as no-speech exit code 4, got $no_audio_file_status" >&2
  exit 1
fi

grep -q "recording too short or no speech detected" "$artifact_dir/no-audio-file.err"

set +e
printf '\n' | \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$fixture" \
COMLINK_MOCK_RECORDER_FAIL=1 \
COMLINK_WHISPER_CPP="$mock_whisper" \
COMLINK_WHISPER_MODEL="$mock_model" \
cargo run --quiet -- record --format json \
  > "$artifact_dir/recorder-failed.out" \
  2> "$artifact_dir/recorder-failed.err"
recorder_failed_status=$?
set -e

if [ "$recorder_failed_status" -ne 2 ]; then
  echo "expected recorder failure exit code 2, got $recorder_failed_status" >&2
  exit 1
fi

grep -q "mock microphone permission denied" "$artifact_dir/recorder-failed.err"
if grep -qi "broken pipe" "$artifact_dir/recorder-failed.err"; then
  echo "recorder failure should surface ffmpeg stderr, not broken pipe" >&2
  exit 1
fi

echo "Phase 1 E2E passed. Artifacts: $artifact_dir"
