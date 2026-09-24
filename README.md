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

## Install with Homebrew

On macOS (Apple Silicon) and Linux (x86-64, glibc 2.39 or newer):

```sh
brew tap ferntrail/tap
brew install ferntrail/tap/bitgarth-web
bitgarth-web
```

Open [http://127.0.0.1:8080](http://127.0.0.1:8080). The launcher works from any
working directory and keeps persistent data in `$(brew --prefix)/var/bitgarth`,
outside the versioned package directory. To run it in the background under
launchd or systemd:

```sh
brew services start ferntrail/tap/bitgarth-web
```

`brew install ferntrail/tap/bitgarth-cli` installs the `bitgarth` CLI from the
same tap. See [FernTrail's Homebrew tap](https://github.com/FernTrail/homebrew-tap)
for service management, the per-service environment file, and platform details.

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

If the server stops before opening the interface, see
[Startup troubleshooting](docs/user/startup-troubleshooting.md) for diagnostics
and safe recovery.

Linux downloads are built on Ubuntu 24.04 and require glibc 2.39 or newer,
OpenSSL 3 runtime libraries and system CA certificates. macOS downloads target
Apple Silicon and use Apple's system crypto frameworks. Windows downloads
target x86-64 and require the Microsoft Visual C++ runtime, in addition to the
included crypto DLL. These archives are web servers, not desktop app bundles.

## Build from source

### Build the web server binary

Install the Rust toolchain, then add the WebAssembly target and the Dioxus CLI
at the version `Cargo.toml` pins. On Debian and Ubuntu also install
`pkg-config` and `libssl-dev`.
For native Windows prerequisites, see [Windows development](#windows-development).

```sh
rustup target add wasm32-unknown-unknown
dx_version="$(sed -n 's/^dioxus.*version *= *"\([^"]*\)".*/\1/p' Cargo.toml | head -1)"
cargo install dioxus-cli --locked --version "${dx_version}"
```

A prebuilt `dx` from [Dioxus releases](https://github.com/DioxusLabs/dioxus/releases)
works too, as long as the version matches. Then build:

```sh
./scripts/dx-web-build
```

The build writes `target/dx/bitgarth-app/release/web/server` (`server.exe` on Windows) with its `public/`
directory beside it. Run it **from the repository root** so the relative path to
`assets/catalog/unsynced_asset_catalog.json` resolves — the server refuses to
start without the catalog:

```sh
./target/dx/bitgarth-app/release/web/server
```

Open [http://127.0.0.1:8080](http://127.0.0.1:8080). `IP`, `PORT` and
`BITGARTH_PROJECT_DIR` behave exactly as they do for the downloaded archives.

To run the build somewhere else, copy `server`, its `public/` directory and
`assets/catalog/unsynced_asset_catalog.json` into one directory — that is the
layout the release archives ship. Windows also needs the OpenSSL crypto DLL
beside the executable, or its directory on `PATH`, as described below.

### Windows development

Build and test natively on Windows using **Git Bash**, supplied by Git for
Windows. WSL is not required for the application build, Rust tests, or browser
E2E tests. The Bash scripts orchestrate the native Windows tools; they do not
turn a Windows build into a Linux build.
The repository's `.gitattributes` keeps shell scripts at LF line endings even
when Git's Windows `core.autocrlf` setting is enabled. Preserve LF when editing
scripts; CRLF can break Bash's shebang and option parsing.

Install these prerequisites and make their executables available in Git Bash:

- Visual Studio Build Tools with the C++ build tools and Windows SDK, plus CMake.
- Rust's stable MSVC toolchain (`x86_64-pc-windows-msvc`), including rustfmt and
  Clippy; add the WebAssembly target and matching Dioxus CLI as shown above.
- Node.js 24 and npm, `cargo-deny`, RTK, and hledger. The exact CI tool versions
  are recorded in [ci.yml](.github/workflows/ci.yml).
- OpenSSL development libraries and the matching runtime DLL. The Windows
  release workflow uses vcpkg's `openssl:x64-windows` package.

For an existing vcpkg installation at `C:\dev\vcpkg`, configure OpenSSL in
Git Bash as follows (adjust the installation path):

```sh
VCPKG_ROOT=/c/dev/vcpkg
"$VCPKG_ROOT/vcpkg.exe" install openssl:x64-windows
export OPENSSL_DIR="$(cygpath -m "$VCPKG_ROOT/installed/x64-windows")"
export PATH="$VCPKG_ROOT/installed/x64-windows/bin:$PATH"
```

An existing OpenSSL installation also works if `OPENSSL_DIR` points to it;
set `OPENSSL_INCLUDE_DIR` and `OPENSSL_LIB_DIR` if its headers and libraries
use a nonstandard layout. Its runtime DLL directory must be on `PATH` when
running tests or the server. Downloaded web release archives already contain
the required crypto DLL.

From the repository root in Git Bash:

```sh
bash --version
rustc -vV                    # host: x86_64-pc-windows-msvc
node -p process.platform     # win32
npm ci
npm run e2e:install-browsers
./scripts/verify-full
```

The npm test commands explicitly invoke Bash, so keep Git Bash's `bash.exe`
on `PATH`. Windows' `C:\Windows\System32\bash.exe` launches WSL and is not a
substitute. If starting from PowerShell, open Git Bash, or invoke it explicitly:

```powershell
& 'C:\Program Files\Git\bin\bash.exe' -lc 'cd /c/path/to/BitGarth && ./scripts/verify-full'
```

The native web server is `target/dx/bitgarth-app/release/web/server.exe`; the
CLI release build (`cargo build --release -p bitgarth-cli`) produces
`target/release/bitgarth.exe`. Run the server from the repository root, or copy
the complete release layout described above.

WSL builds and tests target Linux by default. A green WSL run does **not**
validate the Windows `.exe`, Windows DLL loading, or Windows filesystem
behavior. Use native Windows verification for those. The standalone export
fixture checks Unix symlinks and executable permissions, and the performance
wrapper uses zsh; run those Unix tooling checks in a separate Linux/WSL
checkout on a Linux filesystem with LF line endings. Ruby is needed for the
notarization workflow's stubbed regression test. These extra tooling tests are
not prerequisites for building the Windows application.

### Build the Docker image

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
Dioxus CLI, `cargo-deny`, Node.js, npm, Playwright Chromium, hledger, and
[RTK](https://github.com/rtk-ai/rtk). RTK is required because the test wrappers
invoke it directly; having the `rtk` binary on `PATH` is sufficient. Then run:

```sh
./scripts/verify-full
```

On Windows, follow [Windows development](#windows-development). Docker Buildx
is needed for Docker image builds, not for this verification gate.

[ci.yml](.github/workflows/ci.yml) runs this entire gate on Ubuntu for pushes to
`main`; it does not currently run on pull requests. The gate checks formatting,
Clippy, allow-annotation and migration guards, dependency policy, unit/DB/HTTP
tests, the release web build, and Chromium E2E tests.

[release.yml](.github/workflows/release.yml) runs on `v*` tags or manual
dispatch. It tests and builds the CLI on Linux, macOS, and Windows, builds the
web server on each platform, and smoke-tests the extracted web packages. It
also signs/notarizes macOS artifacts and validates release assets. It does not
run `verify-full` or depend on a successful CI job, so a successful release
build is not evidence that the full test suite passed.

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

`npm run e2e` runs an incremental release web build with `dev-config` before
starting the Node helper tests and Playwright. The harness uses a fresh isolated
data directory and records server output under `test-results/` on every platform.

The default E2E lane uses the `docker` channel. Also run the web-channel update
test when verifying native web-server behavior (Git Bash on Windows):

```sh
BITGARTH_E2E_CHANNEL=web npm run e2e -- tests/e2e/specs/software-update-generic.spec.mjs
```

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
