#!/usr/bin/env bash
set -euo pipefail

[[ $# -eq 2 && "$1" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ && "$2" =~ ^(amd64|arm64)$ ]] || {
    echo 'usage: smoke-docker.sh vX.Y.Z amd64|arm64' >&2
    exit 2
}
tag="$1"
arch="$2"
sha="$(git rev-parse --short=12 "${tag}^{commit}")"
image="bitgarth/bitgarth:${tag#v}-${sha}-${arch}"
container="$(docker run --detach --rm --network none --platform "linux/${arch}" "$image")"
trap '
    if [[ "$?" -ne 0 ]]; then docker logs "$container" >&2 || true; fi
    docker rm -f -v "$container" >/dev/null 2>&1 || true
' EXIT

for ((attempt=0; attempt<90; attempt++)); do
    state="$(docker inspect --format '{{.State.Status}}|{{.State.Health.Status}}' "$container")"
    case "$state" in
        'running|healthy')
            docker exec "$container" sh -ec '
                test "$(id -u)" != 0
                curl --fail --silent --max-time 5 http://localhost:8080/health >/dev/null
                curl --fail --silent --max-time 5 http://localhost:8080/ | grep -q "<script"
                test -n "$(find /srv/public -name "*.wasm" -print -quit)"
            '
            echo "Docker startup passed: $image"
            exit 0
            ;;
        'running|starting') sleep 2 ;;
        *) echo "Docker startup failed: $state" >&2; exit 1 ;;
    esac
done
echo 'Docker startup timed out' >&2
exit 1
