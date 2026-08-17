# BitGarth

garth, noun:
A clearing in the woods. A garden.

BitGarth is a privacy-focused app for collecting wallet and financial data in
one place without pooling that private data into somebody else's database.
Self-hosted data stays in the BitGarth instance you control.

- Website: [bitgarth.app](https://bitgarth.app/)
- Official hosted service: [my.bitgarth.app](https://my.bitgarth.app/)

# Running BitGarth

## Self-host with the official image

Installing or upgrading a docker instance:

```sh
curl -fsSL https://bitgarth.app/docker.sh | sh
```

Open [http://localhost:8080](http://localhost:8080). 

It stores persistent application data in the `bitgarth-data` Docker volume.

See [Environment Variables](docs/user/environment-variables.md) before exposing
an instance through a reverse proxy or changing its storage configuration.

## Build from source

The Docker build is the supported source-build path:

```sh
VERSION="$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)"
GIT_SHA="$(git rev-parse HEAD)"

docker build \
  --build-arg "GIT_SHORT_SHA=$(git rev-parse --short=12 HEAD)" \
  --build-arg "GIT_SHA=${GIT_SHA}" \
  --build-arg "IMAGE_VERSION=${VERSION}" \
  --tag bitgarth:local .

docker run --rm \
  -p 8080:8080 \
  -v bitgarth-data:/data \
  bitgarth:local
```

# Running Tests

To run the complete source verification gate, install the Rust toolchain,
Dioxus CLI, `cargo-deny`, Node.js, npm, Playwright Chromium, Docker Buildx, and
[RTK](https://github.com/rtk-ai/rtk). RTK is required because the test wrappers
invoke it directly; having the `rtk` binary on `PATH` is sufficient. Then run:

```sh
./scripts/verify-full
```

Specific test suites can be run using the commands below.

## Unit Tests

```shell
./scripts/tests-unit
```

```shell
./scripts/tests-db-unit
```

## Integration Tests

```shell
./scripts/tests-integration
```

## Browser E2E Tests (Playwright)

Install JS dependencies and Chromium:

```shell
npm install
npm ci
npm run e2e:install-browsers
```

Run E2E tests:

```shell
npm run e2e
```

`npm run e2e` checks whether the release web build is missing or stale and rebuilds it before starting Playwright.

# Command-Line Client

Build or install the `bitgarth` CLI from the workspace:

```shell
cargo install --path crates/bitgarth-cli
```

Pair interactively, or provide every value for scripts:

```shell
bitgarth pair
bitgarth --profile personal pair https://your-bitgarth.example.com/
bitgarth --profile personal balancesheet
# Short alias:
bitgarth --profile personal bs
```

`pair` accepts a browser URL and uses its scheme, host, and port. Paths, query
parameters, and fragments are ignored. List or rename local profiles with
`bitgarth profile list` and `bitgarth profile rename personal primary`.
Remove only the local profile with `bitgarth profile remove primary`; revoke
the server-side capability from **Settings → Paired Clients** when access must
end.

HTTPS is required by default. For a trusted plain-HTTP URL, either pass
`--allow-insecure-http` or type `yes` after the interactive warning.
See [Security & Privacy](docs/user/security.md#paired-cli-access) for the full
security model.

# Documentation

- [Security & Privacy](docs/user/security.md)
- [Self-hosting environment variables](docs/user/environment-variables.md)
- [Sync architecture](docs/user/sync-architecture.md)
- [Release notes](docs/release-notes/)

# Support and security

This is a source-first publication for auditability and self-hosting. It has no
support SLA.

For vulnerabilities, follow [SECURITY.md](SECURITY.md). Do not report security
issues through public issue trackers or pull requests.

Pull requests are not accepted by default. See
[CONTRIBUTING.md](CONTRIBUTING.md) before proposing a change.

# Ownership

Copyright © 2026 [FernTrail B.V.](https://ferntrail.tech/), Netherlands Chamber
of Commerce (KVK) 42108345.

# License

BitGarth is source-available under the [Functional Source License 1.1 with
Apache 2.0 future license](LICENSE.md) (`FSL-1.1-ALv2`). Each published version
additionally becomes available under the Apache License 2.0 on the second
anniversary of its publication.
