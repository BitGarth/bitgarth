#!/usr/bin/env bash
set -euo pipefail

target="${1:?expected Rust target triple}"
package_id="$(cargo pkgid --locked dioxus)"
version="${package_id##*@}"

# Pin the ZIP digest for each runner alongside its locked Dioxus version.
case "$version/$target" in
  0.7.10/x86_64-unknown-linux-gnu) sha256=61d2132eaf3cd70ca1ac89a7ecc891fcd770e917a5869e29d465459371b26c83 ;;
  0.7.10/aarch64-apple-darwin)    sha256=4aff37aea6b8fbce7bee3f8bbbea1bd71f60e0fa09537f35759f6494da861e37 ;;
  0.7.10/x86_64-pc-windows-msvc)  sha256=45eb4f87b7f86fdba8508ee7e0ef1f9e13cf8f2ffcc6a2d5858d0e8ea7dd1531 ;;
  *) echo "Missing Dioxus CLI checksum for $version/$target" >&2; exit 1 ;;
esac

work_dir="$(mktemp -d)"
trap 'rm -rf "$work_dir"' EXIT
archive="$work_dir/dx.zip"
curl --fail --location --retry 3 --silent --show-error --output "$archive" \
  "https://github.com/DioxusLabs/dioxus/releases/download/v$version/dx-$target.zip"
digest="$(openssl dgst -sha256 -r "$archive")"
if [[ "${digest%% *}" != "$sha256" ]]; then
  echo "Dioxus CLI checksum mismatch for $version/$target" >&2
  exit 1
fi

binary=dx
if [[ "$target" == *-windows-msvc ]]; then binary=dx.exe; fi
unzip -q "$archive" "$binary" -d "$work_dir"
bin_dir="${CARGO_HOME:-$HOME/.cargo}/bin"
mkdir -p "$bin_dir"
install -m 755 "$work_dir/$binary" "$bin_dir/$binary"
