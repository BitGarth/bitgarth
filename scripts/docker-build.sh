#!/usr/bin/env bash
set -euo pipefail

DOCKER_REPO="${DOCKER_REPO:-bitgarth/bitgarth}"
SCRIPT_DIR="$(cd "$(dirname "${BASH_SOURCE[0]}")" && pwd)"
PROJECT_ROOT="$(cd "${SCRIPT_DIR}/.." && pwd)"

STAGING_FORMAT='{{if .Manifest.Manifests}}{{range .Manifest.Manifests}}{{if ne .Platform.Architecture "unknown"}}{{printf "runtime|%s|%s|%s\n" .Digest .Platform.OS .Platform.Architecture}}{{end}}{{end}}{{else}}{{printf "runtime|%s|%s|%s\n" .Manifest.Digest .Image.OS .Image.Architecture}}{{end}}{{printf "labels|%s|%s\n" (index .Image.Config.Labels "org.opencontainers.image.version") (index .Image.Config.Labels "org.opencontainers.image.revision")}}'
MANIFEST_FORMAT='{{if .Manifest.Manifests}}{{range .Manifest.Manifests}}{{if ne .Platform.Architecture "unknown"}}{{printf "runtime|%s|%s|%s\n" .Digest .Platform.OS .Platform.Architecture}}{{end}}{{end}}{{else}}{{printf "runtime|%s|%s|%s\n" .Manifest.Digest .Image.OS .Image.Architecture}}{{end}}'

usage() {
    cat <<'USAGE'
Usage:
  ./scripts/docker-build.sh build-amd64 vX.Y.Z [--no-cache]
  ./scripts/docker-build.sh build-arm64 vX.Y.Z [--no-cache]
  ./scripts/docker-build.sh publish-manifest vX.Y.Z

Environment:
  DOCKER_REPO  Docker repository (default: bitgarth/bitgarth)
USAGE
}

die() {
    echo "Error: $*" >&2
    exit 1
}

require_tools() {
    command -v git >/dev/null 2>&1 || die 'git is not available'
    command -v docker >/dev/null 2>&1 || die 'docker is not available'
    docker buildx version >/dev/null 2>&1 || die 'docker buildx is not available'
}

read_tagged_version() {
    local tag="$1"
    local line
    while IFS= read -r line; do
        case "${line}" in
            'version = "'*)
                line="${line#version = \"}"
                printf '%s\n' "${line%%\"*}"
                return 0
                ;;
        esac
    done < <(git -C "${PROJECT_ROOT}" show "${tag}:Cargo.toml")
    return 1
}

load_release() {
    local tag="$1"
    local object_type
    local cargo_version

    [[ "${tag}" =~ ^v[0-9]+\.[0-9]+\.[0-9]+$ ]] || die "release tag must have form vX.Y.Z: ${tag}"
    command -v git >/dev/null 2>&1 || die 'git is not available'
    object_type="$(git -C "${PROJECT_ROOT}" cat-file -t "refs/tags/${tag}" 2>/dev/null || true)"
    [[ "${object_type}" == tag ]] || die "local annotated tag not found: ${tag}"

    RELEASE_TAG="${tag}"
    VERSION="${tag#v}"
    cargo_version="$(read_tagged_version "${tag}" || true)"
    [[ "${cargo_version}" == "${VERSION}" ]] || die "tag ${tag} does not match tagged Cargo.toml version '${cargo_version}'"
    GIT_SHA="$(git -C "${PROJECT_ROOT}" rev-parse "${tag}^{commit}")"
    GIT_SHORT_SHA="$(git -C "${PROJECT_ROOT}" rev-parse --short=12 "${tag}^{commit}")"
}

host_arch() {
    case "$(uname -m)" in
        x86_64|amd64) printf 'amd64\n' ;;
        arm64|aarch64) printf 'arm64\n' ;;
        *) die "unsupported host architecture: $(uname -m)" ;;
    esac
}

