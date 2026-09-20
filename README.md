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

## Web server downloads

Download the `bitgarth-web` archive for your platform from
[GitHub Releases](https://github.com/BitGarth/bitgarth/releases) and extract it
into its own directory. Keep all extracted files together:

- `bitgarth-web` (`bitgarth-web.exe` on Windows).
- `public/`: generated HTML, JavaScript, WASM and static assets.
- `assets/catalog/unsynced_asset_catalog.json`: required at server startup.
- Windows also includes the OpenSSL crypto DLL used by SQLCipher.

The executable alone is insufficient. Run it from the extracted directory so
the catalog's relative path resolves:

```sh
./bitgarth-web
```

On macOS, do not double-click `bitgarth-web` in Finder — run it from your
terminal. It is a command-line program, not an app bundle, so Finder refuses it
with "Apple could not verify" and offers no way to continue. Run
`./bitgarth-web` in your terminal instead, then click **Open** at the one-time
"downloaded from the Internet" prompt; afterwards it launches normally from
anywhere, including from Finder. The macOS downloads are signed and notarized
by FernTrail B.V. — Finder refuses every bare command-line binary regardless of
notarization.

On Windows, run `./bitgarth-web.exe` in PowerShell instead. Open
[http://127.0.0.1:8080](http://127.0.0.1:8080) in a browser. `IP` and `PORT`
override the listen address; `BITGARTH_PROJECT_DIR` selects the persistent data
directory. See [Environment Variables](docs/user/environment-variables.md)
before exposing the server through a reverse proxy.

Linux downloads are built on Ubuntu 24.04 and require glibc 2.39 or newer,
OpenSSL 3 runtime libraries and system CA certificates. macOS downloads target
Apple Silicon and use Apple's system crypto frameworks. Windows downloads
target x86-64 and require the Microsoft Visual C++ runtime, in addition to the
included crypto DLL. These archives are web servers, not desktop app bundles.

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

Prebuilt `bitgarth-cli` archives are also on
[GitHub Releases](https://github.com/BitGarth/bitgarth/releases). On macOS, do
not double-click the extracted `bitgarth` in Finder — run it from your
terminal. It is a command-line program, not an app bundle, so Finder refuses it
with "Apple could not verify" and offers no way to continue. Run `./bitgarth`
in your terminal instead, then click **Open** at the one-time "downloaded from
the Internet" prompt; afterwards it launches normally from anywhere, including
from Finder. The macOS download is signed and notarized by FernTrail B.V. —
Finder refuses every bare command-line binary regardless of notarization.

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
