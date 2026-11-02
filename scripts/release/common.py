"""Read-only release evidence shared by the release commands (stdlib only)."""

import base64
import json
import os
from pathlib import Path
import re
import subprocess
import tempfile
from urllib.parse import urlencode

REPO = "BitGarth/bitgarth"
IMAGE = "bitgarth/bitgarth"
TAG_PATTERN = r"v(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)\.(0|[1-9][0-9]*)"
DIGEST_PATTERN = r"sha256:[0-9a-f]{64}"


class ReleaseError(Exception):
    """Missing, failed, or inconsistent release evidence."""


class Pending(ReleaseError):
    """Evidence has not finished arriving yet."""


def version(tag):
    match = re.fullmatch(TAG_PATTERN, tag)
    if not match:
        raise ReleaseError("expected a stable vMAJOR.MINOR.PATCH tag")
    return tuple(map(int, match.groups()))


def command(*args, timeout=120, input=None):
    try:
        return subprocess.run(args, check=True, capture_output=True, text=True,
                              timeout=timeout, input=input).stdout.strip()
    except (OSError, subprocess.TimeoutExpired) as error:
        raise ReleaseError(f"{args[0]} unavailable or timed out: {error}") from error
    except subprocess.CalledProcessError as error:
        raise ReleaseError(f"{args[0]} failed: {error.stderr.strip()}") from error


def api(path, repo=REPO):
    return json.loads(command("gh", "api", f"repos/{repo}/{path}"))


def pages(path, key=None, repo=REPO):
    page = 1
    separator = "&" if "?" in path else "?"
    while True:
        result = api(f"{path}{separator}per_page=100&page={page}", repo)
        items = result[key] if key else result
        if not isinstance(items, list):
            raise ReleaseError("invalid paginated GitHub response")
        yield from items
        if len(items) < 100:
            return
        page += 1


def public_commit(tag, expected_sha=None):
    version(tag)
    ref = api(f"git/ref/tags/{tag}")["object"]
    if ref["type"] != "tag":
        raise ReleaseError("public release must have an annotated tag")
    annotation = api(f"git/tags/{ref['sha']}")
    obj = annotation["object"]
    sha = obj["sha"]
    if (annotation["tag"] != tag or obj["type"] != "commit"
            or not re.fullmatch(r"[0-9a-f]{40}", sha)
            or (expected_sha is not None and sha != expected_sha)):
        raise ReleaseError("public tag does not match the expected release commit")
    return sha


def source_file(path, sha, repo=REPO):
    blob = api(f"contents/{path}?{urlencode({'ref': sha})}", repo)
    if blob.get("encoding") != "base64" or blob.get("type") != "file":
        raise ReleaseError(f"expected a source file: {path}")
    return base64.b64decode(blob["content"])


def asset_names(tag):
    return {"SHA256SUMS"} | {
        f"bitgarth-{product}-{tag}-{platform}.{extension}"
        for product in ("cli", "web")
        for platform, extension in (("linux-x86_64-gnu", "tar.gz"),
                                    ("macos-aarch64", "tar.gz"),
                                    ("windows-x86_64-msvc", "zip"))
    }


def validate_release(release, tag):
    if release["tag_name"] != tag or release["draft"] or release["prerelease"]:
        raise ReleaseError("release is not the requested published stable release")
    missing = asset_names(tag) - {asset["name"] for asset in release["assets"]}
    if missing:
        raise Pending("release assets missing: " + ", ".join(sorted(missing)))
    if "[!WARNING]" in (release.get("body") or ""):
        raise ReleaseError("release still contains a publication/notarization warning")


def require_workflow(workflow, sha, branch, repo=REPO):
    query = urlencode({"head_sha": sha, "event": "push"})
    runs = [run for run in pages(f"actions/workflows/{workflow}/runs?{query}",
                                "workflow_runs", repo)
            if run["head_sha"] == sha and run["head_branch"] == branch
            and run["event"] == "push"]
    if not runs:
        raise Pending(f"{workflow}: no push run for exact commit {sha} ({branch})")
    latest = max(runs, key=lambda run: run["id"])
    if latest["status"] != "completed":
        raise Pending(f"{workflow}: {latest['status']}")
    if latest["conclusion"] != "success":
        raise ReleaseError(f"{workflow}: {latest['conclusion']}")
    return latest["id"]


