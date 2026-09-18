#!/bin/zsh
set -euo pipefail

command -v bun >/dev/null || { print -u2 "Bun is required."; exit 1; }
command -v cargo >/dev/null || { print -u2 "Rust/Cargo is required."; exit 1; }
command -v xcodebuild >/dev/null || { print -u2 "Xcode is required."; exit 1; }

LOCAL_APP="${PWD}/src-tauri/target/debug/bundle/macos/DevSync.app"

bun run build
if ! bun tauri build --debug --bundles app,dmg --config src-tauri/tauri.local.conf.json; then
  print -u2 "DevSync.app may still have been produced; DMG creation is optional for local use."
fi

if [[ -d "$LOCAL_APP" ]]; then
  # Tauri's unsigned debug bundle can leave only the executable signed. macOS
  # 27 rejects that bundle when launched from Finder or Launch Services.
  codesign --force --deep --sign - "$LOCAL_APP"
  codesign --verify --deep --strict --verbose=2 "$LOCAL_APP"
else
  print -u2 "DevSync.app was not produced."
  exit 1
fi
