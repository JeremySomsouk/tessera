"""Exercise the real POSIX installer with offline release and system fixtures."""
import hashlib
import io
import os
import shutil
from pathlib import Path
import subprocess
import tarfile
import tempfile
import unittest

INSTALLER = Path(__file__).resolve().parents[1] / "install.sh"
RELEASES = "https://github.com/JeremySomsouk/tessera/releases"
MOCK = r'''#!/usr/bin/env python3
import os, pathlib, plistlib, shutil, subprocess, sys
command = pathlib.Path(sys.argv[0]).name
args = sys.argv[1:]
root = pathlib.Path(os.environ["FIXTURE"])
with (root / "calls").open("a") as log:
    log.write(command + " " + " ".join(args) + "\n")
if command == "uname":
    print(os.environ["PLATFORM"] if args == ["-s"] else os.environ["ARCH"])
elif command == "curl":
    if os.environ.get("NETWORK_FAILURE"):
        sys.exit(22)
    if args[-1].endswith("/latest"):
        print("https://github.com/JeremySomsouk/tessera/releases/tag/v9.8.7", end="")
    else:
        source = root / args[-1].rsplit("/", 1)[-1]
        if not source.exists():
            sys.exit(22)
        shutil.copyfile(source, args[args.index("-o") + 1])
elif command == "pgrep":
    sys.exit(0 if os.environ.get("RUNNING") else 1)
elif command == "hdiutil":
    if args[0] == "attach":
        app = pathlib.Path(args[args.index("-mountpoint") + 1]) / "Tessera.app"
        (app / "Contents/MacOS").mkdir(parents=True)
        binary = app / "Contents/MacOS/tessera"
        binary.write_text("#!/bin/sh\necho Tessera\n")
        binary.chmod(0o755)
        (app / "Contents/Info.plist").write_bytes(plistlib.dumps({"CFBundleIdentifier": "fr.somsouk.tessera"}))
    else:
        shutil.rmtree(args[1])
elif command == "mv":
    if os.environ.get("LINK_MOVE_FAILURE") and args[-1].endswith("/.local/bin/tessera"):
        sys.exit(1)
    sys.exit(subprocess.run(["/bin/mv", *args]).returncode)
elif command == "ditto":
    if os.environ.get("COPY_FAILURE"):
        sys.exit(1)
    shutil.copytree(*args, symlinks=True)
elif command == "codesign":
    sys.exit(1 if os.environ.get("BAD_SIGNATURE") else 0)
'''