staging_ref() {
    local arch="$1"
    printf '%s:%s-%s-%s\n' "${DOCKER_REPO}" "${VERSION}" "${GIT_SHORT_SHA}" "${arch}"
}

inspect_remote() {
    local ref="$1"
    local format="$2"
    local output
    if output="$(docker buildx imagetools inspect "${ref}" --format "${format}" 2>&1)"; then
        printf '%s\n' "${output}"
        return 0
    fi
    case "${output}" in
        *'manifest unknown'*|*'not found'*) return 2 ;;
        *) echo "Error: could not inspect ${ref}: ${output}" >&2; return 1 ;;
    esac
}

inspect_staging() {
    local ref="$1"
    local expected_arch="$2"
    local output
    local status
    local kind
    local first
    local second
    local third
    local runtime_count=0
    local digest=''
    local os=''
    local arch=''
    local image_version=''
    local image_revision=''

    if output="$(inspect_remote "${ref}" "${STAGING_FORMAT}")"; then
        :
    else
        status=$?
        return "${status}"
    fi

    while IFS='|' read -r kind first second third; do
        case "${kind}" in
            runtime)
                runtime_count=$((runtime_count + 1))
                digest="${first}"
                os="${second}"
                arch="${third}"
                ;;
            labels)
                image_version="${first}"
                image_revision="${second}"
                ;;
        esac
    done <<< "${output}"

    if [[ "${runtime_count}" -ne 1 || "${os}" != linux || "${arch}" != "${expected_arch}" || \
          "${image_version}" != "${VERSION}" || "${image_revision}" != "${GIT_SHA}" ]]; then
        echo "Error: staging image ${ref} does not match the requested release" >&2
        return 3
    fi
    printf '%s\n' "${digest}"
}

inspect_manifest() {
    local ref="$1"
    local expected_amd64="$2"
    local expected_arm64="$3"
    local output
    local status
    local kind
    local digest
    local os
    local arch
    local amd64_digest=''
    local arm64_digest=''
    local runtime_count=0

    if output="$(inspect_remote "${ref}" "${MANIFEST_FORMAT}")"; then
        :
    else
        status=$?
        return "${status}"
    fi

    while IFS='|' read -r kind digest os arch; do
        [[ "${kind}" == runtime ]] || continue
        runtime_count=$((runtime_count + 1))
        [[ "${os}" == linux ]] || { echo "Error: unexpected runtime platform in ${ref}: ${os}/${arch}" >&2; return 3; }
        case "${arch}" in
            amd64)
                [[ -z "${amd64_digest}" ]] || { echo "Error: duplicate amd64 image in ${ref}" >&2; return 3; }
                amd64_digest="${digest}"
                ;;
            arm64)
                [[ -z "${arm64_digest}" ]] || { echo "Error: duplicate arm64 image in ${ref}" >&2; return 3; }
                arm64_digest="${digest}"
                ;;
            *) echo "Error: unexpected runtime platform in ${ref}: linux/${arch}" >&2; return 3 ;;
        esac
    done <<< "${output}"

    if [[ "${runtime_count}" -ne 2 || "${amd64_digest}" != "${expected_amd64}" || \
          "${arm64_digest}" != "${expected_arm64}" ]]; then
        return 3
    fi
}

