#!/usr/bin/env python3
"""Build both strict amd64 snaps from one verified public GitHub release."""

import argparse
import base64
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
from urllib.request import Request, urlopen


ROOT = Path(__file__).resolve().parents[2]
REPO = "BitGarth/bitgarth"
PRODUCTS = ("cli", "web")


def validate_tag(tag):
    if not re.fullmatch(r"v[0-9]+\.[0-9]+\.[0-9]+", tag):
        raise ValueError(f"invalid stable release tag: {tag}")
    return tag[1:]


def archive_name(product, tag):
    return f"bitgarth-{product}-{tag}-linux-x86_64-gnu.tar.gz"


def digest(path):
    sha = hashlib.sha256()
    with path.open("rb") as source:
        for block in iter(lambda: source.read(1024 * 1024), b""):
            sha.update(block)
    return sha.hexdigest()


def parse_checksums(contents, required):
    result = {}
    for line in contents.splitlines():
        match = re.fullmatch(r"([0-9a-fA-F]{64}) [ *]([^\r\n]+)", line)
        if not match:
            raise ValueError(f"invalid SHA256SUMS line: {line!r}")
        name = match.group(2)
        if name in result:
            raise ValueError(f"duplicate checksum entry: {name}")
        result[name] = match.group(1).lower()
    for name in required:
        if name not in result:
            raise ValueError(f"missing checksum entry: {name}")
    return {name: result[name] for name in required}


def extract_archive(archive, destination):
    with tarfile.open(archive, "r:gz") as package:
        members = package.getmembers()
        seen = set()
        for member in members:
            path = PurePosixPath(member.name)
            if (path.is_absolute() or ".." in path.parts or not path.parts or
                    not (member.isfile() or member.isdir())):
                raise ValueError(f"unsafe archive member: {member.name}")
            if path in seen:
                raise ValueError(f"duplicate archive member: {member.name}")
            seen.add(path)
        destination.mkdir(parents=True)
        for member in members:
            target = destination.joinpath(*PurePosixPath(member.name).parts)
            if member.isdir():
                target.mkdir(parents=True, exist_ok=True)
            else:
                target.parent.mkdir(parents=True, exist_ok=True)
                with package.extractfile(member) as source, target.open("wb") as output:
                    shutil.copyfileobj(source, output)
                target.chmod(member.mode & 0o777)


def validate_payload(product, payload):
    binary = payload / ("bitgarth" if product == "cli" else "bitgarth-web")
    if not binary.is_file() or not os.access(binary, os.X_OK):
        raise ValueError(f"missing executable: {binary}")
    if product == "web":
        public = payload / "public"
        if not public.is_dir() or not any(path.is_file() for path in public.rglob("*")):
            raise ValueError(f"missing web resource: {public}")
        catalog = payload / "assets/catalog/unsynced_asset_catalog.json"
        if not catalog.is_file():
            raise ValueError(f"missing web resource: {catalog}")


def render_template(contents, version):
    if contents.count("@VERSION@") != 1:
        raise ValueError("snapcraft template must contain exactly one @VERSION@ token")
    return contents.replace("@VERSION@", version)


def fetch_release(tag):
    request = Request(f"https://api.github.com/repos/{REPO}/releases/tags/{tag}",
                      headers={"User-Agent": "BitGarth-Snap-Build"})
    with urlopen(request, timeout=30) as response:
        release = json.load(response)
    return {"tagName": release["tag_name"], "isDraft": release["draft"],
            "isPrerelease": release["prerelease"], "url": release["html_url"],
            "assets": release["assets"]}


def validate_release(release, tag):
    if (release.get("tagName") != tag or release.get("isDraft") is not False or
            release.get("isPrerelease") is not False):
        raise ValueError(f"release {tag} is absent, draft, or prerelease")
    names = [asset["name"] for asset in release["assets"]]
    for name in ("SHA256SUMS", *(archive_name(product, tag) for product in PRODUCTS)):
        if names.count(name) != 1:
            raise ValueError(f"release must have exactly one {name}")
    return release["url"]


def download_asset(tag, name, destination):
    request = Request(f"https://github.com/{REPO}/releases/download/{tag}/{name}",
                      headers={"User-Agent": "BitGarth-Snap-Build"})
    with urlopen(request, timeout=60) as response, (destination / name).open("xb") as output:
        shutil.copyfileobj(response, output)


def exact_license(tag):
    request = Request(f"https://api.github.com/repos/{REPO}/contents/LICENSE.md?ref={tag}",
                      headers={"User-Agent": "BitGarth-Snap-Build"})
    with urlopen(request, timeout=30) as result:
        response = json.load(result)
    if response.get("encoding") != "base64":
        raise ValueError(f"unexpected license encoding at {tag}")
    return base64.b64decode(response["content"], validate=False)


