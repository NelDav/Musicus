#!/usr/bin/env bash
# Downloads the pinned pdf.js build (Apache-2.0) that the web client uses to
# render PDF pages, verifies its checksum and places it in
# crates/musicus-web/public/vendor/pdfjs/. Run once before building the client.
#
# We ship pdf.js' "legacy" build: the modern one relies on very new JavaScript
# APIs (e.g. Map.prototype.getOrInsertComputed) that older tablets lack.
set -euo pipefail

VERSION="6.5.438"
INTEGRITY="sha512-vg8Pw1x+AFKy4AMDvhuWl5cOYV8d698UoEANoFFhc79hqXDnZQqnOXoxw3HG907WHOg1JYOvLhP756boddFxpA=="

ROOT="$(cd "$(dirname "$0")/.." && pwd)"
TARGET="$ROOT/crates/musicus-web/public/vendor/pdfjs"
if [[ -f "$TARGET/VERSION" && "$(cat "$TARGET/VERSION")" == "$VERSION-legacy" ]]; then
    echo "pdf.js $VERSION already present"
    exit 0
fi

WORK="$(mktemp -d)"
trap 'rm -rf "$WORK"' EXIT
curl -fsSL "https://registry.npmjs.org/pdfjs-dist/-/pdfjs-dist-$VERSION.tgz" -o "$WORK/pdfjs.tgz"
ACTUAL="sha512-$(openssl dgst -sha512 -binary "$WORK/pdfjs.tgz" | base64 -w0)"
if [[ "$ACTUAL" != "$INTEGRITY" ]]; then
    echo "checksum mismatch for pdfjs-dist $VERSION" >&2
    exit 1
fi
tar -xzf "$WORK/pdfjs.tgz" -C "$WORK"

rm -rf "$TARGET"
mkdir -p "$TARGET"
cp "$WORK/package/legacy/build/pdf.min.mjs" "$WORK/package/legacy/build/pdf.worker.min.mjs" "$TARGET/"
cp "$WORK/package/LICENSE" "$TARGET/LICENSE"
echo "$VERSION-legacy" > "$TARGET/VERSION"
echo "pdf.js $VERSION installed in $TARGET"
