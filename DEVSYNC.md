# DevSync

DevSync uses the signing already configured in an Xcode project. It never asks
for an Apple ID or stores credentials, certificates, or provisioning-profile
contents.

This repository began as iloader and retains its legacy sideloading modules for
compatibility. DevSync is the active product route; the legacy modules are not
reachable from the DevSync UI.

## Workspace and build pipeline

Each saved workspace records its folder, selected Xcode container and scheme,
optional **Pre-Build Command**, metadata, build provenance, and last install.
The command is a user-provided build-recipe step and runs with that workspace's
folder as its current directory; DevSync does not inspect source code,
`package.json`, or infer commands. Leave it empty for a normal Swift/Xcode
project. For example, configure `npm run ios:bundle` for a project that must
refresh its web bundle before Xcode builds.

Sync Now and Auto Sync share the same deployment coordinator: prepare the
registered project, run its configured Pre-Build Command (if any), run
`xcodebuild` with `generic/platform=iOS`, inspect signing, validate the app's
Bundle ID, and install to the selected iPhone. A non-zero Pre-Build Command
stops this pipeline immediately: Xcode is not launched and no prior app is
installed. The UI reports Preparing, Pre-Build, Xcode Build, Signing,
Installing, and Completed; the Pre-Build stage is omitted when no command is
configured.

Artifact selection accepts only a `.app` whose `CFBundleIdentifier` matches
the workspace **and whose app bundle or Info.plist was written after this
specific build started**. This rejects test runners, stale/wrong-bundle
artifacts, and any product from an earlier invocation.

## Device and install pipeline

Devices are discovered with `xcrun devicectl list devices --json-output`; the
JSON file is the supported stable interface on current Xcode releases. DevSync
records the selected CoreDevice identifier and install uses `devicectl device
install app` without uninstalling first, so normal Xcode update-in-place
behavior can preserve the application container.

Some Xcode releases list a paired local-network device without a reliable
connection state. DevSync marks that state as unknown and performs a
`devicectl device info details` reachability check before deployment. It never
silently replaces a saved device selection; transient CoreDevice installation
failures trigger one fresh device enumeration and a bounded retry, otherwise
the workspace remains waiting for the selected iPhone.

## Signing expiration

After each successful build DevSync decodes the artifact's
`embedded.mobileprovision` with macOS `security cms`. It stores only the team
identifier, profile UUID, app identifier, profile name, expiration time, and
derived status (`valid`, `expiringSoon`, `expired`, or `unknown`). It does not
derive expiry from installation time. The refresh threshold is **24 hours**:
more than 24 hours is healthy; exactly 24 hours or less needs renewal; expired
profiles are urgent. Unknown profiles trigger an inspection build so Xcode can
create and validate a fresh profile. Automatic refresh builds/signs before
checking device reachability; installation waits for an available selected
device, and all work is serialized per workspace.

## Background operation

DevSync runs a conservative 30-minute reconciliation cycle. The centralized
refresh policy requests a rebuild and normal in-place install at 24 hours or
less remaining, immediately for expired profiles, and requests attention when
profile metadata cannot be trusted. If the selected iPhone is unavailable, the
workspace remains `waitingForDevice` and is reconsidered later. The existing
per-workspace operation guard prevents a background refresh from racing a
manual refresh.

Closing the main window hides it while DevSync stays available in the macOS
menu bar. The menu has Open DevSync, Sync All, and Quit; Quit is the only
normal way to terminate the background manager. Launch at Login is an explicit
off-by-default setting backed by macOS LaunchAgent support.

## Source changes and Auto Sync

DevSync watches registered workspace folders using filesystem events. It
recognizes `.swift`, `.m`, `.mm`, `.h`, `.ts`, `.tsx`, `.js`, `.jsx`, `.mjs`,
`.css`, `.html`, `.plist`, `.json`, `.png`, `.jpg`, `.jpeg`, `.heic`, `.svg`,
`.xcodeproj`, `.xcworkspace`, and `Assets.xcassets` changes (plus existing
native project-file types). It ignores `.git`, `DerivedData`, `node_modules`,
`dist`, and `build`, as well as the other existing dependency/build output and
editor-temporary paths. Relevant events are coalesced for five seconds, then
increment a persisted source revision and mark the workspace as having changes.

During any DevSync Build Only, Sync Now, or Auto Sync deployment, that
workspace's watcher ignores source-like filesystem events. It remains muted for
five seconds after the operation to drain delayed filesystem notifications.
This prevents a configured recipe that writes generated JavaScript, CSS, or
HTML back into its project (for example, an iOS web-bundle copy step) from
starting a second deployment. The rule is workspace-generic: it does not name
or inspect any external project or output directory.

Manual mode only surfaces **Changes detected**. With Auto Sync enabled, the
scheduler deploys the latest revision when the selected iPhone is available;
otherwise it stays waiting and avoids duplicate builds. A successful install
only clears dirty state when the deployed revision equals the newest observed
source revision, preventing a build of revision B from clearing a later
revision C.

Normal same-bundle-ID updates never uninstall first. `devicectl device install app`
updates the existing same-bundle-ID app rather than creating a duplicate.
Application-container
preservation remains a manual acceptance check unless harmless app state can be
observed before and after an update.

If Xcode metadata reports a changed bundle ID for a registered workspace,
DevSync marks it for attention and will not silently deploy it as a new app.

## Build storage and cleanup

Every workspace uses one reusable directory at DevSync Application Support:
`DevSync/DerivedData/<workspace-id>`. Every `xcodebuild` invocation uses that
same `-derivedDataPath`, so DevSync does not make one complete build tree per
sync. Xcode's normal incremental products and intermediates remain there.
Build and install logs have fixed names (`DevSync-build.log` and
`DevSync-install.log`) and are replaced by the latest operation; DevSync does
not archive `.app` artifacts separately.

**Advanced → Clean Build Cache** removes only DevSync-managed
`DevSync/DerivedData`. It never touches project source, global Xcode caches,
provisioning/certificates, Apple account configuration, or iPhone apps.

## Current limitations

The build/install workspace flow, structured device discovery, artifact
verification, signing expiry, refresh policy, menu-bar lifecycle,
launch-at-login, and source watching/Auto Sync are implemented. Selective
macOS notifications and menu-bar live workspace summaries are the remaining
background-management work. Legacy iLoader code remains compiled for
compatibility but is not the DevSync UI route. The macOS bundle identifier and
updater endpoint intentionally remain unchanged until a signed release
migration is planned. DevSync does not bypass Apple Personal Team app limits or
delete apps to make room.

## Local packaging and limits

Run `bun run devsync:build` for local packaging. It verifies Bun, Cargo, and
Xcode, builds the frontend, and invokes Tauri with updater artifacts disabled,
so no `TAURI_SIGNING_PRIVATE_KEY` is needed. The local app output is
`src-tauri/target/debug/bundle/macos/DevSync.app`; DMG generation is attempted
but optional.

DevSync never uninstalls an app, changes certificates, or removes another app
to make room. Recognized Apple errors for the free development-app limit are
shown as: **Your iPhone has reached Apple's free Personal Team app limit.**
