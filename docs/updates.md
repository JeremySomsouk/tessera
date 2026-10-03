# macOS updates

Tessera uses Sparkle 2.10.0 for native update checks, Ed25519 verification, background downloads and installation on quit. The framework is fetched from its upstream GitHub release at packaging time, checked against a pinned SHA-256 digest, and copied into `Contents/Frameworks`. Rust loads only that framework from the application bundle and retains the updater on the AppKit main thread. Cargo binaries and non-macOS builds have no updater.

Automatic checking and downloading are enabled by default, with launch and daily checks. Users can change both settings in Workspace & commands. Sparkle owns and persists these settings in macOS defaults. Updates never trigger a forced Tessera restart; application quit remains the point at which shells stop and downloaded updates install. System profiling is disabled.

Each architecture has its own `appcast-arm64.xml` or `appcast-x86_64.xml`, served from the latest GitHub release. Feeds point at version-specific DMG URLs, contain the exact archive size and signature, and use the app's increasing `CFBundleVersion` (`1.minor.patch` for the current 0.x releases). Both feeds and archives are signed; feed signature failures do not expire, and archives must pass signature verification before extraction. GitHub release publication checks for all four assets before ending draft status.

## Release key

The public key in `updates/public-key.txt` is embedded as `SUPublicEDKey`. The corresponding private key is in the maintainer's macOS login Keychain under Sparkle account `fr.somsouk.tessera`. Never commit or print this private key. Keep a secure backup: losing it requires users to manually install an app containing a replacement public key.

Configure the repository Actions secret `SPARKLE_ED25519_PRIVATE_KEY` with the exported Sparkle private seed. The exporter is `bin/generate_keys --account fr.somsouk.tessera -x /path/to/private-file` from the pinned Sparkle distribution. Send the file to `gh secret set SPARKLE_ED25519_PRIVATE_KEY < /path/to/private-file`, then remove the temporary export. Store a backup outside the repository. The secret is supplied only for version-tag packaging jobs, not pull requests. The signing script verifies that the private key matches the committed public key before signing, and verifies both feed and archive signatures after signing. Temporary key exports use mode 0600.

For a local signed build using the login Keychain:

```sh
TESSERA_SIGN_UPDATES=1 bash scripts/bundle-macos.sh
```

Ordinary local/PR builds package the updater but do not generate signed appcasts. Tag builds fail if the release key is missing.

## macOS trust

These bundles are ad hoc signed, not Developer ID signed or notarized. Sparkle's Ed25519 signatures authenticate future updates independently of Apple signing. The first updater-enabled version must be installed manually and may need macOS approval. Installing through Sparkle avoids repeated browser downloads, but does not turn Tessera into Apple-notarized software or guarantee the absence of every Gatekeeper prompt. No code disables Gatekeeper or clears quarantine attributes.

Developer ID signing/notarization can be added later with an Apple Developer Program membership. The current public update key can remain unchanged.

## Verification

Run Rust formatting, Clippy, tests and build as in CI, plus:

```sh
python3 -m unittest discover -s tests -p 'test_*.py'
bash -n scripts/bundle-macos.sh scripts/prepare-sparkle.sh
```

On macOS, signed packaging also verifies the app code signature, DMG checksum, archive signature and appcast signature. The remaining native acceptance check is an end-to-end update from an installed 0.2.2 bundle to a later signed release, including installation on quit and preference persistence.
