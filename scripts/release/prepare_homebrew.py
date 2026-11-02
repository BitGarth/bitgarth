#!/usr/bin/env python3
"""Verify published downloads and leave both Homebrew formulae edited, uncommitted."""

import argparse
import hashlib
from pathlib import Path
import re
import sys
import tempfile

from common import (REPO, ReleaseError, binary_release, command, read_editable,
                    require_latest, source_file, version, write_edits)


def archive_names(tag):
    return [f"bitgarth-{product}-{tag}-{platform}.tar.gz"
            for product in ("cli", "web")
            for platform in ("macos-aarch64", "linux-x86_64-gnu")]


def download_archives(tag, directory):
    args = ["gh", "release", "download", tag, "--repo", REPO, "--dir", str(directory)]
    for name in ["SHA256SUMS", *archive_names(tag)]:
        args += ["--pattern", name]
    command(*args, timeout=600)


def verify_archives(directory, names):
    checksums = {}
    for line in (directory / "SHA256SUMS").read_text().splitlines():
        match = re.fullmatch(r"([0-9a-f]{64}) [ *]([^/\\\s]+)", line)
        if not match or match[2] in checksums:
            raise ReleaseError("malformed or duplicate SHA256SUMS entry")
        checksums[match[2]] = match[1]
    verified = {}
    for name in names:
        digest = hashlib.sha256()
        with (directory / name).open("rb") as stream:
            for chunk in iter(lambda: stream.read(1024 * 1024), b""):
                digest.update(chunk)
        if checksums.get(name) != digest.hexdigest():
            raise ReleaseError(f"archive checksum mismatch or missing checksum: {name}")
        verified[name] = digest.hexdigest()
    return verified


def render_formula(text, product, tag, hashes, license_hash):
    version(tag)
    for platform in ("macos-aarch64", "linux-x86_64-gnu"):
        pattern = (r'(url "https://github\.com/BitGarth/bitgarth/releases/download/)'
                   r'(v[0-9]+\.[0-9]+\.[0-9]+)/bitgarth-' + product +
                   r'-(v[0-9]+\.[0-9]+\.[0-9]+)-' + platform +
                   r'\.tar\.gz("\n[ \t]+sha256 ")[0-9a-f]{64}(")')
        # Keep the formula's indentation and everything outside URL/hash pairs.
        matches = list(re.finditer(pattern, text))
        if len(matches) != 1:
            raise ReleaseError(f"expected one {product}/{platform} URL/hash pair")
        match = matches[0]
        if match[2] != match[3] or version(match[2]) > version(tag):
            raise ReleaseError("formula versions disagree or update would downgrade")
        name = f"bitgarth-{product}-{tag}-{platform}.tar.gz"
        replacement = f'{match[1]}{tag}/{name}' + match[4] + hashes[name] + match[5]
        text = text[:match.start()] + replacement + text[match.end():]
    pattern = (r'(resource "license" do\n[ \t]+url "https://raw\.githubusercontent\.com/'
               r'BitGarth/bitgarth/)v[0-9]+\.[0-9]+\.[0-9]+'
               r'(/LICENSE\.md"\n[ \t]+sha256 ")[0-9a-f]{64}("\n[ \t]+end)')
    text, count = re.subn(pattern, lambda m: m[1] + tag + m[2] + license_hash + m[3], text)
    if count != 1:
        raise ReleaseError("expected one tagged license URL/hash pair")
    return text


def prepare(tag, tap):
    sha, _ = binary_release(tag)
    require_latest(tag)
    paths = [tap / f"Formula/bitgarth-{product}.rb" for product in ("cli", "web")]
    originals = [read_editable(path) for path in paths]
    with tempfile.TemporaryDirectory(prefix="bitgarth-homebrew-") as temporary:
        directory = Path(temporary)
        download_archives(tag, directory)
        hashes = verify_archives(directory, archive_names(tag))
    license_hash = hashlib.sha256(source_file("LICENSE.md", sha)).hexdigest()
    edits = []
    for path, text, product in zip(paths, originals, ("cli", "web")):
        result = render_formula(text, product, tag, hashes, license_hash)
        command("ruby", "-c", input=result)
        edits.append((path, text, result))
    write_edits(edits)
    print("Next: brew style, audit, install and test the edited local tap; no commits or pushes were made.")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("tag")
    parser.add_argument("--tap", required=True, type=Path, help="Homebrew tap checkout")
    args = parser.parse_args()
    try:
        prepare(args.tag, args.tap)
    except (ReleaseError, OSError, ValueError, KeyError, TypeError) as error:
        print(f"error: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main())
