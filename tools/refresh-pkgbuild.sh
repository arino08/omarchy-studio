#!/usr/bin/env bash
# Point the AUR package at a released tag.
#
# pkgver and the source checksum can't be written by hand and stay right —
# they drifted to 0.0.1 once already, against a tag that never existed. Run
# this after a release; CI fails when pkgver and Cargo.toml disagree.
#
#   tools/refresh-pkgbuild.sh            # use the workspace version
#   tools/refresh-pkgbuild.sh 0.9.0      # or a specific released tag
set -euo pipefail

cd "$(dirname "$0")/.."
ver="${1:-$(sed -n 's/^version = "\(.*\)"/\1/p' Cargo.toml | head -1)}"
ver="${ver#v}"  # tolerate the tag name; the URL below adds its own v
url="https://github.com/arino08/omarchy-studio/archive/refs/tags/v${ver}.tar.gz"
pkg=packaging/aur/PKGBUILD

echo "==> tag v${ver}"
tmp=$(mktemp -d)
trap 'rm -rf "$tmp"' EXIT
if ! curl -fsSL "$url" -o "$tmp/src.tar.gz"; then
    echo "no tarball at $url — is v${ver} tagged and pushed?" >&2
    exit 1
fi
# GitHub serves an HTML error page with a 200 in some failure modes.
if ! gzip -t "$tmp/src.tar.gz" 2>/dev/null; then
    echo "downloaded file is not a gzip archive — refusing to record its sum" >&2
    exit 1
fi
sum=$(sha256sum "$tmp/src.tar.gz" | cut -d' ' -f1)
echo "==> sha256 ${sum}"

sed -i "s/^pkgver=.*/pkgver=${ver}/" "$pkg"
sed -i "s/^sha256sums=.*/sha256sums=('${sum}')/" "$pkg"

# .SRCINFO is generated from the PKGBUILD; makepkg does it properly, but the
# file is simple enough to keep in sync without an Arch host in CI.
if command -v makepkg >/dev/null 2>&1; then
    (cd packaging/aur && makepkg --printsrcinfo > .SRCINFO)
else
    sed -i -e "s/^\tpkgver = .*/\tpkgver = ${ver}/" \
        -e "s|^\tsource = .*|\tsource = omarchy-studio-${ver}.tar.gz::${url}|" \
        -e "s/^\tsha256sums = .*/\tsha256sums = ${sum}/" packaging/aur/.SRCINFO
    echo "note: makepkg not installed — .SRCINFO patched with sed, verify on an Arch box"
fi

echo "==> done; commit packaging/aur/"
