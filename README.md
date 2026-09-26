# DevSync

Build, sign, and sync Xcode apps to a paired iPhone over Wi-Fi. DevSync uses the signing already configured in your Xcode project.

[![Build DevSync](https://github.com/Lin-Cris/DevSync/actions/workflows/build.yml/badge.svg)](https://github.com/Lin-Cris/DevSync/actions/workflows/build.yml)
[![Latest release](https://img.shields.io/github/v/release/Lin-Cris/DevSync?display_name=tag&sort=semver)](https://github.com/Lin-Cris/DevSync/releases/latest)

[Download for macOS](https://github.com/Lin-Cris/DevSync/releases/latest/download/DevSync-darwin-universal.dmg) · [All releases](https://github.com/Lin-Cris/DevSync/releases/latest)

## What DevSync does

- Finds Xcode projects, workspaces, schemes, and signing metadata.
- Builds with your existing Xcode signing configuration.
- Installs the matching app on your selected iPhone with `devicectl`.
- Offers manual Sync Now, source watching, Auto Sync, and background signing refresh.
- Keeps build logs and cache in DevSync's application data directory.

DevSync does not request your Apple ID, copy certificates, uninstall apps, or delete project files. An optional **Pre-Build Command** runs in the selected project directory before Xcode builds.

## Requirements

- A Mac with Xcode and its command-line tools
- An iPhone paired with and trusted by the Mac
- Signing and capabilities configured in the Xcode project
- Developer Mode enabled on the iPhone when iOS requires it

## Install

Download the [latest macOS release](https://github.com/Lin-Cris/DevSync/releases/latest). The build supports Apple silicon and Intel Macs. If macOS identifies the app as coming from an unidentified developer, Control-click it in Finder and choose **Open**. Release signing and notarization can be enabled by the maintainer in GitHub Actions.

## Build from source

```sh
bun install --frozen-lockfile
bun run devsync:build
```

For development, run `bun tauri dev`. The local macOS app bundle is written to `src-tauri/target/debug/bundle/macos/DevSync.app`. GitHub Actions also builds Linux release artifacts; the Xcode deployment workflow is macOS-first.

## Documentation and contributing

- [Architecture and operational limits](DEVSYNC.md)
- [Contribution guide](CONTRIBUTING.md)

DevSync began as [iloader](https://github.com/nab138/iloader). Legacy sideloading modules remain for compatibility; the active UI and documented workflow are DevSync.

## License

Existing DevSync source, including previously published DevSync changes, is released under the [MIT License](LICENSE). The original iloader branding has [separate conditions](LICENSE-BRANDING). See [commercial-use details](COMMERCIAL-USE.md). Commercial use of future, separately marked original DevSync material requires prior written permission from Lin-Cris; contact the maintainer privately to discuss licensing.
