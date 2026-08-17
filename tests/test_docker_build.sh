#!/usr/bin/env bash
set -euo pipefail

ROOT="$(cd "$(dirname "${BASH_SOURCE[0]}")/.." && pwd)"
SUBJECT="${ROOT}/scripts/docker-build.sh"
TMP="$(mktemp -d)"
trap 'rm -rf "${TMP}"' EXIT

WORK="${TMP}/repo"
FAKE_BIN="${TMP}/bin"
FAKE_REGISTRY="${TMP}/registry"
FAKE_LOG="${TMP}/docker.log"
FAKE_CONTEXT="${TMP}/context.tar"
OUTPUT=""
FULL_SHA=""
SHORT_SHA=""
PASS_COUNT=0

fail() {
    echo "FAIL: $*" >&2
    exit 1
}

assert_contains() {
    local haystack="$1"
    local needle="$2"
    [[ "${haystack}" == *"${needle}"* ]] || fail "expected '${needle}' in '${haystack}'"
}

assert_not_contains() {
    local haystack="$1"
    local needle="$2"
    [[ "${haystack}" != *"${needle}"* ]] || fail "did not expect '${needle}' in '${haystack}'"
}

assert_eq() {
    [[ "$1" == "$2" ]] || fail "expected '$1' to equal '$2'"
}

registry_key() {
    local key="$1"
    key="${key//\//_}"
    key="${key//:/_}"
    key="${key//@/_}"
    printf '%s\n' "${key}"
}

write_remote() {
    local ref="$1"
    local arch="$2"
    local version="$3"
    local revision="$4"
    local digest="$5"
    printf 'runtime|%s|linux|%s\nlabels|%s|%s\n' \
        "${digest}" "${arch}" "${version}" "${revision}" \
        > "${FAKE_REGISTRY}/$(registry_key "${ref}")"
}

install_fake_tools() {
    mkdir -p "${FAKE_BIN}" "${FAKE_REGISTRY}"

    cat > "${FAKE_BIN}/uname" <<'SH'
#!/usr/bin/env bash
printf '%s\n' "${FAKE_UNAME:?}"
SH

    cat > "${FAKE_BIN}/docker" <<'SH'
#!/usr/bin/env bash
set -euo pipefail

registry_key() {
    local key="$1"
    key="${key//\//_}"
    key="${key//:/_}"
    key="${key//@/_}"
    printf '%s\n' "${key}"
}

printf '%q ' "$@" >> "${FAKE_LOG:?}"
printf '\n' >> "${FAKE_LOG}"

if [[ "${1:-}" == "buildx" && "${2:-}" == "version" ]]; then
    exit 0
fi

if [[ "${1:-}" == "buildx" && "${2:-}" == "imagetools" && "${3:-}" == "inspect" ]]; then
    ref="${4:?}"
    file="${FAKE_REGISTRY:?}/$(registry_key "${ref}")"
    if [[ ! -f "${file}" ]]; then
        echo "manifest unknown: ${ref}" >&2
        exit 1
    fi
    cat "${file}"
    exit 0
fi

if [[ "${1:-}" == "buildx" && "${2:-}" == "imagetools" && "${3:-}" == "create" ]]; then
    shift 3
    target=""
    sources=()
    while [[ "$#" -gt 0 ]]; do
        case "$1" in
            --tag) target="$2"; shift 2 ;;
            *) sources+=("$1"); shift ;;
        esac
    done
    [[ -n "${target}" ]] || { echo 'missing target tag' >&2; exit 1; }
    target_file="${FAKE_REGISTRY:?}/$(registry_key "${target}")"
    if [[ "${#sources[@]}" -eq 1 && "${sources[0]}" != *@sha256:* ]]; then
        source_file="${FAKE_REGISTRY}/$(registry_key "${sources[0]}")"
        cp "${source_file}" "${target_file}"
        exit 0
    fi
    : > "${target_file}"
    for source in "${sources[@]}"; do
        digest="${source#*@}"
        case "${digest}" in
            sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa)
                printf 'runtime|%s|linux|amd64\n' "${digest}" >> "${target_file}"
                ;;
            sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb)
                printf 'runtime|%s|linux|arm64\n' "${digest}" >> "${target_file}"
                ;;
            *) echo "unexpected source digest: ${digest}" >&2; exit 1 ;;
        esac
    done
    exit 0
fi