class InstallerTests(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.home = self.root / "home with spaces"
        self.home.mkdir()
        self.mockbin = self.root / "mockbin"
        self.mockbin.mkdir()
        for tool in ("uname", "curl", "pgrep", "hdiutil", "ditto", "codesign", "mv"):
            script = self.mockbin / tool
            script.write_text(MOCK)
            script.chmod(0o755)
        self.env = dict(os.environ, HOME=str(self.home),
                        PATH=f"{self.mockbin}:{os.environ['PATH']}",
                        FIXTURE=str(self.root), PLATFORM="Linux", ARCH="x86_64",
                        TMPDIR=str(self.root))
        self.env.pop("TESSERA_VERSION", None)
        self.binary = self.home / ".local/bin/tessera"

    def release(self, platform="Linux", arch="x86_64", corrupt=False):
        self.env.update(PLATFORM=platform, ARCH=arch)
        normalized = "arm64" if arch in ("arm64", "aarch64") else arch
        asset = f"Tessera-{normalized}.dmg" if platform == "Darwin" else f"Tessera-linux-{normalized}.tar.gz"
        archive = self.root / asset
        if platform == "Darwin":
            archive.write_bytes(b"DMG fixture")
        else:
            payload = b"#!/bin/sh\necho Tessera\n"
            with tarfile.open(archive, "w:gz") as tar:
                entry = tarfile.TarInfo("tessera")
                entry.size = len(payload)
                entry.mode = 0o755
                tar.addfile(entry, io.BytesIO(payload))
        digest = "0" * 64 if corrupt else hashlib.sha256(archive.read_bytes()).hexdigest()
        (self.root / "SHA256SUMS").write_text(f"{digest}  {asset}\n")

    def run_installer(self, source=None):
        result = subprocess.run(["sh"], input=source or INSTALLER.read_text(),
                                env=self.env, text=True, capture_output=True)
        self.assertFalse(list(self.root.glob("tessera-install.*")))
        self.assertFalse(list(self.home.glob("Applications/.tessera-install.*")))
        self.assertFalse(list(self.home.glob(".local/bin/.tessera-install.*")))
        return result

    def test_linux_architectures_and_reinstall(self):
        for arch in ("x86_64", "aarch64"):
            with self.subTest(arch=arch):
                self.release(arch=arch)
                result = self.run_installer()
                self.assertEqual(result.returncode, 0, result.stderr)
                self.assertTrue(os.access(self.binary, os.X_OK))
                self.assertIn('export PATH="$HOME/.local/bin:$PATH"', result.stdout)
                self.assertEqual(subprocess.check_output([self.binary], text=True).strip(), "Tessera")
                self.assertEqual(self.run_installer().returncode, 0)
        calls = (self.root / "calls").read_text()
        self.assertIn(f"{RELEASES}/download/v9.8.7/SHA256SUMS", calls)
        self.assertNotIn("latest/download", calls)

    def test_macos_architectures_install_bundle_and_stable_link(self):
        for arch in ("x86_64", "arm64"):
            with self.subTest(arch=arch):
                self.release(platform="Darwin", arch=arch)
                result = self.run_installer()
                self.assertEqual(result.returncode, 0, result.stderr)
                target = self.home / "Applications/Tessera.app/Contents/MacOS/tessera"
                self.assertEqual(self.binary.resolve(), target.resolve())
                self.assertIn("hdiutil detach", (self.root / "calls").read_text())
                self.binary.unlink()
                shutil.rmtree(self.home / "Applications/Tessera.app")

    def test_checksum_failure_preserves_existing_binary(self):
        self.release(corrupt=True)
        self.binary.parent.mkdir(parents=True)
        self.binary.write_text("existing install")
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("checksum mismatch", result.stderr)
        self.assertEqual(self.binary.read_text(), "existing install")

    def test_missing_platform_asset(self):
        self.release()
        (self.root / "SHA256SUMS").write_text("0" * 64 + "  unrelated.tar.gz\n")
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("does not provide", result.stderr)
        self.assertFalse(self.binary.exists())

    def test_pinned_release(self):
        self.release()
        self.env["TESSERA_VERSION"] = "v1.2.3"
        result = self.run_installer()
        self.assertEqual(result.returncode, 0, result.stderr)
        calls = (self.root / "calls").read_text()
        self.assertNotIn(f"{RELEASES}/latest", calls)
        self.assertIn(f"{RELEASES}/download/v1.2.3/", calls)

    def test_unsupported_platform_or_architecture(self):
        for platform, arch in (("FreeBSD", "x86_64"), ("Linux", "riscv64")):
            self.env.update(PLATFORM=platform, ARCH=arch)
            result = self.run_installer()
            self.assertNotEqual(result.returncode, 0)
            self.assertIn("Unsupported", result.stderr)
            self.assertFalse(self.binary.exists())

    def test_invalid_version_and_network_failure(self):
        self.release()
        self.env["TESSERA_VERSION"] = "../bad"
        self.assertNotEqual(self.run_installer().returncode, 0)
        self.env.pop("TESSERA_VERSION")
        self.env["NETWORK_FAILURE"] = "1"
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("Cannot find", result.stderr)
        self.assertFalse(self.binary.exists())

    def test_unrelated_symlink_is_preserved(self):
        self.release()
        self.binary.parent.mkdir(parents=True)
        target = self.root / "other"
        target.write_text("keep")
        self.binary.symlink_to(target)
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("unrelated symlink", result.stderr)
        self.assertEqual(target.read_text(), "keep")
        self.assertTrue(self.binary.is_symlink())

    def test_running_macos_app_and_bad_signature(self):
        self.release(platform="Darwin")
        self.env["RUNNING"] = "1"
        result = self.run_installer()
        self.assertIn("Quit Tessera", result.stderr)
        self.env.pop("RUNNING")
        self.env["BAD_SIGNATURE"] = "1"
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse(self.binary.exists())
        self.assertFalse((self.home / "Applications/Tessera.app").exists())
        self.assertIn("hdiutil detach", (self.root / "calls").read_text())

    def test_symlink_archive_is_rejected_without_touching_target(self):
        self.release()
        archive = self.root / "Tessera-linux-x86_64.tar.gz"
        target = self.root / "keep"
        target.write_text("untouched")
        with tarfile.open(archive, "w:gz") as tar:
            entry = tarfile.TarInfo("tessera")
            entry.type = tarfile.SYMTYPE
            entry.linkname = str(target)
            tar.addfile(entry)
        digest = hashlib.sha256(archive.read_bytes()).hexdigest()
        (self.root / "SHA256SUMS").write_text(f"{digest}  {archive.name}\n")
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertIn("symlink", result.stderr)
        self.assertEqual(target.read_text(), "untouched")
        self.assertFalse(self.binary.exists())

    def test_failed_macos_command_install_removes_new_bundle(self):
        self.release(platform="Darwin")
        self.env["LINK_MOVE_FAILURE"] = "1"
        result = self.run_installer()
        self.assertNotEqual(result.returncode, 0)
        self.assertFalse((self.home / "Applications/Tessera.app").exists())
        self.assertFalse(self.binary.exists())

    @unittest.skipUnless(Path("/usr/libexec/PlistBuddy").exists(), "uses native macOS plist reader")
    def test_macos_reinstall_failures_preserve_previous_application(self):
        self.release(platform="Darwin")
        self.assertEqual(self.run_installer().returncode, 0)
        target = self.binary.resolve()
        target.write_text("previous version")
        for failure in ("COPY_FAILURE", "LINK_MOVE_FAILURE"):
            with self.subTest(failure=failure):
                self.env[failure] = "1"
                self.assertNotEqual(self.run_installer().returncode, 0)
                self.assertEqual(target.read_text(), "previous version")
                self.assertEqual(self.binary.resolve(), target)
                self.env.pop(failure)
        self.assertEqual(self.run_installer().returncode, 0)
        self.assertIn("echo Tessera", target.read_text())

    def test_truncated_script_does_not_install(self):
        self.release()
        truncated = INSTALLER.read_text().rsplit('main "$@"', 1)[0]
        result = self.run_installer(source=truncated)
        self.assertEqual(result.returncode, 0, result.stderr)
        self.assertFalse(self.binary.exists())
        self.assertFalse((self.root / "calls").exists())


if __name__ == "__main__":
    unittest.main()
