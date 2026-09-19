# DevSync Aurora State Field Design QA

Date: 2026-09-19
Reference: user supplied Aurora State Field screenshot
Surface: rebuilt macOS DevSync.app

## Visual checks

- final result: passed
- dark translucent utility window with custom macOS traffic controls: passed
- left navigation hierarchy matches Project, Devices, Signing, Settings: passed
- Project page uses a central animated Aurora state field: passed
- four stage progress rail presents Watch, Build, Sign, Install: passed
- right rail presents Target Device, Automatic signing, and Activity cards: passed
- project picker, device picker, and secondary project details remain interactive: passed
- small viewport layout collapses the right rail and keeps the primary Sync action reachable: passed

## Data checks

- project name and path are read from the persisted DevSync workspace: passed
- signing availability and remaining time are read from the inspected provisioning profile: passed
- device model, OS version, connection state, and transport are read from CoreDevice: passed
- battery percentage is not shown because DevSync does not expose a battery source: passed
- activity rows are persisted from watcher and deployment transitions: passed

## Functional checks

- `bun run build`: passed
- `cargo check --manifest-path src-tauri/Cargo.toml`: passed
- `cargo test --manifest-path src-tauri/Cargo.toml`: passed, 34 tests
- `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`: passed
- packaged `.app` signature verification: passed
- Devices, Signing, Settings navigation: passed in the rebuilt app
- Project details disclosure: passed in the rebuilt app

## Limits

The screenshot reference is a static 1586 × 992 image while the available native display was smaller. The design was checked in the rebuilt native window at the available display size and at the responsive breakpoint. A real Sync Now install was not started during visual verification because it would deploy to the selected iPhone.
