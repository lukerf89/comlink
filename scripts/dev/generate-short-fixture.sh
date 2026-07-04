#!/usr/bin/env bash
set -euo pipefail

repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
out="$repo_root/tests/fixtures/audio/short.wav"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT

if ! command -v ffmpeg >/dev/null 2>&1; then
  echo "ffmpeg is required to generate tests/fixtures/audio/short.wav" >&2
  exit 1
fi

if command -v say >/dev/null 2>&1; then
  say -o "$tmp_dir/short.aiff" "Comlink phase zero fixture."
  ffmpeg -hide_banner -loglevel error -y -i "$tmp_dir/short.aiff" -ac 1 -ar 16000 "$out"
else
  ffmpeg -hide_banner -loglevel error -y \
    -f lavfi -i "sine=frequency=880:duration=1.0" \
    -ac 1 -ar 16000 "$out"
fi

echo "wrote $out"
