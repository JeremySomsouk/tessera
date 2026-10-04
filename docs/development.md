# Development and releases

## Build from source


With Rust stable and Xcode Command Line Tools installed:

```sh
git clone https://github.com/JeremySomsouk/tessera.git
cd tessera
cargo install --path . --locked
tessera
```

This starts the native desktop application. It uses `$SHELL` as a login shell (defaults to `/bin/zsh` on macOS) and preserves shell startup files. Run ordinary commands, Vim, SSH, `claude`, or `codex` directly in a pane.

To build a Finder application and release DMG:

```sh
bash scripts/bundle-macos.sh
open dist/Tessera.app
```

CI produces separate Intel (`Tessera-x86_64.dmg`) and Apple Silicon (`Tessera-arm64.dmg`) artifacts in the **Build and verify** workflow. Open the DMG and drag Tessera to Applications before installing hooks. Bundles are ad-hoc signed, not notarized; macOS may require Open from the application's context menu.

To publish a version, tag the release commit containing this workflow and push the tag:

```sh
git tag v0.3.1
git push origin v0.3.1
```

Pushing a version tag (`v` followed by a digit) runs all checks and builds both DMGs, then publishes them on the [GitHub Releases page](https://github.com/JeremySomsouk/tessera/releases) with checked-in release notes when available, otherwise generated notes. New releases stay in draft until both macOS DMGs, signed update feeds, Linux x86-64/ARM64 archives, `SHA256SUMS`, and `install.sh` upload successfully. Linux x86-64 builds use Ubuntu 22.04; ARM64 builds use Ubuntu 24.04. Release tags require the `SPARKLE_ED25519_PRIVATE_KEY` Actions secret; see [update signing](updates.md). A failed publishing job can be rerun to finish the release. Branch pushes and pull requests upload Actions artifacts only.

## Validation


```sh
cargo fmt --all -- --check
cargo clippy --locked --all-targets -- -D warnings
cargo test --locked
# Includes the local socket transport test on an unrestricted host:
cargo test --locked -- --include-ignored
python3 -m unittest discover -s tests -p 'test_*.py'
sh -n install.sh scripts/package-linux.sh
shellcheck install.sh scripts/package-linux.sh
```

Linux source builds need `libxkbcommon-dev`, `libwayland-dev`, and `libegl1-mesa-dev`. Releases include macOS bundles and Linux binaries. No Windows frontend is implemented.

[Architecture](architecture.md) · [Performance](performance.md) · [Release signing](updates.md)