publish_manifest() {
    local tag="$1"
    local amd64_ref
    local arm64_ref
    local version_ref
    local latest_ref
    local amd64_digest
    local arm64_digest
    local status

    load_release "${tag}"
    require_tools
    amd64_ref="$(staging_ref amd64)"
    arm64_ref="$(staging_ref arm64)"
    version_ref="${DOCKER_REPO}:${VERSION}"
    latest_ref="${DOCKER_REPO}:latest"

    if amd64_digest="$(inspect_staging "${amd64_ref}" amd64)"; then
        :
    else
        status=$?
        [[ "${status}" -eq 2 ]] && die "missing amd64 staging image: ${amd64_ref}"
        return "${status}"
    fi
    if arm64_digest="$(inspect_staging "${arm64_ref}" arm64)"; then
        :
    else
        status=$?
        [[ "${status}" -eq 2 ]] && die "missing arm64 staging image: ${arm64_ref}"
        return "${status}"
    fi

    if inspect_manifest "${version_ref}" "${amd64_digest}" "${arm64_digest}"; then
        echo "Version manifest already exists and matches: ${version_ref}"
    else
        status=$?
        case "${status}" in
            2)
                docker buildx imagetools create \
                    --tag "${version_ref}" \
                    "${DOCKER_REPO}@${amd64_digest}" \
                    "${DOCKER_REPO}@${arm64_digest}"
                inspect_manifest "${version_ref}" "${amd64_digest}" "${arm64_digest}" || \
                    die "published version manifest failed verification: ${version_ref}"
                ;;
            3) die "existing version manifest differs: ${version_ref}" ;;
            *) return "${status}" ;;
        esac
    fi

    if inspect_manifest "${latest_ref}" "${amd64_digest}" "${arm64_digest}"; then
        echo "Latest manifest already matches: ${latest_ref}"
    else
        status=$?
        case "${status}" in
            2|3)
                docker buildx imagetools create --tag "${latest_ref}" "${version_ref}"
                inspect_manifest "${latest_ref}" "${amd64_digest}" "${arm64_digest}" || \
                    die "published latest manifest failed verification: ${latest_ref}"
                ;;
            *) return "${status}" ;;
        esac
    fi

    echo "Published multi-platform images:"
    echo "  ${version_ref}"
    echo "  ${latest_ref}"
}

build_arch() {
    local arch="$1"
    local tag="$2"
    shift 2
    local no_cache=0
    local actual_arch
    local ref
    local digest
    local status
    local build_args

    if [[ "$#" -gt 1 || ( "$#" -eq 1 && "$1" != --no-cache ) ]]; then
        die "build-${arch} accepts only an optional --no-cache"
    fi
    [[ "$#" -eq 1 ]] && no_cache=1

    load_release "${tag}"
    actual_arch="$(host_arch)"
    [[ "${actual_arch}" == "${arch}" ]] || die "build-${arch} requires a native ${arch} host; found ${actual_arch}"
    require_tools
    ref="$(staging_ref "${arch}")"

    if digest="$(inspect_staging "${ref}" "${arch}")"; then
        echo "Staging image already exists and matches: ${ref}@${digest}"
        return 0
    else
        status=$?
        [[ "${status}" -eq 2 ]] || return "${status}"
    fi

    build_args=(
        buildx build
        --platform "linux/${arch}"
        --tag "${ref}"
        --build-arg "GIT_SHORT_SHA=${GIT_SHORT_SHA}"
        --build-arg "GIT_SHA=${GIT_SHA}"
        --build-arg "IMAGE_VERSION=${VERSION}"
    )
    [[ "${no_cache}" -eq 1 ]] && build_args+=(--no-cache)
    build_args+=(--push -)

    echo "Building ${ref} from ${RELEASE_TAG} (${GIT_SHA})"
    git -C "${PROJECT_ROOT}" archive --format=tar "${RELEASE_TAG}" | docker "${build_args[@]}"
    digest="$(inspect_staging "${ref}" "${arch}")" || die "pushed staging image failed validation: ${ref}"
    echo "Published staging image: ${ref}@${digest}"
}

main() {
    case "${1:-}" in
        -h|--help|'') usage ;;
        build-amd64)
            [[ "$#" -ge 2 ]] || die 'build-amd64 requires vX.Y.Z'
            build_arch amd64 "$2" "${@:3}"
            ;;
        build-arm64)
            [[ "$#" -ge 2 ]] || die 'build-arm64 requires vX.Y.Z'
            build_arch arm64 "$2" "${@:3}"
            ;;
        publish-manifest)
            [[ "$#" -eq 2 ]] || die 'publish-manifest requires exactly vX.Y.Z'
            publish_manifest "$2"
            ;;
        *) die "unknown command: $1" ;;
    esac
}

main "$@"
