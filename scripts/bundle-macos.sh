#!/bin/bash
set -euo pipefail
cd "$(dirname "$0")/.."
if [[ "$(uname -s)" != Darwin ]]; then
  echo "macOS is required to build an application bundle." >&2
  exit 1
fi
sparkle_root=$(bash scripts/prepare-sparkle.sh)
cargo build --release --locked
package_id=$(cargo pkgid --locked)
bundle_version=${package_id##*#}
bundle_version=${bundle_version##*@}
if [[ ! "$bundle_version" =~ ^[0-9]+\.[0-9]+\.[0-9]+$ ]]; then
  echo "Bundle version must be a numeric release version: $bundle_version" >&2
  exit 1
fi
# Preserve increasing build versions from the original build 1 while releases are 0.x.
IFS=. read -r release_major release_minor release_patch <<< "$bundle_version"
bundle_build="$((release_major + 1)).$release_minor.$release_patch"
bundle="dist/Tessera.app"
mkdir -p "$bundle/Contents/MacOS" "$bundle/Contents/Resources" "$bundle/Contents/Frameworks"
ditto "$sparkle_root/Sparkle.framework" "$bundle/Contents/Frameworks/Sparkle.framework"
cp "$sparkle_root/LICENSE" "$bundle/Contents/Resources/Sparkle-LICENSE.txt"
cp target/release/tessera "$bundle/Contents/MacOS/tessera"
# Build the complete macOS icon family from the committed 1024px PNG.
icon_work=$(mktemp -d "dist/.tessera-icon.XXXXXX")
trap 'rm -rf "$icon_work"' EXIT
iconset="$icon_work/Tessera.iconset"
mkdir -p "$iconset"
for size in 16 32 128 256 512; do
  sips -z "$size" "$size" assets/app-icon.png --out "$iconset/icon_${size}x${size}.png" >/dev/null
  retina=$((size * 2))
  sips -z "$retina" "$retina" assets/app-icon.png --out "$iconset/icon_${size}x${size}@2x.png" >/dev/null
done
iconutil -c icns "$iconset" -o "$bundle/Contents/Resources/Tessera.icns"
architecture=$(uname -m)
public_key=$(cat updates/public-key.txt)
cat > "$bundle/Contents/Info.plist" <<PLIST
<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleExecutable</key><string>tessera</string>
<key>CFBundleIdentifier</key><string>fr.somsouk.tessera</string>
<key>CFBundleName</key><string>Tessera</string>
<key>CFBundleIconFile</key><string>Tessera.icns</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleShortVersionString</key><string>$bundle_version</string>
<key>CFBundleVersion</key><string>$bundle_build</string>
<key>NSHighResolutionCapable</key><true/>
<key>SUFeedURL</key><string>https://github.com/JeremySomsouk/tessera/releases/latest/download/appcast-$architecture.xml</string>
<key>SUPublicEDKey</key><string>$public_key</string>
<key>SUEnableAutomaticChecks</key><true/>
<key>SUAutomaticallyUpdate</key><true/>
<key>SUEnableSystemProfiling</key><false/>
<key>SUVerifyUpdateBeforeExtraction</key><true/>
<key>SURequireSignedFeed</key><true/>
<key>SUSignedFeedFailureExpirationInterval</key><real>0</real>
<key>LSMinimumSystemVersion</key><string>11.0</string>
</dict></plist>
PLIST
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleShortVersionString' "$bundle/Contents/Info.plist")" = "$bundle_version"
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleVersion' "$bundle/Contents/Info.plist")" = "$bundle_build"
# Verify that the bundle points at a complete, decodable icon before signing.
test "$(/usr/libexec/PlistBuddy -c 'Print :CFBundleIconFile' "$bundle/Contents/Info.plist")" = Tessera.icns
iconutil -c iconset "$bundle/Contents/Resources/Tessera.icns" -o "$icon_work/verified.iconset"
test -s "$icon_work/verified.iconset/icon_512x512@2x.png"
codesign --force --deep --sign - "$bundle"
codesign --verify --deep --strict "$bundle"
# Stage only the application and install shortcut, keeping build files out of the DMG.
dmg_root="$icon_work/dmg"
mkdir -p "$dmg_root"
ditto "$bundle" "$dmg_root/Tessera.app"
ln -s /Applications "$dmg_root/Applications"
dmg="dist/Tessera-$(uname -m).dmg"
hdiutil create -volname Tessera -srcfolder "$dmg_root" -fs HFS+ -format UDZO -ov "$dmg"
hdiutil verify "$dmg"
if [[ -n "${SPARKLE_ED25519_PRIVATE_KEY:-}" || "${TESSERA_SIGN_UPDATES:-0}" == 1 ]]; then
  python3 scripts/create-appcast.py --sparkle "$sparkle_root" --architecture "$architecture" \
    --version "$bundle_version" --build "$bundle_build" --archive "$dmg" \
    --output "dist/appcast-$architecture.xml"
elif [[ "${GITHUB_REF:-}" == refs/tags/v* ]]; then
  echo "Release requires SPARKLE_ED25519_PRIVATE_KEY to sign automatic updates." >&2
  exit 1
fi
echo "Created $bundle and $dmg"
