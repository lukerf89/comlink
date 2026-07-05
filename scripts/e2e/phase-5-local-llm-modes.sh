#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
audio_fixture="$repo_root/tests/fixtures/audio/short.wav"
artifact_dir="$repo_root/docs/validation/artifacts/phase-5"
tmp_dir="$(mktemp -d)"
server_pid=""

cleanup() {
  if [ -n "$server_pid" ]; then
    kill "$server_pid" >/dev/null 2>&1 || true
    wait "$server_pid" >/dev/null 2>&1 || true
  fi
  python3 - "$tmp_dir" "$repo_root" "$artifact_dir" <<'PY' || true
import pathlib
import sys

tmp_dir = sys.argv[1]
repo_root = sys.argv[2]
artifact_dir = pathlib.Path(sys.argv[3])
for path in artifact_dir.glob("*"):
    if path.is_file():
        text = path.read_text(errors="ignore")
        text = text.replace(tmp_dir, "<tmp>").replace(repo_root, "<repo>")
        path.write_text(text)
PY
  rm -rf "$tmp_dir"
}

trap cleanup EXIT

mkdir -p "$artifact_dir"

if ! command -v cargo >/dev/null 2>&1; then
  echo "cargo is required for the Phase 5 E2E suite" >&2
  exit 1
fi

if [ ! -f "$audio_fixture" ]; then
  "$repo_root/scripts/dev/generate-short-fixture.sh"
fi

mock_ffmpeg="$tmp_dir/mock-ffmpeg"
mock_whisper="$tmp_dir/mock-whisper"
mock_model="$tmp_dir/mock-model.bin"
comlink_home="$tmp_dir/comlink-home"
profile_file="$tmp_dir/concise-profile.json"
ollama_request="$artifact_dir/fake-ollama-request.json"
server_port="$(python3 - <<'PY'
import socket

sock = socket.socket()
sock.bind(("127.0.0.1", 0))
print(sock.getsockname()[1])
sock.close()
PY
)"

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

printf 'um turn this into super base prompt .\n' > "$out.txt"
SH
chmod +x "$mock_whisper"
printf 'mock model\n' > "$mock_model"

cat > "$profile_file" <<'JSON'
{
  "name": "concise",
  "summary": "Prefer concise, direct developer prompts.",
  "examples": [
    {
      "input": "please maybe make the tests pass",
      "output": "Make the tests pass."
    }
  ]
}
JSON

python3 - "$server_port" "$ollama_request" <<'PY' &
import http.server
import json
import sys

port = int(sys.argv[1])
request_path = sys.argv[2]

class Handler(http.server.BaseHTTPRequestHandler):
    def do_POST(self):
        length = int(self.headers.get("content-length", "0"))
        body = self.rfile.read(length)
        with open(request_path, "wb") as handle:
            handle.write(body)
        response = json.dumps({"response": "Rewrite from fake Ollama.", "done": True}).encode()
        self.send_response(200)
        self.send_header("content-type", "application/json")
        self.send_header("content-length", str(len(response)))
        self.end_headers()
        self.wfile.write(response)

    def log_message(self, format, *args):
        pass

server = http.server.HTTPServer(("127.0.0.1", port), Handler)
server.serve_forever()
PY
server_pid="$!"
sleep 0.2

echo "binary: cargo run --"
echo "audio fixture: $audio_fixture"
echo "isolated COMLINK_HOME: $comlink_home"
echo "fake Ollama endpoint: http://127.0.0.1:$server_port/api/generate"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- models select tiny --path "$mock_model" \
  > "$artifact_dir/models-select.out"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- vocab add "super base" "Supabase" \
  > "$artifact_dir/vocab-add.out"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- styles import "$profile_file" \
  > "$artifact_dir/styles-import.out"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- modes add prompt \
  --instruction "Convert rough dictation into a concise coding prompt. Preserve facts and technical terms." \
  --deterministic-mode memo \
  --description "Local coding prompt rewrite" \
  --style-profile concise \
  > "$artifact_dir/modes-add.out"

COMLINK_HOME="$comlink_home" \
cargo run --quiet -- modes list --format json \
  > "$artifact_dir/modes-list.json"

python3 - "$artifact_dir/modes-list.json" <<'PY'
import json
import sys

modes = json.load(open(sys.argv[1]))
prompt = next((mode for mode in modes if mode["name"] == "prompt"), None)
if not prompt:
    raise SystemExit("configured prompt mode missing")
if prompt["llm_instruction"] is None:
    raise SystemExit("prompt mode should include llm_instruction")
