# Installation


On macOS or Linux (Intel/AMD x86-64 and ARM64):

```sh
curl -fsSL https://github.com/JeremySomsouk/tessera/releases/latest/download/install.sh | sh
tessera
```

The command requires a release containing `install.sh` and `SHA256SUMS`; older releases do not provide these assets. It downloads a prebuilt release and verifies its SHA-256 checksum. No Rust toolchain or `sudo` is needed. macOS installs the complete app with automatic updates in `~/Applications/Tessera.app`; Linux installs the executable in `~/.local/bin`. Both platforms provide the `tessera` command there. If that directory is outside your `PATH`, the installer prints the command to add to your shell startup file:

```sh
export PATH="$HOME/.local/bin:$PATH"
```

On macOS, you can also open Tessera from `~/Applications`. Quit Tessera before rerunning the installer. Bundles are ad hoc signed, not notarized; macOS may ask you to approve the app. Linux requires a graphical desktop with X11/Wayland, OpenGL/EGL and libxkbcommon runtime libraries (Ubuntu 22.04+ on x86-64, Ubuntu 24.04+ on ARM64, or compatible distributions). The installer does not install system packages or agent hooks.

Rerun the installer to update Linux. To install a specific release that includes installer assets:

```sh
curl -fsSL https://github.com/JeremySomsouk/tessera/releases/latest/download/install.sh | TESSERA_VERSION=0.3.1 sh
```

To remove Tessera, first uninstall any agent hooks using the commands below, then remove `~/.local/bin/tessera` and, on macOS, `~/Applications/Tessera.app`. Your saved preferences and history remain.

See [source builds](development.md) and [macOS updates](updates.md).
