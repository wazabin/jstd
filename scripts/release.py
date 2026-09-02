#!/usr/bin/env python3
"""Set the workspace release version, commit it, and create its v-tag."""

from __future__ import annotations

import argparse
from pathlib import Path
import re
import subprocess
import sys

ROOT = Path(__file__).resolve().parents[1]
MANIFEST = ROOT / "Cargo.toml"
VERSION_RE = re.compile(
    r"^(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)\.(?:0|[1-9]\d*)"
    r"(?:-[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?"
    r"(?:\+[0-9A-Za-z-]+(?:\.[0-9A-Za-z-]+)*)?$"
)


def git(*args: str, check: bool = True) -> subprocess.CompletedProcess[str]:
    return subprocess.run(["git", *args], cwd=ROOT, text=True, check=check)


def replace_once(text: str, pattern: str, replacement: str) -> str:
    text, replacements = re.subn(pattern, replacement, text, count=1, flags=re.MULTILINE)
    if replacements != 1:
        raise RuntimeError(f"expected exactly one match for {pattern!r}")
    return text


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("version", help="SemVer version to release, without the v prefix")
    version = parser.parse_args().version

    if not VERSION_RE.fullmatch(version):
        parser.error(f"not a SemVer version: {version!r}")

    status = subprocess.check_output(["git", "status", "--porcelain"], cwd=ROOT, text=True)
    if status:
        raise SystemExit("refusing to release from a dirty working tree")

    tag = f"v{version}"
    if git("rev-parse", "-q", "--verify", f"refs/tags/{tag}", check=False).returncode == 0:
        raise SystemExit(f"tag already exists: {tag}")

    manifest = MANIFEST.read_text()
    manifest = replace_once(
        manifest,
        r"(\[workspace\.package\][\s\S]*?^version = )\"[^\"]+\"",
        rf'\g<1>"{version}"',
    )
    manifest = replace_once(
        manifest,
        r"(^jstd_derive = \{[^\n]*?\bversion = )\"[^\"]+\"",
        rf'\g<1>"{version}"',
    )
    MANIFEST.write_text(manifest)

    git("add", "Cargo.toml")
    git("commit", "-m", f"chore: release {tag}")
    git("tag", "-a", tag, "-m", tag)
    print(f"Created release commit and tag {tag}. Push with: git push origin HEAD {tag}")


if __name__ == "__main__":
    try:
        main()
    except RuntimeError as error:
        print(f"error: {error}", file=sys.stderr)
        raise SystemExit(1)
