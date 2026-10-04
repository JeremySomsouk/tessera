#!/bin/sh
# Install a published Tessera release without Rust or administrator privileges.
set -eu

fail() { printf 'Tessera: %s\n' "$*" >&2; exit 1; }
need() { command -v "$1" >/dev/null 2>&1 || fail "Required command missing: $1"; }

main() {
    [ -n "${HOME:-}" ] || fail 'HOME is not set.'
    for tool in curl uname mktemp awk mkdir mv rm chmod; do need "$tool"; done
    platform=$(uname -s)
    machine=$(uname -m)
    case "$machine" in
        arm64|aarch64) architecture=arm64 ;;
        x86_64|amd64) architecture=x86_64 ;;
        *) fail "Unsupported architecture: $machine" ;;
    esac
    case "$platform" in
        Darwin)
            for tool in hdiutil ditto codesign pgrep readlink ln; do need "$tool"; done
            asset="Tessera-$architecture.dmg"
            app="$HOME/Applications/Tessera.app"
            # Replacing a running bundle can interfere with its updater and helpers.
            if pgrep -x tessera >/dev/null 2>&1; then
                fail 'Quit Tessera before installing or updating it.'
            fi
            if [ -e "$app" ] || [ -L "$app" ]; then
                [ ! -L "$app" ] || fail "$app is a symlink; install manually."
                identifier=$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIdentifier' "$app/Contents/Info.plist" 2>/dev/null) || fail "$app is not a Tessera application."
                [ "$identifier" = fr.somsouk.tessera ] || fail "$app belongs to another application."
            fi
            ;;
        Linux) asset="Tessera-linux-$architecture.tar.gz"; need tar ;;
        *) fail "Unsupported operating system: $platform (macOS and Linux only)." ;;
    esac
    if command -v sha256sum >/dev/null 2>&1; then
        checksum_tool=sha256sum
    elif command -v shasum >/dev/null 2>&1; then
        checksum_tool=shasum
    else
        fail 'Install sha256sum or shasum to verify the download.'
    fi
    bin="$HOME/.local/bin"
    [ ! -d "$bin/tessera" ] || fail "$bin/tessera is a directory."
    if [ -L "$bin/tessera" ]; then
        [ "$platform" = Darwin ] || fail "$bin/tessera is an unrelated symlink; move it before installing."
        [ "$(readlink "$bin/tessera")" = "$app/Contents/MacOS/tessera" ] || fail "$bin/tessera is an unrelated symlink; move it before installing."
    fi
    work=$(mktemp -d "${TMPDIR:-/tmp}/tessera-install.XXXXXX")
    mounted=0
    app_stage=
    bin_stage=
    installed=0
    app_replaced=0
    cleanup() {
        if [ "$mounted" = 1 ]; then hdiutil detach "$work/mount" >/dev/null 2>&1 || :; fi
        if [ -n "$app_stage" ]; then
            if [ "$installed" = 0 ]; then
                if [ -d "$app_stage/previous.app" ]; then
                    rm -rf "$app"
                    mv "$app_stage/previous.app" "$app" || return
                elif [ "$app_replaced" = 1 ]; then
                    rm -rf "$app"
                fi
            fi
            rm -rf "$app_stage"
        fi
        [ -z "$bin_stage" ] || rm -rf "$bin_stage"
        rm -rf "$work"
    }
    trap cleanup 0
    trap 'exit 1' HUP INT TERM
    releases=https://github.com/JeremySomsouk/tessera/releases
    version=${TESSERA_VERSION:-}
    if [ -z "$version" ]; then
        # Resolve latest once; all subsequent downloads use that exact tag.
        resolved=$(curl --proto '=https' --proto-redir '=https' -fsSL --retry 2 --connect-timeout 15 --max-time 300 \
            -o /dev/null -w '%{url_effective}' "$releases/latest") || fail 'Cannot find the latest release. Check your connection.'
        case "$resolved" in
            "$releases"/tag/v*) version=${resolved##*/} ;;
            *) fail 'Unexpected latest release URL.' ;;
        esac
    fi
    version=${version#v}
    printf '%s\n' "$version" | awk '/^[0-9]+\.[0-9]+\.[0-9]+$/ { ok=1 } END { exit !ok }' || fail 'Release version must be a version such as 0.2.7.'
    base="$releases/download/v$version"
    download "$base/SHA256SUMS" "$work/SHA256SUMS"
    expected=$(awk -v asset="$asset" '$2 == asset { print $1; count++ } END { if (count != 1) exit 1 }' "$work/SHA256SUMS") || fail "This release does not provide $asset."
    [ "${#expected}" = 64 ] || fail 'Invalid release checksum.'
    case "$expected" in *[!0-9a-fA-F]*) fail 'Invalid release checksum.' ;; esac
    printf 'Downloading %s…\n' "$asset"
    download "$base/$asset" "$work/$asset"
    if [ "$checksum_tool" = sha256sum ]; then
        actual=$(sha256sum "$work/$asset")
    else
        actual=$(shasum -a 256 "$work/$asset")
    fi
    [ "${actual%% *}" = "$expected" ] || fail 'Download checksum mismatch; nothing was installed.'
    mkdir -p "$bin"
    bin_stage=$(mktemp -d "$bin/.tessera-install.XXXXXX")
    if [ "$platform" = Darwin ]; then
        mkdir -p "$work/mount" "$HOME/Applications"
        hdiutil attach "$work/$asset" -readonly -nobrowse -mountpoint "$work/mount" >/dev/null
        mounted=1
        codesign --verify --deep --strict "$work/mount/Tessera.app"
        app_stage=$(mktemp -d "$HOME/Applications/.tessera-install.XXXXXX")
        ditto "$work/mount/Tessera.app" "$app_stage/Tessera.app"
        hdiutil detach "$work/mount" >/dev/null
        mounted=0
        ln -s "$app/Contents/MacOS/tessera" "$bin_stage/tessera"
        if [ -d "$app" ]; then mv "$app" "$app_stage/previous.app"; fi
        mv "$app_stage/Tessera.app" "$app"
        app_replaced=1
        mv -f "$bin_stage/tessera" "$bin/tessera"
        printf 'Installed %s\n' "$app"
    else
        # Extract only the executable, never arbitrary archive paths.
        tar -xzf "$work/$asset" -C "$bin_stage" tessera
        [ -f "$bin_stage/tessera" ] || fail 'Release archive has no tessera executable.'
        [ ! -L "$bin_stage/tessera" ] || fail 'Release executable is a symlink.'
        chmod 755 "$bin_stage/tessera"
        mv -f "$bin_stage/tessera" "$bin/tessera"
    fi
    installed=1
    printf 'Installed %s\n' "$bin/tessera"
    # Print a literal command for the user's shell, preserving its variables.
    # shellcheck disable=SC2016
    case ":${PATH:-}:" in
        *":$bin:"*) printf 'Run: tessera\n' ;;
        *) printf 'Add this to your shell startup file, then open a new terminal:\n  export PATH="$HOME/.local/bin:$PATH"\nRun now: "%s/tessera"\n' "$bin" ;;
    esac
    if [ "$platform" = Darwin ]; then
        printf 'You can also open Tessera from ~/Applications. macOS may ask you to approve the app.\n'
    else
        printf 'A graphical desktop and runtime libraries for X11/Wayland and OpenGL are required.\n'
    fi
}

download() {
    curl --proto '=https' --proto-redir '=https' -fsSL --retry 2 --connect-timeout 15 --max-time 300 -o "$2" "$1" || fail "Cannot download $1. Check your connection and the release assets."
}

# Keep execution at the end so a truncated piped download cannot start installing.
main "$@"
