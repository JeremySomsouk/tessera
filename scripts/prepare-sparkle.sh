#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
version=2.10.0
checksum=c2bf58aa8387266ac179357b1415d6f2635f044da8be41042af32425dae6da0c
root="$PWD/target/sparkle/$version"
if [[ ! -f "$root/.verified-$checksum" ]]; then
  mkdir -p "$root"
  archive="$root/Sparkle.tar.xz"
  curl --fail --location --retry 3 --connect-timeout 15 --max-time 180 \
    "https://github.com/sparkle-project/Sparkle/releases/download/$version/Sparkle-$version.tar.xz" -o "$archive"
  actual=$(shasum -a 256 "$archive")
  test "${actual%% *}" = "$checksum"
  tar -xf "$archive" -C "$root"
  test -f "$root/Sparkle.framework/Sparkle"
  test -x "$root/bin/sign_update"
  touch "$root/.verified-$checksum"
fi
printf '%s\n' "$root"
