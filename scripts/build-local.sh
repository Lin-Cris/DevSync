#!/bin/zsh
set -euo pipefail

command -v bun >/dev/null || { print -u2 "Bun is required."; exit 1; }
command -v cargo >/dev/null || { print -u2 "Rust/Cargo is required."; exit 1; }
command -v xcodebuild >/dev/null || { print -u2 "Xcode is required."; exit 1; }

bun run build
bun tauri build --debug --bundles app,dmg --config src-tauri/tauri.local.conf.json || {
  print -u2 "DevSync.app may still have been produced; DMG creation is optional for local use."
  exit 1
}