if prompt["style_profile"] != "concise":
    raise SystemExit(f"unexpected style profile: {prompt}")
PY

COMLINK_HOME="$comlink_home" \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$audio_fixture" \
COMLINK_WHISPER_CPP="$mock_whisper" \
cargo run --quiet -- transcribe "$audio_fixture" --mode prompt --no-llm --format json \
  > "$artifact_dir/transcribe-no-llm.json" \
  2> "$artifact_dir/transcribe-no-llm.err"

python3 - "$artifact_dir/transcribe-no-llm.json" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
if data["mode"] != "prompt":
    raise SystemExit(f"unexpected mode: {data['mode']}")
if data["final_text"] != "turn this into Supabase prompt.":
    raise SystemExit(f"unexpected deterministic fallback: {data['final_text']!r}")
if data["llm"]["status"] != "skipped":
    raise SystemExit(f"--no-llm should mark skipped: {data['llm']}")
PY

COMLINK_HOME="$comlink_home" \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$audio_fixture" \
COMLINK_WHISPER_CPP="$mock_whisper" \
COMLINK_LLM_ENABLED=true \
COMLINK_LLM_PROVIDER=ollama \
COMLINK_LLM_ENDPOINT="http://127.0.0.1:$server_port/api/generate" \
COMLINK_LLM_MODEL=fake-local \
cargo run --quiet -- transcribe "$audio_fixture" --mode prompt --format json \
  > "$artifact_dir/transcribe-fake-ollama.json" \
  2> "$artifact_dir/transcribe-fake-ollama.err"

python3 - "$artifact_dir/transcribe-fake-ollama.json" "$ollama_request" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
request = json.load(open(sys.argv[2]))
if data["final_text"] != "Rewrite from fake Ollama.":
    raise SystemExit(f"LLM rewrite did not apply: {data['final_text']!r}")
if data["llm"]["status"] != "rewritten":
    raise SystemExit(f"unexpected LLM status: {data['llm']}")
record = data["llm"]["request"]
if record["input_kind"] != "text":
    raise SystemExit("LLM request record must be text-only")
if record["context_policy"] != "text-only; no audio; no external context":
    raise SystemExit(f"unexpected context policy: {record}")
if request["model"] != "fake-local":
    raise SystemExit(f"fake Ollama request used wrong model: {request}")
for forbidden in ["audio_path", "source_path", "normalized_sample_rate_hz", "normalized_channels"]:
    if forbidden in request:
        raise SystemExit(f"LLM request should not include {forbidden}")
PY

kill "$server_pid" >/dev/null 2>&1 || true
wait "$server_pid" >/dev/null 2>&1 || true
server_pid=""

COMLINK_HOME="$comlink_home" \
COMLINK_FFMPEG="$mock_ffmpeg" \
COMLINK_MOCK_FIXTURE="$audio_fixture" \
COMLINK_WHISPER_CPP="$mock_whisper" \
COMLINK_LLM_ENABLED=true \
COMLINK_LLM_PROVIDER=ollama \
COMLINK_LLM_ENDPOINT="http://127.0.0.1:$server_port/api/generate" \
COMLINK_LLM_MODEL=fake-local \
cargo run --quiet -- transcribe "$audio_fixture" --mode prompt --format json \
  > "$artifact_dir/transcribe-llm-fallback.json" \
  2> "$artifact_dir/transcribe-llm-fallback.err"

python3 - "$artifact_dir/transcribe-llm-fallback.json" <<'PY'
import json
import sys

data = json.load(open(sys.argv[1]))
if data["final_text"] != "turn this into Supabase prompt.":
    raise SystemExit("LLM failure should preserve deterministic fallback")
if data["llm"]["status"] != "fallback":
    raise SystemExit(f"LLM failure should mark fallback: {data['llm']}")
if not data["warnings"]:
    raise SystemExit("LLM fallback should add a warning")
PY

COMLINK_HOME="$comlink_home" \
COMLINK_LLM_ENABLED=true \
COMLINK_LLM_PROVIDER=ollama \
COMLINK_LLM_ENDPOINT="http://127.0.0.1:$server_port/api/generate" \
COMLINK_LLM_MODEL=fake-local \
cargo run --quiet -- privacy audit --format json \
  > "$artifact_dir/privacy-audit.json"

python3 - "$artifact_dir/privacy-audit.json" <<'PY'
import json
import sys

audit = json.load(open(sys.argv[1]))
if "enabled" not in audit["llm"] or "text-only" not in audit["llm"]:
    raise SystemExit(f"privacy audit should report local LLM posture: {audit['llm']}")
PY

echo "Phase 5 local LLM modes validation passed."