if [[ "${1:-}" == "buildx" && "${2:-}" == "build" ]]; then
    shift 2
    tag=""
    platform=""
    version=""
    revision=""
    while [[ "$#" -gt 0 ]]; do
        case "$1" in
            --tag) tag="$2"; shift 2 ;;
            --platform) platform="$2"; shift 2 ;;
            --build-arg)
                case "$2" in
                    IMAGE_VERSION=*) version="${2#IMAGE_VERSION=}" ;;
                    GIT_SHA=*) revision="${2#GIT_SHA=}" ;;
                esac
                shift 2
                ;;
            *) shift ;;
        esac
    done
    cat > "${FAKE_CONTEXT:?}"
    arch="${platform#linux/}"
    case "${arch}" in
        amd64) digest="sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa" ;;
        arm64) digest="sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb" ;;
        *) echo "unexpected platform: ${platform}" >&2; exit 1 ;;
    esac
    printf 'runtime|%s|linux|%s\nlabels|%s|%s\n' \
        "${digest}" "${arch}" "${version}" "${revision}" \
        > "${FAKE_REGISTRY:?}/$(registry_key "${tag}")"
    exit 0
fi

echo "unexpected docker invocation: $*" >&2
exit 1
SH

    ln -s /usr/bin/dirname "${FAKE_BIN}/dirname"
    chmod +x "${FAKE_BIN}/uname" "${FAKE_BIN}/docker"
}

setup_fixture() {
    rm -rf "${WORK}" "${FAKE_REGISTRY}"
    mkdir -p "${WORK}/scripts" "${FAKE_REGISTRY}"
    : > "${FAKE_LOG}"
    rm -f "${FAKE_CONTEXT}"
    cp "${SUBJECT}" "${WORK}/scripts/docker-build.sh"
    chmod +x "${WORK}/scripts/docker-build.sh"
    printf '[package]\nname = "fixture"\nversion = "1.2.3"\n' > "${WORK}/Cargo.toml"
    printf 'FROM scratch\n' > "${WORK}/Dockerfile"
    printf 'tagged\n' > "${WORK}/marker.txt"
    git -C "${WORK}" init -q
    git -C "${WORK}" config user.email test@example.com
    git -C "${WORK}" config user.name Test
    git -C "${WORK}" add .
    git -C "${WORK}" commit -qm tagged
    git -C "${WORK}" tag -a v1.2.3 -m 'Fixture v1.2.3'
    FULL_SHA="$(git -C "${WORK}" rev-parse 'v1.2.3^{commit}')"
    SHORT_SHA="$(git -C "${WORK}" rev-parse --short=12 'v1.2.3^{commit}')"
    printf 'head\n' > "${WORK}/marker.txt"
    git -C "${WORK}" add marker.txt
    git -C "${WORK}" commit -qm 'advance head'
    printf 'dirty\n' > "${WORK}/untracked.txt"
}

run_subject() {
    (
        cd "${WORK}"
        PATH="${FAKE_BIN}:${PATH}" \
        DOCKER_REPO="example/bitgarth" \
        FAKE_UNAME="${FAKE_UNAME}" \
        FAKE_REGISTRY="${FAKE_REGISTRY}" \
        FAKE_LOG="${FAKE_LOG}" \
        FAKE_CONTEXT="${FAKE_CONTEXT}" \
        ./scripts/docker-build.sh "$@"
    ) 2>&1
}

expect_success() {
    if ! OUTPUT="$(run_subject "$@")"; then
        fail "command failed: $*; output: ${OUTPUT}"
    fi
}

expect_failure() {
    if OUTPUT="$(run_subject "$@")"; then
        fail "command unexpectedly succeeded: $*"
    fi
}

pass() {
    PASS_COUNT=$((PASS_COUNT + 1))
}

test_requires_annotated_matching_tag() {
    setup_fixture
    FAKE_UNAME=x86_64
    expect_failure build-amd64 1.2.3
    assert_contains "${OUTPUT}" 'vX.Y.Z'
    expect_failure build-amd64 v9.9.9
    assert_contains "${OUTPUT}" 'annotated tag'
    git -C "${WORK}" tag v1.2.4
    expect_failure build-amd64 v1.2.4
    assert_contains "${OUTPUT}" 'annotated tag'
    git -C "${WORK}" tag -a v1.2.5 -m mismatch
    expect_failure build-amd64 v1.2.5
    assert_contains "${OUTPUT}" 'Cargo.toml version'
    pass
}

