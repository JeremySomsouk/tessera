#!/usr/bin/env python3
"""Sign architecture-specific update feeds with Sparkle's own tooling."""
import argparse
import base64
import os
from pathlib import Path
import subprocess
import tempfile
import xml.etree.ElementTree as ET

SPARKLE = "http://www.andymatuschak.org/xml-namespaces/sparkle"
RELEASES = "https://github.com/JeremySomsouk/tessera/releases/download"
ET.register_namespace("sparkle", SPARKLE)


def appcast(version, build, architecture, archive, signature):
    root = ET.Element("rss", {"version": "2.0"})
    channel = ET.SubElement(root, "channel")
    ET.SubElement(channel, "title").text = "Tessera updates"
    item = ET.SubElement(channel, "item")
    ET.SubElement(item, "title").text = f"Tessera {version}"
    ET.SubElement(item, f"{{{SPARKLE}}}version").text = build
    ET.SubElement(item, f"{{{SPARKLE}}}shortVersionString").text = version
    ET.SubElement(item, f"{{{SPARKLE}}}minimumSystemVersion").text = "11.0"
    ET.SubElement(item, "enclosure", {
        "url": f"{RELEASES}/v{version}/Tessera-{architecture}.dmg",
        "length": str(archive.stat().st_size),
        "type": "application/octet-stream",
        f"{{{SPARKLE}}}edSignature": signature,
    })
    return ET.ElementTree(root)


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--sparkle", type=Path, required=True)
    parser.add_argument("--architecture", choices=["arm64", "x86_64"], required=True)
    parser.add_argument("--version", required=True)
    parser.add_argument("--build", required=True)
    parser.add_argument("--archive", type=Path, required=True)
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    key = os.environ.get("SPARKLE_ED25519_PRIVATE_KEY")
    signer = args.sparkle / "bin/sign_update"
    public_key = Path("updates/public-key.txt").read_text().strip()
    # Nothing private is passed on argv or written inside the repository.
    with tempfile.TemporaryDirectory(prefix="tessera-signing-") as temporary:
        if key:
            key_path = Path(temporary) / "private-key"
            key_path.write_text(key)
            key_path.chmod(0o600)
            account = "fr.somsouk.tessera.ci"
            result = subprocess.run([
                str(args.sparkle / "bin/generate_keys"), "--account", account, "-f", str(key_path)
            ], capture_output=True, text=True, check=True)
            if public_key not in result.stdout:
                raise ValueError("Signing key does not match updates/public-key.txt")
            signing = ["--ed-key-file", str(key_path)]
        else:
            result = subprocess.run([
                str(args.sparkle / "bin/generate_keys"), "--account", "fr.somsouk.tessera", "-p"
            ], capture_output=True, text=True, check=True)
            if result.stdout.strip() != public_key:
                raise ValueError("Keychain key does not match updates/public-key.txt")
            signing = ["--account", "fr.somsouk.tessera"]
        signature = subprocess.check_output([str(signer), *signing, "-p", str(args.archive)], text=True).strip()
        if len(base64.b64decode(signature, validate=True)) != 64:
            raise ValueError("Invalid archive signature")
        tree = appcast(args.version, args.build, args.architecture, args.archive, signature)
        ET.indent(tree)
        tree.write(args.output, encoding="utf-8", xml_declaration=True)
        subprocess.run([str(signer), *signing, "-p", str(args.output)], check=True)
        subprocess.run([str(signer), *signing, "--verify", str(args.output)], check=True)
        subprocess.run([str(signer), *signing, "--verify", str(args.archive), signature], check=True)
    print(f"Signed and verified {args.output}")


if __name__ == "__main__":
    main()