def verify_snap(package, product, version, payload, destination):
    subprocess.run(["unsquashfs", "-no-progress", "-dest", str(destination),
                    str(package)], check=True, stdout=subprocess.DEVNULL)
    metadata = (destination / "meta/snap.yaml").read_text()
    expected = {
        "name": f"bitgarth-{product}", "version": version,
        "base": "core24", "confinement": "strict",
    }
    for key, value in expected.items():
        if not re.search(rf"^{key}: ['\"]?{re.escape(value)}['\"]?$", metadata,
                         re.MULTILINE):
            raise ValueError(f"snap metadata missing {key}: {value}")
    if not re.search(r"(?m)^architectures:\s*(?:\n\s*-?\s*amd64\s*|\[amd64\])", metadata):
        raise ValueError("snap metadata missing amd64 architecture")
    app = f"bitgarth-{product}"
    launcher = f"bin/launch-{product}"
    if f"  {app}:" not in metadata or f"command: {launcher}" not in metadata:
        raise ValueError(f"snap metadata missing {app} app/launcher")
    if product == "web" and "daemon: simple" not in metadata:
        raise ValueError("web snap is not a simple daemon")
    if product == "cli" and "daemon:" in metadata:
        raise ValueError("CLI snap must not be a daemon")
    if not (destination / launcher).stat().st_mode & 0o111:
        raise ValueError(f"launcher not executable: {launcher}")
    if not (destination / "meta/gui/icon.png").is_file():
        raise ValueError("snap missing icon")
    if product == "web" and not os.access(destination / "meta/hooks/configure", os.X_OK):
        raise ValueError("web snap missing configure hook")
    for source in payload.rglob("*"):
        if source.is_file():
            staged = destination / source.relative_to(payload)
            if not staged.is_file() or digest(staged) != digest(source):
                raise ValueError(f"packaged resource differs: {source.relative_to(payload)}")


def pack_product(product, tag, version, archive, temporary, output):
    project = temporary / f"project-{product}"
    project.mkdir()
    payload = project / "payload"
    extract_archive(archive, payload)
    validate_payload(product, payload)
    license_file = payload / "LICENSE.md"
    if not license_file.is_file():
        license_file.write_bytes(exact_license(tag))
    launcher_dir = project / "launcher"
    launcher_dir.mkdir()
    shutil.copy2(ROOT / f"packaging/snap/{product}/launch-{product}", launcher_dir)
    snap_dir = project / "snap"
    snap_dir.mkdir()
    (snap_dir / "gui").mkdir()
    shutil.copy2(ROOT / "packaging/snap/icon.png", snap_dir / "gui/icon.png")
    if product == "web":
        (snap_dir / "hooks").mkdir()
        shutil.copy2(ROOT / "packaging/snap/web/configure", snap_dir / "hooks/configure")
    template = (ROOT / f"packaging/snap/{product}/snapcraft.yaml.in").read_text()
    (snap_dir / "snapcraft.yaml").write_text(render_template(template, version))
    subprocess.run(["snapcraft", "pack", "--destructive-mode"], cwd=project, check=True)
    packages = list(project.glob("*.snap"))
    if len(packages) != 1:
        raise ValueError(f"expected one {product} snap, found {len(packages)}")
    verify_snap(packages[0], product, version, payload, temporary / f"inspect-{product}")
    filename = f"bitgarth-{product}_{version}_amd64.snap"
    shutil.copy2(packages[0], output / filename)
    return {"file": filename, "version": version, "architecture": "amd64",
            "sha256": digest(output / filename)}


def build_release(tag, output):
    version = validate_tag(tag)
    if sys.platform != "linux" or platform.machine() not in ("x86_64", "amd64"):
        raise ValueError("Snap builds require Linux amd64")
    if output.is_symlink() or (output.exists() and (not output.is_dir() or any(output.iterdir()))):
        raise ValueError(f"output directory must be absent or empty: {output}")
    release = fetch_release(tag)
    release_url = validate_release(release, tag)
    names = [archive_name(product, tag) for product in PRODUCTS]
    with tempfile.TemporaryDirectory(prefix="bitgarth-snap-") as directory:
        temporary = Path(directory)
        downloads = temporary / "downloads"
        downloads.mkdir()
        for name in (*names, "SHA256SUMS"):
            download_asset(tag, name, downloads)
        checksums = parse_checksums((downloads / "SHA256SUMS").read_text(), names)
        for name in names:
            if digest(downloads / name) != checksums[name]:
                raise ValueError(f"checksum mismatch: {name}")
        output.mkdir(parents=True, exist_ok=True)
        packages = {}
        for product, name in zip(PRODUCTS, names):
            packages[f"bitgarth-{product}"] = pack_product(
                product, tag, version, downloads / name, temporary, output)
        snapcraft = subprocess.run(["snapcraft", "--version"], capture_output=True,
                                   text=True, check=True).stdout.strip()
        manifest = {"release_tag": tag, "release_url": release_url,
                    "source_archives": checksums, "snapcraft_version": snapcraft,
                    "packages": packages}
        (output / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n")
        return manifest


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--tag", required=True)
    parser.add_argument("--output-dir", required=True, type=Path)
    args = parser.parse_args()
    try:
        manifest = build_release(args.tag, args.output_dir)
    except (OSError, ValueError, subprocess.CalledProcessError, tarfile.TarError) as error:
        parser.exit(1, f"snap build failed: {error}\n")
    print(json.dumps(manifest, indent=2))


if __name__ == "__main__":
    main()