test_reports_missing_git_before_tag_validation() {
    setup_fixture
    FAKE_UNAME=x86_64
    if OUTPUT="$(
        cd "${WORK}"
        PATH="${FAKE_BIN}" \
        DOCKER_REPO="example/bitgarth" \
        FAKE_UNAME="${FAKE_UNAME}" \
        FAKE_REGISTRY="${FAKE_REGISTRY}" \
        FAKE_LOG="${FAKE_LOG}" \
        FAKE_CONTEXT="${FAKE_CONTEXT}" \
        /bin/bash ./scripts/docker-build.sh build-amd64 v1.2.3 2>&1
    )"; then
        fail 'command unexpectedly succeeded without git'
    fi
    assert_contains "${OUTPUT}" 'git is not available'
    assert_not_contains "${OUTPUT}" 'annotated tag'
    pass
}

test_builds_tagged_tree_on_native_amd64() {
    setup_fixture
    FAKE_UNAME=x86_64
    expect_success build-amd64 v1.2.3
    log="$(cat "${FAKE_LOG}")"
    assert_contains "${log}" '--platform linux/amd64'
    assert_contains "${log}" "--tag example/bitgarth:1.2.3-${SHORT_SHA}-amd64"
    assert_contains "${log}" "GIT_SHORT_SHA=${SHORT_SHA}"
    assert_contains "${log}" "GIT_SHA=${FULL_SHA}"
    assert_contains "${log}" 'IMAGE_VERSION=1.2.3'
    assert_contains "${log}" '--push'
    archived_sha="$(git get-tar-commit-id < "${FAKE_CONTEXT}")"
    assert_eq "${archived_sha}" "${FULL_SHA}"
    pass
}

test_refuses_non_native_build() {
    setup_fixture
    FAKE_UNAME=arm64
    expect_failure build-amd64 v1.2.3
    assert_contains "${OUTPUT}" 'requires a native amd64 host'
    assert_not_contains "$(cat "${FAKE_LOG}")" 'buildx build'
    pass
}

test_builds_native_arm64_without_cache() {
    setup_fixture
    FAKE_UNAME=aarch64
    expect_success build-arm64 v1.2.3 --no-cache
    log="$(cat "${FAKE_LOG}")"
    assert_contains "${log}" '--platform linux/arm64'
    assert_contains "${log}" "--tag example/bitgarth:1.2.3-${SHORT_SHA}-arm64"
    assert_contains "${log}" '--no-cache'
    pass
}

test_existing_matching_staging_tag_is_idempotent() {
    setup_fixture
    FAKE_UNAME=x86_64
    ref="example/bitgarth:1.2.3-${SHORT_SHA}-amd64"
    write_remote "${ref}" amd64 1.2.3 "${FULL_SHA}" \
        sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
    expect_success build-amd64 v1.2.3
    assert_contains "${OUTPUT}" 'already exists and matches'
    assert_not_contains "$(cat "${FAKE_LOG}")" 'buildx build'
    pass
}

test_existing_mismatched_staging_tag_is_rejected() {
    setup_fixture
    FAKE_UNAME=x86_64
    ref="example/bitgarth:1.2.3-${SHORT_SHA}-amd64"
    write_remote "${ref}" amd64 1.2.3 deadbeef \
        sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
    expect_failure build-amd64 v1.2.3
    assert_contains "${OUTPUT}" 'does not match the requested release'
    assert_not_contains "$(cat "${FAKE_LOG}")" 'buildx build'
    pass
}

test_dockerfile_declares_oci_labels() {
    grep -Fq 'ARG IMAGE_VERSION' "${ROOT}/Dockerfile" || fail 'missing IMAGE_VERSION arg'
    grep -Fq 'ARG GIT_SHA' "${ROOT}/Dockerfile" || fail 'missing GIT_SHA arg'
    grep -Fq 'org.opencontainers.image.version="${IMAGE_VERSION}"' "${ROOT}/Dockerfile" || fail 'missing version label'
    grep -Fq 'org.opencontainers.image.revision="${GIT_SHA}"' "${ROOT}/Dockerfile" || fail 'missing revision label'
    pass
}

seed_both_staging() {
    AMD_REF="example/bitgarth:1.2.3-${SHORT_SHA}-amd64"
    ARM_REF="example/bitgarth:1.2.3-${SHORT_SHA}-arm64"
    AMD_DIGEST=sha256:aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa
    ARM_DIGEST=sha256:bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb
    write_remote "${AMD_REF}" amd64 1.2.3 "${FULL_SHA}" "${AMD_DIGEST}"
    write_remote "${ARM_REF}" arm64 1.2.3 "${FULL_SHA}" "${ARM_DIGEST}"
}

