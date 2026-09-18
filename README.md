# DevSync

DevSync keeps Xcode apps built, signed, and synced to a paired iPhone over Wi-Fi.

[![Build DevSync](https://github.com/Lin-Cris/DevSync/actions/workflows/build.yml/badge.svg)](https://github.com/Lin-Cris/DevSync/actions/workflows/build.yml)
[![Latest release](https://img.shields.io/github/v/release/Lin-Cris/DevSync?display_name=tag&sort=semver)](https://github.com/Lin-Cris/DevSync/releases/latest)

## Download

### macOS

[**Download DevSync for macOS**](https://github.com/Lin-Cris/DevSync/releases/latest/download/DevSync-macos-universal.dmg) · [View all releases](https://github.com/Lin-Cris/DevSync/releases/latest)

The macOS build is universal for Apple silicon and Intel Macs. If macOS warns that the app is from an unidentified developer, open it from Finder with Control-click → Open. Release signing and notarization can be enabled by the maintainer in GitHub Actions.

## What it does

- Discovers Xcode projects, workspaces, schemes, and signing metadata.
- Builds with the signing already configured in Xcode.
- Installs the fresh matching app artifact to a selected iPhone with `devicectl`.
- Supports manual Sync Now, source watching, Auto Sync, and background refresh.
- Renews development signing before it expires without storing Apple credentials.
- Keeps build logs and cache inside DevSync's application data directory.

DevSync does not ask for an Apple ID, copy certificates, uninstall apps, or delete project files. A user-configured Pre-Build Command is executed in the selected project directory before an Xcode build.

## Requirements

- macOS with Xcode and its command-line tools installed
- An iPhone paired and trusted by the Mac
- Signing and capabilities configured in the Xcode project
- Developer Mode enabled on the iPhone when required by iOS

## Build from source

```sh
bun install --frozen-lockfile
bun run devsync:build
```

For development:

```sh
bun tauri dev
```

The local package is written to `src-tauri/target/debug/bundle/macos/DevSync.app`. GitHub Actions builds release artifacts for macOS, Windows, and Linux; DevSync's Xcode deployment workflow is macOS-first.

## Project notes

The detailed architecture and operational limits are documented in [DEVSYNC.md](DEVSYNC.md). The repository began as [iloader](https://github.com/nab138/iloader) and still contains its legacy sideloading modules for compatibility. The active UI and documented product path are DevSync. Original iloader source attribution and branding restrictions remain in [LICENSE-BRANDING](LICENSE-BRANDING).

## Contributing

Bug reports, focused fixes, and documentation improvements are welcome. Please read [CONTRIBUTING.md](CONTRIBUTING.md) before opening a pull request.

## License

DevSync source changes are released under the MIT License. See [LICENSE](LICENSE). Some legacy source and branding assets retain their original attribution and restrictions; see [LICENSE-BRANDING](LICENSE-BRANDING).
