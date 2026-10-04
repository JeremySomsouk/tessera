#!/usr/bin/env python3
"""Publish an explicitly requested version tag after main's verification jobs."""
import os
from pathlib import Path
import re
import subprocess


def release_tag(message, version):
    markers = re.findall(r"\[release (v[0-9]+\.[0-9]+\.[0-9]+)\]", message)
    if len(markers) != 1 or markers[0] != f"v{version}" or not message.startswith(f"[release {markers[0]}]"):
        raise ValueError("Exactly one release marker matching Cargo.toml is required")
    return markers[0]


def publish(tag, commit, repository):
    if not re.fullmatch(r"[0-9a-f]{40}", commit):
        raise ValueError("Expected a full commit SHA")
    head = subprocess.check_output(["git", "rev-parse", "HEAD"], text=True).strip()
    if head != commit:
        raise ValueError("Checkout does not match the verified main commit")
    ref = f"refs/tags/{tag}"
    existing = subprocess.run(["git", "rev-parse", "--verify", f"{ref}^{{commit}}"], text=True, capture_output=True)
    if existing.returncode == 0:
        if existing.stdout.strip() != commit:
            raise ValueError("Existing release tag points to another commit; refusing to overwrite")
    else:
        subprocess.run(["git", "tag", tag, commit], check=True)
        subprocess.run(["git", "push", "origin", ref], check=True)
    # GITHUB_TOKEN tag pushes do not trigger push workflows. An explicit
    # workflow_dispatch is supported and runs the existing signed release flow.
    subprocess.run(["gh", "workflow", "run", "ci.yml", "--ref", tag, "--repo", repository], check=True)


def main():
    # Only the ubuntu-latest tagging job needs Python 3.11+ TOML parsing.
    # Pure validation/publishing helpers remain importable by Python 3.10 CI.
    import tomllib

    if os.environ.get("GITHUB_REF") != "refs/heads/main":
        raise ValueError("Release tagging requires the main branch")
    version = tomllib.loads(Path("Cargo.toml").read_text())["package"]["version"]
    tag = release_tag(os.environ["RELEASE_MESSAGE"], version)
    publish(tag, os.environ["GITHUB_SHA"], os.environ["GH_REPO"])


if __name__ == "__main__":
    main()
