"""Check that a Snap artifact contains the exact package files in its manifest."""

import argparse
import hashlib
import json
from pathlib import Path
import re


def verify(directory, tag):
    if not re.fullmatch(r"v\d+\.\d+\.\d+", tag):
        raise ValueError("invalid release tag")
    version = tag[1:]
    manifest = json.loads((directory / "manifest.json").read_text())
    names = ("bitgarth-cli", "bitgarth-web")
    if manifest.get("release_tag") != tag or set(manifest.get("packages", {})) != set(names):
        raise ValueError("manifest release or package names do not match")
    expected_files = {"manifest.json"}
    for name in names:
        item = manifest["packages"][name]
        filename = f"{name}_{version}_amd64.snap"
        expected_files.add(filename)
        if (item.get("file") != filename or item.get("version") != version or
                item.get("architecture") != "amd64" or
                not re.fullmatch(r"[0-9a-f]{64}", item.get("sha256", ""))):
            raise ValueError(f"invalid manifest entry for {name}")
        package = directory / filename
        if package.is_symlink() or not package.is_file():
            raise ValueError(f"missing regular snap file: {filename}")
        digest = hashlib.sha256()
        with package.open("rb") as source:
            for chunk in iter(lambda: source.read(1024 * 1024), b""):
                digest.update(chunk)
        if digest.hexdigest() != item["sha256"]:
            raise ValueError(f"snap hash mismatch: {filename}")
    if {path.name for path in directory.iterdir()} != expected_files:
        raise ValueError("artifact has unexpected files")


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--directory", required=True, type=Path)
    args = parser.parse_args()
    try:
        verify(args.directory, args.tag)
    except (OSError, ValueError, TypeError, json.JSONDecodeError) as error:
        parser.exit(1, f"snap artifact verification failed: {error}\n")
    print(f"verified two snap hashes for {args.tag}")


if __name__ == "__main__":
    main()
