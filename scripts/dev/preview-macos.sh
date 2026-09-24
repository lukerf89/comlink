#!/usr/bin/env bash
set -euo pipefail
repo_root="$(cd "$(dirname "${BASH_SOURCE[0]}")/../.." && pwd)"
if [[ "$(uname -s)" != Darwin ]]; then
  echo "The native design preview requires macOS 14 or newer and Swift 5.9+." >&2
  exit 1
fi
swift build --package-path "$repo_root/prototypes/macos"
bin_dir="$(swift build --package-path "$repo_root/prototypes/macos" --show-bin-path)"
app_dir="$repo_root/prototypes/macos/.build/Comlink Preview.app"
mkdir -p "$app_dir/Contents/MacOS"
cp "$bin_dir/ComlinkPreview" "$app_dir/Contents/MacOS/ComlinkPreview"
cat > "$app_dir/Contents/Info.plist" <<'PLIST'
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>ComlinkPreview</string>
<key>CFBundleIdentifier</key><string>dev.comlink.design-preview</string>
<key>CFBundleName</key><string>Comlink Preview</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleVersion</key><string>1</string>
<key>LSMinimumSystemVersion</key><string>14.0</string>
<key>LSUIElement</key><true/>
<key>NSHighResolutionCapable</key><true/>
</dict></plist>
PLIST
printf 'Preview app: %s\n' "$app_dir"
if [[ "${1:-}" != --build-only ]]; then
  open "$app_dir"
fi
