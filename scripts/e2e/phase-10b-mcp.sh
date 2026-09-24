#!/usr/bin/env bash
set -euo pipefail

# Phase 10b E2E: local stdio MCP server (`comlink mcp`).
# A python3 driver launches `comlink mcp` the way an MCP client does, speaks
# JSON-RPC over its stdin/stdout, and runs: initialize -> tools/list ->
# meeting_start (refused) -> `comlink config set mcp.allow_start true` (no
# server restart) -> meeting_start -> meeting_status -> meeting_stop -> poll
# meeting_status until stopped -> meeting_get_transcript md + json ->
# resources/list + resources/read md + json -> meeting_list, then a
# near-silent meeting whose warning must reach the agent.
#
# Fails on: any stdout byte that is not a JSON-RPC 2.0 frame, any server
# stderr, missing exports, an open network socket on the live server
# (`lsof -i`), or an HTTP stack (hyper/reqwest/axum) in the rmcp dependency
# tree. Isolated temp config/data dir, mock recorder/whisper.

export COMLINK_RECORD_DEVICE="${COMLINK_RECORD_DEVICE:-:0}"
unset COMLINK_MCP_ALLOW_START

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
artifact_dir="$repo_root/docs/validation/artifacts/phase-10b"
tmp_dir="$(mktemp -d)"

cleanup() {
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
  echo "phase-10b E2E failed: $*" >&2
  exit 1
}

for tool in cargo jq python3 lsof; do
  command -v "$tool" >/dev/null 2>&1 || fail "$tool is required"
done