def binary_release(tag, expected_sha=None):
    sha = public_commit(tag, expected_sha)
    ci_run = require_workflow("ci.yml", sha, "main")
    jobs = list(pages(f"actions/runs/{ci_run}/jobs", "jobs"))
    if not any(job["name"] == "Run all tests" and job["conclusion"] == "success"
               for job in jobs):
        raise ReleaseError("Run all tests has not passed for the public commit")
    require_workflow("release.yml", sha, tag)
    release = api(f"releases/tags/{tag}")
    validate_release(release, tag)
    manifest = source_file("Cargo.toml", sha).decode()
    match = re.search(r'^version = "([^"]+)"', manifest, re.MULTILINE)
    if not match or match[1] != tag[1:]:
        raise ReleaseError("tagged manifest version does not match release tag")
    return sha, release


def require_latest(tag):
    stable = [version(item["tag_name"]) for item in pages("releases")
              if not item["draft"] and not item["prerelease"]
              and re.fullmatch(TAG_PATTERN, item["tag_name"])]
    if not stable or max(stable) != version(tag):
        raise ReleaseError("requested tag is not the latest numeric published stable release")


def docker_release(tag, sha):
    ref = f"{IMAGE}:{tag[1:]}"
    manifest = json.loads(command("docker", "buildx", "imagetools", "inspect", ref, "--raw"))
    runtimes = [item for item in manifest.get("manifests", [])
                if item.get("platform", {}).get("architecture") != "unknown"]
    platforms = [(item["platform"]["os"], item["platform"]["architecture"])
                 for item in runtimes]
    if sorted(platforms) != [("linux", "amd64"), ("linux", "arm64")]:
        raise ReleaseError("Docker release must contain exactly linux/amd64 and linux/arm64")
    digest = command("docker", "buildx", "imagetools", "inspect", ref,
                     "--format", "{{.Manifest.Digest}}")
    if not re.fullmatch(DIGEST_PATTERN, digest):
        raise ReleaseError("invalid multi-platform Docker digest")
    for item in runtimes:
        if not re.fullmatch(DIGEST_PATTERN, item["digest"]):
            raise ReleaseError("invalid platform digest")
        labels = json.loads(command("docker", "buildx", "imagetools", "inspect",
                                    f"{IMAGE}@{item['digest']}", "--format",
                                    "{{json .Image.Config.Labels}}"))
        if (labels.get("org.opencontainers.image.version") != tag[1:]
                or labels.get("org.opencontainers.image.revision") != sha):
            raise ReleaseError("Docker image labels do not match the public release")
    latest_digest = command("docker", "buildx", "imagetools", "inspect", f"{IMAGE}:latest",
                            "--format", "{{.Manifest.Digest}}")
    if latest_digest != digest:
        raise ReleaseError("Docker latest does not point to the requested version manifest")
    return digest


def read_editable(path):
    if path.is_symlink() or not path.is_file():
        raise ReleaseError(f"expected a regular editable file: {path}")
    return path.read_text()


def write_edits(edits):
    """Validate all originals first, then atomically replace each changed file."""
    for path, before, _ in edits:
        if read_editable(path) != before:
            raise ReleaseError(f"file changed during preparation: {path}")
    for path, before, after in edits:
        if before == after:
            print(f"Unchanged: {path}")
            continue
        temporary = None
        try:
            with tempfile.NamedTemporaryFile(mode="w", dir=path.parent, delete=False) as stream:
                temporary = Path(stream.name)
                stream.write(after)
            temporary.chmod(path.stat().st_mode & 0o777)
            os.replace(temporary, path)
        finally:
            if temporary is not None:
                temporary.unlink(missing_ok=True)
        print(f"Updated (uncommitted): {path}")