write_manifest() {
    local ref="$1"
    local amd_digest="$2"
    local arm_digest="$3"
    printf 'runtime|%s|linux|amd64\nruntime|%s|linux|arm64\n' \
        "${amd_digest}" "${arm_digest}" \
        > "${FAKE_REGISTRY}/$(registry_key "${ref}")"
}

test_publish_requires_both_matching_staging_images() {
    setup_fixture
    FAKE_UNAME=x86_64
    seed_both_staging
    rm "${FAKE_REGISTRY}/$(registry_key "${ARM_REF}")"
    expect_failure publish-manifest v1.2.3
    assert_contains "${OUTPUT}" 'missing arm64 staging image'
    assert_not_contains "$(cat "${FAKE_LOG}")" 'imagetools create'

    setup_fixture
    seed_both_staging
    write_remote "${ARM_REF}" arm64 9.9.9 "${FULL_SHA}" "${ARM_DIGEST}"
    expect_failure publish-manifest v1.2.3
    assert_contains "${OUTPUT}" 'does not match the requested release'
    assert_not_contains "$(cat "${FAKE_LOG}")" 'imagetools create'
    pass
}

test_publish_creates_and_verifies_version_before_latest() {
    setup_fixture
    FAKE_UNAME=arm64
    seed_both_staging
    expect_success publish-manifest v1.2.3
    version_file="${FAKE_REGISTRY}/$(registry_key 'example/bitgarth:1.2.3')"
    latest_file="${FAKE_REGISTRY}/$(registry_key 'example/bitgarth:latest')"
    [[ -f "${version_file}" ]] || fail 'version manifest was not created'
    [[ -f "${latest_file}" ]] || fail 'latest manifest was not created'
    assert_eq "$(cat "${version_file}")" "$(cat "${latest_file}")"
    log="$(cat "${FAKE_LOG}")"
    case "${log}" in
        *'imagetools create --tag example/bitgarth:1.2.3'*'imagetools inspect example/bitgarth:1.2.3'*'imagetools create --tag example/bitgarth:latest'*) ;;
        *) fail 'latest was not created after version verification' ;;
    esac
    pass
}

test_publish_refuses_different_existing_version() {
    setup_fixture
    FAKE_UNAME=x86_64
    seed_both_staging
    write_manifest example/bitgarth:1.2.3 \
        sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc \
        "${ARM_DIGEST}"
    expect_failure publish-manifest v1.2.3
    assert_contains "${OUTPUT}" 'existing version manifest differs'
    assert_not_contains "$(cat "${FAKE_LOG}")" 'imagetools create'
    pass
}

test_publish_is_idempotent_and_repairs_latest() {
    setup_fixture
    FAKE_UNAME=aarch64
    seed_both_staging
    write_manifest example/bitgarth:1.2.3 "${AMD_DIGEST}" "${ARM_DIGEST}"
    write_manifest example/bitgarth:latest \
        sha256:cccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccccc \
        "${ARM_DIGEST}"
    expect_success publish-manifest v1.2.3
    log="$(cat "${FAKE_LOG}")"
    assert_not_contains "${log}" 'imagetools create --tag example/bitgarth:1.2.3'
    assert_contains "${log}" 'imagetools create --tag example/bitgarth:latest'
    assert_eq \
        "$(cat "${FAKE_REGISTRY}/$(registry_key 'example/bitgarth:1.2.3')")" \
        "$(cat "${FAKE_REGISTRY}/$(registry_key 'example/bitgarth:latest')")"
    pass
}

install_fake_tools
test_requires_annotated_matching_tag
test_reports_missing_git_before_tag_validation
test_builds_tagged_tree_on_native_amd64
test_refuses_non_native_build
test_builds_native_arm64_without_cache
test_existing_matching_staging_tag_is_idempotent
test_existing_mismatched_staging_tag_is_rejected
test_dockerfile_declares_oci_labels
test_publish_requires_both_matching_staging_images
test_publish_creates_and_verifies_version_before_latest
test_publish_refuses_different_existing_version
test_publish_is_idempotent_and_repairs_latest
echo "docker build tests: ${PASS_COUNT} passed"
