#!/bin/sh
set -eu
cd "$(dirname "$0")/.."
[ "$(uname -s)" = Linux ] || { echo 'Linux is required to package a Linux release.' >&2; exit 1; }
case "$(uname -m)" in
    x86_64) architecture=x86_64 ;;
    aarch64|arm64) architecture=arm64 ;;
    *) echo 'Unsupported Linux architecture.' >&2; exit 1 ;;
esac
cargo build --release --locked
mkdir -p dist
tar -czf "dist/Tessera-linux-$architecture.tar.gz" -C target/release tessera
