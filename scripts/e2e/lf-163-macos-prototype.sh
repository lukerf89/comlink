#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
tmp_dir="$(mktemp -d)"
trap 'rm -rf "$tmp_dir"' EXIT
export COMLINK_HOME="$tmp_dir/comlink"
mkdir -p "$COMLINK_HOME"
printf 'Config/data isolation: %s (preview does not read or write config)\n' "$COMLINK_HOME"
swift run --package-path "$repo_root/prototypes/macos" PrototypeChecks
"$repo_root/scripts/dev/preview-macos.sh" --build-only
app_dir="$repo_root/prototypes/macos/.build/Comlink Preview.app"
printf 'Binary: %s\n' "$app_dir/Contents/MacOS/ComlinkPreview"
plutil -lint "$app_dir/Contents/Info.plist"
test -x "$app_dir/Contents/MacOS/ComlinkPreview"
test -z "$(ls -A "$COMLINK_HOME")"
echo 'PASS: native build, state/keyboard tests, app bundle, unused isolated config.'
echo 'Run scripts/dev/preview-macos.sh for the interactive native UI checklist.'