mkdir -p "$artifact_dir"
rm -f "$artifact_dir"/*

cd "$repo_root"
cargo build --quiet
binary="$repo_root/target/debug/comlink"

# No HTTP stack may be compiled in with the MCP SDK.
cargo tree -e features -i rmcp > "$artifact_dir/cargo-tree-rmcp.txt"
cargo tree -e normal > "$artifact_dir/cargo-tree.txt"
if grep -E -i '(^|[^a-z])(hyper|reqwest|axum)( |$|v)' "$artifact_dir/cargo-tree.txt"; then
  fail "an HTTP crate (hyper/reqwest/axum) is in the dependency tree"
fi

mock_ffmpeg="$tmp_dir/mock-ffmpeg"
mock_ffprobe="$tmp_dir/mock-ffprobe"
mock_whisper="$tmp_dir/mock-whisper"
mock_model="$tmp_dir/mock-model.bin"
comlink_home="$tmp_dir/comlink-home"
comlink_data="$tmp_dir/comlink-data"
silence_fixture="$repo_root/tests/fixtures/audio/silence.wav"

cat > "$mock_ffmpeg" <<'SH'
#!/usr/bin/env bash
set -euo pipefail
out="${@: -1}"
mkdir -p "$(dirname "$out")"
for index in 0 1; do
  chunk="$(printf "$out" "$index")"
  if [ -n "${COMLINK_MOCK_SILENT_WAV:-}" ] && [ -f "${COMLINK_MOCK_SILENT_WAV}.on" ]; then
    cp "$COMLINK_MOCK_SILENT_WAV" "$chunk"
  else
    printf 'mock wav %s\n' "$index" > "$chunk"
  fi
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
index="$(basename "$wav" .wav)"
printf 'Phase 10b fixture segment %s.\n' "${index#chunk-}" > "$out.txt"
SH

chmod +x "$mock_ffmpeg" "$mock_ffprobe" "$mock_whisper"
printf 'mock model\n' > "$mock_model"
# The silent-capture switch: the mock recorder copies the silence fixture
# while "$silent_wav.on" exists.
silent_wav="$tmp_dir/silence.wav"
cp "$silence_fixture" "$silent_wav"

echo "binary: $binary"
echo "config: $comlink_home/config.json"
echo "data: $comlink_data"
echo "artifacts: $artifact_dir"

export COMLINK_HOME="$comlink_home"
export COMLINK_DATA_DIR="$comlink_data"
export COMLINK_FFMPEG="$mock_ffmpeg"
export COMLINK_FFPROBE="$mock_ffprobe"
export COMLINK_WHISPER_CPP="$mock_whisper"
export COMLINK_WHISPER_MODEL="$mock_model"
export COMLINK_LLM_ENABLED=false
export COMLINK_MOCK_SILENT_WAV="$silent_wav"

python3 - "$binary" "$artifact_dir" "$silent_wav" <<'PY'
import json
import os
import subprocess
import sys
import threading
import time
import queue

binary, artifact_dir, silent_wav = sys.argv[1], sys.argv[2], sys.argv[3]


def fail(message):
    print(f"phase-10b E2E failed: {message}", file=sys.stderr)
    sys.exit(1)


def artifact(name, text):
    with open(os.path.join(artifact_dir, name), "w") as handle:
        handle.write(text)


stderr_path = os.path.join(artifact_dir, "mcp.stderr")
stderr_file = open(stderr_path, "wb")
server = subprocess.Popen(
    [binary, "mcp"],
    stdin=subprocess.PIPE,
    stdout=subprocess.PIPE,
    stderr=stderr_file,
)
print(f"mcp server pid: {server.pid}")

raw_stdout = []
lines = queue.Queue()


def pump():
    for raw in iter(server.stdout.readline, b""):
        raw_stdout.append(raw)
        lines.put(raw)
    lines.put(None)


threading.Thread(target=pump, daemon=True).start()
next_id = [1]
frames = []


def send(message):
    server.stdin.write((json.dumps(message) + "\n").encode())
    server.stdin.flush()


def request(method, params):
    request_id = next_id[0]
    next_id[0] += 1
    send({"jsonrpc": "2.0", "id": request_id, "method": method, "params": params})
    deadline = time.time() + 60
    while time.time() < deadline:
        try:
            raw = lines.get(timeout=1)
        except queue.Empty:
            continue
        if raw is None:
            fail(f"server closed stdout while waiting for {method}")
        message = json.loads(raw)
        frames.append(message)
        if message.get("id") == request_id:
            return message
    fail(f"timed out waiting for {method}")


def call(name, arguments):
    response = request("tools/call", {"name": name, "arguments": arguments})
    if "result" not in response:
        fail(f"{name} was a protocol error: {response}")
    return response["result"]


def ok(name, arguments):
    result = call(name, arguments)
    if result.get("isError") is not False:
        fail(f"{name} failed: {json.dumps(result)}")
    return result["structuredContent"]


def poll_stopped(session_id):
    for _ in range(600):
        status = ok("meeting_status", {"id": session_id})
        if status["status"] == "stopped":
            return status
        if status["status"] == "failed":
            fail(f"finalize failed: {status}")
        time.sleep(0.1)
    fail(f"{session_id} never reached stopped")


init = request(
    "initialize",
    {
        "protocolVersion": "2025-06-18",
        "capabilities": {},
        "clientInfo": {"name": "phase-10b-e2e", "version": "0"},
    },
)
if init["result"]["protocolVersion"] != "2025-06-18":
    fail(f"unexpected protocol version {init}")
send({"jsonrpc": "2.0", "method": "notifications/initialized"})
artifact("initialize.json", json.dumps(init, indent=2))

# No network sockets while the server is live.
lsof = subprocess.run(
    ["lsof", "-a", "-p", str(server.pid), "-i"], capture_output=True, text=True
)
artifact("lsof-network.txt", lsof.stdout + lsof.stderr)
if lsof.stdout.strip():
    fail(f"comlink mcp has network sockets open:\n{lsof.stdout}")

tools = request("tools/list", {})
names = sorted(tool["name"] for tool in tools["result"]["tools"])
expected = ["meeting_get_transcript", "meeting_list", "meeting_start", "meeting_status", "meeting_stop"]
if names != expected:
    fail(f"tools/list names {names}")
annotations = {tool["name"]: tool["annotations"] for tool in tools["result"]["tools"]}
if annotations["meeting_stop"]["destructiveHint"] is not True:
    fail("meeting_stop must be destructive")
if annotations["meeting_start"]["readOnlyHint"] is not False:
    fail("meeting_start must not be read-only")
for name in ["meeting_status", "meeting_get_transcript", "meeting_list"]:
    if annotations[name]["readOnlyHint"] is not True:
        fail(f"{name} must be read-only")
artifact("tools-list.json", json.dumps(tools, indent=2))

start_args = {"source": "mic-only", "mode": "raw", "no_llm": True}
refused = call("meeting_start", start_args)
if refused.get("isError") is not True or refused["structuredContent"]["error_code"] != "mcp_start_disabled":
    fail(f"meeting_start was not refused: {refused}")
if "comlink config set mcp.allow_start true" not in refused["structuredContent"]["message"]:
    fail("refusal does not name the config command")
artifact("start-refused.json", json.dumps(refused, indent=2))

config_set = subprocess.run(
    [binary, "config", "set", "mcp.allow_start", "true"], capture_output=True, text=True
)
if config_set.returncode != 0 or config_set.stderr:
    fail(f"config set failed: {config_set.stderr}")

started = call("meeting_start", start_args)
if started.get("isError") is not False:
    fail(f"meeting_start failed after opt-in: {started}")
if "Consent reminder" not in started["content"][0]["text"]:
    fail("consent reminder is not the first content block")
session_id = started["structuredContent"]["session_id"]
artifact("start.json", json.dumps(started, indent=2))
time.sleep(0.5)

status = ok("meeting_status", {})
if status["status"] != "recording" or status["session_id"] != session_id:
    fail(f"unexpected status {status}")
for key in ["stale", "stale_reason", "error", "finalize_log", "warnings", "audio_level"]:
    if key not in status:
        fail(f"meeting_status lacks {key}")
artifact("status-recording.json", json.dumps(status, indent=2))

stopped = ok("meeting_stop", {})
if stopped["status"] != "transcribing":
    fail(f"meeting_stop did not detach: {stopped}")
artifact("stop.json", json.dumps(stopped, indent=2))

final = poll_stopped(session_id)
artifact("status-final.json", json.dumps(final, indent=2))

markdown = ok("meeting_get_transcript", {"format": "md"})
if "Phase 10b fixture segment 00000" not in markdown["content"]:
    fail("markdown transcript is missing the fixture text")
artifact("transcript.md", markdown["content"])
as_json = ok("meeting_get_transcript", {"id": session_id, "format": "json"})
if not isinstance(as_json["content"], dict) or as_json["content"]["schema_version"] != "comlink.meeting.v1":
    fail("json transcript content is not a comlink.meeting.v1 object")
artifact("transcript.json", json.dumps(as_json, indent=2))

resources = request("resources/list", {})
uris = [resource["uri"] for resource in resources["result"]["resources"]]
for suffix in ["md", "json"]:
    uri = f"comlink://meetings/{session_id}/transcript.{suffix}"
    if uri not in uris:
        fail(f"{uri} not listed")
    read = request("resources/read", {"uri": uri})
    if "result" not in read:
        fail(f"resources/read {uri} failed: {read}")
    artifact(f"resource-transcript.{suffix}", read["result"]["contents"][0]["text"])

for path_key in ["json_export", "markdown_export"]:
    path = started["structuredContent"][path_key]
    if not os.path.isfile(path):
        fail(f"missing export {path}")

listed = ok("meeting_list", {})
if listed["sessions"][0]["session_id"] != session_id:
    fail("meeting_list does not show the meeting")
artifact("list.json", json.dumps(listed, indent=2))

# A near-silent meeting: the warning must reach the agent.
open(silent_wav + ".on", "w").close()
silent = ok("meeting_start", start_args)
time.sleep(0.5)
ok("meeting_stop", {})
silent_status = poll_stopped(silent["session_id"])
os.remove(silent_wav + ".on")
silent_transcript = ok("meeting_get_transcript", {"format": "md"})
for label, payload in [("status", silent_status), ("transcript", silent_transcript)]:
    if not any("near-silent" in warning for warning in payload["warnings"]):
        fail(f"near-silent warning missing from meeting_{label}: {payload['warnings']}")
artifact("silent-transcript.json", json.dumps(silent_transcript, indent=2))

# Error mapping stays a tool error, never a crash.
unknown = call("meeting_status", {"id": "no-such-meeting"})
if unknown.get("isError") is not True or unknown["structuredContent"]["error_code"] != "meeting_session_not_found":
    fail(f"unknown id not mapped: {unknown}")

server.stdin.close()
code = server.wait(timeout=30)
stderr_file.close()
time.sleep(0.2)
if code != 0:
    fail(f"comlink mcp exited {code}")

# Every stdout byte must be a JSON-RPC 2.0 frame.
for raw in raw_stdout:
    try:
        message = json.loads(raw)
    except ValueError:
        fail(f"non-JSON stdout: {raw!r}")
    if message.get("jsonrpc") != "2.0":
        fail(f"not JSON-RPC 2.0: {raw!r}")
    if "id" in message and ("result" in message) == ("error" in message):
        fail(f"malformed response: {raw!r}")
artifact("frames.jsonl", "".join(raw.decode() for raw in raw_stdout))

stderr_text = open(stderr_path).read()
if stderr_text:
    fail(f"comlink mcp wrote to stderr:\n{stderr_text}")
print(f"phase-10b E2E: {len(raw_stdout)} JSON-RPC frames, 0 stderr bytes, no sockets")
PY

# The CLI sees the MCP-created meetings.
if ! "$binary" meet export --format md > "$artifact_dir/cli-export.md" 2> "$artifact_dir/cli-export.err"; then
  fail "CLI could not export the MCP-created meeting"
fi
"$binary" privacy audit --format json > "$artifact_dir/privacy-audit.json"
jq -e '.mcp.transport == "stdio" and .mcp.network_listener == false and .mcp.allow_start == true' \
  "$artifact_dir/privacy-audit.json" >/dev/null || fail "privacy audit does not report MCP state"

echo "phase-10b E2E passed"
