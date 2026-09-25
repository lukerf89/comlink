#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
export COMLINK_HOME="$tmp_dir/comlink"
mkdir -p "$COMLINK_HOME"
printf 'Config/data isolation: %s (preview does not read or write CLI config)\n' "$COMLINK_HOME"
swift run --package-path "$repo_root/prototypes/macos" PrototypeChecks
"$repo_root/scripts/dev/preview-macos.sh" --build-only
app_dir="$repo_root/prototypes/macos/.build/Comlink Preview.app"
printf 'Binary: %s\n' "$app_dir/Contents/MacOS/ComlinkPreview"
plutil -lint "$app_dir/Contents/Info.plist"
test -x "$app_dir/Contents/MacOS/ComlinkPreview"
codesign --verify --strict "$app_dir"
"$app_dir/Contents/MacOS/ComlinkPreview" --check-startup > "$tmp_dir/startup.json" 2> "$tmp_dir/startup.err"
if [[ -s "$tmp_dir/startup.err" ]]; then
  cat "$tmp_dir/startup.err" >&2
  exit 1
fi
python3 - "$tmp_dir/startup.json" <<'PYCODE'
import json, sys
with open(sys.argv[1]) as source:
    assert json.load(source) == {"startup": "ok"}
PYCODE
test -z "$(ls -A "$COMLINK_HOME")"
echo 'PASS: native build, state/keyboard tests, app bundle, startup JSON/stderr, unused isolated CLI config.'
echo 'Run scripts/dev/preview-macos.sh for the interactive native UI checklist.'
