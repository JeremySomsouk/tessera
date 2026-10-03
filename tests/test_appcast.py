"""Portable checks for the architecture-specific signed-update manifest."""
import importlib.util
from pathlib import Path
import tempfile
import unittest

spec = importlib.util.spec_from_file_location("appcast", Path(__file__).parents[1] / "scripts/create-appcast.py")
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


class AppcastTests(unittest.TestCase):
    def test_feed_selects_the_architecture_and_exact_published_version(self):
        with tempfile.TemporaryDirectory() as directory:
            archive = Path(directory) / "archive.dmg"
            archive.write_bytes(b"signed archive")
            for architecture in ["arm64", "x86_64"]:
                tree = module.appcast("0.2.2", "1.2.2", architecture, archive, "signature")
                item = tree.getroot().find("channel/item")
                enclosure = item.find("enclosure")
                self.assertEqual(enclosure.get("url"), f"{module.RELEASES}/v0.2.2/Tessera-{architecture}.dmg")
                self.assertEqual(enclosure.get("length"), str(archive.stat().st_size))
                self.assertEqual(enclosure.get(f"{{{module.SPARKLE}}}edSignature"), "signature")
                self.assertEqual(item.find(f"{{{module.SPARKLE}}}version").text, "1.2.2")
                self.assertEqual(item.find(f"{{{module.SPARKLE}}}shortVersionString").text, "0.2.2")
                self.assertEqual(item.find(f"{{{module.SPARKLE}}}minimumSystemVersion").text, "11.0")


if __name__ == "__main__":
    unittest.main()
