# Contributing to DevSync

Small, focused pull requests are welcome.

Before opening a pull request:

1. Run `bun run build`.
2. Run `cargo fmt --manifest-path src-tauri/Cargo.toml -- --check`.
3. Run `cargo test --manifest-path src-tauri/Cargo.toml`.
4. Describe any macOS device or Xcode validation that was not available.

DevSync changes should preserve the existing Xcode signing setup and must not
add credentials, provisioning profiles, build products, or personal workspace
data to the repository.
