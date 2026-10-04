import importlib.util
from pathlib import Path
import subprocess
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("tag_release", Path(__file__).parents[1] / "scripts/tag-release.py")
release = importlib.util.module_from_spec(spec)
spec.loader.exec_module(release)
SHA = "a" * 40


class TagReleaseTests(unittest.TestCase):
    def test_marker_must_match_package_and_be_unambiguous(self):
        self.assertEqual(release.release_tag("[release v0.4.0] Release", "0.4.0"), "v0.4.0")
        for message in ["ordinary merge", "[release v0.3.1]", "[release v0.4.0] [release v0.4.0]"]:
            with self.assertRaises(ValueError):
                release.release_tag(message, "0.4.0")

    @patch.object(release.subprocess, "check_output", return_value=SHA)
    @patch.object(release.subprocess, "run")
    def test_existing_different_tag_is_never_overwritten(self, run, _):
        run.return_value = subprocess.CompletedProcess([], 0, stdout="b" * 40)
        with self.assertRaises(ValueError):
            release.publish("v0.4.0", SHA, "owner/repo")
        self.assertEqual(run.call_count, 1)

    @patch.object(release.subprocess, "check_output", return_value=SHA)
    @patch.object(release.subprocess, "run")
    def test_new_tag_targets_verified_commit_and_dispatches_tag_build(self, run, _):
        run.return_value = subprocess.CompletedProcess([], 1)
        release.publish("v0.4.0", SHA, "owner/repo")
        commands = [call.args[0] for call in run.call_args_list]
        self.assertEqual(commands[1], ["git", "tag", "v0.4.0", SHA])
        self.assertEqual(commands[2], ["git", "push", "origin", "refs/tags/v0.4.0"])
        self.assertEqual(commands[3], ["gh", "workflow", "run", "ci.yml", "--ref", "v0.4.0", "--repo", "owner/repo"])

    @patch.object(release.subprocess, "check_output", return_value="b" * 40)
    @patch.object(release.subprocess, "run")
    def test_unverified_checkout_cannot_tag(self, run, _):
        with self.assertRaises(ValueError):
            release.publish("v0.4.0", SHA, "owner/repo")
        run.assert_not_called()
